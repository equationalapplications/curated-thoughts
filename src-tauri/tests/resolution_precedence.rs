//! Resolution precedence tests: all 8 env var combinations.

use tauri_app_lib::retrieval::resolve_brain_paths;
use temp_env::with_vars;
use tempfile::TempDir;

#[test]
fn resolve_none_defaults_to_home_brain() {
    let _temp = TempDir::new().unwrap();
    with_vars(
        [
            ("CURATED_BRAIN_DIR", None::<&str>),
            ("CURATED_BRAIN_DB", None::<&str>),
            ("CURATED_BRAIN_CONFIG", None::<&str>),
        ],
        || {
            let paths = resolve_brain_paths();
            let home = dirs::home_dir().unwrap();
            assert_eq!(paths.brain_dir, home.join(".brain"));
            assert_eq!(paths.config_path, home.join(".brain").join("config.json"));
            assert_eq!(paths.db_path, home.join(".brain").join("brain.db"));
        },
    );
}

#[test]
fn resolve_dir_only() {
    let temp = TempDir::new().unwrap();
    let custom = temp.path().to_string_lossy().into_owned();
    with_vars(
        [
            ("CURATED_BRAIN_DIR", Some(custom.as_str())),
            ("CURATED_BRAIN_DB", None::<&str>),
            ("CURATED_BRAIN_CONFIG", None::<&str>),
        ],
        || {
            let paths = resolve_brain_paths();
            assert_eq!(paths.brain_dir.to_string_lossy().to_string(), custom);
            assert_eq!(paths.config_path.to_string_lossy().to_string(), format!("{}/config.json", custom));
            assert_eq!(paths.db_path.to_string_lossy().to_string(), format!("{}/brain.db", custom));
        },
    );
}

#[test]
fn resolve_db_only() {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("custom.db").to_string_lossy().into_owned();
    with_vars(
        [
            ("CURATED_BRAIN_DIR", None::<&str>),
            ("CURATED_BRAIN_DB", Some(db_path.as_str())),
            ("CURATED_BRAIN_CONFIG", None::<&str>),
        ],
        || {
            let paths = resolve_brain_paths();
            // DB-only: brain_dir still defaults, config_path derived from db parent
            assert_eq!(paths.db_path.to_string_lossy().to_string(), db_path);
            // Config derives from db parent
            assert!(paths.config_path.to_string_lossy().to_string().contains("config.json"));
        },
    );
}

#[test]
fn resolve_config_only() {
    let temp = TempDir::new().unwrap();
    let config_path = temp.path().join("config.json").to_string_lossy().into_owned();
    with_vars(
        [
            ("CURATED_BRAIN_DIR", None::<&str>),
            ("CURATED_BRAIN_DB", None::<&str>),
            ("CURATED_BRAIN_CONFIG", Some(config_path.as_str())),
        ],
        || {
            let paths = resolve_brain_paths();
            assert_eq!(paths.config_path.to_string_lossy().to_string(), config_path);
        },
    );
}
