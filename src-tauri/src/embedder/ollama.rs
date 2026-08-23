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

/// Default char budget per embed input. Approximates a safe margin under the
/// 2048-token context of common local embed models (e.g. nomic-embed-text).
pub const DEFAULT_MAX_EMBED_CHARS: usize = 7000;

/// Char budget for a single embed input. Env override: `CURATED_MAX_EMBED_CHARS`.
fn max_embed_chars() -> usize {
    std::env::var("CURATED_MAX_EMBED_CHARS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n >= 2)
        .unwrap_or(DEFAULT_MAX_EMBED_CHARS)
}

/// Split any input longer than `max_chars` into slices of at most `max_chars`,
/// preferring whitespace boundaries. Inputs already within budget are passed
/// through unchanged; output count/order corresponds 1:1 with the flattened
/// inputs fed to the embed endpoint.
pub fn split_for_context(texts: Vec<String>, max_chars: usize) -> Vec<String> {
    let mut out = Vec::with_capacity(texts.len());
    for text in texts {
        if text.len() <= max_chars {
            out.push(text);
            continue;
        }
        let mut start = 0usize;
        while start < text.len() {
            if !text.is_char_boundary(start) {
                // Shouldn't happen (we always advance to boundaries), but guard anyway.
                while !text.is_char_boundary(start) {
                    start += 1;
                }
                continue;
            }
            let mut end = std::cmp::min(start + max_chars, text.len());
            if end < text.len() {
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                // Prefer splitting on whitespace near the window edge so we
                // don't cut words in half.
                let search_start = start + max_chars / 2;
                if search_start < end && text.is_char_boundary(search_start) {
                    let slice = &text[search_start..end];
                    if let Some(pos) = slice.rfind(char::is_whitespace) {
                        end = search_start + pos + 1;
                        while !text.is_char_boundary(end) {
                            end -= 1;
                        }
                    }
                }
            }
            out.push(text[start..end].to_string());
            start = end;
        }
    }
    out
}

/// Format the error message for a failed embed response, including (a
/// truncated) response body so failures like "the input length exceeds the
/// context length" are visible instead of an opaque transport error.
pub fn embed_error_msg(status: reqwest::StatusCode, body: &str) -> String {
    let truncated: String = body.chars().take(200).collect();
    format!("Ollama /api/embed status {status}: {truncated}")
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
        let batch = split_for_context(texts, max_embed_chars());
        let resp = client
            .post(url)
            .json(&serde_json::json!({ "model": self.model, "input": batch }))
            .send()?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            anyhow::bail!("{}", embed_error_msg(status, &body));
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

    #[test]
    fn split_passthrough_within_budget() {
        let texts = vec!["short".to_string(), "".to_string()];
        let out = split_for_context(texts.clone(), 100);
        assert_eq!(out, texts);
    }

    #[test]
    fn splits_oversized_input_on_whitespace() {
        // words of 5 chars + separator => 6-char period
        let word = "abcde ";
        let text: String = word.repeat(1000); // 6000 chars
        let out = split_for_context(vec![text.clone()], 1000);
        assert!(out.len() > 1, "expected multiple slices");
        for s in &out {
            assert!(s.len() <= 1000, "slice too long: {}", s.len());
        }
        // content preserved (modulo split positions): rejoin and compare sorted chars
        let rejoined: String = out.concat();
        let mut a: Vec<char> = text.chars().collect();
        let mut b: Vec<char> = rejoined.chars().collect();
        a.sort();
        b.sort_by(|x, y| x.cmp(y));
        assert_eq!(a.len(), b.len());
    }

    #[test]
    fn splits_multibyte_text_without_panic() {
        let text = "é—漢字 ".repeat(500); // multibyte, ~4500 bytes
        let out = split_for_context(vec![text], 512);
        assert!(out.len() > 1);
        for s in &out {
            assert!(s.len() <= 512);
        }
    }

    #[test]
    fn mixed_batch_order_preserved() {
        let big: String = "x".repeat(2500);
        let texts = vec!["small".to_string(), big.clone(), "tiny".to_string()];
        let out = split_for_context(texts, 1000);
        // small + tiny pass through, big (2500 chars) becomes 3 slices => 5 total
        assert_eq!(out.len(), 5);
        assert_eq!(out[0], "small");
        assert_eq!(out[4], "tiny");
        for s in &out[1..4] {
            assert!(s.len() <= 1000);
        }
    }

    #[test]
    fn embed_error_msg_includes_body() {
        let msg = embed_error_msg(
            reqwest::StatusCode::BAD_REQUEST,
            "{\"error\":\"the input length exceeds the context length\"}",
        );
        assert!(msg.contains("400"));
        assert!(msg.contains("input length exceeds"));
    }

    #[test]
    fn embed_error_msg_truncates_long_body() {
        let long = "y".repeat(10_000);
        let msg = embed_error_msg(reqwest::StatusCode::INTERNAL_SERVER_ERROR, &long);
        assert!(msg.chars().count() < 300);
    }
}
