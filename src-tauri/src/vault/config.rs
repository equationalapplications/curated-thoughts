use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::BrainConfig;
use crate::embedder::EmbedProfile;
use crate::retrieval::BrainPaths;
use crate::vault::safe_path::IMMUTABLE_DIR;

/// Migration errors for vault folder structure changes
#[derive(Debug, Clone, thiserror::Error)]
pub enum MigrationError {
    #[error(
        "Both 'documents' and 'immutable-source-files' folders exist. Manual intervention required: move files from '{old}' to '{new}', then restart."
    )]
    BothFoldersExist { old: PathBuf, new: PathBuf },
    #[error("IO error during migration: {0}")]
    Io(String),
}

impl From<std::io::Error> for MigrationError {
    fn from(e: std::io::Error) -> Self {
        MigrationError::Io(e.to_string())
    }
}

pub struct VaultConfig {
    config_path: PathBuf,
}

impl VaultConfig {
    pub fn new(config_path: PathBuf) -> Self {
        VaultConfig { config_path }
    }

    /// Strict load for read-modify-write setters. A MISSING file is a fresh
    /// install and legitimately starts from defaults; an EXISTING file that
    /// fails strict load is an error -- falling back to `BrainConfig::default()`
    /// here would write defaults over every intact section (valid-JSON files
    /// with a bad field type are the dangerous case: `write()`'s merge guard
    /// passes and the clobber silently succeeds). Incident #178.
    fn load_strict_or_fresh(paths: &BrainPaths) -> anyhow::Result<BrainConfig> {
        if paths.config_path.exists() {
            BrainConfig::load(paths)
        } else {
            Ok(BrainConfig::default())
        }
    }

