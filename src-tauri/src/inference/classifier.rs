//! Optional System-One classifier for ontology backfill (spec CT-REQ-CLASS-01).
//!
//! Bridges core-llm-wiki's vendor-neutral `LLMProvider.classify` to TypeSafe's
//! Jev, either on Cloudflare Workers AI (`typesafe/jev`) or any endpoint that
//! speaks the Jev wire format. Fact text leaves the device, so every call is
//! gated by the privacy mode exactly like external generation.

use crate::inference::classifier_secrets::ClassifierSecretStore;
use crate::privacy::allows_external_generation;
use crate::privacy::PrivacyMode;
use crate::retrieval::BrainPaths;
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::time::Duration;

pub const NOT_AVAILABLE: &str = "classifier-not-available";
pub const DEFAULT_MIN_CONFIDENCE: f64 = 0.5;
pub const DEFAULT_TIMEOUT_SECS: u64 = 30;
pub const CLOUDFLARE_API_BASE: &str = "https://api.cloudflare.com/client/v4";
pub const JEV_MODEL: &str = "typesafe/jev";
const CONFIG_KEY: &str = "classifier";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ClassifierProviderKind {
    #[default]
    Unconfigured,
    JevHttp,
    CloudflareJev,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ClassifierConfig {
    #[serde(default)]
    pub provider: ClassifierProviderKind,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub account_id: Option<String>,
    /// Credential-presence indicator serialized to disk. The actual key never
    /// lands in `config.json`; it is fetched from the keychain at read time.
    /// Tests use this to assert "key is configured" without exposing the value.
    #[serde(default)]
    pub has_api_key: bool,
    /// In-memory only. Never serialized to disk and never sent over IPC.
    /// `read_classifier_config` populates it from the secret store;
    /// `write_classifier_config` consumes it and writes to the store.
    #[serde(skip)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub min_confidence: Option<f64>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ClassifierStatus {
    pub available: bool,
    pub min_confidence: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ClassifierQuestion {
    Choice {
        options: Vec<String>,
        #[serde(default)]
        instructions: Option<String>,
    },
    Binary {
        instructions: String,
    },
    Score {
        levels: Vec<String>,
        #[serde(default)]
        instructions: Option<String>,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ClassifyRequest {
    pub state: String,
    pub questions: BTreeMap<String, ClassifierQuestion>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ClassifierAnswer {
    Choice {
        choice: String,
        confidence: f64,
        probabilities: BTreeMap<String, f64>,
    },
    Binary {
        probability: f64,
    },
    Score {
        score: f64,
        confidence: f64,
        probabilities: Vec<f64>,
    },
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ClassifyResponse {
    pub answers: BTreeMap<String, ClassifierAnswer>,
}

// ── Config ──────────────────────────────────────────────────────────────────

/// Reads the top-level `classifier` key. It is not a typed `BrainConfig`
/// block; `BrainConfig` round-trips it through `preserved_keys`.
///
/// The `api_key` field is hydrated from `secrets`, never from disk:
/// on-disk JSON only carries `has_api_key` so a stolen `config.json` does not
/// leak the credential.
pub fn read_classifier_config(
    paths: &BrainPaths,
    secrets: &dyn ClassifierSecretStore,
) -> Result<ClassifierConfig> {
    let report = crate::config::BrainConfig::load_lenient(paths)
        .map_err(|e| anyhow!("config.json failed to load: {e}"))?;
    let mut cfg: ClassifierConfig = match report
        .config
        .preserved_keys
        .as_ref()
        .and_then(|v| v.get(CONFIG_KEY))
        .cloned()
    {
        None => ClassifierConfig::default(),
        Some(v) => {
            serde_json::from_value(v).context("classifier block in config.json is malformed")?
        }
    };
    cfg.api_key = secrets.get()?;
    cfg.has_api_key = cfg
        .api_key
        .as_deref()
        .map(|k| !k.is_empty())
        .unwrap_or(false);
    Ok(cfg)
}

pub fn write_classifier_config(
    paths: &BrainPaths,
    cfg: &ClassifierConfig,
    secrets: &dyn ClassifierSecretStore,
) -> Result<()> {
    validate(cfg)?;
    match cfg.api_key.as_deref() {
        // Explicit empty string from the panel ("Clear stored token").
        Some("") => secrets.delete()?,
        // Non-empty trimmed value: replace the stored credential.
        Some(key) if !key.trim().is_empty() => secrets.set(key.trim())?,
        // None: caller did not touch the keyring — leave it alone.
        None => {}
        // Some("   ") (whitespace only): same as no-op; we don't store it.
        Some(_) => {}
    }
    let mut config = crate::config::BrainConfig::load_lenient(paths)
        .map_err(|e| anyhow!("config.json failed to load: {e}"))?
        .config;
    let mut obj = match config.preserved_keys.take() {
        Some(Value::Object(m)) => m,
        _ => Map::new(),
    };
    let persisted = ClassifierConfig {
        api_key: None,
        has_api_key: cfg
            .api_key
            .as_deref()
            .map(|k| !k.is_empty())
            .unwrap_or(false),
        ..cfg.clone()
    };
    obj.insert(CONFIG_KEY.into(), serde_json::to_value(&persisted)?);
    config.preserved_keys = Some(Value::Object(obj));
    config.write(paths)
}

pub fn validate(cfg: &ClassifierConfig) -> Result<()> {
    if let Some(c) = cfg.min_confidence {
        if !c.is_finite() || !(0.0..=1.0).contains(&c) {
            bail!("min_confidence must be between 0 and 1");
        }
    }
    if cfg.timeout_secs == Some(0) {
        bail!("timeout_secs must be at least 1");
    }
    match cfg.provider {
        ClassifierProviderKind::Unconfigured => Ok(()),
        ClassifierProviderKind::JevHttp => {
            let url = cfg.url.as_deref().unwrap_or("").trim();
            if !(url.starts_with("https://") || url.starts_with("http://")) {
                bail!("jev_http requires an http(s) url");
            }
            Ok(())
        }
        ClassifierProviderKind::CloudflareJev => {
            let id = cfg.account_id.as_deref().unwrap_or("");
            if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
                bail!("cloudflare_jev requires an alphanumeric account_id");
            }
            let key_ok = cfg
                .api_key
                .as_deref()
                .map(|k| !k.trim().is_empty())
                .unwrap_or(false);
            if !key_ok {
                bail!("cloudflare_jev requires a non-empty API token");
            }
            Ok(())
        }
    }
}

pub fn endpoint(cfg: &ClassifierConfig) -> Result<String> {
    validate(cfg)?;
    match cfg.provider {
        ClassifierProviderKind::Unconfigured => bail!(NOT_AVAILABLE),
        ClassifierProviderKind::JevHttp => Ok(cfg.url.as_deref().unwrap_or("").trim().to_string()),
        ClassifierProviderKind::CloudflareJev => Ok(format!(
            "{CLOUDFLARE_API_BASE}/accounts/{}/ai/run",
            cfg.account_id.as_deref().unwrap_or("")
        )),
    }
}

pub fn is_available(cfg: &ClassifierConfig, mode: PrivacyMode) -> bool {
    allows_external_generation(mode) && endpoint(cfg).is_ok()
}

pub fn status(cfg: &ClassifierConfig, mode: PrivacyMode) -> ClassifierStatus {
    ClassifierStatus {
        available: is_available(cfg, mode),
        min_confidence: cfg.min_confidence.unwrap_or(DEFAULT_MIN_CONFIDENCE),
    }
}

// ── Jev mapping ─────────────────────────────────────────────────────────────

pub fn to_jev_questions(questions: &BTreeMap<String, ClassifierQuestion>) -> Value {
    let mut out = Map::new();
    for (key, q) in questions {
        let mut obj = Map::new();
        match q {
            ClassifierQuestion::Choice {
                options,
                instructions,
            } => {
                obj.insert("type".into(), json!("choice"));
                if let Some(i) = instructions {
                    obj.insert("instructions".into(), json!(i));
                }
                let criteria: Map<String, Value> =
                    options.iter().map(|o| (o.clone(), json!(o))).collect();
                obj.insert("criteria".into(), Value::Object(criteria));
            }
            ClassifierQuestion::Score {
                levels,
                instructions,
            } => {
                obj.insert("type".into(), json!("score"));
                if let Some(i) = instructions {
                    obj.insert("instructions".into(), json!(i));
                }
                obj.insert("criteria".into(), json!(levels));
            }
            ClassifierQuestion::Binary { instructions } => {
                obj.insert("type".into(), json!("noul"));
                obj.insert("instructions".into(), json!(instructions));
                obj.insert("criteria".into(), json!({ "true": "Yes", "false": "No" }));
            }
        }
        out.insert(key.clone(), Value::Object(obj));
    }
    Value::Object(out)
}

pub fn request_body(cfg: &ClassifierConfig, req: &ClassifyRequest) -> Value {
    let inner = json!({ "state": req.state, "questions": to_jev_questions(&req.questions) });
    match cfg.provider {
        ClassifierProviderKind::CloudflareJev => json!({ "model": JEV_MODEL, "input": inner }),
        _ => inner,
    }
}

fn finite(v: Option<&Value>, what: &str, key: &str) -> Result<f64> {
    v.and_then(Value::as_f64)
        .filter(|n| n.is_finite())
        .ok_or_else(|| anyhow!("classifier answer {key}: {what} missing or not a finite number"))
}

fn finite_map(v: Option<&Value>, key: &str) -> Result<BTreeMap<String, f64>> {
    let obj = v
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("classifier answer {key}: probabilities missing"))?;
    obj.iter()
        .map(|(k, p)| Ok((k.clone(), finite(Some(p), "probability", key)?)))
        .collect()
}

/// Shape-checks the response. Range and off-list checks stay in core, which
/// validates classifier answers as untrusted.
pub fn from_jev_response(
    requested: &BTreeMap<String, ClassifierQuestion>,
    body: &Value,
) -> Result<ClassifyResponse> {
    if body.get("success") == Some(&Value::Bool(false)) {
        bail!("classifier returned an error envelope");
    }
    let root = body.get("result").filter(|v| v.is_object()).unwrap_or(body);
    let answers = root
        .get("answers")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("classifier response has no answers object"))?;

    let mut out = BTreeMap::new();
    for (key, q) in requested {
        let a = answers
            .get(key)
            .ok_or_else(|| anyhow!("classifier response missing answer {key}"))?;
        let ty = a.get("type").and_then(Value::as_str).unwrap_or("");
        let answer = match q {
            ClassifierQuestion::Choice { .. } => {
                if ty != "choice" {
                    bail!("classifier answer {key}: expected choice, got {ty}");
                }
                ClassifierAnswer::Choice {
                    choice: a
                        .get("choice")
                        .and_then(Value::as_str)
                        .ok_or_else(|| anyhow!("classifier answer {key}: choice missing"))?
                        .to_string(),
                    confidence: finite(a.get("confidence"), "confidence", key)?,
                    probabilities: finite_map(a.get("probabilities"), key)?,
                }
            }
            ClassifierQuestion::Score { .. } => {
                if ty != "score" {
                    bail!("classifier answer {key}: expected score, got {ty}");
                }
                let by_level = finite_map(a.get("probabilities"), key)?;
                let mut levels: Vec<(u32, f64)> = by_level
                    .into_iter()
                    .map(|(k, p)| {
                        k.parse::<u32>()
                            .map(|n| (n, p))
                            .map_err(|_| anyhow!("classifier answer {key}: non-numeric level {k}"))
                    })
                    .collect::<Result<_>>()?;
                levels.sort_by_key(|(n, _)| *n);
                ClassifierAnswer::Score {
                    score: finite(a.get("score"), "score", key)?,
                    confidence: finite(a.get("confidence"), "confidence", key)?,
                    probabilities: levels.into_iter().map(|(_, p)| p).collect(),
                }
            }
            ClassifierQuestion::Binary { .. } => {
                if ty != "noul" {
                    bail!("classifier answer {key}: expected noul, got {ty}");
                }
                ClassifierAnswer::Binary {
                    probability: finite(a.get("noul"), "noul", key)?,
                }
            }
        };
        out.insert(key.clone(), answer);
    }
    Ok(ClassifyResponse { answers: out })
}

// ── HTTP ────────────────────────────────────────────────────────────────────

pub async fn classify_with(
    cfg: &ClassifierConfig,
    mode: PrivacyMode,
    req: &ClassifyRequest,
) -> Result<ClassifyResponse> {
    if !is_available(cfg, mode) {
        bail!(NOT_AVAILABLE);
    }
    let url = endpoint(cfg)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(
            cfg.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS),
        ))
        .build()?;
    let mut rb = client.post(&url).json(&request_body(cfg, req));
    if let Some(key) = cfg.api_key.as_deref().filter(|k| !k.is_empty()) {
        rb = rb.bearer_auth(key);
    }
    let resp = rb.send().await.context("classifier request failed")?;
    let status = resp.status();
    if !status.is_success() {
        bail!("classifier HTTP {status}");
    }
    let body: Value = resp
        .json()
        .await
        .context("classifier response is not JSON")?;
    from_jev_response(&req.questions, &body)
}

// ── Tauri commands ──────────────────────────────────────────────────────────

fn current_brain() -> (std::path::PathBuf, BrainPaths) {
    let brain_dir = std::path::PathBuf::from(crate::get_brain_dir_inner());
    let paths = crate::retrieval::brain_paths_for(&brain_dir);
    (brain_dir, paths)
}

fn current_mode(brain_dir: &std::path::Path) -> Result<PrivacyMode> {
    Ok(crate::privacy::effective_mode(
        &crate::privacy::read_privacy_config(brain_dir)?,
    ))
}

/// Engine `LLMProvider.classify` bridge. Errors (including
/// `classifier-not-available`) reach core as a thrown `classify`, which it
/// counts as `skipped` and retries next pass.
#[tauri::command]
pub async fn classify(request: ClassifyRequest) -> Result<ClassifyResponse, String> {
    let (brain_dir, paths) = current_brain();
    let cfg = read_classifier_config(
        &paths,
        &super::classifier_secrets::KeyringClassifierSecretStore,
    )
    .map_err(|e| e.to_string())?;
    let mode = current_mode(&brain_dir).map_err(|e| e.to_string())?;
    classify_with(&cfg, mode, &request)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn classifier_status() -> Result<ClassifierStatus, String> {
    let (brain_dir, paths) = current_brain();
    let cfg = read_classifier_config(
        &paths,
        &super::classifier_secrets::KeyringClassifierSecretStore,
    )
    .map_err(|e| e.to_string())?;
    let mode = current_mode(&brain_dir).map_err(|e| e.to_string())?;
    Ok(status(&cfg, mode))
}

#[tauri::command]
pub fn get_classifier_config() -> Result<ClassifierConfig, String> {
    let (_, paths) = current_brain();
    read_classifier_config(
        &paths,
        &super::classifier_secrets::KeyringClassifierSecretStore,
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_classifier_config(
    config: ClassifierConfig,
    app: tauri::AppHandle,
) -> Result<(), String> {
    use tauri::Emitter;
    let store = super::classifier_secrets::KeyringClassifierSecretStore;
    let (_, paths) = current_brain();
    // Preserve any key already in the keychain when the FE sends `api_key:
    // null` (the user saved a non-key field). Loading first gives us the
    // current key, and only an explicit empty-string payload ("Clear stored
    // token") will remove it.
    let mut merged = config.clone();
    if merged.api_key.is_none() {
        merged.api_key = read_classifier_config(&paths, &store)
            .ok()
            .and_then(|cur| cur.api_key);
    }
    write_classifier_config(&paths, &merged, &store).map_err(|e| e.to_string())?;
    let _ = app.emit("classifier-config-changed", ());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choice_req() -> ClassifyRequest {
        serde_json::from_value(json!({
            "state": "Alice is a person.",
            "questions": { "okf_type": { "kind": "choice", "options": ["Person", "Place"] } }
        }))
        .unwrap()
    }

    fn jev_cfg(url: &str) -> ClassifierConfig {
        ClassifierConfig {
            provider: ClassifierProviderKind::JevHttp,
            url: Some(url.into()),
            api_key: Some("k".into()),
            ..Default::default()
        }
    }

    #[test]
    fn maps_all_three_question_kinds_to_jev() {
        let req: ClassifyRequest = serde_json::from_value(json!({
            "state": "s",
            "questions": {
                "a": { "kind": "choice", "options": ["X", "Y"], "instructions": "pick" },
                "b": { "kind": "score", "levels": ["low", "high"] },
                "c": { "kind": "binary", "instructions": "is it?" }
            }
        }))
        .unwrap();
        let q = to_jev_questions(&req.questions);
        assert_eq!(
            q["a"],
            json!({ "type": "choice", "instructions": "pick", "criteria": { "X": "X", "Y": "Y" } })
        );
        assert_eq!(
            q["b"],
            json!({ "type": "score", "criteria": ["low", "high"] })
        );
        assert_eq!(
            q["c"],
            json!({ "type": "noul", "instructions": "is it?", "criteria": { "true": "Yes", "false": "No" } })
        );
    }

    #[test]
    fn cloudflare_body_wraps_input_and_names_model() {
        let cfg = ClassifierConfig {
            provider: ClassifierProviderKind::CloudflareJev,
            account_id: Some("abc123".into()),
            api_key: Some("tok".into()),
            ..Default::default()
        };
        let body = request_body(&cfg, &choice_req());
        assert_eq!(body["model"], JEV_MODEL);
        assert_eq!(body["input"]["state"], "Alice is a person.");
        assert_eq!(
            endpoint(&cfg).unwrap(),
            "https://api.cloudflare.com/client/v4/accounts/abc123/ai/run"
        );
        assert!(request_body(&jev_cfg("https://x"), &choice_req())
            .get("model")
            .is_none());
    }

    #[test]
    fn parses_all_answer_kinds_and_unwraps_cloudflare_envelope() {
        let req: ClassifyRequest = serde_json::from_value(json!({
            "state": "s",
            "questions": {
                "a": { "kind": "choice", "options": ["X", "Y"] },
                "b": { "kind": "score", "levels": ["l0", "l1", "l2"] },
                "c": { "kind": "binary", "instructions": "?" }
            }
        }))
        .unwrap();
        let body = json!({ "success": true, "result": { "answers": {
            "a": { "type": "choice", "choice": "X", "confidence": 0.9, "probabilities": { "X": 0.9, "Y": 0.1 } },
            "b": { "type": "score", "score": 1.4, "confidence": 0.6, "legend": {}, "probabilities": { "10": 0.0, "2": 0.3, "0": 0.1, "1": 0.6 } },
            "c": { "type": "noul", "noul": 0.25 }
        }}});
        let out = from_jev_response(&req.questions, &body).unwrap();
        assert_eq!(
            serde_json::to_value(&out).unwrap(),
            json!({ "answers": {
                "a": { "kind": "choice", "choice": "X", "confidence": 0.9, "probabilities": { "X": 0.9, "Y": 0.1 } },
                "b": { "kind": "score", "score": 1.4, "confidence": 0.6, "probabilities": [0.1, 0.6, 0.3, 0.0] },
                "c": { "kind": "binary", "probability": 0.25 }
            }})
        );
    }

    #[test]
    fn rejects_missing_key_wrong_type_non_finite_and_error_envelope() {
        let req = choice_req();
        let bad = [
            json!({ "answers": {} }),
            json!({ "answers": { "okf_type": { "type": "noul", "noul": 0.5 } } }),
            json!({ "answers": { "okf_type": { "type": "choice", "choice": "Person", "confidence": "high", "probabilities": {} } } }),
            json!({ "success": false, "errors": [{ "code": 1, "message": "nope" }] }),
            json!({ "unexpected": true }),
        ];
        for body in bad {
            assert!(from_jev_response(&req.questions, &body).is_err(), "{body}");
        }
    }

    #[test]
    fn validate_rejects_bad_configs() {
        let mut c = jev_cfg("ftp://x");
        assert!(validate(&c).is_err());
        c.url = Some("https://x".into());
        c.min_confidence = Some(1.5);
        assert!(validate(&c).is_err());
        let cf = ClassifierConfig {
            provider: ClassifierProviderKind::CloudflareJev,
            account_id: Some("abc/../evil".into()),
            ..Default::default()
        };
        assert!(validate(&cf).is_err());
    }

    #[test]
    fn availability_follows_privacy_mode_and_config() {
        let cfg = jev_cfg("https://x");
        assert!(!is_available(&cfg, PrivacyMode::Strict));
        assert!(is_available(&cfg, PrivacyMode::Ephemeral));
        assert!(is_available(&cfg, PrivacyMode::Connected));
        assert!(!is_available(
            &ClassifierConfig::default(),
            PrivacyMode::Connected
        ));
        assert_eq!(
            status(&cfg, PrivacyMode::Connected).min_confidence,
            DEFAULT_MIN_CONFIDENCE
        );
    }

    #[tokio::test]
    async fn strict_mode_makes_no_request() {
        let mut server = mockito::Server::new_async().await;
        let mock = server.mock("POST", "/").expect(0).create_async().await;
        let err = classify_with(&jev_cfg(&server.url()), PrivacyMode::Strict, &choice_req())
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), NOT_AVAILABLE);
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn round_trips_over_http_with_bearer_key() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/")
            .match_header("authorization", "Bearer k")
            .match_body(mockito::Matcher::PartialJson(
                json!({ "state": "Alice is a person." }),
            ))
            .with_header("content-type", "application/json")
            .with_body(
                json!({ "model": "jev-1.13.0", "answers": { "okf_type": {
                    "type": "choice", "choice": "Person", "confidence": 0.8,
                    "probabilities": { "Person": 0.8, "Place": 0.2 } } },
                    "usage": { "input_tokens": 10, "output_tokens": 1 } })
                .to_string(),
            )
            .create_async()
            .await;
        let out = classify_with(
            &jev_cfg(&server.url()),
            PrivacyMode::Connected,
            &choice_req(),
        )
        .await
        .unwrap();
        mock.assert_async().await;
        assert!(matches!(
            out.answers.get("okf_type"),
            Some(ClassifierAnswer::Choice { choice, .. }) if choice == "Person"
        ));
    }

    #[tokio::test]
    async fn http_error_status_is_an_error() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/")
            .with_status(503)
            .create_async()
            .await;
        let err = classify_with(
            &jev_cfg(&server.url()),
            PrivacyMode::Connected,
            &choice_req(),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("503"), "{err}");
    }

    #[test]
    fn config_round_trips_through_preserved_keys_without_touching_other_blocks() {
        use crate::inference::classifier_secrets::InMemoryClassifierSecretStore;
        use std::sync::{Arc, Mutex};

        let dir = tempfile::tempdir().unwrap();
        let paths = BrainPaths {
            brain_dir: dir.path().to_path_buf(),
            config_path: dir.path().join("config.json"),
            db_path: dir.path().join("brain.db"),
        };
        std::fs::write(&paths.config_path, r#"{"someFutureKey": {"keep": true}}"#).unwrap();

        let store = InMemoryClassifierSecretStore(Mutex::new(None));
        assert_eq!(
            read_classifier_config(&paths, &store).unwrap(),
            ClassifierConfig::default()
        );

        let cfg = ClassifierConfig {
            provider: ClassifierProviderKind::CloudflareJev,
            account_id: Some("abc123".into()),
            api_key: Some("tok".into()),
            min_confidence: Some(0.7),
            ..Default::default()
        };
        write_classifier_config(&paths, &cfg, &store).unwrap();
        let round = read_classifier_config(&paths, &store).unwrap();
        assert_eq!(round.api_key, Some("tok".into()));
        assert!(round.has_api_key);
        assert_eq!(round.provider, ClassifierProviderKind::CloudflareJev);
        assert_eq!(round.account_id.as_deref(), Some("abc123"));
        assert_eq!(round.min_confidence, Some(0.7));

        let raw: Value =
            serde_json::from_str(&std::fs::read_to_string(&paths.config_path).unwrap()).unwrap();
        assert_eq!(raw["someFutureKey"], json!({ "keep": true }));
        assert_eq!(raw["classifier"]["provider"], "cloudflare_jev");
        // The key never lands on disk — only the presence indicator does.
        assert!(raw["classifier"].get("api_key").is_none());
        assert_eq!(raw["classifier"]["has_api_key"], json!(true));

        // Suppress unused-import warnings for `Arc` if future tests don't need it.
        let _ = Arc::new(0);
    }

    #[test]
    fn explicit_empty_string_deletes_keystore_but_none_leaves_it_alone() {
        use crate::inference::classifier_secrets::InMemoryClassifierSecretStore;
        use std::sync::Mutex;

        let dir = tempfile::tempdir().unwrap();
        let paths = BrainPaths {
            brain_dir: dir.path().to_path_buf(),
            config_path: dir.path().join("config.json"),
            db_path: dir.path().join("brain.db"),
        };
        std::fs::write(&paths.config_path, "{}").unwrap();
        let store = InMemoryClassifierSecretStore(Mutex::new(None));

        // Use a JevHttp provider here — the keyring write/delete contract
        // is provider-agnostic, but JevHttp's `validate` does not require a
        // key so we can exercise None / whitespace / "" without first
        // merging the stored key (the Tauri `set_classifier_config` command
        // does that merge for CloudflareJev; tested separately below).
        let seeded = ClassifierConfig {
            provider: ClassifierProviderKind::JevHttp,
            url: Some("https://x".into()),
            api_key: Some("tok".into()),
            ..Default::default()
        };
        write_classifier_config(&paths, &seeded, &store).unwrap();
        assert_eq!(store.get().unwrap().as_deref(), Some("tok"));

        // None is a leave-alone: the stored key survives.
        let cfg_none = ClassifierConfig {
            api_key: None,
            ..seeded.clone()
        };
        write_classifier_config(&paths, &cfg_none, &store).unwrap();
        assert_eq!(store.get().unwrap().as_deref(), Some("tok"));
        let round = read_classifier_config(&paths, &store).unwrap();
        assert!(round.has_api_key);

        // Whitespace-only is also a leave-alone: the panel never sends this,
        // but the contract should treat it the same as None.
        let cfg_ws = ClassifierConfig {
            api_key: Some("   ".into()),
            ..seeded.clone()
        };
        write_classifier_config(&paths, &cfg_ws, &store).unwrap();
        assert_eq!(store.get().unwrap().as_deref(), Some("tok"));

        // Explicit empty string is the "Clear stored token" affordance.
        let cfg_clear = ClassifierConfig {
            api_key: Some("".into()),
            ..seeded.clone()
        };
        write_classifier_config(&paths, &cfg_clear, &store).unwrap();
        assert_eq!(store.get().unwrap(), None);
        let round = read_classifier_config(&paths, &store).unwrap();
        assert!(round.api_key.is_none());
        assert!(!round.has_api_key);
    }

    /// `set_classifier_config` (Tauri command) must merge the existing keyring
    /// entry when the FE sends `api_key: null` so a save of non-key fields
    /// never wipes the stored credential. Validates against a CloudflareJev
    /// config to confirm the merge happens before `validate` runs.
    #[test]
    fn set_classifier_config_merges_existing_key_when_payload_says_null() {
        use crate::inference::classifier_secrets::InMemoryClassifierSecretStore;
        use std::sync::Mutex;

        let dir = tempfile::tempdir().unwrap();
        let paths = BrainPaths {
            brain_dir: dir.path().to_path_buf(),
            config_path: dir.path().join("config.json"),
            db_path: dir.path().join("brain.db"),
        };
        std::fs::write(&paths.config_path, "{}").unwrap();
        let store = InMemoryClassifierSecretStore(Mutex::new(None));

        // Seed a Cloudflare config with a key.
        let seeded = ClassifierConfig {
            provider: ClassifierProviderKind::CloudflareJev,
            account_id: Some("abc123".into()),
            api_key: Some("tok".into()),
            ..Default::default()
        };
        // Inline the merge the Tauri command performs so we don't need a
        // full AppHandle — same shape as `set_classifier_config`.
        write_classifier_config(&paths, &seeded, &store).unwrap();
        assert_eq!(store.get().unwrap().as_deref(), Some("tok"));

        // Simulate the panel saving without touching the key field.
        let mut payload = ClassifierConfig {
            provider: ClassifierProviderKind::CloudflareJev,
            account_id: Some("abc123".into()),
            api_key: None,
            ..Default::default()
        };
        // Mirror the Tauri-command merge.
        if payload.api_key.is_none() {
            payload.api_key = read_classifier_config(&paths, &store).unwrap().api_key;
        }
        write_classifier_config(&paths, &payload, &store).unwrap();
        // Key survived — the leave-alone contract worked.
        assert_eq!(store.get().unwrap().as_deref(), Some("tok"));
    }

    #[test]
    fn cloudflare_validate_rejects_missing_or_empty_api_key() {
        let mut cfg = ClassifierConfig {
            provider: ClassifierProviderKind::CloudflareJev,
            account_id: Some("abc123".into()),
            ..Default::default()
        };
        assert!(validate(&cfg).is_err());
        cfg.api_key = Some("".into());
        assert!(validate(&cfg).is_err());
        cfg.api_key = Some("   ".into());
        assert!(validate(&cfg).is_err());
        cfg.api_key = Some("tok".into());
        assert!(validate(&cfg).is_ok());
    }

    #[test]
    fn api_key_is_never_serialized_to_disk_even_when_classify_loads_it() {
        use crate::inference::classifier_secrets::InMemoryClassifierSecretStore;
        use std::sync::Mutex;

        let dir = tempfile::tempdir().unwrap();
        let paths = BrainPaths {
            brain_dir: dir.path().to_path_buf(),
            config_path: dir.path().join("config.json"),
            db_path: dir.path().join("brain.db"),
        };
        std::fs::write(&paths.config_path, "{}").unwrap();
        let store = InMemoryClassifierSecretStore(Mutex::new(None));

        let cfg = ClassifierConfig {
            provider: ClassifierProviderKind::JevHttp,
            url: Some("https://x".into()),
            api_key: Some("secret-token".into()),
            ..Default::default()
        };
        write_classifier_config(&paths, &cfg, &store).unwrap();

        let on_disk = std::fs::read_to_string(&paths.config_path).unwrap();
        assert!(
            !on_disk.contains("secret-token"),
            "api_key must never appear on disk, got: {on_disk}"
        );
        // Reading back via the same store returns the in-memory value.
        let round = read_classifier_config(&paths, &store).unwrap();
        assert_eq!(round.api_key.as_deref(), Some("secret-token"));
        assert!(round.has_api_key);
    }
}
