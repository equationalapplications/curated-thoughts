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
//! Both implementations use the **non-blocking** exclusive file lock
//! (`fs4::FileExt::try_lock` in fs4 1.x — named `try_lock_exclusive` in
//! 0.7), which works on Linux (flock),
//! macOS (fcntl), and Windows (LockFileEx). Contention surfaces as
//! `Err(TryLockError)`, not `Ok(false)` — see the
//! API note on `try_lock_exclusive` below. Keeping the lock-file path
//! and semantics identical ensures the two lockers see each other
//! across a desktop/CLI hand-off.
//!
//! The error-mapping helper `map_try_lock_err` (which distinguishes
//! `TryLockError::WouldBlock` contention from real I/O failures —
//! issue #146) is part of that same intentional duplication and must
//! stay in sync across both copies.

use anyhow::{anyhow, Result};
// Note: `fs4::FileExt` is intentionally NOT imported — on Rust >= 1.89
// std's inherent `File::try_lock`/`unlock` shadow the trait methods, so
// the fs4 trait method is called via UFCS in `try_lock_exclusive` below.
use fs4::TryLockError;
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
        Self::try_lock_exclusive(&file)?;
        Ok(Self {
            _file: file,
            _path: lock_path,
        })
    }

    /// Platform-native `try_lock` (fs4 1.x; was `try_lock_exclusive` in
    /// 0.7) with descriptive, cause-appropriate error messages.
    ///
    /// **API note:** in `fs4` 1.x, `FileExt::try_lock` returns
    /// `Result<(), TryLockError>` (contention surfaces as
    /// `Err(TryLockError)`; fs4 0.7 named the same operation
    /// `try_lock_exclusive` and returned `std::io::Result<()>` with
    /// `Err(AlreadyLocked)` / `Err(WouldBlock)`). In either version there
    /// is NO `Ok(false)` case (the CodeRabbit review on PR #96 mistook it
    /// for a `Result<bool, _>` API — that's POSIX `flock(LOCK_EX | LOCK_NB)`,
    /// not `fs4`). We keep the simple `?`-propagation and only document
    /// the actual semantics here. Mirrors
    /// `src-tauri/src/watcher/fs_watcher.rs` (PR #110 migrated that copy).
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
/// reliable. **Deliberately duplicated** in
/// `src-tauri/src/watcher/fs_watcher.rs` — keep both copies in sync.
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
