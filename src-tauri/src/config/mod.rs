use crate::embedder::EmbedProfile;
pub use crate::inference::config::{EmbeddingConfig, GenerationConfig};
use crate::ontology_config::OntologyConfigBlock;
use crate::privacy::PrivacyConfig;
use crate::retrieval::BrainPaths;
use crate::trusted_links::TrustedLink;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use thiserror::Error;
use uuid::Uuid;

/// Fatal errors from `BrainConfig::load_lenient`.
///
/// Malformed top-level JSON, non-object roots, and present-but-non-string
/// `vault_path` values are classified as hard errors because masking them once
/// silently reset users' vault paths and forced re-onboarding (final-review M1,
/// and the same failure class as today's `inference::write_config` silently
/// replacing a malformed config with `{}`). Callers MUST propagate these as
/// typed errors rather than matching on a `diagnostics: Vec<String>` string.
/// The only IO condition returned as `Ok` is "file missing" — that is the
/// normal post-onboarding state, not corruption.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("config.json is malformed JSON: {0}")]
    MalformedJson(#[from] serde_json::Error),
    #[error("config.json root must be a JSON object (got {actual})")]
    NonObjectRoot { actual: &'static str },
    #[error("config.json vault_path is present but not a string")]
    VaultPathNotString,
    #[error("config.json could not be read: {0}")]
    Io(#[from] std::io::Error),
}

/// Classifies a parsed-but-non-object JSON value for the error variant.
fn root_kind(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

/// Tier stamped on deposit-ingested entries when config does not say otherwise.
///
/// Shipped default per spec §3.2: deposits are agent-written notes under active
/// revision, and `"fact"` invokes the librarian's "ANCHOR TRUTH — do not propose
/// modifications" framing, which would freeze exactly the content agents are
/// expected to keep correcting.
pub const DEFAULT_DEPOSIT_TIER: &str = "wisdom";

/// Wiki-layer settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WikiConfig {
    /// Tier stamped on deposit-ingested entries. Shipped default `"wisdom"`.
    #[serde(default)]
    pub deposit_default_tier: Option<String>,
}

/// Unified configuration for a brain directory.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BrainConfig {
    /// User's vault root path (e.g., ~/Curated-Thoughts).
    pub vault_path: Option<String>,
    /// Embedding model profile (local Ollama or external).
    pub embed_profile: Option<EmbedProfile>,
    /// Whether the vault has migrated to v2 (immutable-source-files folder structure).
    ///
    /// Deliberately NOT `#[serde(default)]`. `load()`'s strict deserialize is
    /// the gate that routes a config with a missing or malformed block to
    /// `load_lenient`, which records a diagnostic and reports the block as
    /// missing. Defaulting these fields here makes the strict parse succeed on
    /// an incomplete file, so `load()` returns a config that silently forgot
    /// the user's settings — and the next `write()` persists that loss.
    pub migrated_to_v2: bool,
    /// LLM generation config (model, provider, base_url).
    pub generation: GenerationConfig,
    /// Embedding config (model, provider, base_url).
    pub embedding: EmbeddingConfig,
    /// Privacy mode and settings.
    pub privacy: PrivacyConfig,
    /// User's ontology selection (which schema the wiki engine is seeded with).
    #[serde(default)]
    pub ontology: OntologyConfigBlock,
    /// Wiki-layer settings (deposit tier default).
    #[serde(default)]
    pub wiki: WikiConfig,
    /// Approved symlink `(link, target)` pairs. Written only by the approval
    /// flows (`ct trust`, the Desktop review prompt) — never hand-edited.
    #[serde(default)]
    pub trusted_links: Vec<TrustedLink>,
    /// Preserved raw JSON for unknown keys (round-trip vehicle).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preserved_keys: Option<serde_json::Value>,
    /// Preserved raw JSON for unknown keys inside generation block.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preserved_generation: Option<serde_json::Value>,
    /// Preserved raw JSON for unknown keys inside embedding block.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preserved_embedding: Option<serde_json::Value>,
    /// Preserved raw JSON for unknown keys inside privacy block.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preserved_privacy: Option<serde_json::Value>,
    /// Preserved raw JSON for unknown keys inside the ontology block.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preserved_ontology: Option<serde_json::Value>,
    /// Preserved raw JSON for unknown keys inside the wiki block. `wiki` is in
    /// `known_keys`, so its unknown nested keys are not covered by
    /// `preserved_keys` and would be dropped by `write()` without this.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preserved_wiki: Option<serde_json::Value>,
    /// Raw generation block JSON used when typed deserialization fails.
    /// When set, write() emits this verbatim instead of the typed generation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_generation: Option<serde_json::Value>,
    /// Raw embedding block JSON used when typed deserialization fails.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_embedding: Option<serde_json::Value>,
    /// Raw privacy block JSON used when typed deserialization fails.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_privacy: Option<serde_json::Value>,
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
    /// True if an `ontology` block was present but failed to parse (e.g. an
    /// unrecognized `schema` value). Distinct from "absent" — callers that
    /// treat a `None` selection as "never chosen, use the desktop default"
    /// must NOT apply that fallback here, or an invalid selection like
    /// `{"ontology":{"schema":"unknown"}}` would silently start the General
    /// ontology instead of surfacing the parse failure.
    pub ontology_unparseable: bool,
}

/// Serializable summary of which config blocks were silently defaulted
/// during a lenient load.  Designed for frontend consumption so the UI can
/// show user-facing errors instead of silently falling back to defaults.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MissingBlocks {
    pub generation: bool,
    pub embedding: bool,
    pub vault_path: bool,
    pub privacy: bool,
}

