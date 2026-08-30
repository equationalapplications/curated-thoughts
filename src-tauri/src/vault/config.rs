use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::BrainConfig;
use crate::embedder::EmbedProfile;
use crate::retrieval::BrainPaths;
use crate::vault::safe_path::IMMUTABLE_DIR;

/// Migration errors for vault folder structure changes
#[derive(Debug, Clone, thiserror::Error)]
pub enum MigrationError {
    #[error("Both 'documents' and 'immutable-source-files' folders exist. Manual intervention required: move files from '{old}' to '{new}', then restart.")]
    BothFoldersExist { old: PathBuf, new: PathBuf },
    #[error("IO error during migration: {0}")]
    Io(String),
}

impl From<std::io::Error> for MigrationError {
    fn from(e: std::io::Error) -> Self {
        MigrationError::Io(e.to_string())
    }
}

#[derive(Deserialize, Serialize, Default)]
struct ConfigFile {
    vault_path: Option<String>,
    #[serde(default)]
    embed_profile: Option<EmbedProfile>,
    #[serde(default)]
    migrated_to_v2: bool,
}

impl ConfigFile {
    /// Lenient read: a config file whose `embed_profile` field uses an
    /// unrecognized variant (e.g. `external` written by an older schema) must
    /// not poison the whole file — that silently reset the vault path and
    /// forced users back through onboarding. Leniency applies ONLY to
    /// `embed_profile`: it is dropped (falling back to the default profile)
    /// when it fails to parse. Malformed JSON or an invalid `vault_path` is a
    /// real error and propagates.
    fn from_text(text: &str) -> Result<Self> {
        match serde_json::from_str::<ConfigFile>(text) {
            Ok(cfg) => Ok(cfg),
            Err(first_err) => {
                // Retry with embed_profile removed; only tolerate failure that
                // is attributable to embed_profile itself.
                let mut value: serde_json::Value = serde_json::from_str(text)?;
                if let Some(obj) = value.as_object_mut() {
                    obj.remove("embed_profile");
                }
                let cfg: ConfigFile = serde_json::from_value(value)?;
                if cfg.vault_path.is_none() && text.contains("\"vault_path\"") {
                    // vault_path was present but unparseable — do not mask it.
                    return Err(anyhow::anyhow!(first_err));
                }
                Ok(cfg)
            }
        }
    }
}

pub struct VaultConfig {
    config_path: PathBuf,
}

impl VaultConfig {
    pub fn new(config_path: PathBuf) -> Self {
        VaultConfig { config_path }
    }

    pub fn default_vault_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| std::env::temp_dir())
            .join("Curated-Thoughts")
    }

    pub fn default_config_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_default()
            .join(".brain")
            .join("config.json")
    }

    /// Build a BrainPaths that points at our own config_path, using the same
    /// directory layout conventions as resolve_brain_paths().
    fn brain_paths(&self) -> BrainPaths {
        let brain_dir = self
            .config_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        BrainPaths {
            brain_dir: brain_dir.clone(),
            config_path: self.config_path.clone(),
            db_path: brain_dir.join("brain.db"),
        }
    }

    pub fn get_vault_path(&self) -> Result<Option<String>> {
        let paths = self.brain_paths();
        let report = BrainConfig::load_lenient(&paths);
        Ok(report.config.vault_path)
    }

    pub fn set_vault_path(&self, path: &str) -> Result<()> {
        let paths = self.brain_paths();
        let mut config = BrainConfig::load(&paths)
            .unwrap_or_else(|_| BrainConfig::default());
        config.vault_path = Some(path.to_string());
        config.write(&paths)
    }

    #[allow(dead_code)]
    pub fn vault_root(&self) -> anyhow::Result<Option<std::path::PathBuf>> {
        Ok(self.get_vault_path()?.map(std::path::PathBuf::from))
    }

    pub fn get_embed_profile(&self) -> Result<EmbedProfile> {
        let paths = self.brain_paths();
        let report = BrainConfig::load_lenient(&paths);
        Ok(report.config.embed_profile.unwrap_or_default())
    }

    #[allow(dead_code)]
    pub fn set_embed_profile(&self, profile: EmbedProfile) -> Result<()> {
        let paths = self.brain_paths();
        let mut config = BrainConfig::load(&paths)
            .unwrap_or_else(|_| BrainConfig::default());
        config.embed_profile = Some(profile);
        config.write(&paths)
    }

    pub fn has_migrated_to_v2(&self) -> Result<bool> {
        let paths = self.brain_paths();
        let report = BrainConfig::load_lenient(&paths);
        Ok(report.config.migrated_to_v2)
    }

    pub fn set_migrated_to_v2(&self) -> Result<()> {
        let paths = self.brain_paths();
        let mut config = BrainConfig::load(&paths)
            .unwrap_or_else(|_| BrainConfig::default());
        config.migrated_to_v2 = true;
        config.write(&paths)
    }
}

