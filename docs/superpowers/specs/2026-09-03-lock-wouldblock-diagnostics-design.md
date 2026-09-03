# VaultLock: Distinguish WouldBlock from Real I/O Errors — Design

**Date:** 2026-09-03
**Status:** Draft for implementation
**Branch:** `fix/lock-wouldblock-diagnostics`
**Priority:** P2 (diagnostics correctness; closes issue #146; CodeRabbit finding on PR #144)

## 1. Problem

Both `VaultLock` copies map **every** `try_lock` failure to a contention
message:

- `tools/src/lock.rs:85-88` —
  `file.try_lock().map_err(|e| anyhow!("vault is already locked by another watcher instance: {e}"))`
- `src-tauri/src/watcher/fs_watcher.rs:164-167` — same shape.

In fs4 1.x (both crates are on fs4 1.1.0 — #144 migrated `tools`, #110
migrated src-tauri), `FileExt::try_lock` returns
`Result<(), TryLockError>` where (verified in the fs4 1.1.0 source,
`src/try_lock_error.rs`):

- `TryLockError::WouldBlock` — contention; the operation would block.
- `TryLockError::Error(io::Error)` — a real lock-acquisition failure
  (permission, I/O error, …).

fs4's own `From<io::Error> for TryLockError` collapses any `io::Error` of
kind `WouldBlock` into `TryLockError::WouldBlock`, so variant matching is
reliable. Reporting a permission error as "another watcher holds the lock"
sends the user hunting for a phantom second process.

The two copies are **documented intentional duplication** (lock.rs header);
they must change together, identically.

## 2. Approach

In BOTH files, factor the mapping into a small private helper so it is unit
testable without needing to force real lock failures, and match on the
variant:

```rust
fn map_try_lock_err(e: fs4::TryLockError) -> anyhow::Error {
    match e {
        fs4::TryLockError::WouldBlock => {
            anyhow!("vault is already locked by another watcher instance")
        }
        fs4::TryLockError::Error(err) => {
            anyhow!("failed to acquire vault lock: {err}")
        }
    }
}

// call site (both copies):
Self::try_lock_exclusive(&file)?;   // inside: file.try_lock().map_err(map_try_lock_err)
```

- **Contention message drops the `{e}` suffix** ("… would block" adds
  nothing for users); the I/O-error message KEEPS the underlying error for
  diagnosis. Existing tests assert only `contains("lock")` — both messages
  satisfy them.
- Keep each copy's existing API-note doc comment (fs4 0.7 vs 1.x history)
  and the duplication header; extend the duplication note to mention the
  helper is also duplicated and must stay in sync.
- Import `fs4::TryLockError` in both files.

## 3. Testing

Inline `#[cfg(test)]` unit tests in BOTH files (both already have inline
test modules; `tools/lock.rs` also carries the lock canary suite):

- `wouldblock_maps_to_contention_message` —
  `map_try_lock_err(TryLockError::WouldBlock).to_string()` contains
  "already locked" and does NOT contain "failed to acquire".
- `io_error_maps_to_acquire_failure_message` —
  `map_try_lock_err(TryLockError::Error(io::Error::from(
  io::ErrorKind::PermissionDenied)))` → message starts with
  "failed to acquire vault lock" and embeds the underlying error text.
- Existing contention canary tests (real two-handle flock contention in
  `tools/lock.rs` tests and the src-tauri canary ported in PR #145) must
  stay green — they exercise the `WouldBlock` arm end-to-end and prove the
  refactor didn't flip the arms.

**Sabotage check:** swap the two match arms → both unit tests fail.

**Gate:**
`cargo test -p ct-tools --lib lock` (or the tools crate's lock test
target, matching how #144/#145 ran it) and
`cargo test --manifest-path src-tauri/Cargo.toml --lib watcher` /
the fs_watcher canary test, plus `cargo check` on both crates.

## 4. Out of scope

- Unifying the two lock copies into one shared crate/module — the
  duplication is documented and deliberate; issue #146 explicitly says
  "changed together", not "merged".
- Blocking/retry lock acquisition; both call sites want try-semantics.
