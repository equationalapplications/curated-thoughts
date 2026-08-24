use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

use crate::embedder::EmbedProfile;

#[derive(Deserialize, Serialize, Default)]
struct ConfigFile {
    vault_path: Option<String>,
    #[serde(default)]
    embed_profile: Option<EmbedProfile>,
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

    fn read(&self) -> Result<ConfigFile> {
        if !self.config_path.exists() {
            return Ok(ConfigFile::default());
        }
        let text = fs::read_to_string(&self.config_path)?;
        Ok(ConfigFile::from_text(&text)?)
    }

    fn write(&self, cfg: &ConfigFile) -> Result<()> {
        if let Some(parent) = self.config_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&self.config_path, serde_json::to_string_pretty(cfg)?)?;
        Ok(())
    }

    pub fn get_vault_path(&self) -> Result<Option<String>> {
        Ok(self.read()?.vault_path)
    }

    pub fn set_vault_path(&self, path: &str) -> Result<()> {
        let mut cfg = self.read()?;
        cfg.vault_path = Some(path.to_string());
        self.write(&cfg)
    }

    #[allow(dead_code)]
    pub fn vault_root(&self) -> anyhow::Result<Option<std::path::PathBuf>> {
        Ok(self.get_vault_path()?.map(std::path::PathBuf::from))
    }

    pub fn get_embed_profile(&self) -> Result<EmbedProfile> {
        Ok(self.read()?.embed_profile.unwrap_or_default())
    }

    #[allow(dead_code)]
    pub fn set_embed_profile(&self, profile: EmbedProfile) -> Result<()> {
        let mut cfg = self.read()?;
        cfg.embed_profile = Some(profile);
        self.write(&cfg)
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
        // Written by an unknown/future schema variant. The whole file used to
        // fail deserialization, resetting the vault path and re-triggering
        // onboarding. Now only the embed profile is dropped.
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
  "embed_profile": { "type": "holographic", "model": "xz-9000" },
  "vault_path": "/home/tester/vault"
}"#,
        )
        .unwrap();
        let cfg = VaultConfig::new(path);
        assert_eq!(
            cfg.get_vault_path().unwrap(),
            Some("/home/tester/vault".to_string())
        );
        // The invalid embed_profile is discarded; default profile applies.
        assert_eq!(cfg.get_embed_profile().unwrap(), EmbedProfile::default());
    }

    #[test]
    fn external_embed_profile_round_trips() {
        // `external` is a first-class supported variant (OpenRouter etc.); it
        // must survive config read/write instead of being dropped as legacy.
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
  "embed_profile": { "type": "external", "base_url": "https://openrouter.ai/api/v1", "model": "openai/text-embedding-3-small", "api_key": null },
  "vault_path": "/home/tester/vault"
}"#,
        )
        .unwrap();
        let cfg = VaultConfig::new(path);
        match cfg.get_embed_profile().unwrap() {
            EmbedProfile::External { profile } => {
                assert_eq!(profile.base_url, "https://openrouter.ai/api/v1");
                assert_eq!(profile.model, "openai/text-embedding-3-small");
            }
            other => panic!("expected external profile, got {other:?}"),
        }
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
}