/// Migrate vault folder structure from v1 (documents/) to v2 (immutable-source-files/)
pub fn migrate_vault(vault_root: &Path) -> Result<(), MigrationError> {
    let old = vault_root.join("documents");
    let new = vault_root.join(IMMUTABLE_DIR);

    match (old.exists(), new.exists()) {
        (true, false) => {
            // Normal migration: rename documents to immutable-source-files
            fs::rename(&old, &new)?;
            Ok(())
        }
        (false, _) => {
            // Idempotent: nothing to migrate (either already migrated or fresh vault)
            Ok(())
        }
        (true, true) => {
            // Both folders exist - user needs to resolve manually
            Err(MigrationError::BothFoldersExist { old, new })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedder::{CloudProvider, EmbedProfile};
    use tempfile::TempDir;

    fn make_config(tmp: &TempDir) -> VaultConfig {
        VaultConfig::new(tmp.path().join("config.json"))
    }

    #[test]
    fn test_default_vault_path_ends_with_curated_thoughts() {
        let p = VaultConfig::default_vault_path();
        assert_eq!(p.file_name().unwrap().to_str().unwrap(), "Curated-Thoughts");
        assert!(p.is_absolute());
    }

    #[test]
    fn test_get_returns_none_when_file_absent() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_config(&tmp);
        assert_eq!(cfg.get_vault_path().unwrap(), None);
    }

    #[test]
    fn test_set_then_get_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_config(&tmp);
        cfg.set_vault_path("/Users/test/brain").unwrap();
        assert_eq!(
            cfg.get_vault_path().unwrap(),
            Some("/Users/test/brain".to_string())
        );
    }

    #[test]
    fn test_set_overwrites_existing_path() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_config(&tmp);
        cfg.set_vault_path("/first").unwrap();
        cfg.set_vault_path("/second").unwrap();
        assert_eq!(cfg.get_vault_path().unwrap(), Some("/second".to_string()));
    }

    #[test]
    fn test_vault_root_returns_none_when_unset() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_config(&tmp);
        assert!(cfg.vault_root().unwrap().is_none());
    }

    #[test]
    fn test_vault_root_returns_path_when_set() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_config(&tmp);
        cfg.set_vault_path("/vault/root").unwrap();
        assert_eq!(
            cfg.vault_root().unwrap(),
            Some(std::path::PathBuf::from("/vault/root"))
        );
    }

    #[test]
    fn legacy_embed_profile_variant_does_not_poison_config() {
        // Written by an older schema (`external` embed variant). The whole file
        // used to fail deserialization, resetting the vault path and re-triggering
        // onboarding. Now only the embed profile is dropped.
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
  "embed_profile": { "type": "external", "base_url": "https://example.test/v1", "model": "text-embedding-3-small" },
  "vault_path": "/home/tester/vault"
}"#,
        )
        .unwrap();
        let cfg = VaultConfig::new(path);
        assert_eq!(
            cfg.get_vault_path().unwrap(),
            Some("/home/tester/vault".to_string())
        );
        // `external` is now a REAL variant (was legacy/unsupported when this
        // test was written), so it round-trips instead of falling back.
        let prof = cfg.get_embed_profile().unwrap();
        assert!(matches!(prof, EmbedProfile::External { .. }));
    }

    #[test]
    fn malformed_json_is_an_error() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.json");
        std::fs::write(&path, "{ not json at all").unwrap();
        let cfg = VaultConfig::new(path);
        assert!(cfg.get_vault_path().is_err());
    }

    #[test]
    fn invalid_vault_path_type_is_an_error() {
        // vault_path: 42 must propagate as an error, not silently reset config.
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.json");
        std::fs::write(&path, r#"{"vault_path": 42}"#).unwrap();
        let cfg = VaultConfig::new(path);
        assert!(cfg.get_vault_path().is_err());
    }

    #[test]
    fn embed_profile_defaults_when_absent() {
        let cfg = make_config(&TempDir::new().unwrap());
        assert_eq!(cfg.get_embed_profile().unwrap(), EmbedProfile::default());
    }

    #[test]
    fn embed_profile_roundtrip_local() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_config(&tmp);
        let p = EmbedProfile::Local { model: "mx".into() };
        cfg.set_embed_profile(p.clone()).unwrap();
        assert_eq!(cfg.get_embed_profile().unwrap(), p);
    }

    #[test]
    fn embed_profile_roundtrip_cloud() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_config(&tmp);
        let p = EmbedProfile::Cloud {
            provider: CloudProvider::Cohere,
            model: "x".into(),
            api_key: "abc".into(),
        };
        cfg.set_embed_profile(p.clone()).unwrap();
        assert_eq!(cfg.get_embed_profile().unwrap(), p);
    }

    #[test]
    fn migration_success_when_old_exists_new_does_not() {
        let tmp = TempDir::new().unwrap();
        let vault = tmp.path().join("vault");
        let old_dir = vault.join("documents");
        let new_dir = vault.join(IMMUTABLE_DIR);
        fs::create_dir_all(&old_dir).unwrap();
        fs::write(old_dir.join("test.txt"), b"test").unwrap();

        assert!(old_dir.exists());
        assert!(!new_dir.exists());

        migrate_vault(&vault).unwrap();

        assert!(!old_dir.exists());
        assert!(new_dir.exists());
        assert!(new_dir.join("test.txt").exists());
    }

    #[test]
    fn migration_idempotent_when_old_does_not_exist() {
        let tmp = TempDir::new().unwrap();
        let vault = tmp.path().join("vault");
        fs::create_dir_all(&vault).unwrap();

        // Fresh vault with neither folder
        migrate_vault(&vault).unwrap();

        // Already migrated vault
        fs::create_dir_all(vault.join(IMMUTABLE_DIR)).unwrap();
        migrate_vault(&vault).unwrap();
    }

    #[test]
    fn migration_fails_when_both_folders_exist() {
        let tmp = TempDir::new().unwrap();
        let vault = tmp.path().join("vault");
        let old_dir = vault.join("documents");
        let new_dir = vault.join(IMMUTABLE_DIR);
        
        fs::create_dir_all(&old_dir).unwrap();
        fs::write(old_dir.join("old.txt"), b"old").unwrap();
        fs::create_dir_all(&new_dir).unwrap();
        fs::write(new_dir.join("new.txt"), b"new").unwrap();

        let result = migrate_vault(&vault);
        assert!(result.is_err());
        match result {
            Err(MigrationError::BothFoldersExist { old, new }) => {
                assert_eq!(old, old_dir);
                assert_eq!(new, new_dir);
            }
            _ => panic!("Expected BothFoldersExist error"),
        }

        // Both folders should still exist
        assert!(old_dir.exists());
        assert!(new_dir.exists());
        assert!(old_dir.join("old.txt").exists());
        assert!(new_dir.join("new.txt").exists());
    }
}
