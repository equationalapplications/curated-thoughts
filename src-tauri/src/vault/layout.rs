//! Vault filesystem layout creation.
//!
//! Creates the canonical Curated-Thoughts directory structure:

use std::path::Path;
use anyhow::Result;

/// Creates the standard Curated-Thoughts vault layout under `vault_root`.
///
/// Callers: `set_vault_path` (lib.rs) and `onboard::run_onboard`.
pub fn create_vault_layout(vault_root: &Path) -> Result<()> {
    // v2 layout: migrate a v1 vault (documents/ → immutable-source-files/)
    // BEFORE creating subdirs, otherwise both folders would exist and the
    // startup migration would block with BothFoldersExist.
    crate::vault::config::migrate_vault(vault_root)?;

    for subdir in &[
        crate::vault::safe_path::IMMUTABLE_DIR,
        crate::vault::safe_path::WIKI_DIR,
    ] {
        std::fs::create_dir_all(vault_root.join(subdir))?;
    }

    std::fs::create_dir_all(vault_root.join(crate::vault::safe_path::AGENTS_DEPOSIT_DIR))?;

    std::fs::create_dir_all(vault_root.join(".brain").join("converted"))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn creates_all_required_directories() {
        let temp = TempDir::new().unwrap();
        let vault = temp.path();

        create_vault_layout(vault).unwrap();

        assert!(vault.join("immutable-source-files").is_dir());
        assert!(vault.join("wiki").is_dir());
        assert!(vault.join("immutable-source-files/agents").is_dir());
        assert!(vault.join(".brain/converted").is_dir());
    }

    #[test]
    fn is_idempotent() {
        let temp = TempDir::new().unwrap();
        let vault = temp.path();

        create_vault_layout(vault).unwrap();
        create_vault_layout(vault).unwrap(); // must not fail

        assert!(vault.join("wiki").is_dir());
    }
}
