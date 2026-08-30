use tauri_app_lib::config::BrainConfig;
use tauri_app_lib::retrieval::BrainPaths;
use std::fs;
use tempfile::TempDir;

#[test]
fn load_strict_succeeds_on_valid_json() {
    let temp = TempDir::new().unwrap();
    let config_path = temp.path().join("config.json");

    let json =
        r#"{"vault_path":"~/vault","migrated_to_v2":true,"generation":{},"embedding":{},"privacy":{}}"#;
    fs::write(&config_path, json).unwrap();

    let paths = BrainPaths {
        brain_dir: temp.path().to_path_buf(),
        config_path: config_path.clone(),
        db_path: temp.path().join("brain.db"),
    };

    let result = BrainConfig::load(&paths);
    assert!(result.is_ok(), "strict load should succeed on valid JSON");
    let config = result.unwrap();
    assert_eq!(config.vault_path, Some("~/vault".to_string()));
    assert!(config.migrated_to_v2);
}

#[test]
fn load_strict_fails_on_malformed_json() {
    let temp = TempDir::new().unwrap();
    let config_path = temp.path().join("config.json");

    fs::write(&config_path, "{ invalid json }").unwrap();

    let paths = BrainPaths {
        brain_dir: temp.path().to_path_buf(),
        config_path,
        db_path: temp.path().join("brain.db"),
    };

    let result = BrainConfig::load(&paths);
    assert!(result.is_err(), "strict load should fail on malformed JSON");
}

#[test]
fn load_strict_fails_on_unparseable_vault_path() {
    let temp = TempDir::new().unwrap();
    let config_path = temp.path().join("config.json");

    // vault_path is present but not a string
    let json =
        r#"{"vault_path":123,"generation":{},"embedding":{},"privacy":{}}"#;
    fs::write(&config_path, json).unwrap();

    let paths = BrainPaths {
        brain_dir: temp.path().to_path_buf(),
        config_path,
        db_path: temp.path().join("brain.db"),
    };

    let result = BrainConfig::load(&paths);
    assert!(
        result.is_err(),
        "strict load should fail on unparseable vault_path"
    );
}

#[test]
fn load_lenient_drops_unparseable_embed_profile() {
    let temp = TempDir::new().unwrap();
    let config_path = temp.path().join("config.json");

    // embed_profile has unknown variant
    let json = r#"{"vault_path":"~/vault","embed_profile":"unknown_variant","migrated_to_v2":true,"generation":{},"embedding":{},"privacy":{}}"#;
    fs::write(&config_path, json).unwrap();

    let paths = BrainPaths {
        brain_dir: temp.path().to_path_buf(),
        config_path,
        db_path: temp.path().join("brain.db"),
    };

    let report = BrainConfig::load_lenient(&paths).unwrap();
    assert_eq!(report.config.vault_path, Some("~/vault".to_string()));
    assert_eq!(report.config.embed_profile, None); // defaulted, not fatal
    assert!(report
        .diagnostics
        .iter()
        .any(|d| d.contains("embed_profile")));
}

#[test]
fn load_lenient_fails_on_malformed_json() {
    let temp = TempDir::new().unwrap();
    let config_path = temp.path().join("config.json");

    fs::write(&config_path, "{ invalid json }").unwrap();

    let paths = BrainPaths {
        brain_dir: temp.path().to_path_buf(),
        config_path,
        db_path: temp.path().join("brain.db"),
    };

    // Malformed top-level JSON is propagated as a typed ConfigError.
    let result = BrainConfig::load_lenient(&paths);
    assert!(result.is_err(), "malformed JSON must be fatal");
}

#[test]
fn load_lenient_fails_on_non_object_root() {
    let temp = TempDir::new().unwrap();
    let config_path = temp.path().join("config.json");

    // Valid JSON but the root is an array — cannot receive object overlays.
    fs::write(&config_path, "[1, 2, 3]").unwrap();

    let paths = BrainPaths {
        brain_dir: temp.path().to_path_buf(),
        config_path,
        db_path: temp.path().join("brain.db"),
    };

    let result = BrainConfig::load_lenient(&paths);
    assert!(result.is_err(), "non-object root must be fatal");
}

#[test]
fn load_lenient_fails_on_non_object_null_root() {
    let temp = TempDir::new().unwrap();
    let config_path = temp.path().join("config.json");

    // Valid JSON, root is null.
    fs::write(&config_path, "null").unwrap();

    let paths = BrainPaths {
        brain_dir: temp.path().to_path_buf(),
        config_path,
        db_path: temp.path().join("brain.db"),
    };

    let result = BrainConfig::load_lenient(&paths);
    assert!(result.is_err(), "null root must be fatal");
}

#[test]
fn load_lenient_tracks_missing_blocks() {
    let temp = TempDir::new().unwrap();
    let config_path = temp.path().join("config.json");

    // Only vault_path, missing generation/embedding/privacy blocks
    let json = r#"{"vault_path":"~/vault"}"#;
    fs::write(&config_path, json).unwrap();

    let paths = BrainPaths {
        brain_dir: temp.path().to_path_buf(),
        config_path,
        db_path: temp.path().join("brain.db"),
    };

    let report = BrainConfig::load_lenient(&paths).unwrap();
    assert!(report.generation_missing);
    assert!(report.embedding_missing);
    assert!(report.privacy_missing);
    assert!(!report.vault_path_missing);
}

#[test]
fn load_lenient_preserves_unknown_keys() {
    let temp = TempDir::new().unwrap();
    let config_path = temp.path().join("config.json");

    // Include an unknown top-level key
    let json = r#"{"vault_path":"~/vault","unknown_field":"hello","another_unknown":42,"generation":{},"embedding":{},"privacy":{}}"#;
    fs::write(&config_path, json).unwrap();

    let paths = BrainPaths {
        brain_dir: temp.path().to_path_buf(),
        config_path,
        db_path: temp.path().join("brain.db"),
    };

    let report = BrainConfig::load_lenient(&paths).unwrap();
    let preserved = report.config.preserved_keys;
    assert!(
        preserved.is_some(),
        "unknown keys should be preserved in preserved_keys"
    );
    let preserved_obj = preserved.unwrap();
    assert_eq!(
        preserved_obj.get("unknown_field").and_then(|v| v.as_str()),
        Some("hello")
    );
    assert_eq!(
        preserved_obj.get("another_unknown").and_then(|v| v.as_i64()),
        Some(42)
    );
}