//! Verify each consumer routes through the unified accessor.

use tauri_app_lib::config::BrainConfig;
use tauri_app_lib::retrieval::BrainPaths;
use tempfile::TempDir;
use std::fs;

fn temp_paths() -> (TempDir, BrainPaths) {
    let temp = TempDir::new().unwrap();
    let config_path = temp.path().join("config.json");
    let json = r#"{"vault_path":"~/v","generation":{"model":"gpt4"},"embedding":{},"privacy":{}}"#;
    fs::write(&config_path, json).unwrap();
    let paths = BrainPaths {
        brain_dir: temp.path().to_path_buf(),
        config_path: config_path.clone(),
        db_path: temp.path().join("brain.db"),
    };
    (temp, paths)
}

#[test]
fn brain_config_load_reads_from_brain_paths() {
    let (_temp, paths) = temp_paths();
    let result = BrainConfig::load(&paths);
    assert!(result.is_ok());
    assert_eq!(result.unwrap().vault_path, Some("~/v".to_string()));
}

#[test]
fn brain_config_write_preserves_unknown_keys() {
    let (_temp, paths) = temp_paths();
    let mut config = BrainConfig::load(&paths).unwrap();
    config.vault_path = Some("~/new".to_string());
    config.write(&paths).unwrap();

    let content = fs::read_to_string(&paths.config_path).unwrap();
    let value: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(value["vault_path"], "~/new");
    assert_eq!(
        value["generation"]["model"],
        "gpt4",
        "generation block preserved"
    );
}

#[test]
fn load_lenient_returns_diagnostics() {
    let (_temp, paths) = temp_paths();
    let report = BrainConfig::load_lenient(&paths);
    // Valid config should have no diagnostics (or no malformed-related ones)
    assert!(
        report.diagnostics.is_empty() || !report.diagnostics.iter().any(|d| d.contains("malformed")),
        "valid config should not have malformed diagnostics"
    );
}

#[test]
fn consumer_load_and_write_roundtrip() {
    let (_temp, paths) = temp_paths();
    // load_lenient (used by retrieval facade etc)
    let loaded = BrainConfig::load_lenient(&paths);
    let mut cfg = loaded.config;
    cfg.generation.model_name = Some("updated-model".to_string());
    cfg.write(&paths).expect("write succeeds");

    // Reload to verify round trip
    let reloaded = BrainConfig::load(&paths).expect("reload succeeds");
    assert_eq!(
        reloaded.generation.model_name,
        Some("updated-model".to_string())
    );
}
