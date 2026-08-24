//! Local embedding via [Ollama](https://github.com/ollama/ollama) HTTP API.

use std::sync::OnceLock;

use anyhow::{anyhow, Result};
use serde::Deserialize;

use super::EmbedProfile;

static CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();

fn shared_client() -> Result<reqwest::blocking::Client, String> {
    // Only a successfully built client is cached; a build failure is returned
    // without being stored, so the next call retries the builder once whatever
    // transient condition broke it has cleared.
    if let Some(client) = CLIENT.get() {
        return Ok(client.clone());
    }
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| format!("{e:#}"))?; // full anyhow-style chain, not just top-level msg
    let _ = CLIENT.set(client.clone());
    Ok(client)
}

fn default_ollama_base() -> String {
    default_ollama_base_with(std::env::var("OLLAMA_HOST").ok().as_deref())
}

/// Inner helper taking the env lookup as a parameter so tests can exercise
/// the fallback without touching process-global state.
fn default_ollama_base_with(ollama_host: Option<&str>) -> String {
    // NOTE: never use an IP-literal placeholder here — Hermes-style secret
    // redaction rewrites IPs to `[IP_ADDRESS]`, which reqwest rejects with
    // "builder error: invalid IPv6 address" (bracketed host must be valid
    // IPv6). Ollama is loopback-only in supported setups, so localhost is the
    // safe default.
    ollama_host.unwrap_or("http://localhost:11434").to_string()
}

/// Default byte budget per embed input. Approximates a safe margin under the
/// 2048-token context of common local embed models (e.g. nomic-embed-text).
///
/// Caveat: 7000 bytes ≈ 2048 tokens only holds for English ASCII text (~3.4
/// bytes/token). CJK text can cost one or more tokens per 3-byte code point,
/// and dense code tokenizes worse than prose, so such chunks can exceed the
/// model's token limit even under this byte budget.
pub const DEFAULT_MAX_EMBED_BYTES: usize = 7000;

/// Max bytes read from a non-success response body before decoding for display.
const ERROR_BODY_MAX_BYTES: u64 = 8192;

/// Minimum byte budget for a single split window. A UTF-8 code point can be up
/// to 4 bytes long; smaller windows could produce zero progress on inputs that
/// begin with a multi-byte character (infinite loop).
pub const MIN_MAX_EMBED_BYTES: usize = 4;

