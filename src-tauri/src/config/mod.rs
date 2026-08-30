use serde::{Deserialize, Serialize};
use crate::embedder::EmbedProfile;
use crate::inference::config::{GenerationConfig, EmbeddingConfig};
use crate::privacy::PrivacyConfig;

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
