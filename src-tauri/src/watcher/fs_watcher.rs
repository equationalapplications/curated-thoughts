use anyhow::{anyhow, Result};
use fs4::FileExt;
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread,
    time::Duration,
};

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind", content = "path")]
pub enum VaultEvent {
    Added(String),
    Modified(String),
    Deleted(String),
}

pub struct WatcherHandle {
    cancel: Arc<AtomicBool>,
    join: thread::JoinHandle<()>,
    /// Optional vault lock held by this watcher. Released on `stop()` (before
    /// joining the watcher thread) so a subsequent watcher acquire cannot
    /// race against an exiting thread. See spec §7 deadlock prevention.
    lock: Option<VaultLock>,
}

impl WatcherHandle {
    pub fn stop(mut self) {
        // Drop the vault lock FIRST so a new watcher can acquire it before the
        // watcher thread is joined (avoids a deadlock window during vault switch).
        drop(self.lock.take());
        self.cancel.store(true, Ordering::SeqCst);
        let _ = self.join.join();
    }

    /// Attach a [`VaultLock`] to this handle. The lock is released when
    /// [`stop`](Self::stop) is called (or when the handle is dropped).
    pub fn with_lock(mut self, lock: VaultLock) -> Self {
        self.lock = Some(lock);
        self
    }
}

