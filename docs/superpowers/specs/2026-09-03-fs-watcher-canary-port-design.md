# Port lock symlink-truncation canary to `fs_watcher.rs` — Design

**Date:** 2026-09-03
**Status:** Draft
**Branch:** `test/fs-watcher-canary-port`
**Priority:** P2 (test-only regression gate; closes issue #141)

## 1. Problem

`src-tauri/src/watcher/fs_watcher.rs` has the correct `truncate(false)` lock
behavior but NO regression test — a future refactor could reintroduce
`truncate(true)` and silently destroy a symlinked lock path's target. The
duplicate in `tools/src/lock.rs` HAS the canary test (from PR #129); this
port mirrors it onto the original, where CI (`rust-ubuntu`, `rust-macos`)
will actually gate it.

## 2. Approach

Single test ported into `fs_watcher.rs`'s existing `mod tests`, following
the issue's two carried constraints:

1. Assert ONLY that the canary is unchanged — no `.expect()` on `acquire`
   (a future hardening pass that rejects symlinked lock paths must still
   pass this test).
2. `#[cfg(unix)]` on the test itself, not the module (Windows symlink
   creation needs Developer Mode; sibling tests stay enabled).

## 3. Acceptance (from the issue)

- Test exists in `fs_watcher.rs` and passes.
- It FAILS if `truncate(false)` reverts to `truncate(true)`.

## 4. Out of scope

Any production-code change; the tools crate; #140/#143.

## 5. Open questions

None — the issue fully specifies the work.
