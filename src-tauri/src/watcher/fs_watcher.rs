use anyhow::{anyhow, Result};
// Note: `fs4::FileExt` is intentionally NOT imported — on Rust >= 1.89
// std's inherent `File::try_lock`/`unlock` shadow the trait methods, so
// the fs4 trait method is called via UFCS in `try_lock_exclusive` below.
use fs4::TryLockError;
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc,
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind", content = "path")]
pub enum VaultEvent {
    Added(String),
    Modified(String),
    Deleted(String),
}

/// Unix-secs "now" helper for the arming/error latches (`0` on clock
/// failure — which reads as "never" to every consumer, the safe direction).
fn unix_secs_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Record a notify runtime error into `last_error_at` and log it. Extracted
/// as a free function (rather than a method on `WatcherHandle`) so the
/// event-loop closure can own the `Arc` and tests can exercise the
/// error-latch semantics directly — see the spec Tests section: dropping the
/// notify sender yields `Disconnected`, not an `Err` item, so this path
/// cannot be driven from a live watcher without channel tricks.
pub fn record_watcher_error(last_error_at: &AtomicU64, err: &notify::Error) {
    last_error_at.store(unix_secs_now(), Ordering::SeqCst);
    eprintln!("[watch] notify error: {err}");
}

pub struct WatcherHandle {
    cancel: Arc<AtomicBool>,
    join: thread::JoinHandle<()>,
    /// Unix-secs timestamp set once `watch()` returns Ok. `0` means the
    /// watcher never armed (spec §1). Read by the periodic self-check
    /// monitor to distinguish a healthy watcher from a never-armed one.
    pub armed_at: Arc<AtomicU64>,
    /// Unix-secs timestamp bumped whenever the event loop consumes a
    /// `notify::Error`. `0` = no error seen. The self-check monitor treats a
    /// bump since its last clean tick as degradation.
    pub last_error_at: Arc<AtomicU64>,
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

