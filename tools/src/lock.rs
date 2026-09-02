//! Standalone `VaultLock` for the `ct watch` headless command.
//!
//! This is a **deliberate duplicate** of
//! `curated_thoughts::watcher::VaultLock` (the desktop-mode type in
//! `src-tauri/src/watcher/fs_watcher.rs`). The cargo dependency direction
//! is `tools -> src-tauri`, so the desktop crate cannot re-export a
//! type that lives here without creating a cyclic package dep. A
//! workspace migration (planned for phase-3) can collapse this into a
//! single shared definition; until then we accept ~30 LOC of
//! duplication so `ct watch` can lock the vault when no desktop is
//! running.
//!
//! Both implementations use the **non-blocking**
//! `fs4::FileExt::try_lock_exclusive`, which works on Linux (flock),
//! macOS (fcntl), and Windows (LockFileEx). Contention surfaces as
//! `Err(AlreadyLocked)` / `Err(WouldBlock)`, not `Ok(false)` — see the
//! API note on `try_lock_exclusive` below. Keeping the lock-file path
//! and semantics identical ensures the two lockers see each other
//! across a desktop/CLI hand-off.

use anyhow::{anyhow, Result};
use fs4::FileExt;
use std::{
    fs,
    path::{Path, PathBuf},
};

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
        // lock (try_lock_exclusive) holds without modifying the file's
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
        Self::try_lock_exclusive(&file)?;
        Ok(Self {
            _file: file,
            _path: lock_path,
        })
    }

    /// Platform-native `try_lock_exclusive` with a single, descriptive
    /// error message on contention.
    ///
    /// **API note:** in `fs4` 0.7, `FileExt::try_lock_exclusive` returns
    /// `std::io::Result<()>` (contention surfaces as
    /// `Err(AlreadyLocked)` / `Err(WouldBlock)`, NOT `Ok(false)`). The
    /// previous `map_err`-only path was therefore correct in behavior;
    /// the CodeRabbit review on PR #96 mistook it for a
    /// `Result<bool, _>` API (that's POSIX `flock(LOCK_EX | LOCK_NB)`,
    /// not `fs4`). We keep the simple `?`-propagation and only document
    /// the actual semantics here.
    fn try_lock_exclusive(file: &fs::File) -> Result<()> {
        file.try_lock_exclusive()
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
    use tempfile::TempDir;

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
    /// symlink target for no benefit. Mirrors the fix `6c1113e` made in
    /// `src-tauri/src/watcher/fs_watcher.rs`.
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
