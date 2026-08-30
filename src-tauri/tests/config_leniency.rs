//! Leniency policy tests: per-field drops, hard errors, missing blocks.

use tauri_app_lib::config::BrainConfig;
use tauri_app_lib::retrieval::BrainPaths;
use tempfile::TempDir;
use std::fs;

fn temp_paths(json: &str) -> (TempDir, BrainPaths) {
    let temp = TempDir::new().unwrap();
    let config_path = temp.path().join("config.json");
    let brain_dir = temp.path().to_path_buf();
    if !json.is_empty() {
        fs::write(&config_path, json).unwrap();
    }
    let paths = BrainPaths {
        brain_dir,
        config_path: config_path.clone(),
        db_path: temp.path().join("brain.db"),
    };
    (temp, paths)
}

#[test]
fn leniency_drop_unknown_embed_variant() {
    let json = r#"{"vault_path":"~/v","embed_profile":"unknown_variant","generation":{},"embedding":{},"privacy":{}}"#;
    let (_temp, paths) = temp_paths(json);

    let report = BrainConfig::load_lenient(&paths);
    assert_eq!(report.config.embed_profile, None);
    assert!(report.diagnostics.iter().any(|d| d.contains("embed_profile")));
}

#[test]
fn leniency_hard_fail_on_malformed_json() {
    let (_temp, paths) = temp_paths("{ invalid }");

    let report = BrainConfig::load_lenient(&paths);
    assert!(report.diagnostics.iter().any(|d| d.contains("malformed")));
}

#[test]
fn leniency_hard_fail_on_unparseable_vault_path() {
    // vault_path present but not a string
    let json = r#"{"vault_path":123,"generation":{},"embedding":{},"privacy":{}}"#;
    let (_temp, paths) = temp_paths(json);

    let report = BrainConfig::load_lenient(&paths);
    // unparseable vault_path is a hard error (returns early with diagnostics)
    assert!(!report.diagnostics.is_empty());
}

#[test]
fn leniency_missing_blocks_marked() {
    let json = r#"{"vault_path":"~/v"}"#;
    let (_temp, paths) = temp_paths(json);

    let report = BrainConfig::load_lenient(&paths);
    assert!(report.generation_missing);
    assert!(report.embedding_missing);
    assert!(report.privacy_missing);
    assert!(!report.vault_path_missing);
}

#[test]
fn leniency_missing_vault_path_marked() {
    let json = r#"{"generation":{},"embedding":{},"privacy":{}}"#;
    let (_temp, paths) = temp_paths(json);

    let report = BrainConfig::load_lenient(&paths);
    assert!(report.vault_path_missing);
}