    pub fn default_vault_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(std::env::temp_dir)
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
        let report = BrainConfig::load_lenient(&paths)
            .map_err(|e| anyhow::anyhow!("config.json failed to load: {e}"))?;
        Ok(report.config.vault_path)
    }

    pub fn set_vault_path(&self, path: &str) -> Result<()> {
        let paths = self.brain_paths();
        let mut config = Self::load_strict_or_fresh(&paths)?;
        config.vault_path = Some(path.to_string());
        config.write(&paths)
    }

    #[allow(dead_code)]
    pub fn vault_root(&self) -> anyhow::Result<Option<std::path::PathBuf>> {
        Ok(self.get_vault_path()?.map(std::path::PathBuf::from))
    }

    pub fn get_embed_profile(&self) -> Result<EmbedProfile> {
        let paths = self.brain_paths();
        let report = BrainConfig::load_lenient(&paths)
            .map_err(|e| anyhow::anyhow!("config.json failed to load: {e}"))?;
        // An ABSENT key is a fresh install and legitimately defaults. A LOAD
        // FAILURE is not: it propagates via `?` above and must never reach
        // here. Keeping these two cases visibly distinct is the whole point
        // of this branch -- `unwrap_or_default()` collapsed them.
        match report.config.embed_profile {
            Some(profile) => Ok(profile),
            None => {
                static WARNED: std::sync::Once = std::sync::Once::new();
                let fallback = EmbedProfile::default();
                WARNED.call_once(|| {
                    eprintln!(
                        "[embed] no embed_profile configured; defaulting to {fallback:?}. \
                         Set one in config.json to embed with a different model."
                    );
                });
                Ok(fallback)
            }
        }
    }

    #[allow(dead_code)]
    pub fn set_embed_profile(&self, profile: EmbedProfile) -> Result<()> {
        let paths = self.brain_paths();
        let mut config = Self::load_strict_or_fresh(&paths)?;
        config.embed_profile = Some(profile);
        config.write(&paths)
    }

    pub fn has_migrated_to_v2(&self) -> Result<bool> {
        let paths = self.brain_paths();
        let report = BrainConfig::load_lenient(&paths)
            .map_err(|e| anyhow::anyhow!("config.json failed to load: {e}"))?;
        Ok(report.config.migrated_to_v2)
    }

    pub fn set_migrated_to_v2(&self) -> Result<()> {
        let paths = self.brain_paths();
        let mut config = Self::load_strict_or_fresh(&paths)?;
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
    fn set_vault_path_on_missing_config_creates_it() {
        // Fresh install: no config.json yet. The setter must create it with
        // defaults plus the requested vault path.
        let tmp = TempDir::new().unwrap();
        let cfg = make_config(&tmp);
        cfg.set_vault_path("/fresh/install").unwrap();
        assert_eq!(cfg.get_vault_path().unwrap(), Some("/fresh/install".into()));
    }

    #[test]
    fn set_vault_path_on_unreadable_config_errors_and_leaves_file_untouched() {
        // A config that cannot be parsed must never be silently replaced
        // with defaults: that converts corrupt input into corrupt output --
        // every modeled section resets while unknown keys survive, producing
        // a plausible-looking but hollow config (the 2026-09-03/09-04 live
        // ~/.brain corruption incidents, see issue #178). Fail loudly, leave
        // the evidence on disk.
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.json");
        std::fs::write(&path, "{ not json at all").unwrap();
        let before = std::fs::read(&path).unwrap();

        let cfg = VaultConfig::new(path.clone());
        let result = cfg.set_vault_path("/should/not/happen");
        assert!(
            result.is_err(),
            "must error on unreadable config, got {result:?}"
        );

        let after = std::fs::read(&path).unwrap();
        assert_eq!(before, after, "file must be untouched on failure");
    }

    #[test]
    fn set_migrated_to_v2_on_unreadable_config_errors_and_leaves_file_untouched() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.json");
        std::fs::write(&path, "[]").unwrap(); // non-object root: strict load fails
        let before = std::fs::read(&path).unwrap();

        let cfg = VaultConfig::new(path.clone());
        assert!(cfg.set_migrated_to_v2().is_err());

        assert_eq!(before, std::fs::read(&path).unwrap());
    }

    #[test]
    fn set_vault_path_never_clobbers_when_strict_load_fails_on_existing_file() {
        // The subtle case: file is VALID JSON (so `write()`'s merge guard
        // passes) but strict `load()` rejects a field (`vault_path` must be
        // a string). `unwrap_or_else(BrainConfig::default)` would then write
        // defaults over every intact modeled section -- the live-corruption
        // shape from issue #178. The setter must fail instead, touching
        // nothing.
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.json");
        let valuable = r#"{
  "vault_path": 123,
  "generation": {
    "provider": "external",
    "external_url": "https://api.z.ai/api/coding/paas/v4",
    "model_name": "glm-5.3-flash"
  }
}"#;
        std::fs::write(&path, valuable).unwrap();

        let cfg = VaultConfig::new(path.clone());
        let result = cfg.set_vault_path("/should/not/happen");
        assert!(result.is_err(), "must error, got {result:?}");

        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            after, valuable,
            "config must be byte-identical after failure"
        );
    }

    #[test]
    fn set_vault_path_overwrites_existing_path() {
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
    fn absent_embed_profile_defaults_to_local_and_is_explicit() {
        // A fresh install has no embed_profile key. Defaulting is correct
        // behavior; what must not happen is defaulting on a LOAD FAILURE
        // (see load_failure_never_yields_local_profile).
        let cfg = make_config(&TempDir::new().unwrap());
        let prof = cfg.get_embed_profile().expect("absent key must not error");
        assert_eq!(prof, EmbedProfile::default());
        assert!(
            matches!(prof, EmbedProfile::Local { .. }),
            "default must still be the documented Local profile"
        );
    }

    #[test]
    fn load_failure_never_yields_local_profile() {
        // A config that cannot be parsed must surface as an error. Silently
        // reinterpreting it as the Local/Ollama default would embed new
        // content with a different model than the existing corpus, which
        // corrupts similarity scores across the whole index with no error.
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.json");
        std::fs::write(&path, "{ not json at all").unwrap();
        let cfg = VaultConfig::new(path);

        let result = cfg.get_embed_profile();
        assert!(
            result.is_err(),
            "malformed config must error, got {result:?}"
        );
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
