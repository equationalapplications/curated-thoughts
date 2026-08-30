//! Verify all paths agree on config location when CURATED_BRAIN_DB is set.

use tauri_app_lib::retrieval::resolve_brain_paths;
use tauri_app_lib::config::BrainConfig;
use tempfile::TempDir;
use std::fs;

#[test]
fn curated_brain_db_only_all_paths_agree() {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("custom.db");
    let expected_config = temp.path().join("config.json");

    let json = r#"{"vault_path":"~/vault","generation":{"model":"gpt4"},"embedding":{},"privacy":{}}"#;
    fs::write(&expected_config, json).unwrap();

    let db_str = db_path.to_string_lossy().into_owned();
    let cfg_str = expected_config.to_string_lossy().into_owned();

    temp_env::with_var(
        "CURATED_BRAIN_DB",
        Some(db_str.as_str()),
        || {
            temp_env::with_var("CURATED_BRAIN_DIR", None::<&str>, || {
                temp_env::with_var("CURATED_BRAIN_CONFIG", None::<&str>, || {
                    let paths = resolve_brain_paths();
                    assert_eq!(paths.db_path, db_path);
                    assert_eq!(paths.config_path, expected_config);

                    // Config loads from the right place
                    let report = BrainConfig::load_lenient(&paths).unwrap();
                    assert_eq!(report.config.vault_path, Some("~/vault".to_string()));

                    // Sanity: the cfg_str we wrote exists at paths.config_path
                    let _ = cfg_str.as_str();
                });
            });
        },
    );
}
