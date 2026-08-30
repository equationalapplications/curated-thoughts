//! Task 16: verify the settings panel never writes a new API key and
//! preserves any legacy plaintext key already on disk via the
//! raw-document merge in `inference::config::write_config`.

use std::sync::Mutex;
use tauri_app_lib::config::BrainConfig;
use tauri_app_lib::inference::config::{
    write_config, EmbeddingConfig, GenerationConfig, GenerationProviderKind, LlmConfig,
};
use tauri_app_lib::inference::{
    update_provider_with_brain_path, GenerationProvider, InferenceState,
};
use tauri_app_lib::privacy::{self, PrivacyMode};
use tauri_app_lib::retrieval::BrainPaths;
use tempfile::TempDir;

fn allow_external_generation(brain_path: &std::path::Path) {
    privacy::write_privacy_mode(brain_path, PrivacyMode::Ephemeral, true).expect("privacy mode");
}

#[test]
fn write_config_preserves_existing_api_key_when_new_value_is_null() {
    let tmp = TempDir::new().unwrap();
    let brain_path = tmp.path();
    let config_path = brain_path.join("config.json");

    // Seed config.json with a legacy plaintext api_key.
    let seed = r#"{
        "generation": {"provider": "external", "external_url": "http://x", "api_key": "legacy-secret"},
        "embedding": {"provider": "fastembed"},
        "privacy": {}
    }"#;
    std::fs::write(&config_path, seed).unwrap();

    // Panel save with api_key unset.
    let new_config = LlmConfig {
        generation: GenerationConfig {
            provider: GenerationProviderKind::External,
            model_path: None,
            model_name: Some("gpt-4o".to_string()),
            external_url: Some("http://new/v1".to_string()),
            api_key: None,
            timeout_secs: None,
        },
        embedding: EmbeddingConfig::default(),
    };
    write_config(brain_path, &new_config).expect("write succeeds");

    // The legacy key must still be on disk; the new model/url fields landed.
    let written = std::fs::read_to_string(&config_path).unwrap();
    let value: serde_json::Value = serde_json::from_str(&written).unwrap();
    assert_eq!(
        value["generation"]["api_key"], "legacy-secret",
        "legacy api_key preserved by raw-document merge"
    );
    assert_eq!(value["generation"]["model_name"], "gpt-4o");
    assert_eq!(value["generation"]["external_url"], "http://new/v1");
}

#[test]
fn write_config_does_not_write_api_key_when_none_supplied_and_disk_empty() {
    let tmp = TempDir::new().unwrap();
    let brain_path = tmp.path();

    // No config.json on disk yet.
    let new_config = LlmConfig {
        generation: GenerationConfig {
            provider: GenerationProviderKind::External,
            model_path: None,
            model_name: Some("gpt-4o".to_string()),
            external_url: Some("http://x/v1".to_string()),
            api_key: None,
            timeout_secs: None,
        },
        embedding: EmbeddingConfig::default(),
    };
    write_config(brain_path, &new_config).expect("write succeeds");

    let config_path = brain_path.join("config.json");
    let value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
    // Nothing on disk to preserve, so api_key must not be a string value.
    // (The field may be absent or explicit null; either is fine — only a
    // *string* value would indicate a credential leak.)
    match value["generation"].get("api_key") {
        None => {}
        Some(v) if v.is_null() => {}
        Some(v) => panic!("api_key must not be a non-null value after a panel save; got: {v}"),
    }
}

#[test]
fn update_provider_panel_save_does_not_overwrite_existing_api_key() {
    let tmp = TempDir::new().unwrap();
    let brain_path = tmp.path();

    let config_path = brain_path.join("config.json");
    // Seed with a legacy plaintext key.  Seed BEFORE setting the privacy mode:
    // both write the same `config.json`, so seeding second would clobber the
    // privacy block and leave the brain in strict mode.
    std::fs::write(
        &config_path,
        r#"{"generation": {"provider": "external", "external_url": "http://x", "api_key": "legacy-secret"}, "embedding": {}, "privacy": {}}"#,
    )
    .unwrap();
    allow_external_generation(brain_path);

    let state = InferenceState(Mutex::new(GenerationProvider::External {
        base_url: "http://x".to_string(),
        api_key: Some("legacy-secret".to_string()),
        model_name: "old".to_string(),
    }));
    // Panel save: incoming config has a non-null api_key (sent by current
    // frontend), but the backend must strip it before persisting.
    let incoming = GenerationConfig {
        provider: GenerationProviderKind::External,
        model_path: None,
        model_name: Some("new-model".to_string()),
        external_url: Some("http://new/v1".to_string()),
        api_key: Some("brand-new-key-should-not-persist".to_string()),
        timeout_secs: None,
    };

    update_provider_with_brain_path(brain_path, incoming, &state, None).expect("update succeeds");

    let report = BrainConfig::load_lenient(&BrainPaths {
        brain_dir: brain_path.to_path_buf(),
        config_path: config_path.clone(),
        db_path: brain_path.join("brain.db"),
    })
    .unwrap();
    // The disk api_key must remain the legacy one, not the new value.
    assert_eq!(
        report.config.generation.api_key.as_deref(),
        Some("legacy-secret"),
        "panel save must not overwrite an existing api_key"
    );
    assert_eq!(
        report.config.generation.model_name.as_deref(),
        Some("new-model")
    );
}

#[test]
fn update_provider_panel_save_writes_no_api_key_when_disk_had_none() {
    let tmp = TempDir::new().unwrap();
    let brain_path = tmp.path();

    let config_path = brain_path.join("config.json");
    // Seed before setting the privacy mode — see the note above.
    std::fs::write(
        &config_path,
        r#"{"generation": {"provider": "external", "external_url": "http://x"}, "embedding": {}, "privacy": {}}"#,
    )
    .unwrap();
    allow_external_generation(brain_path);

    let state = InferenceState(Mutex::new(GenerationProvider::Unconfigured));
    // Panel save with api_key = None (the realistic case once the field is dropped).
    let incoming = GenerationConfig {
        provider: GenerationProviderKind::External,
        model_path: None,
        model_name: Some("gpt-4o".to_string()),
        external_url: Some("http://new/v1".to_string()),
        api_key: None,
        timeout_secs: None,
    };

    update_provider_with_brain_path(brain_path, incoming, &state, None).expect("update succeeds");

    let raw = std::fs::read_to_string(&config_path).unwrap();
    let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
    // Either absent or explicit null is fine — only a non-null value would
    // indicate a credential leak from the panel save.
    match value["generation"].get("api_key") {
        None => {}
        Some(v) if v.is_null() => {}
        Some(v) => panic!("api_key must not be a non-null value after a panel save; got: {v}"),
    }
}
