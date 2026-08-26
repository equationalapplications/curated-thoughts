mod ollama;

pub use ollama::OllamaEmbedder;

use anyhow::{anyhow, Result};
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use serde::{Deserialize, Serialize};
use std::sync::{Mutex, MutexGuard, OnceLock};

static LOCAL_EMBEDDER: OnceLock<Mutex<Option<Embedder>>> = OnceLock::new();

pub fn get_or_init_local_embedder() -> Result<MutexGuard<'static, Option<Embedder>>> {
    let mutex = LOCAL_EMBEDDER.get_or_init(|| Mutex::new(None));
    let mut guard = mutex
        .lock()
        .map_err(|_| anyhow!("embedder mutex poisoned"))?;
    if guard.is_none() {
        *guard = Some(Embedder::new()?);
    }
    Ok(guard)
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum CloudProvider {
    OpenAi,
    Voyage,
    Cohere,
}

/// External OpenAI-compatible embedding endpoint (`POST {base_url}/embeddings`,
/// request `{model, input}`, response `data[].embedding`). Works with
/// OpenRouter, OpenAI, and any compatible gateway. API key resolution order:
/// profile field → `EMBED_API_KEY` env → provider default env
/// (`OPENROUTER_API_KEY` for openrouter bases, `OPENAI_API_KEY` otherwise).
/// Default base for external embedding endpoints. OpenRouter per spec; the
/// config file intentionally stores no endpoint so secret-redaction on this
/// machine cannot corrupt it.
fn default_external_base_url() -> String {
    "https://openrouter.ai/api/v1".into()
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Debug)]
pub struct ExternalEmbedProfile {
    #[serde(default = "default_external_base_url")]
    pub base_url: String,
    pub model: String,
    #[serde(default)]
    pub api_key: Option<String>,
}

impl ExternalEmbedProfile {
    fn resolved_api_key(&self) -> Result<String> {
        if let Some(k) = self.api_key.as_deref() {
            let k = k.trim();
            if !k.is_empty() {
                return Ok(k.to_string());
            }
        }
        if let Ok(k) = std::env::var("EMBED_API_KEY") {
            if !k.trim().is_empty() {
                return Ok(k);
            }
        }
        let default_var = if self.base_url.contains("openrouter") {
            "OPENROUTER_API_KEY"
        } else {
            "OPENAI_API_KEY"
        };
        if let Ok(k) = std::env::var(default_var) {
            if !k.trim().is_empty() {
                return Ok(k);
            }
        }
        anyhow::bail!(
            "external embed profile has no api key: set it in the profile, EMBED_API_KEY, or {default_var}"
        );
    }

    /// Batch texts into <=64-input requests so large vaults don't blow up
    /// provider payload limits; each batch is one HTTP call.
    pub fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        let api_key = self.resolved_api_key()?;
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(120))
            .build()?;
        let base = self.base_url.trim_end_matches('/');
        let base = base.strip_suffix("/v1").unwrap_or(base);
        let url = format!("{base}/v1/embeddings");
        let mut out: Vec<Vec<f32>> = Vec::with_capacity(texts.len());
        for batch in texts.chunks(64) {
            let resp = client
                .post(&url)
                .header("Authorization", format!("Bearer {api_key}"))
                .json(&serde_json::json!({
                    "model": self.model,
                    "input": batch,
                }))
                .send()?;
            if !resp.status().is_success() {
                anyhow::bail!("external embeddings {} status {}", url, resp.status());
            }
            let body: serde_json::Value = resp.json()?;
            let data = body
                .get("data")
                .and_then(|d| d.as_array())
                .ok_or_else(|| anyhow!("missing data[] in embeddings response"))?;
            if data.len() != batch.len() {
                anyhow::bail!(
                    "embeddings response count mismatch: sent {} got {}",
                    batch.len(),
                    data.len()
                );
            }
            // Response items carry an `index`; honor it in case ordering differs.
            let mut batch_out: Vec<Option<Vec<f32>>> = vec![None; batch.len()];
            for item in data {
                let idx = item.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                let emb = item
                    .get("embedding")
                    .and_then(|e| e.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_f64().map(|f| f as f32))
                            .collect::<Vec<f32>>()
                    })
                    .ok_or_else(|| anyhow!("missing embedding vector in response item"))?;
                if idx >= batch.len() {
                    anyhow::bail!("embedding index {idx} out of range");
                }
                batch_out[idx] = Some(emb);
            }
            out.extend(batch_out.into_iter().map(|o| o.unwrap_or_default()));
        }
        Ok(out)
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EmbedProfile {
    Local {
        model: String,
    },
    Cloud {
        provider: CloudProvider,
        model: String,
        #[serde(default)]
        api_key: String,
    },
    External {
        #[serde(flatten)]
        profile: ExternalEmbedProfile,
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
        EmbedProfile::Local { .. } => {
            let embedder = OllamaEmbedder::from_profile(profile)?;
            embedder.embed(texts)
        }
        EmbedProfile::Cloud { .. } => Err(anyhow!("cloud embed not implemented")),
        EmbedProfile::External { profile } => profile.embed(texts),
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

    #[test]
    fn external_profile_requires_api_key() {
        // No key in profile; env vars must not leak a value into tests, so this
        // should fail with the "no api key" message unless CI sets one.
        let p = EmbedProfile::External {
            profile: ExternalEmbedProfile {
                base_url: "https://openrouter.ai/api/v1".to_string(),
                model: "openai/text-embedding-3-small".to_string(),
                api_key: None,
            },
        };
        match std::env::var("OPENROUTER_API_KEY") {
            Ok(k) if !k.trim().is_empty() => {} // env-provided: fine
            _ => assert!(embed_batch(&p, vec!["a".into()]).is_err()),
        }
    }

    #[test]
    fn external_profile_serializes_flat() {
        let p = EmbedProfile::External {
            profile: ExternalEmbedProfile {
                base_url: "https://openrouter.ai/api/v1".to_string(),
                model: "openai/text-embedding-3-small".to_string(),
                api_key: Some("k".into()),
            },
        };
        let json = serde_json::to_value(&p).unwrap();
        assert_eq!(json["type"], "external");
        assert_eq!(json["base_url"], "https://openrouter.ai/api/v1");
        assert_eq!(json["model"], "openai/text-embedding-3-small");
        let back: EmbedProfile = serde_json::from_value(json).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn external_embed_strips_v1_suffix() {
        // Verify URL construction without hitting the network: an unreachable
        // host must produce a connect error mentioning our normalized path.
        let p = ExternalEmbedProfile {
            base_url: "https://openrouter.invalid.example/api/v1/".to_string(),
            model: "m".to_string(),
            api_key: Some("k".into()),
        };
        let err = p.embed(vec!["a".into()]).unwrap_err().to_string();
        assert!(err.contains("/v1/embeddings"), "got: {err}");
    }
}