/// Truncate an offending config value for a diagnostic line so a
/// hand-edited giant string cannot flood the log. Char-boundary safe.
/// Cost is O(MAX + 1) chars, not O(n): we only iterate up to `MAX + 1`
/// characters to decide whether truncation is needed (Copilot follow-up
/// on PR #147 — the earlier `chars().count()` pass walked the whole
/// string, defeating the flood-mitigation intent for huge values).
fn truncate_for_diag(value: &str) -> String {
    const MAX: usize = 120;
    let over_limit = value.chars().take(MAX + 1).count() > MAX;
    if !over_limit {
        value.to_string()
    } else {
        let mut cut: String = value.chars().take(MAX).collect();
        cut.push('…');
        cut
    }
}

impl BrainConfig {
    /// Load config from disk with no leniency. Malformed top-level JSON is fatal.
    /// Missing or unparseable vault_path is fatal (never masked).
    /// Returns an error if config.json does not exist.
    pub fn load(paths: &BrainPaths) -> Result<BrainConfig> {
        let text = fs::read_to_string(&paths.config_path)?;
        let value: serde_json::Value = serde_json::from_str(&text)?;

        let obj = match value.as_object() {
            Some(o) => o.clone(),
            None => bail!("config.json root must be a JSON object"),
        };

        // vault_path type errors are fatal — validate before attempting deserialize.
        if let Some(vp) = obj.get("vault_path") {
            if !vp.is_string() && !vp.is_null() {
                bail!("vault_path must be a string");
            }
        }

        // Preserve unknown keys for round-trip
        let known_keys = [
            "vault_path",
            "embed_profile",
            "migrated_to_v2",
            "generation",
            "embedding",
            "privacy",
            "ontology",
            "wiki",
            "trusted_links",
        ];
        let unknown_keys: serde_json::Map<String, serde_json::Value> = obj
            .iter()
            .filter(|(k, _)| !known_keys.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let preserved_keys = if unknown_keys.is_empty() {
            None
        } else {
            Some(serde_json::Value::Object(unknown_keys))
        };

        // Capture raw blocks up-front (before re-serializing), so we can restore
        // them verbatim when serde silently defaults unknown enum variants.
        let raw_gen = obj.get("generation").cloned();
        let raw_emb = obj.get("embedding").cloned();
        let raw_priv = obj.get("privacy").cloned();

        // Re-use the already-parsed `value` — `obj` is its inner map,
        // re-serializing back to a Value just to deserialize again is wasted
        // work. The pre-validation above guarantees the root is an object
        // and vault_path is a valid string/null, so the strict deserialize
        // here is the only thing that can fail (unknown enum variants,
        // schema mismatches in typed blocks).
        match serde_json::from_value::<BrainConfig>(value) {
            Ok(mut cfg) => {
                cfg.preserved_keys = preserved_keys;

                // Extract nested unknown keys from generation block
                if let Some(gen_val) = obj.get("generation").and_then(|v| v.as_object()) {
                    let known_gen_keys = [
                        "provider",
                        "model_path",
                        "model_name",
                        "external_url",
                        "api_key",
                        "timeout_secs",
                    ];
                    let unknown: serde_json::Map<String, serde_json::Value> = gen_val
                        .iter()
                        .filter(|(k, _)| !known_gen_keys.contains(&k.as_str()))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    cfg.preserved_generation = if unknown.is_empty() {
                        None
                    } else {
                        Some(serde_json::Value::Object(unknown))
                    };
                }

                // Extract nested unknown keys from embedding block
                if let Some(emb_val) = obj.get("embedding").and_then(|v| v.as_object()) {
                    let known_emb_keys = ["provider", "external_url"];
                    let unknown: serde_json::Map<String, serde_json::Value> = emb_val
                        .iter()
                        .filter(|(k, _)| !known_emb_keys.contains(&k.as_str()))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    cfg.preserved_embedding = if unknown.is_empty() {
                        None
                    } else {
                        Some(serde_json::Value::Object(unknown))
                    };
                }

                // Extract nested unknown keys from privacy block
                if let Some(priv_val) = obj.get("privacy").and_then(|v| v.as_object()) {
                    let known_priv_keys = [
                        "mode",
                        "chosen",
                        "ephemeral_disclosure_acknowledged",
                        "migration_disclosure_acknowledged",
                    ];
                    let unknown: serde_json::Map<String, serde_json::Value> = priv_val
                        .iter()
                        .filter(|(k, _)| !known_priv_keys.contains(&k.as_str()))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    cfg.preserved_privacy = if unknown.is_empty() {
                        None
                    } else {
                        Some(serde_json::Value::Object(unknown))
                    };
                }

                // Extract nested unknown keys from ontology block
                if let Some(ont_val) = obj.get("ontology").and_then(|v| v.as_object()) {
                    let known_ont_keys = ["schema"];
                    let unknown: serde_json::Map<String, serde_json::Value> = ont_val
                        .iter()
                        .filter(|(k, _)| !known_ont_keys.contains(&k.as_str()))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    cfg.preserved_ontology = if unknown.is_empty() {
                        None
                    } else {
                        Some(serde_json::Value::Object(unknown))
                    };
                }

                // Extract nested unknown keys from wiki block
                if let Some(wiki_val) = obj.get("wiki").and_then(|v| v.as_object()) {
                    let known_wiki_keys = ["deposit_default_tier"];
                    let unknown: serde_json::Map<String, serde_json::Value> = wiki_val
                        .iter()
                        .filter(|(k, _)| !known_wiki_keys.contains(&k.as_str()))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    cfg.preserved_wiki = if unknown.is_empty() {
                        None
                    } else {
                        Some(serde_json::Value::Object(unknown))
                    };
                }

                // On the typed-success path, the typed fields are authoritative.
                // Only the lenient-fallback path (below) sets `raw_*`, so callers
                // who mutate `cfg.generation` / `cfg.embedding` / `cfg.privacy`
                // and call `write()` see their mutations land on disk. Unknown
                // enum variants are no longer preserved verbatim here — that
                // behavior was a public-API footgun because it silently dropped
                // typed mutations. See PR #120 review finding.

                Ok(cfg)
            }
            Err(_e) => {
                // Strict load failed.  We pre-validated vault_path above and
                // confirmed the root is an object, so any remaining strict
                // error is from generation/embedding/privacy blocks (unknown
                // enum variants or other schema mismatches).  Fall through to
                // lenient loading to recover, and restore the original raw
                // blocks verbatim so they survive the write cycle.  JSON
                // itself already parsed successfully above, so a Result Err
                // from load_lenient here would only be the non-object-root
                // case (impossible by construction) or vault_path (already
                // pre-validated) — propagate just in case.
                let mut report = BrainConfig::load_lenient(paths)?;
                report.config.raw_generation = raw_gen;
                report.config.raw_embedding = raw_emb;
                report.config.raw_privacy = raw_priv;
                report.config.preserved_keys = preserved_keys;
                Ok(report.config)
            }
        }
    }

    /// Load config from disk with per-field leniency.
    /// Malformed top-level JSON or a non-object root is fatal and returned as
    /// `Err(ConfigError)`. Missing or unparseable fields (except `vault_path`)
    /// are dropped to defaults; a missing file is `Ok` with all `*_missing`
    /// flags set (callers decide whether missing config is a hard error).
    pub fn load_lenient(paths: &BrainPaths) -> Result<LoadReport, ConfigError> {
        let mut report = LoadReport {
            config: BrainConfig::default(),
            diagnostics: vec![],
            generation_missing: false,
            embedding_missing: false,
            vault_path_missing: false,
            privacy_missing: false,
            ontology_unparseable: false,
        };

        let text = match fs::read_to_string(&paths.config_path) {
            Ok(t) => t,
            // The only IO condition treated as "absent configuration" is a
            // missing file — that is the normal post-onboarding state.
            // Permission-denied, a directory in the path, and any other I/O
            // failure are propagated as `ConfigError::Io` so the startup hook
            // surfaces the real failure instead of silently re-onboarding.
            // Matches the contract documented on `ConfigError` above.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                report
                    .diagnostics
                    .push(format!("config.json not found: {}", e));
                report.generation_missing = true;
                report.embedding_missing = true;
                report.vault_path_missing = true;
                report.privacy_missing = true;
                return Ok(report);
            }
            Err(e) => return Err(ConfigError::Io(e)),
        };

        let value: serde_json::Value = serde_json::from_str(&text).map_err(ConfigError::from)?;
        let obj = value
            .as_object()
            .ok_or_else(|| ConfigError::NonObjectRoot {
                actual: root_kind(&value),
            })?
            .clone();

        // Preserve unknown keys for round-trip
        let known_keys = [
            "vault_path",
            "embed_profile",
            "migrated_to_v2",
            "generation",
            "embedding",
            "privacy",
            "ontology",
            "wiki",
            "trusted_links",
        ];
        let unknown_keys: serde_json::Map<String, serde_json::Value> = obj
            .iter()
            .filter(|(k, _)| !known_keys.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        report.config.preserved_keys = if unknown_keys.is_empty() {
            None
        } else {
            Some(serde_json::Value::Object(unknown_keys))
        };

        // Extract nested unknown keys from generation block
        if let Some(gen_val) = obj.get("generation").and_then(|v| v.as_object()) {
            let known_gen_keys = [
                "provider",
                "model_path",
                "model_name",
                "external_url",
                "api_key",
                "timeout_secs",
            ];
            let unknown: serde_json::Map<String, serde_json::Value> = gen_val
                .iter()
                .filter(|(k, _)| !known_gen_keys.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            report.config.preserved_generation = if unknown.is_empty() {
                None
            } else {
                Some(serde_json::Value::Object(unknown))
            };
        }

        // Extract nested unknown keys from embedding block
        if let Some(emb_val) = obj.get("embedding").and_then(|v| v.as_object()) {
            let known_emb_keys = ["provider", "external_url"];
            let unknown: serde_json::Map<String, serde_json::Value> = emb_val
                .iter()
                .filter(|(k, _)| !known_emb_keys.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            report.config.preserved_embedding = if unknown.is_empty() {
                None
            } else {
                Some(serde_json::Value::Object(unknown))
            };
        }

        // Extract nested unknown keys from privacy block
        if let Some(priv_val) = obj.get("privacy").and_then(|v| v.as_object()) {
            let known_priv_keys = [
                "mode",
                "chosen",
                "ephemeral_disclosure_acknowledged",
                "migration_disclosure_acknowledged",
            ];
            let unknown: serde_json::Map<String, serde_json::Value> = priv_val
                .iter()
                .filter(|(k, _)| !known_priv_keys.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            report.config.preserved_privacy = if unknown.is_empty() {
                None
            } else {
                Some(serde_json::Value::Object(unknown))
            };
        }

        // Extract nested unknown keys from ontology block
        if let Some(ont_val) = obj.get("ontology").and_then(|v| v.as_object()) {
            let known_ont_keys = ["schema"];
            let unknown: serde_json::Map<String, serde_json::Value> = ont_val
                .iter()
                .filter(|(k, _)| !known_ont_keys.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            report.config.preserved_ontology = if unknown.is_empty() {
                None
            } else {
                Some(serde_json::Value::Object(unknown))
            };
        }

        // Extract nested unknown keys from wiki block
        if let Some(wiki_val) = obj.get("wiki").and_then(|v| v.as_object()) {
            let known_wiki_keys = ["deposit_default_tier"];
            let unknown: serde_json::Map<String, serde_json::Value> = wiki_val
                .iter()
                .filter(|(k, _)| !known_wiki_keys.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            report.config.preserved_wiki = if unknown.is_empty() {
                None
            } else {
                Some(serde_json::Value::Object(unknown))
            };
        }

        // vault_path: hard error if present but not a string
        if let Some(vp) = obj.get("vault_path") {
            match vp.as_str() {
                Some(s) => report.config.vault_path = Some(s.to_string()),
                None if !vp.is_null() => return Err(ConfigError::VaultPathNotString),
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

        // ontology: an unparseable block is NOT the same as "never chosen" —
        // it must not silently fall back to the desktop default (that would
        // start an ontology the user never selected). Flag it via
        // `ontology_unparseable` so `get_ontology_selection` can propagate
        // the failure instead of masking it.
        if let Some(ont) = obj.get("ontology") {
            match serde_json::from_value::<OntologyConfigBlock>(ont.clone()) {
                Ok(o) => report.config.ontology = o,
                Err(e) => {
                    report
                        .diagnostics
                        .push(format!("ontology block unparseable: {}", e));
                    report.ontology_unparseable = true;
                }
            }
        }

        // wiki: lenient. `wiki` is in `known_keys`, so it is excluded from
        // `preserved_keys` — without this branch a config that falls through
        // from the strict path (e.g. an unknown `generation.provider` variant)
        // would silently lose `deposit_default_tier` and every deposit would
        // be classified at the shipped default instead of the configured one.
        if let Some(w) = obj.get("wiki") {
            match serde_json::from_value::<WikiConfig>(w.clone()) {
                Ok(cfg) => report.config.wiki = cfg,
                Err(e) => {
                    report
                        .diagnostics
                        .push(format!("wiki block unparseable: {}", e));
                }
            }
        }

        // trusted_links: lenient — an unparseable entry is dropped, the rest
        // survive. This is the only mutable-from-config surface for the
        // ledger; a corruption in one entry must not nuke the whole list.
        //
        // Beyond JSON validity, each entry's `link` must be vault-relative
        // (issue #140). `TrustedLink::link` feeds the walker's
        // `vault_root.join(link)`, and `Path::join` replaces the base on an
        // absolute/rooted argument, so a hand-edited ledger must not smuggle
        // one past the approval write path's guard (PR #144). Same predicate
        // as the write path — one rule, two boundaries. Non-conforming
        // entries are dropped with a diagnostic (fail-closed: the symlink
        // reverts to `Pending` and is never followed), matching the block's
        // existing drop-one-keep-the-rest semantics.
        if let Some(tl) = obj.get("trusted_links").and_then(|v| v.as_array()) {
            let mut kept = Vec::with_capacity(tl.len());
            for entry in tl {
                match serde_json::from_value::<TrustedLink>(entry.clone()) {
                    Ok(e) => {
                        if crate::trusted_links::is_vault_relative_link(&e.link) {
                            kept.push(e);
                        } else {
                            report.diagnostics.push(format!(
                                "trusted_links entry rejected: link {:?} is not vault-relative (absolute, rooted, or contains `..`)",
                                truncate_for_diag(&e.link)
                            ));
                        }
                    }
                    Err(err) => report
                        .diagnostics
                        .push(format!("trusted_links entry unparseable: {}", err)),
                }
            }
            report.config.trusted_links = kept;
        }

        Ok(report)
    }

    /// Write config to disk using raw-document merge (preserves unknown keys).
    /// - Reads existing JSON as Value tree.
    /// - Overlays modeled sections (generation, embedding, privacy, etc.).
    /// - Writes temp file with unique name, syncs, then renames.
    /// - Malformed existing JSON is an error; file left untouched.
    pub fn write(&self, paths: &BrainPaths) -> Result<()> {
        // Read existing document, if it exists.
        let mut root = if paths.config_path.exists() {
            let text = fs::read_to_string(&paths.config_path)?;
            let value: serde_json::Value = serde_json::from_str(&text)
                .map_err(|e| anyhow::anyhow!("malformed config.json: {}", e))?;

            if !value.is_object() {
                bail!("config.json root must be a JSON object");
            }
            value
        } else {
            serde_json::json!({})
        };

        // Ensure root is an object (checked above, but be explicit for overlay).
        let obj = root.as_object_mut().unwrap();

        // Build modeled sections as Values, then merge preserved nested keys into them
        // before inserting into the root object.
        //
        // If a block failed to deserialize (captured in raw_*), use that verbatim
        // so the unparseable block is preserved unchanged through the write cycle.

        // Generation section
        let gen_value = if let Some(ref raw) = self.raw_generation {
            raw.clone()
        } else {
            let mut gen_value = serde_json::to_value(&self.generation)?;
            if let Some(ref preserved) = self.preserved_generation {
                if let Some(gen_obj) = gen_value.as_object_mut() {
                    if let Some(preserved_obj) = preserved.as_object() {
                        for (k, v) in preserved_obj {
                            gen_obj.insert(k.clone(), v.clone());
                        }
                    }
                }
            }
            gen_value
        };

        // Embedding section
        let emb_value = if let Some(ref raw) = self.raw_embedding {
            raw.clone()
        } else {
            let mut emb_value = serde_json::to_value(&self.embedding)?;
            if let Some(ref preserved) = self.preserved_embedding {
                if let Some(emb_obj) = emb_value.as_object_mut() {
                    if let Some(preserved_obj) = preserved.as_object() {
                        for (k, v) in preserved_obj {
                            emb_obj.insert(k.clone(), v.clone());
                        }
                    }
                }
            }
            emb_value
        };

        // Privacy section
        let priv_value = if let Some(ref raw) = self.raw_privacy {
            raw.clone()
        } else {
            let mut priv_value = serde_json::to_value(&self.privacy)?;
            if let Some(ref preserved) = self.preserved_privacy {
                if let Some(priv_obj) = priv_value.as_object_mut() {
                    if let Some(preserved_obj) = preserved.as_object() {
                        for (k, v) in preserved_obj {
                            priv_obj.insert(k.clone(), v.clone());
                        }
                    }
                }
            }
            priv_value
        };

        // Ontology section
        let mut ont_value = serde_json::to_value(&self.ontology)?;
        if let Some(ref preserved) = self.preserved_ontology {
            if let (Some(ont_obj), Some(preserved_obj)) =
                (ont_value.as_object_mut(), preserved.as_object())
            {
                for (k, v) in preserved_obj {
                    ont_obj.insert(k.clone(), v.clone());
                }
            }
        }

        // Insert modeled sections with preserved nested keys merged in.
        obj.insert(
            "vault_path".to_string(),
            serde_json::to_value(&self.vault_path)?,
        );
        obj.insert(
            "embed_profile".to_string(),
            serde_json::to_value(&self.embed_profile)?,
        );
        obj.insert(
            "migrated_to_v2".to_string(),
            serde_json::to_value(self.migrated_to_v2)?,
        );
        obj.insert("generation".to_string(), gen_value);
        obj.insert("embedding".to_string(), emb_value);
        obj.insert("privacy".to_string(), priv_value);
        obj.insert("ontology".to_string(), ont_value);
        let mut wiki_value = serde_json::to_value(&self.wiki)?;
        if let Some(ref preserved) = self.preserved_wiki {
            if let (Some(wiki_obj), Some(preserved_obj)) =
                (wiki_value.as_object_mut(), preserved.as_object())
            {
                for (k, v) in preserved_obj {
                    wiki_obj.insert(k.clone(), v.clone());
                }
            }
        }
        obj.insert("wiki".to_string(), wiki_value);
        obj.insert(
            "trusted_links".to_string(),
            serde_json::to_value(&self.trusted_links)?,
        );

        // Merge preserved top-level keys back in.
        if let Some(ref preserved) = self.preserved_keys {
            if let Some(preserved_obj) = preserved.as_object() {
                for (k, v) in preserved_obj {
                    obj.insert(k.clone(), v.clone());
                }
            }
        }

        // Write to temp file with unique name.
        let nonce = Uuid::new_v4();
        let pid = std::process::id();
        let tmp_name = format!("config.json.{}.{}.tmp", pid, nonce);
        let parent = paths
            .config_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        // A fresh install has no brain dir yet — create it so the first write
        // (e.g. from --onboard) doesn't fail with ENOENT.
        fs::create_dir_all(parent)?;
        let tmp_path = parent.join(&tmp_name);

        let json = serde_json::to_string_pretty(&root)?;

        // Write and sync before rename.
        {
            let mut file = std::fs::File::create(&tmp_path)?;
            file.write_all(json.as_bytes())?;
            file.sync_data()?;
        }

        // Atomic rename.
        fs::rename(&tmp_path, &paths.config_path)?;

        Ok(())
    }

    /// The configured deposit tier, read from the brain config on disk.
    ///
    /// Convenience for commit-path callers, which have a `Connection` but no
    /// `BrainConfig`. Any load failure degrades to the shipped default rather
    /// than failing the commit — a deposit landing at the default tier is a
    /// working state, a dropped proposal is not.
    pub fn deposit_default_tier_on_disk() -> String {
        let paths = crate::retrieval::resolve_brain_paths();
        match BrainConfig::load_lenient(&paths) {
            Ok(report) => report.config.deposit_default_tier().to_string(),
            Err(e) => {
                eprintln!(
                    "config: could not read wiki.deposit_default_tier ({e}); using {DEFAULT_DEPOSIT_TIER:?}"
                );
                DEFAULT_DEPOSIT_TIER.to_string()
            }
        }
    }

    /// The tier stamped on deposit-ingested entries (spec §3.2).
    ///
    /// Defaults to `"wisdom"`: deposits are agent-written and revisable, and
    /// `"fact"` invokes anchor-truth freeze semantics on agents that routinely
    /// revise. An out-of-vocabulary value falls back rather than reaching the
    /// DB and tripping the V16 CHECK — config is hand-editable.
    pub fn deposit_default_tier(&self) -> &str {
        match self.wiki.deposit_default_tier.as_deref() {
            Some(t) if crate::db::schema::is_valid_tier(t) => {
                // Reborrow from the field so the returned lifetime is `&self`,
                // not the temporary `as_deref` binding.
                self.wiki
                    .deposit_default_tier
                    .as_deref()
                    .unwrap_or(DEFAULT_DEPOSIT_TIER)
            }
            Some(other) => {
                eprintln!(
                    "config: wiki.deposit_default_tier {other:?} is not one of {:?}; using {DEFAULT_DEPOSIT_TIER:?}",
                    crate::db::schema::VALID_TIERS
                );
                DEFAULT_DEPOSIT_TIER
            }
            None => DEFAULT_DEPOSIT_TIER,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A config carrying only a wiki block. Built from `Default` rather than
    /// `from_str("{}")` on purpose: `BrainConfig`'s typed blocks deliberately
    /// have no `#[serde(default)]`, because that is the gate routing an
    /// incomplete config to `load_lenient` instead of silently accepting it.
    fn cfg_with_deposit_tier(tier: Option<&str>) -> BrainConfig {
        BrainConfig {
            wiki: WikiConfig {
                deposit_default_tier: tier.map(str::to_string),
            },
            ..Default::default()
        }
    }

    #[test]
    fn deposit_default_tier_defaults_to_wisdom() {
        // Deposits are agent-written notes under active revision. 'fact' would
        // invoke the librarian's "do not propose modifications" framing and
        // freeze exactly the content agents keep correcting (spec §3.2).
        assert_eq!(cfg_with_deposit_tier(None).deposit_default_tier(), "wisdom");
    }

    #[test]
    fn deposit_default_tier_can_be_set_to_fact() {
        assert_eq!(
            cfg_with_deposit_tier(Some("fact")).deposit_default_tier(),
            "fact"
        );
    }

    #[test]
    fn invalid_deposit_default_tier_falls_back_to_wisdom() {
        // Config is hand-editable; an out-of-vocabulary value must not reach
        // the DB and trip the V16 CHECK.
        assert_eq!(
            cfg_with_deposit_tier(Some("anchor")).deposit_default_tier(),
            "wisdom"
        );
    }

    /// The regression guard for the strict-vs-lenient asymmetry. A config
    /// missing a typed block must NOT deserialize strictly — that failure is
    /// what routes `load()` into `load_lenient`, which records a diagnostic
    /// and flags the block as missing instead of silently forgetting it.
    #[test]
    fn strict_deserialize_rejects_a_config_missing_typed_blocks() {
        assert!(
            serde_json::from_str::<BrainConfig>("{}").is_err(),
            "an empty config must fail strict deserialize, not default silently"
        );
        assert!(
            serde_json::from_str::<BrainConfig>(r#"{"wiki":{"deposit_default_tier":"fact"}}"#)
                .is_err(),
            "a config with only a wiki block is still missing generation/embedding/privacy"
        );
    }
}
