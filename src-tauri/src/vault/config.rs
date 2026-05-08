use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

use crate::embedder::EmbedProfile;

#[derive(Serialize, Deserialize, Default)]
struct ConfigFile {
    vault_path: Option<String>,
    #[serde(default)]
    embed_profile: Option<EmbedProfile>,
}

pub struct VaultConfig {
    config_path: PathBuf,
}

impl VaultConfig {
    pub fn new(config_path: PathBuf) -> Self {
        VaultConfig { config_path }
    }

    #[allow(dead_code)]
    pub fn default_path() -> PathBuf {
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
        Ok(serde_json::from_str(&text)?)
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
