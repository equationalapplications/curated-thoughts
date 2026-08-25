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
    /// Per-request LLM timeout in seconds (librarian synthesis HTTP client).
    /// `None` preserves the historical 600s default.
    #[serde(default)]
    pub timeout_secs: Option<u64>,
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

    let mut existing = if path.exists() {
        let contents = std::fs::read_to_string(&path)?;
        serde_json::from_str::<serde_json::Value>(&contents)
            .unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    if !existing.is_object() {
        existing = serde_json::json!({});
    }

    let generation = serde_json::to_value(&config.generation)?;
    let embedding = serde_json::to_value(&config.embedding)?;

    let obj = existing.as_object_mut().unwrap();
    obj.insert("generation".to_string(), generation);
    obj.insert("embedding".to_string(), embedding);

    let json = serde_json::to_string_pretty(&existing)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

pub fn resolve_model_path(brain_dir: &Path, relative: &str) -> PathBuf {
    let relative_path = std::path::Path::new(relative);
    if relative_path.is_absolute()
        || relative_path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        brain_dir.join("models").join(
            relative_path
                .file_name()
                .unwrap_or_else(|| std::ffi::OsStr::new("")),
        )
    } else {
        brain_dir.join(relative_path)
    }
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
        assert_eq!(
            loaded.generation.provider,
            GenerationProviderKind::Unconfigured
        );
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
                timeout_secs: None,
            },
            embedding: EmbeddingConfig::default(),
        };
        write_config(dir.path(), &config).unwrap();
        let loaded = read_config(dir.path());
        assert_eq!(loaded.generation.provider, GenerationProviderKind::Sidecar);
        assert_eq!(
            loaded.generation.model_path.as_deref(),
            Some("models/llama-3.2-3b.gguf")
        );
    }

    #[test]
    fn relative_model_path_joins_with_brain_dir() {
        let brain = Path::new("/home/user/.brain");
        let abs = resolve_model_path(brain, "models/llama-3.2-3b.gguf");
        assert_eq!(
            abs,
            PathBuf::from("/home/user/.brain/models/llama-3.2-3b.gguf")
        );
    }

    #[test]
    fn missing_config_returns_default() {
        let dir = TempDir::new().unwrap();
        let loaded = read_config(dir.path());
        assert_eq!(
            loaded.generation.provider,
            GenerationProviderKind::Unconfigured
        );
    }

    #[test]
    fn timeout_secs_defaults_to_none_when_key_absent() {
        let json =
            r#"{"generation": {"provider": "external"}, "embedding": {"provider": "fastembed"}}"#;
        let config: LlmConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.generation.timeout_secs, None);
    }

    #[test]
    fn timeout_secs_parsed_when_present() {
        let json = r#"{"generation": {"provider": "external", "timeout_secs": 90}, "embedding": {"provider": "fastembed"}}"#;
        let config: LlmConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.generation.timeout_secs, Some(90));
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