pub fn spawn_vault_watcher<F>(vault_path: PathBuf, callback: F) -> Result<WatcherHandle>
where
    F: Fn(VaultEvent) + Send + 'static,
{
    let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher = RecommendedWatcher::new(tx, Config::default())?;
    watcher.watch(&vault_path, RecursiveMode::Recursive)?;

    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_thread = cancel.clone();
    let join = thread::spawn(move || {
        let _keep = watcher;
        loop {
            if cancel_thread.load(Ordering::SeqCst) {
                break;
            }
            match rx.recv_timeout(Duration::from_millis(150)) {
                Ok(Ok(event)) => {
                    for path in event.paths {
                        let path_str = path.to_string_lossy().to_string();
                        let vault_event = match event.kind {
                            EventKind::Create(_) => VaultEvent::Added(path_str),
                            EventKind::Modify(_) => VaultEvent::Modified(path_str),
                            EventKind::Remove(_) => VaultEvent::Deleted(path_str),
                            _ => continue,
                        };
                        callback(vault_event);
                    }
                }
                Ok(Err(_)) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });

    Ok(WatcherHandle {
        cancel,
        join,
        lock: None,
    })
}

/// Cross-platform exclusive lock for a vault directory.
///
/// Used by desktop-mode vault reconciliation to ensure only one watcher
/// holds the vault open at a time. The lock is implemented via
/// `fs4::FileExt::try_lock`, which works on Linux (flock), macOS
/// (fcntl), and Windows (LockFileEx). Holding the lock keeps the file
/// alive via `_file`; releasing it happens implicitly when the struct
/// drops and `fs::File` closes.
///
/// On Windows, the file must be opened without the
/// `FILE_SHARE_READ`/`FILE_SHARE_WRITE` masks for the exclusive lock to
/// fail when another holder exists — `fs4` handles this via its platform
/// implementation, so callers do not need to set flags themselves.
///
/// **API note:** in `fs4` 1.x, `FileExt::try_lock` returns
/// `Result<(), TryLockError>` (it surfaces contention via
/// `Err(TryLockError)` rather than `Ok(false)`; fs4 0.7 named the same
/// operation `try_lock_exclusive` and returned `std::io::Result<()>`).
/// The previous `map_err`-only path was correct in behavior; the
/// CodeRabbit review on PR #96 mistook the API for one returning
/// `Result<bool, _>` (that's POSIX `flock(LOCK_EX | LOCK_NB)`, not
/// `fs4`). We keep the simple `?`-map and only document the actual
/// semantics.
#[derive(Debug)]
pub struct VaultLock {
    /// Keep the lock file handle alive for the lifetime of the guard;
    /// closing the file releases the OS-level lock.
    _file: fs::File,
    /// Stored for diagnostics and for the `path()` accessor.
    _path: PathBuf,
}

impl VaultLock {
    /// Acquire the exclusive vault lock for `vault`.
    ///
    /// On success returns a guard whose drop releases the lock.
    /// On contention returns `Err` with a message identifying the
    /// existing holder (when the platform exposes one).
    pub fn acquire(vault: &Path) -> Result<Self> {
        let lock_path = vault.join(".curated_thoughts.lock");
        // Open for read+write with create-if-missing, but DO NOT truncate.
        // If `lock_path` happens to be a symlink, opening it for write would
        // follow the link and truncate its target — opening any file in a
        // location a principal can race into would let starting the watcher
        // destroy content the application didn't otherwise touch. The OS
        // lock (try_lock) holds without modifying the file's
        // contents, so truncate is unnecessary.
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .read(true)
            .open(&lock_path)
            .map_err(|e| {
                anyhow!(
                    "failed to open vault lock file {}: {e}",
                    lock_path.display()
                )
            })?;
        // `fs4::FileExt::try_lock` (1.x; was `try_lock_exclusive` in 0.7)
        // returns `Result<(), TryLockError>`. Contention surfaces as
        // `Err(TryLockError)` — the `?`-propagation below is sufficient;
        // no `Ok(false)` case exists in either version of `fs4`.
        Self::try_lock_exclusive(&file)?;
        Ok(Self {
            _file: file,
            _path: lock_path,
        })
    }

    /// Platform-native exclusive try-lock with a single, descriptive
    /// error message on contention.
    fn try_lock_exclusive(file: &fs::File) -> Result<()> {
        file.try_lock()
            .map_err(|e| anyhow!("vault is already locked by another watcher instance: {e}"))
    }

    /// Return the on-disk path of the lock file (for diagnostics).
    pub fn path(&self) -> &Path {
        &self._path
    }
}

impl Drop for VaultLock {
    fn drop(&mut self) {
        // Release the OS lock explicitly; fs::File's Drop will close
        // the handle. Unlock failure here is non-fatal (the file is
        // being closed anyway), so we swallow the error.
        let _ = self._file.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, sync::mpsc, time::Duration};
    use tempfile::TempDir;

    #[test]
    fn test_watcher_detects_new_file() {
        let tmp = TempDir::new().unwrap();
        let (tx, rx) = mpsc::channel::<VaultEvent>();
        let handle = spawn_vault_watcher(tmp.path().to_path_buf(), move |e| {
            tx.send(e).ok();
        })
        .unwrap();

        fs::write(tmp.path().join("note.md"), "hello").unwrap();

        let event = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("no event received");
        assert!(matches!(event, VaultEvent::Added(_)));
        handle.stop();
    }

    #[test]
    fn test_watcher_detects_deleted_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("note.md");
        fs::write(&path, "hello").unwrap();

        let (tx, rx) = mpsc::channel::<VaultEvent>();
        let handle = spawn_vault_watcher(tmp.path().to_path_buf(), move |e| {
            tx.send(e).ok();
        })
        .unwrap();

        std::thread::sleep(Duration::from_millis(200));
        fs::remove_file(&path).unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut found = false;
        while std::time::Instant::now() < deadline {
            let remaining = deadline.duration_since(std::time::Instant::now());
            if let Ok(event) = rx.recv_timeout(remaining) {
                if matches!(event, VaultEvent::Deleted(_)) {
                    found = true;
                    break;
                }
            }
        }
        assert!(found, "no Deleted event received within timeout");
        handle.stop();
    }

    #[test]
    fn test_watcher_delivers_absolute_paths() {
        let tmp = TempDir::new().unwrap();
        let (tx, rx) = mpsc::channel::<VaultEvent>();
        let handle = spawn_vault_watcher(tmp.path().to_path_buf(), move |e| {
            tx.send(e).ok();
        })
        .unwrap();
        fs::write(tmp.path().join("note.md"), "hello").unwrap();

        let event = rx.recv_timeout(Duration::from_secs(5)).expect("no event");
        let path_str = match event {
            VaultEvent::Added(p) | VaultEvent::Modified(p) | VaultEvent::Deleted(p) => p,
        };
        let path = Path::new(&path_str);
        assert!(
            path.is_absolute(),
            "watcher delivered non-absolute path: {}",
            path_str
        );
        handle.stop();
    }

    #[test]
    fn vault_lock_blocks_second_acquire() {
        let tmp = TempDir::new().unwrap();
        let _first = VaultLock::acquire(tmp.path()).expect("first acquire succeeds");
        let second = VaultLock::acquire(tmp.path());
        assert!(
            second.is_err(),
            "second acquire on same vault must fail, got {:?}",
            second
        );
        let msg = format!("{}", second.err().unwrap());
        assert!(
            msg.contains("locked") || msg.contains("lock"),
            "error message should mention lock contention, got: {msg}"
        );
    }

    #[test]
    fn vault_lock_released_on_drop() {
        let tmp = TempDir::new().unwrap();
        {
            let _first = VaultLock::acquire(tmp.path()).expect("first acquire succeeds");
        }
        // First guard is dropped here; second acquire must now succeed.
        let second = VaultLock::acquire(tmp.path());
        assert!(
            second.is_ok(),
            "second acquire after drop must succeed, got {:?}",
            second
        );
    }

    /// A symlinked lock path must not have its target truncated by
    /// `acquire`. The lock is advisory and held on the open handle —
    /// the file's bytes are never read or written — so opening with
    /// `truncate(true)` would destroy an attacker- or accident-planted
    /// symlink target for no benefit. Ported from `tools/src/lock.rs`
    /// (PR #129) per issue #141; this copy runs in CI on every PR.
    ///
    /// Unix-only: `std::os::windows::fs::symlink_file` requires
    /// Developer Mode or `SeCreateSymbolicLinkPrivilege`, which we
    /// cannot rely on in a developer environment. `#[cfg(unix)]` is on
    /// this test alone so the sibling tests still run on Windows.
    #[cfg(unix)]
    #[test]
    fn vault_lock_does_not_truncate_symlink_target() {
        let tmp = TempDir::new().unwrap();
        let canary = tmp.path().join("canary.txt");
        let contents = "do not truncate me";
        fs::write(&canary, contents).unwrap();

        std::os::unix::fs::symlink(&canary, tmp.path().join(".curated_thoughts.lock")).unwrap();

        {
            // Deliberately NOT `.expect(...)`: if a later hardening pass
            // makes `acquire` reject a symlinked lock path outright, it
            // returns `Err`, the canary is still intact, and this test
            // must keep passing unmodified. The only assertion that
            // matters is the canary's contents below.
            let _guard = VaultLock::acquire(tmp.path());
        }

        assert_eq!(
            fs::read_to_string(&canary).unwrap(),
            contents,
            "acquire must not truncate the symlink's target"
        );
    }
}