    /// Liveness probe (spec §1). Linux: at least one entry under
    /// `/proc/self/fd` whose readlink target contains `inotify` — the same
    /// signal the incident evidence came from (0 fds = backend closed =
    /// dead). This counts the whole process's inotify fds, so it is a lower
    /// bound: it can never false-negative the incident signature, though in
    /// principle another inotify user could mask a death (no other inotify
    /// user exists in the tree today). Other platforms: always `true` — no
    /// OS signal is available there, so the `last_error_at` latch is the
    /// portable signal.
    pub fn is_alive(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            let fd_dir = "/proc/self/fd";
            match fs::read_dir(fd_dir) {
                Ok(entries) => {
                    for entry in entries.flatten() {
                        if let Ok(target) = fs::read_link(entry.path()) {
                            if target.to_string_lossy().contains("inotify") {
                                return true;
                            }
                        }
                    }
                    false
                }
                // If /proc is unreadable (exotic sandbox), fail OPEN: the
                // alternative would latch degraded on every tick on such a
                // system, which is noise, not signal.
                Err(e) => {
                    eprintln!("[watch] is_alive: cannot scan {fd_dir}: {e}; assuming alive");
                    true
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            true
        }
    }
}

pub fn spawn_vault_watcher<F>(vault_path: PathBuf, callback: F) -> Result<WatcherHandle>
where
    F: Fn(VaultEvent) + Send + 'static,
{
    let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher = RecommendedWatcher::new(tx, Config::default())?;
    watcher.watch(&vault_path, RecursiveMode::Recursive)?;

    let armed_at = Arc::new(AtomicU64::new(unix_secs_now()));
    let last_error_at = Arc::new(AtomicU64::new(0));
    let loop_last_error_at = last_error_at.clone();
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
                // Log + latch instead of silently swallowing (spec §1): the
                // self-check monitor reads `last_error_at` to surface
                // degraded watcher health.
                Ok(Err(e)) => record_watcher_error(&loop_last_error_at, &e),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });

    Ok(WatcherHandle {
        cancel,
        join,
        armed_at,
        last_error_at,
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

    /// Platform-native exclusive try-lock with descriptive,
    /// cause-appropriate error messages.
    fn try_lock_exclusive(file: &fs::File) -> Result<()> {
        // UFCS (not `file.try_lock()`): on Rust >= 1.89 the inherent
        // `std::fs::File::try_lock` shadows fs4's trait method; call
        // fs4's explicitly so the error type is `fs4::TryLockError`.
        fs4::FileExt::try_lock(file).map_err(map_try_lock_err)
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

/// Distinguish pure lock contention (`TryLockError::WouldBlock`) from a
/// real lock-acquisition failure (`TryLockError::Error(io::Error)`, e.g.
/// permission denied). Reporting a permission error as "another watcher
/// holds the lock" sends the user hunting for a phantom second process
/// (issue #146, a CodeRabbit finding on PR #144).
///
/// fs4's `From<io::Error>` impl collapses any `io::Error` of kind
/// `WouldBlock` into `TryLockError::WouldBlock`, so variant matching is
/// reliable. **Deliberately duplicated** in `tools/src/lock.rs` — keep
/// both copies in sync.
fn map_try_lock_err(e: TryLockError) -> anyhow::Error {
    match e {
        TryLockError::WouldBlock => {
            anyhow!("vault is already locked by another watcher instance")
        }
        TryLockError::Error(err) => {
            // Preserve the io::Error as the anyhow SOURCE (not stringified
            // into the message) so callers/logging can inspect the cause
            // chain (Copilot follow-up on PR #148).
            anyhow::Error::new(err).context("failed to acquire vault lock")
        }
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

    /// Issue #146: a `TryLockError::WouldBlock` (pure contention) must map
    /// to the contention message, NOT to an acquire-failure message that
    /// would send the user hunting for a phantom I/O problem.
    #[test]
    fn wouldblock_maps_to_contention_message() {
        let err = map_try_lock_err(fs4::TryLockError::WouldBlock);
        let msg = err.to_string();
        assert!(
            msg.contains("already locked"),
            "contention message should say 'already locked', got: {msg}"
        );
        assert!(
            !msg.contains("failed to acquire"),
            "contention message must NOT use the I/O-failure wording, got: {msg}"
        );
    }

    /// Issue #146 + Copilot follow-up on PR #148: a real lock-acquisition
    /// failure (e.g. permission denied) must map to the acquire-failure
    /// message with the underlying `io::Error` preserved as the anyhow
    /// SOURCE (inspectable via the cause chain), NOT the contention
    /// message that blames another watcher.
    #[test]
    fn io_error_maps_to_acquire_failure_message() {
        let err = map_try_lock_err(fs4::TryLockError::Error(std::io::Error::from(
            std::io::ErrorKind::PermissionDenied,
        )));
        let msg = err.to_string();
        assert!(
            msg.starts_with("failed to acquire vault lock"),
            "I/O-failure message should start with 'failed to acquire vault lock', got: {msg}"
        );
        assert!(
            !msg.contains("already locked"),
            "I/O-failure message must NOT use the contention wording, got: {msg}"
        );
        // The io::Error must ride the anyhow source chain, not be
        // stringified into the outer message (Copilot follow-up on PR #148).
        let src = err
            .source()
            .expect("acquire-failure error must have an io::Error source");
        let io_src = src
            .downcast_ref::<std::io::Error>()
            .expect("source must be the underlying std::io::Error");
        assert_eq!(
            io_src.kind(),
            std::io::ErrorKind::PermissionDenied,
            "source io::Error must preserve the original error kind"
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

    // ── Watcher-arming self-check (spec 2026-09-24 §1 + Tests) ─────────────

    /// A successfully spawned watcher must have `armed_at` set to a nonzero
    /// unix-secs timestamp — `0` is reserved for "spawn failed before
    /// arming", which cannot happen for a handle that exists, but the
    /// self-check monitor keys on this so the invariant is pinned here.
    #[test]
    fn spawned_watcher_sets_armed_at() {
        let tmp = TempDir::new().unwrap();
        let (tx, _rx) = mpsc::channel::<VaultEvent>();
        let handle = spawn_vault_watcher(tmp.path().to_path_buf(), move |e| {
            tx.send(e).ok();
        })
        .expect("spawn succeeds");
        let armed = handle.armed_at.load(std::sync::atomic::Ordering::SeqCst);
        assert!(
            armed > 0,
            "armed_at must be a nonzero unix timestamp after watch() Ok, got {armed}"
        );
        assert_eq!(
            handle
                .last_error_at
                .load(std::sync::atomic::Ordering::SeqCst),
            0,
            "a fresh watcher must have no recorded errors"
        );
        handle.stop();
    }

    /// A live watcher (just armed) must report alive on Linux via the
    /// `/proc/self/fd` inotify scan. On other platforms this is trivially
    /// `true`, so the test documents the contract without asserting an OS
    /// signal that does not exist there.
    #[test]
    fn freshly_spawned_watcher_is_alive() {
        let tmp = TempDir::new().unwrap();
        let (tx, _rx) = mpsc::channel::<VaultEvent>();
        let handle = spawn_vault_watcher(tmp.path().to_path_buf(), move |e| {
            tx.send(e).ok();
        })
        .expect("spawn succeeds");
        assert!(
            handle.is_alive(),
            "a freshly-armed watcher must report alive"
        );
        handle.stop();
    }

    /// `record_watcher_error` must bump `last_error_at` to a nonzero
    /// timestamp. It is a free function (not a handle method) precisely so
    /// this can be unit-tested: dropping the notify sender inside a live
    /// watcher yields `RecvTimeoutError::Disconnected`, never an `Err`
    /// event, so the production path cannot be driven end-to-end without
    /// channel tricks (spec Tests, "record_watcher_error unit test").
    #[test]
    fn record_watcher_error_bumps_last_error_at() {
        let last_error_at = std::sync::atomic::AtomicU64::new(0);
        assert_eq!(
            last_error_at.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "precondition: no error recorded"
        );
        record_watcher_error(
            &last_error_at,
            &notify::Error::io(std::io::Error::other("test notify error")),
        );
        let bumped = last_error_at.load(std::sync::atomic::Ordering::SeqCst);
        assert!(
            bumped > 0,
            "record_watcher_error must bump last_error_at to a nonzero unix timestamp, got {bumped}"
        );
    }
}
