# fs_watcher canary port — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port the symlink-truncation canary test from `tools/src/lock.rs` into `src-tauri/src/watcher/fs_watcher.rs` (issue #141).

**Architecture:** Test-only change inside the existing `mod tests` of `fs_watcher.rs`; no production code touched.

**Tech Stack:** Rust, `tempfile` (already a dev-dependency), `#[cfg(unix)]` std symlink.

**Spec:** docs/superpowers/specs/2026-09-03-fs-watcher-canary-port-design.md

## Global Constraints

- NO production-code changes; only the test module of `src-tauri/src/watcher/fs_watcher.rs`.
- Assert only canary integrity; never `.expect()`/`unwrap()` the `acquire` result (issue constraint 1).
- `#[cfg(unix)]` on the test fn, NOT on `mod tests` (issue constraint 2).
- Build/test flags: `cargo test --manifest-path src-tauri/Cargo.toml --features test-utils,mcp-server --lib watcher -- --test-threads=1` (feature-gated profile; never bare `cargo test` for the lib profile).

---

### Task 1: Port the canary test

**Files:**
- Modify: `src-tauri/src/watcher/fs_watcher.rs` (test module, after `vault_lock_released_on_drop`)

**Interfaces:**
- Consumes: existing `VaultLock::acquire(vault: &Path)` in the same file.
- Produces: `#[cfg(unix)] #[test] fn vault_lock_does_not_truncate_symlink_target()`.

- [ ] **Step 1: Add the test (exact code)**

```rust
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
```

(`fs` and `TempDir` are already imported by `mod tests`.)

- [ ] **Step 2: Run the watcher test subset**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --features test-utils,mcp-server --lib watcher -- --test-threads=1`
Expected: PASS, including `vault_lock_does_not_truncate_symlink_target`.

- [ ] **Step 3: Verify the canary actually bites (acceptance criterion 2)**

Temporarily change `truncate(false)` → `truncate(true)` in `acquire`, re-run the subset, confirm the canary test FAILS, then revert the change. Record both outcomes.

- [ ] **Step 4: Format + commit**

```bash
cargo fmt --manifest-path src-tauri/Cargo.toml
git add src-tauri/src/watcher/fs_watcher.rs
git commit -m "test(watcher): port lock symlink-truncation canary from tools/lock.rs (issue #141)"
```