/// Byte budget for a single embed input. Env override: `CURATED_MAX_EMBED_BYTES`.
fn max_embed_bytes() -> usize {
    std::env::var("CURATED_MAX_EMBED_BYTES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n >= MIN_MAX_EMBED_BYTES)
        .unwrap_or(DEFAULT_MAX_EMBED_BYTES)
}

/// Split any input longer than `max_bytes` into slices of at most `max_bytes`,
/// preferring whitespace boundaries. Inputs already within budget are passed
/// through unchanged; output count/order corresponds 1:1 with the flattened
/// inputs fed to the embed endpoint.
#[cfg(test)] // convenience wrapper used by the unit tests below
fn split_for_context(texts: Vec<String>, max_bytes: usize) -> Vec<String> {
    split_for_context_indexed(texts, max_bytes)
        .into_iter()
        .map(|(_, slice)| slice)
        .collect()
}

/// Like [`split_for_context`], but each slice carries the index of the original
/// input text it came from, so callers can re-aggregate per-source results.
///
/// Budgets below [`MIN_MAX_EMBED_BYTES`] are clamped up to it so the split
/// always makes forward progress, even on multi-byte UTF-8 input.
pub fn split_for_context_indexed(texts: Vec<String>, max_bytes: usize) -> Vec<(usize, String)> {
    let max_bytes = max_bytes.max(MIN_MAX_EMBED_BYTES);
    let mut out = Vec::with_capacity(texts.len());
    for (idx, text) in texts.into_iter().enumerate() {
        if text.len() <= max_bytes {
            out.push((idx, text));
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
            let mut end = std::cmp::min(start + max_bytes, text.len());
            if end < text.len() {
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                // Prefer splitting on whitespace near the window edge so we
                // don't cut words in half.
                let search_start = start + max_bytes / 2;
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
            // Guard against zero progress: if boundary backtracking collapsed
            // the window below one code point, force-advance to the end of the
            // code point at `start`.
            if end <= start {
                end = start;
                while !text.is_char_boundary(end + 1) {
                    end += 1;
                }
                end += 1;
            }
            out.push((idx, text[start..end].to_string()));
            start = end;
        }
    }
    out
}

/// Aggregate per-subchunk embeddings back into one embedding per original
/// source text (element-wise mean over each source's subchunk vectors).
/// Returns exactly one vector per source, in original order.
pub fn aggregate_embeddings(sources: &[usize], embeddings: Vec<Vec<f32>>) -> Result<Vec<Vec<f32>>> {
    anyhow::ensure!(
        sources.len() == embeddings.len(),
        "embedding count {} does not match input slice count {}",
        embeddings.len(),
        sources.len()
    );
    let num_sources = sources.iter().copied().max().map_or(0, |m| m + 1);
    let mut acc: Vec<Option<(Vec<f32>, usize)>> = vec![None; num_sources];
    for (emb, &src) in embeddings.into_iter().zip(sources.iter()) {
        anyhow::ensure!(src < num_sources, "source index {src} out of range");
        match &mut acc[src] {
            Some((sum, count)) => {
                anyhow::ensure!(
                    sum.len() == emb.len(),
                    "inconsistent embedding dimensionality"
                );
                for (s, e) in sum.iter_mut().zip(emb.iter()) {
                    *s += e;
                }
                *count += 1;
            }
            slot @ None => *slot = Some((emb, 1)),
        }
    }
    Ok(acc
        .into_iter()
        .enumerate()
        .map(|(i, slot)| {
            let (sum, count) =
                slot.ok_or_else(|| anyhow!("no embedding produced for input {i}"))?;
            Ok(sum.into_iter().map(|v| v / count as f32).collect())
        })
        .collect::<Result<Vec<_>>>()?)
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
            EmbedProfile::Cloud { .. } | EmbedProfile::External { .. } => {
                Err(anyhow!("cloud embed not implemented"))
            }
        }
    }

    pub fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        let client = shared_client().map_err(anyhow::Error::msg)?;
        let url = format!("{}/api/embed", self.base_url.trim_end_matches('/'));
        // Re-aggregate per-subchunk embeddings so the result is exactly one
        // vector per original input text, in original order. Callers zip this
        // list with the original chunks.
        let (sources, batch): (Vec<usize>, Vec<String>) =
            split_for_context_indexed(texts, max_embed_bytes())
                .into_iter()
                .unzip();
        let resp = client
            .post(url)
            .json(&serde_json::json!({ "model": self.model, "input": batch }))
            .send()?;
        if !resp.status().is_success() {
            let status = resp.status();
            // Bound the read: error bodies can be arbitrarily large, so only
            // take a fixed byte prefix before decoding for display.
            let mut raw = Vec::new();
            let _ = std::io::Read::read_to_end(
                &mut std::io::Read::take(resp, ERROR_BODY_MAX_BYTES),
                &mut raw,
            );
            let body = String::from_utf8_lossy(&raw);
            anyhow::bail!("{}", embed_error_msg(status, &body));
        }
        let body: EmbedResponse = resp.json()?;
        anyhow::ensure!(
            body.embeddings.len() == batch.len(),
            "Ollama returned {} embeddings for {} inputs",
            body.embeddings.len(),
            batch.len()
        );
        aggregate_embeddings(&sources, body.embeddings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedder::{CloudProvider, EmbedProfile};

    #[test]
    fn default_base_url_parses_as_valid_url() {
        // Regression: the old fallback was an IP-redaction placeholder
        // (`[IP_ADDRESS]`-style literal). reqwest rejects any bracketed host
        // that is not valid IPv6 with "builder error: invalid IPv6 address",
        // which broke every embed call on paths without OLLAMA_HOST set.
        let base = default_ollama_base_with(None);
        let parsed = reqwest::Url::parse(&base)
            .unwrap_or_else(|e| panic!("default base {base:?} failed to parse: {e}"));
        assert_eq!(parsed.host_str(), Some("localhost"));
        assert_eq!(parsed.port(), Some(11434));
    }

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
        // content preserved exactly (splits are pure concatenation): the sorted
        // character multisets of input and rejoined output must be identical.
        let rejoined: String = out.concat();
        let mut a: Vec<char> = text.chars().collect();
        let mut b: Vec<char> = rejoined.chars().collect();
        a.sort();
        b.sort_by(|x, y| x.cmp(y));
        assert_eq!(a, b, "rejoined slices must preserve all characters");
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

    #[test]
    fn sub_four_byte_budgets_make_progress_on_multibyte_input() {
        // Regression: budgets < 4 bytes with a multi-byte leading char used to
        // produce an empty slice and never advance `start` (infinite loop).
        for budget in 0..4usize {
            let text = "€漢😀x".repeat(20); // multi-byte code points incl. 3- and 4-byte
            let out = split_for_context(vec![text.clone()], budget);
            assert!(!out.is_empty());
            for s in &out {
                assert!(!s.is_empty(), "empty slice at budget {budget}");
                assert!(s.len() <= MIN_MAX_EMBED_BYTES.max(budget));
            }
            // Every split advances through the input: slices reassemble the text.
            let rejoined: String = out.concat();
            assert_eq!(
                rejoined, text,
                "slices must tile the input at budget {budget}"
            );
        }
    }

    #[test]
    fn every_split_slice_is_nonempty_and_advances() {
        let text = "aé漢😀b ".repeat(100); // mixed ASCII + multibyte
        for budget in [1usize, 2, 3, 5, 7, 512] {
            let out = split_for_context(vec![text.clone()], budget);
            assert!(out.len() > 1, "budget {budget}: expected multiple slices");
            let rejoined: String = out.concat();
            assert_eq!(rejoined, text, "budget {budget}: slices must tile input");
        }
    }

    #[test]
    fn indexed_split_tracks_source_indices() {
        let big: String = "x".repeat(2500);
        let out =
            split_for_context_indexed(vec!["small".to_string(), big, "tiny".to_string()], 1000);
        let sources: Vec<usize> = out.iter().map(|(i, _)| *i).collect();
        assert_eq!(sources, vec![0, 1, 1, 1, 2]);
        assert_eq!(out[0].1, "small");
        assert_eq!(out[4].1, "tiny");
    }

    #[test]
    fn aggregate_embeddings_averages_per_source() {
        // Sources: text0 -> 2 subchunks, text1 -> 1 subchunk.
        let sources = vec![0, 0, 1];
        let embeddings = vec![vec![1.0, 3.0], vec![3.0, 5.0], vec![10.0, 20.0]];
        let agg = aggregate_embeddings(&sources, embeddings).unwrap();
        assert_eq!(agg.len(), 2); // one vector per original text
        assert_eq!(agg[0], vec![2.0, 4.0]); // mean of the two subchunks
        assert_eq!(agg[1], vec![10.0, 20.0]);
    }

    #[test]
    fn aggregate_embeddings_count_mismatch_is_error() {
        let err = aggregate_embeddings(&[0, 1], vec![vec![1.0]]).unwrap_err();
        assert!(err.to_string().contains("does not match"));
    }
}
