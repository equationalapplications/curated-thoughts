mod ollama;

pub use ollama::OllamaEmbedder;

use anyhow::{anyhow, Result};
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use serde::{Deserialize, Serialize};

#[derive(Clone, Eq, Serialize, Deserialize, PartialEq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum CloudProvider {
    OpenAi,
    Voyage,
    Cohere,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EmbedProfile {
    Local { model: String },
    Cloud {
        provider: CloudProvider,
        model: String,
        #[serde(default)]
        api_key: String,
    },
}

impl Default for EmbedProfile {
    fn default() -> Self {
        EmbedProfile::Local {
            model: "nomic-embed-code".to_string(),
        }
    }
}

/// When env `CURATED_EMBED_STUB=constant8`, returns tiny deterministic vectors (pipeline integration tests / CI).
/// Bench fixtures (`tests/scifact.rs`, etc.) still load frozen FastEmbed vectors — do not point those at Ollama.
pub fn embed_batch(profile: &EmbedProfile, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
    if matches!(
        std::env::var("CURATED_EMBED_STUB").ok().as_deref(),
        Some("constant8")
    ) {
        let mut out = Vec::with_capacity(texts.len());
        for i in 0..texts.len() {
            let mut v = vec![0_f32; 8];
            v[0] = (i + 1) as f32 * 1e-4;
            out.push(v);
        }
        return Ok(out);
    }
    match profile {
        EmbedProfile::Local { model } => {
            let o = OllamaEmbedder::new_local(model.as_str());
            o.embed(texts)
        }
        EmbedProfile::Cloud { .. } => Err(anyhow!("cloud embed not implemented")),
    }
}

pub fn embed_one(profile: &EmbedProfile, text: String) -> Result<Vec<f32>> {
    Ok(embed_batch(profile, vec![text])?
        .into_iter()
        .next()
        .unwrap_or_default())
}

/// FastEmbed (MiniLM-L6-V2): kept for reproducible benchmarks and frozen fixtures (`tests/` benches).
pub struct Embedder {
    model: TextEmbedding,
}

impl Embedder {
    pub fn new() -> Result<Self> {
        let model = TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::AllMiniLML6V2).with_show_download_progress(false),
        )?;
        Ok(Embedder { model })
    }

    pub fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        self.model.embed(texts, None)
    }

    pub fn dimensions() -> usize {
        384
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embed_cloud_errors() {
        let p = EmbedProfile::Cloud {
            provider: CloudProvider::OpenAi,
            model: "x".to_string(),
            api_key: "k".to_string(),
        };
        assert!(embed_batch(&p, vec!["a".into()]).is_err());
    }
}
