//! Thin re-export of the vault watcher for the `ct` CLI binary.
//!
//! The canonical watcher implementation lives in the desktop crate at
//! `tauri_app_lib::watcher` (see `src-tauri/src/watcher/fs_watcher.rs`).
//! The original phase-2 plan asked us to move it here, but the cargo
//! dependency direction is `tools -> src-tauri`, so we cannot make the
//! desktop crate re-export a type that lives in `tools/` without
//! creating a cyclic package dependency. Keeping the watcher logic in
//! `src-tauri` and exposing it through this module lets `ct watch`
//! share the production watcher + lock types while remaining
//! causally downstream.
//!
//! `ct watch` uses both:
//! - `tauri_app_lib::watcher::spawn_vault_watcher` (real watcher)
//! - `crate::lock::VaultLock` (standalone, `ct`-side lock used when no
//!   desktop is holding the vault)
//!
//! No logic lives in this file. If you find yourself adding some,
//! stop and fix the duplication boundary first.

// Glob re-export first — this brings the desktop crate's `VaultLock`
// into scope under the `curated_thoughts_tools::watcher::*` namespace.
// The explicit re-export below shadows it (explicit imports beat glob
// imports in Rust name resolution), so callers using
// `curated_thoughts_tools::watcher::VaultLock` get the standalone,
// `ct`-side lock — which acquires the lock on the directory it is
// handed (today: `brain_dir`), not on the vault root. Desktop callers
// who genuinely want the desktop variant should import from
// `tauri_app_lib::watcher` directly.
//
// History: an earlier draft used `pub use crate::lock::VaultLock as
// CtVaultLock` and relied on alias semantics to "shadow" the glob's
// `VaultLock`. That was wrong: the alias introduced a NEW name
// (`CtVaultLock`) without overriding the existing `VaultLock` from
// the glob. Task 8's smoke test surfaced the divergence (the desktop
// path and the `ct watch` path were acquiring the lock at different
// paths); the desktop side was already hot-fixed in 6258cfd. This
// PR fixes the name-resolution bug on the `tools` side so future
// callers cannot re-introduce the same drift.
//
// **Deprecation (CodeRabbit review on PR #96):** the `CtVaultLock`
// alias that used to live here has been removed. No callers in the
// workspace used it (a tree-wide `rg "CtVaultLock"` is part of the
// pre-commit hook), so the API surface loses nothing. The intended
// future cleanup is to collapse the ~30 LOC of duplicated lock code
// between `tools/src/lock.rs` and `src-tauri/src/watcher/fs_watcher.rs`
// into a single canonical definition (probably via a workspace crate
// shared by both) — tracked separately; it can't happen in this PR
// because of the cargo dependency direction
// (`tools -> src-tauri`, so `src-tauri` cannot import from `tools`).
pub use tauri_app_lib::watcher::*;
pub use crate::lock::VaultLock;
