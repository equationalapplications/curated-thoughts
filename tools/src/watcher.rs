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

pub use tauri_app_lib::watcher::*;

// Re-export the `ct`-side lock so callers can import everything from
// one place: `curated_thoughts_tools::watcher::VaultLock`. The desktop
// crate already exports its own `VaultLock` via
// `tauri_app_lib::watcher::*` above; this one shadows it at the
// `tools` namespace with the standalone variant, which is what `ct
// watch` should use.
pub use crate::lock::VaultLock as CtVaultLock;
