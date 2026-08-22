//! Local embedding via [Ollama](https://github.com/ollama/ollama) HTTP API.

use std::sync::LazyLock;

use anyhow::{anyhow, Result};
use serde::Deserialize;

use super::EmbedProfile;

static CLIENT: LazyLock<Result<reqwest::blocking::Client, String>> = LazyLock::new(|| {
    reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| e.to_string())
});

fn default_ollama_base() -> String {
    std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "http://[REDACTED-IP]".into())
}

#[derive(Debug)]
pub struct OllamaEmbedder {
    base_url: String,
    model: String,
}

#[derive(Deserialize)]
struct EmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

impl OllamaEmbedder {
    pub fn new_local(model: impl Into<String>) -> Self {
        Self::with_base_url(default_ollama_base(), model)
    }

    pub fn with_base_url(base_url: impl Into<String>, model: impl Into<String>) -> Self {
        OllamaEmbedder {
            base_url: base_url.into(),
            model: model.into(),
        }
    }

    pub fn from_profile(profile: &EmbedProfile) -> Result<Self> {
        match profile {
            EmbedProfile::Local { model } => Ok(Self::new_local(model.clone())),
            EmbedProfile::Cloud { .. } => Err(anyhow!("cloud embed not implemented")),
        }
    }

    pub fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        let client = CLIENT.as_ref().map_err(|e| anyhow!(e.clone()))?;
        let url = format!("{}/api/embed", self.base_url.trim_end_matches('/'));
        let resp = client
            .post(url)
            .json(&serde_json::json!({ "model": self.model, "input": texts }))
            .send()?;
        if !resp.status().is_success() {
            anyhow::bail!("Ollama /api/embed status {}", resp.status());
        }
        let body: EmbedResponse = resp.json()?;
        Ok(body.embeddings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedder::{CloudProvider, EmbedProfile};

    #[test]
    fn rejects_cloud_profile() {
        let profile = EmbedProfile::Cloud {
            provider: CloudProvider::OpenAi,
            model: "x".into(),
            api_key: "k".into(),
        };
        assert!(OllamaEmbedder::from_profile(&profile).is_err());
    }

    #[test]
    fn from_profile_local_ok() {
        let profile = EmbedProfile::Local { model: "m".into() };
        assert!(OllamaEmbedder::from_profile(&profile).is_ok());
    }
}
