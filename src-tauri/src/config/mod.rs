use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use crate::embedder::EmbedProfile;
pub use crate::inference::config::{GenerationConfig, EmbeddingConfig};
use crate::privacy::PrivacyConfig;
use crate::retrieval::BrainPaths;
use std::fs;

/// Unified configuration for a brain directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrainConfig {
    /// User's vault root path (e.g., ~/Curated-Thoughts).
    pub vault_path: Option<String>,
    /// Embedding model profile (local Ollama or external).
    pub embed_profile: Option<EmbedProfile>,
    /// Whether the vault has migrated to v2 (immutable-source-files folder structure).
    pub migrated_to_v2: bool,
    /// LLM generation config (model, provider, base_url).
    pub generation: GenerationConfig,
    /// Embedding config (model, provider, base_url).
    pub embedding: EmbeddingConfig,
    /// Privacy mode and settings.
    pub privacy: PrivacyConfig,
}

impl Default for BrainConfig {
    fn default() -> Self {
        BrainConfig {
            vault_path: None,
            embed_profile: None,
            migrated_to_v2: false,
            generation: GenerationConfig::default(),
            embedding: EmbeddingConfig::default(),
            privacy: PrivacyConfig::default(),
        }
    }
}

/// Report from lenient load, detailing which fields were silently defaulted.
#[derive(Debug, Clone)]
pub struct LoadReport {
    /// The successfully loaded (or partially defaulted) config.
    pub config: BrainConfig,
    /// One entry per silently-defaulted field.
    pub diagnostics: Vec<String>,
    /// True if generation block was missing and filled by leniency.
    pub generation_missing: bool,
    /// True if embedding block was missing and filled by leniency.
    pub embedding_missing: bool,
    /// True if vault_path was missing.
    pub vault_path_missing: bool,
    /// True if privacy block was missing and filled by leniency.
    pub privacy_missing: bool,
}

impl BrainConfig {
    /// Load config from disk with no leniency. Malformed top-level JSON is fatal.
    /// Missing or unparseable vault_path is fatal (never masked).
    /// Returns an error if config.json does not exist.
    pub fn load(paths: &BrainPaths) -> Result<BrainConfig> {
        let text = fs::read_to_string(&paths.config_path)?;
        let value: serde_json::Value = serde_json::from_str(&text)?;

        // Strict: attempt direct deserialization. No fallback.
        match serde_json::from_value::<BrainConfig>(value) {
            Ok(cfg) => Ok(cfg),
            Err(e) => bail!("Config deserialize error: {}", e),
        }
    }

    /// Load config from disk with per-field leniency.
    /// Malformed top-level JSON is fatal and returned in the report.
    /// Missing or unparseable fields (except vault_path) are dropped to defaults.
    pub fn load_lenient(paths: &BrainPaths) -> LoadReport {
        let mut report = LoadReport {
            config: BrainConfig::default(),
            diagnostics: vec![],
            generation_missing: false,
            embedding_missing: false,
            vault_path_missing: false,
            privacy_missing: false,
        };

        let text = match fs::read_to_string(&paths.config_path) {
            Ok(t) => t,
            Err(e) => {
                report.diagnostics.push(format!("config.json not found: {}", e));
                report.generation_missing = true;
                report.embedding_missing = true;
                report.vault_path_missing = true;
                report.privacy_missing = true;
                return report;
            }
        };

        let value: serde_json::Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                report.diagnostics
                    .push(format!("malformed JSON: {}", e));
                return report;
            }
        };

        let obj = match value.as_object() {
            Some(o) => o,
            None => {
                report.diagnostics.push("root is not a JSON object".to_string());
                return report;
            }
        };

        // vault_path: hard error if present but not a string
        if let Some(vp) = obj.get("vault_path") {
            match vp.as_str() {
                Some(s) => report.config.vault_path = Some(s.to_string()),
                None if !vp.is_null() => {
                    report
                        .diagnostics
                        .push("vault_path present but not a string: hard error".to_string());
                    return report;
                }
                _ => report.vault_path_missing = true,
            }
        } else {
            report.vault_path_missing = true;
        }

        // embed_profile: lenient, drops unparseable variants
        if let Some(ep) = obj.get("embed_profile") {
            match serde_json::from_value::<EmbedProfile>(ep.clone()) {
                Ok(p) => report.config.embed_profile = Some(p),
                Err(_) => {
                    report
                        .diagnostics
                        .push("embed_profile unparseable, using default".to_string());
                }
            }
        }

        // migrated_to_v2: lenient
        if let Some(m) = obj.get("migrated_to_v2") {
            if let Some(b) = m.as_bool() {
                report.config.migrated_to_v2 = b;
            } else {
                report
                    .diagnostics
                    .push("migrated_to_v2 not a bool, using false".to_string());
            }
        }

        // generation: complete block must round-trip, or missing
        if let Some(gen) = obj.get("generation") {
            match serde_json::from_value::<GenerationConfig>(gen.clone()) {
                Ok(g) => report.config.generation = g,
                Err(e) => {
                    report
                        .diagnostics
                        .push(format!("generation block unparseable: {}", e));
                    report.generation_missing = true;
                }
            }
        } else {
            report.generation_missing = true;
        }

        // embedding: complete block must round-trip, or missing
        if let Some(emb) = obj.get("embedding") {
            match serde_json::from_value::<EmbeddingConfig>(emb.clone()) {
                Ok(e) => report.config.embedding = e,
                Err(e) => {
                    report
                        .diagnostics
                        .push(format!("embedding block unparseable: {}", e));
                    report.embedding_missing = true;
                }
            }
        } else {
            report.embedding_missing = true;
        }

        // privacy: complete block must round-trip, or missing
        if let Some(priv_) = obj.get("privacy") {
            match serde_json::from_value::<PrivacyConfig>(priv_.clone()) {
                Ok(p) => report.config.privacy = p,
                Err(e) => {
                    report
                        .diagnostics
                        .push(format!("privacy block unparseable: {}", e));
                    report.privacy_missing = true;
                }
            }
        } else {
            report.privacy_missing = true;
        }

        report
    }
}
