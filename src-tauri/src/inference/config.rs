use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum GenerationProviderKind {
    Unconfigured,
    Sidecar,
    External,
}

impl Default for GenerationProviderKind {
    fn default() -> Self {
        GenerationProviderKind::Unconfigured
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GenerationConfig {
    pub provider: GenerationProviderKind,
    pub model_path: Option<String>,
    pub model_name: Option<String>,
    pub external_url: Option<String>,
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingProviderKind {
    #[default]
    Fastembed,
    External,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EmbeddingConfig {
    pub provider: EmbeddingProviderKind,
    pub external_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LlmConfig {
    pub generation: GenerationConfig,
    pub embedding: EmbeddingConfig,
}

pub fn config_path(brain_dir: &Path) -> PathBuf {
    if let Ok(p) = std::env::var("CURATED_BRAIN_CONFIG") {
        return PathBuf::from(p);
    }
    brain_dir.join("config.json")
}

pub fn read_config(brain_dir: &Path) -> LlmConfig {
    let path = config_path(brain_dir);
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return LlmConfig::default();
    };
    serde_json::from_str(&contents).unwrap_or_default()
}

pub fn write_config(brain_dir: &Path, config: &LlmConfig) -> Result<()> {
    let path = config_path(brain_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(config)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

pub fn resolve_model_path(brain_dir: &Path, relative: &str) -> PathBuf {
    brain_dir.join(relative)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn round_trip_default_config() {
        let dir = TempDir::new().unwrap();
        let config = LlmConfig::default();
        write_config(dir.path(), &config).unwrap();
        let loaded = read_config(dir.path());
        assert_eq!(loaded.generation.provider, GenerationProviderKind::Unconfigured);
        assert_eq!(loaded.embedding.provider, EmbeddingProviderKind::Fastembed);
    }

    #[test]
    fn round_trip_sidecar_config() {
        let dir = TempDir::new().unwrap();
        let config = LlmConfig {
            generation: GenerationConfig {
                provider: GenerationProviderKind::Sidecar,
                model_path: Some("models/llama-3.2-3b.gguf".to_string()),
                model_name: None,
                external_url: None,
                api_key: None,
            },
            embedding: EmbeddingConfig::default(),
        };
        write_config(dir.path(), &config).unwrap();
        let loaded = read_config(dir.path());
        assert_eq!(loaded.generation.provider, GenerationProviderKind::Sidecar);
        assert_eq!(loaded.generation.model_path.as_deref(), Some("models/llama-3.2-3b.gguf"));
    }

    #[test]
    fn relative_model_path_joins_with_brain_dir() {
        let brain = Path::new("/home/user/.brain");
        let abs = resolve_model_path(brain, "models/llama-3.2-3b.gguf");
        assert_eq!(abs, PathBuf::from("/home/user/.brain/models/llama-3.2-3b.gguf"));
    }

    #[test]
    fn missing_config_returns_default() {
        let dir = TempDir::new().unwrap();
        let loaded = read_config(dir.path());
        assert_eq!(loaded.generation.provider, GenerationProviderKind::Unconfigured);
    }

    #[test]
    fn write_config_is_atomic_tmp_then_rename() {
        let dir = TempDir::new().unwrap();
        let config = LlmConfig::default();
        write_config(dir.path(), &config).unwrap();
        let tmp = config_path(dir.path()).with_extension("json.tmp");
        assert!(!tmp.exists(), ".tmp file should be cleaned up by rename");
        assert!(config_path(dir.path()).exists(), "config.json must exist");
    }
}
