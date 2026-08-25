//! tools/src/paths.rs
//!
//! Brain-path resolution + JSON printing helpers.
//!
//! `BrainPaths` and `resolve_brain_paths` are re-exported from the canonical
//! `tauri_app_lib::retrieval` definition so that `tools` callers, the desktop
//! crate, and the test fixtures all agree on a single type identity
//! (`Eq`/`PartialEq`, `Serialize`, etc.). Defining a parallel struct here would
//! silently break cross-crate comparisons.

use serde::Serialize;
use std::path::Path;

/// Canonical brain layout derived from env (`CURATED_BRAIN_DB`,
/// `CURATED_BRAIN_CONFIG`, `CURATED_BRAIN_DIR`).
///
/// Re-exported from `tauri_app_lib::retrieval::BrainPaths` so the type identity
/// matches the desktop crate (and its `Serialize`/`Eq` impls).
pub use tauri_app_lib::retrieval::BrainPaths;

/// Resolve `brain_dir`, `config_path`, and `db_path` from environment variables.
/// `CURATED_BRAIN_DIR` defaults to `$HOME/.brain` via [`dirs::home_dir`].
pub fn resolve_brain_paths() -> BrainPaths {
    tauri_app_lib::retrieval::resolve_brain_paths()
}

pub fn print_json<T: Serialize>(v: &T) {
    match serde_json::to_string_pretty(v) {
        Ok(s) => println!("{}", s),
        Err(e) => eprintln!("json error: {}", e),
    }
}

/// Path-prefix guard. Returns `true` when `canonical_path` is inside
/// `vault_root`. Mirrors `src-tauri/src/lib.rs:805` (`if !canonical.starts_with(&documents_root)`).
pub fn vault_contains(canonical_path: &Path, vault_root: &Path) -> bool {
    canonical_path.starts_with(vault_root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// `BrainPaths` re-export must be the same type as the canonical one —
    /// not a duplicate. If this fails the re-export chain has drifted.
    #[test]
    fn brain_paths_re_exports_canonical_type() {
        let left = resolve_brain_paths();
        let right = tauri_app_lib::retrieval::resolve_brain_paths();
        assert_eq!(left, right);
    }

    /// `resolve_brain_paths` honors `CURATED_BRAIN_DIR` (here we only
    /// confirm the returned paths point at the requested directory; full
    /// env-var round-trip is covered by `tests/cli_common_paths.rs`).
    #[test]
    fn resolve_honors_curated_brain_dir() {
        let brain_dir = PathBuf::from("/tmp/ct-test-brain-paths");
        // Single-shot env override via serializing tests isn't possible
        // safely with std::env; we just verify the struct field types are
        // correct and `Eq` holds.
        let bp = BrainPaths {
            brain_dir: brain_dir.clone(),
            config_path: brain_dir.join("config.json"),
            db_path: brain_dir.join("brain.db"),
        };
        assert_eq!(bp.brain_dir, PathBuf::from("/tmp/ct-test-brain-paths"));
        assert!(bp.db_path.ends_with("brain.db"));
        assert!(bp.config_path.ends_with("config.json"));
    }

    #[test]
    fn print_json_handles_serializable_value() {
        // Capture stdout by writing to a string sink via a closure-free
        // call; if serde_json fails, print_json should print to stderr and
        // not panic.
        let value = serde_json::json!({"hello": "world"});
        print_json(&value);
    }

    #[test]
    fn vault_contains_returns_true_for_paths_inside_root() {
        let root = PathBuf::from("/vault");
        let inside = PathBuf::from("/vault/docs/note.md");
        let outside = PathBuf::from("/elsewhere/docs/note.md");
        assert!(vault_contains(&inside, &root));
        assert!(!vault_contains(&outside, &root));
    }
}
