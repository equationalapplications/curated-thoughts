use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::io::Write;
use uuid::Uuid;
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

impl Default for BrainConfig {
    fn default() -> Self {
        BrainConfig {
            vault_path: None,
            embed_profile: None,
            migrated_to_v2: false,
            generation: GenerationConfig::default(),
            embedding: EmbeddingConfig::default(),
            privacy: PrivacyConfig::default(),
            preserved_keys: None,
            preserved_generation: None,
            preserved_embedding: None,
            preserved_privacy: None,
            raw_generation: None,
            raw_embedding: None,
            raw_privacy: None,
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

        // Re-serialize the known fields for strict deserialization
        let known_value = serde_json::to_value(&obj)?;
        match serde_json::from_value::<BrainConfig>(known_value) {
            Ok(mut cfg) => {
                cfg.preserved_keys = preserved_keys;

                // Extract nested unknown keys from generation block
                if let Some(gen_val) = obj.get("generation").and_then(|v| v.as_object()) {
                    let known_gen_keys = ["provider", "model_path", "model_name", "external_url", "api_key", "timeout_secs"];
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
                    let known_priv_keys = ["mode", "chosen", "ephemeral_disclosure_acknowledged", "migration_disclosure_acknowledged"];
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

                // Even when serde does not error on unknown variants (it silently
                // defaults them), preserve the original raw block so write() can
                // restore it verbatim instead of the defaulted struct.
                cfg.raw_generation = raw_gen;
                cfg.raw_embedding = raw_emb;
                cfg.raw_privacy = raw_priv;

                Ok(cfg)
            }
            Err(e) => {
                // Strict load failed.  We pre-validated vault_path above, so any
                // remaining error is from generation/embedding/privacy blocks
                // (unknown enum variants or other schema mismatches).  Fall through
                // to lenient loading to recover, and restore the original raw blocks
                // verbatim so they survive the write cycle.
                let mut report = BrainConfig::load_lenient(paths);
                report.config.raw_generation = raw_gen;
                report.config.raw_embedding = raw_emb;
                report.config.raw_privacy = raw_priv;
                report.config.preserved_keys = preserved_keys;
                Ok(report.config)
            }
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
            Some(o) => o.clone(),
            None => {
                report.diagnostics.push("root is not a JSON object".to_string());
                return report;
            }
        };

        // Preserve unknown keys for round-trip
        let known_keys = [
            "vault_path",
            "embed_profile",
            "migrated_to_v2",
            "generation",
            "embedding",
            "privacy",
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
            let known_gen_keys = ["provider", "model_path", "model_name", "external_url", "api_key", "timeout_secs"];
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
            let known_priv_keys = ["mode", "chosen", "ephemeral_disclosure_acknowledged", "migration_disclosure_acknowledged"];
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

        // Insert modeled sections with preserved nested keys merged in.
        obj.insert("vault_path".to_string(), serde_json::to_value(&self.vault_path)?);
        obj.insert("embed_profile".to_string(), serde_json::to_value(&self.embed_profile)?);
        obj.insert("migrated_to_v2".to_string(), serde_json::to_value(&self.migrated_to_v2)?);
        obj.insert("generation".to_string(), gen_value);
        obj.insert("embedding".to_string(), emb_value);
        obj.insert("privacy".to_string(), priv_value);

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
        let tmp_path = paths.config_path.parent().unwrap_or_else(|| std::path::Path::new("."))
            .join(&tmp_name);

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
}
