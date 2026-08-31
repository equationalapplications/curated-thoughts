use crate::retrieval::BrainPaths;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum GenerationProviderKind {
    #[default]
    Unconfigured,
    Sidecar,
    External,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GenerationConfig {
    #[serde(default)]
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
    #[serde(default)]
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

/// DEPRECATED: use `BrainConfig::load` or `BrainConfig::load_lenient` instead.
/// This function now delegates to the unified loader for compatibility, returning
/// `LlmConfig::default()` if the strict loader fails. New code should use the
/// `crate::config::BrainConfig` API directly.
#[deprecated(
    since = "0.1.0",
    note = "use crate::config::BrainConfig::load or load_lenient instead"
)]
pub fn read_config(brain_dir: &Path) -> LlmConfig {
    // Derive config_path the old way for back-compat
    let cfg_path = config_path(brain_dir);

    // Use unified strict loader; fall back to defaults on any error to preserve
    // historical behavior (the old implementation never surfaced errors).
    match crate::config::BrainConfig::load(&BrainPaths {
        brain_dir: brain_dir.to_path_buf(),
        config_path: cfg_path,
        db_path: brain_dir.join("brain.db"),
    }) {
        Ok(cfg) => LlmConfig {
            generation: cfg.generation,
            embedding: cfg.embedding,
        },
        Err(_) => LlmConfig::default(),
    }
}

/// DEPRECATED: use [`crate::config::BrainConfig::load_lenient`] +
/// [`crate::config::BrainConfig::write`] instead.  This function does not
/// honor `CURATED_BRAIN_DB` / `CURATED_BRAIN_CONFIG` (it derives the path
/// from the brain dir only), uses a fixed temp filename (race-prone), and
/// silently swallows malformed JSON on disk.  It is retained for the
/// historical test suite and any external callers; new code must use the
/// unified loader.
#[deprecated(
    since = "0.1.0",
    note = "use crate::config::BrainConfig::load_lenient + BrainConfig::write instead"
)]
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

    let mut generation = serde_json::to_value(&config.generation)?;
    let embedding = serde_json::to_value(&config.embedding)?;

    // Raw-document merge for the `api_key` field: the panel never writes
    // new credentials, but a legacy plaintext key already on disk must
    // survive a settings save. If the incoming config carries no key
    // (either `null` or an empty/whitespace-only string — matching
    // `update_provider_with_brain_path` in `inference/mod.rs`, which treats
    // blank incoming values as absent), keep whatever was already on disk.
    let incoming_key_is_blank = generation.get("api_key").is_none_or(|v| {
        v.as_str()
            .map(|s| s.trim().is_empty())
            .unwrap_or(v.is_null())
    });
    if incoming_key_is_blank {
        if let Some(existing_key) = existing
            .get("generation")
            .and_then(|g| g.get("api_key"))
            .and_then(|k| k.as_str())
            .filter(|s| !s.is_empty())
        {
            if let Some(gen_obj) = generation.as_object_mut() {
                gen_obj.insert(
                    "api_key".to_string(),
                    serde_json::Value::String(existing_key.to_string()),
                );
            }
        }
    }

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

/// Resolve the chat-completions endpoint for a configured external base URL.
///
/// If the base already ends in a version segment (`/v<digits>`), treat it as
/// the full path prefix and append only `/chat/completions`; otherwise fall
/// back to the historical `/v1/chat/completions` layout.
///
/// Version detection is STRICTLY `/v<digits>`: qualified versions like
/// `/v1.1` or `/v2_beta` deliberately do NOT match and take the legacy
/// `/v1/chat/completions` fallback. No known OpenAI-compatible gateway uses
/// minor/qualified version paths (industry standard is `/v1`, plus Z.AI's
/// `/v4`); if one ever appears, extend the matcher here rather than
/// special-casing call sites.
pub fn resolve_chat_completions_url(base_url: &str) -> String {
    let base = base_url.trim().trim_end_matches('/');
    let base = base.strip_suffix("/v1").unwrap_or(base);
    let versioned = base
        .rsplit_once("/v")
        .filter(|(_, tail)| !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()))
        .is_some();
    if versioned {
        format!("{base}/chat/completions")
    } else {
        format!("{base}/v1/chat/completions")
    }
}

#[cfg(test)]
#[allow(deprecated)]
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

    #[test]
    fn resolve_chat_completions_url_behavior_contract() {
        let cases: &[(&str, &str)] = &[
            // Spec behavior contract table
            (
                "https://api.z.ai/api/coding/paas/v4",
                "https://api.z.ai/api/coding/paas/v4/chat/completions",
            ),
            (
                "https://openrouter.ai/api/v1",
                "https://openrouter.ai/api/v1/chat/completions",
            ),
            (
                "https://openrouter.ai/api/v1/",
                "https://openrouter.ai/api/v1/chat/completions",
            ),
            (
                "http://localhost:11434",
                "http://localhost:11434/v1/chat/completions",
            ),
            (
                "http://localhost:11434/v1",
                "http://localhost:11434/v1/chat/completions",
            ),
            (
                "https://example.com/api/v2",
                "https://example.com/api/v2/chat/completions",
            ),
            (
                "https://example.com/v10",
                "https://example.com/v10/chat/completions",
            ),
            // Multi-digit versions
            (
                "https://example.com/v99",
                "https://example.com/v99/chat/completions",
            ),
            // Single-digit boundary: /v9 versioned
            (
                "https://example.com/v9",
                "https://example.com/v9/chat/completions",
            ),
            // Qualified versions deliberately do NOT match (review round 1)
            (
                "https://example.com/v1.1",
                "https://example.com/v1.1/v1/chat/completions",
            ),
            (
                "https://example.com/v2_beta",
                "https://example.com/v2_beta/v1/chat/completions",
            ),
            // Empty tail and non-digit tail: NOT versioned
            (
                "https://example.com/v",
                "https://example.com/v/v1/chat/completions",
            ),
            (
                "https://example.com/vX",
                "https://example.com/vX/v1/chat/completions",
            ),
            // Whitespace-padded versioned base (round 1) — pins .trim()
            (
                " https://host/api/paas/v4/ ",
                "https://host/api/paas/v4/chat/completions",
            ),
            // Version segment that is NOT trailing — today's behavior preserved (Q3)
            (
                "https://host/v1/models",
                "https://host/v1/models/v1/chat/completions",
            ),
            // Degenerate inputs fall through to the legacy layout
            ("", "/v1/chat/completions"),
            ("   ", "/v1/chat/completions"),
            // Bare /v1 (degenerate relative base, round 1)
            ("/v1", "/v1/chat/completions"),
        ];
        for (input, expected) in cases {
            assert_eq!(
                &resolve_chat_completions_url(input),
                expected,
                "input: {input:?}"
            );
        }
    }
}
