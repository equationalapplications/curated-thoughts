//! End-to-end config flows.

use tauri_app_lib::config::BrainConfig;
use tauri_app_lib::retrieval::{resolve_brain_paths, BrainPaths};
use tempfile::TempDir;
use std::fs;

#[test]
fn split_env_db_and_config() {
    let temp_db = TempDir::new().unwrap();
    let temp_config = TempDir::new().unwrap();

    let db_path = temp_db.path().join("custom.db");
    let config_path = temp_config.path().join("config.json");

    let json = r#"{"vault_path":"~/vault","generation":{},"embedding":{},"privacy":{}}"#;
    fs::write(&config_path, json).unwrap();

    let db_str = db_path.to_string_lossy().into_owned();
    let cfg_str = config_path.to_string_lossy().into_owned();

    temp_env::with_vars(
        [
            ("CURATED_BRAIN_DB", Some(db_str.as_str())),
            ("CURATED_BRAIN_CONFIG", Some(cfg_str.as_str())),
            ("CURATED_BRAIN_DIR", None::<&str>),
        ],
        || {
            let paths = resolve_brain_paths();
            assert_eq!(paths.config_path, config_path);
            assert_eq!(paths.db_path, db_path);

            let report = BrainConfig::load_lenient(&paths);
            assert_eq!(report.config.vault_path, Some("~/vault".to_string()));
        },
    );
}

#[test]
fn round_trip_preserves_all_fields() {
    let temp = TempDir::new().unwrap();
    let config_path = temp.path().join("config.json");
    let json = r#"{"vault_path":"~/v","migrated_to_v2":true,"generation":{"model_name":"gpt-4"},"embedding":{},"privacy":{"mode":"strict"},"custom_top":"preserve","custom_nested":{"inner":"value"}}"#;
    fs::write(&config_path, json).unwrap();

    let paths = BrainPaths {
        brain_dir: temp.path().to_path_buf(),
        config_path: config_path.clone(),
        db_path: temp.path().join("brain.db"),
    };

    let config = BrainConfig::load(&paths).unwrap();
    config.write(&paths).unwrap();

    let reloaded: BrainConfig =
        serde_json::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
    assert_eq!(reloaded.vault_path, config.vault_path);
    assert_eq!(reloaded.migrated_to_v2, config.migrated_to_v2);

    // Verify raw JSON still has custom keys
    let raw: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
    assert_eq!(raw["custom_top"], "preserve");
    assert_eq!(raw["custom_nested"]["inner"], "value");
}
