use tempfile::TempDir;

#[test]
fn resolve_uses_curated_brain_dir_and_requires_db() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().to_str().unwrap();
    temp_env::with_vars(
        [
            ("CURATED_BRAIN_DIR", Some(dir)),
            ("CURATED_BRAIN_DB", None::<&str>),
            ("CURATED_BRAIN_CONFIG", None::<&str>),
        ],
        || {
            // no brain.db yet → error
            assert!(curated_thoughts_tools::cli_common::resolve().is_err());
            std::fs::write(tmp.path().join("brain.db"), b"").unwrap();
            let brain = curated_thoughts_tools::cli_common::resolve().unwrap();
            assert_eq!(brain.paths.db_path, tmp.path().join("brain.db"));
            assert_eq!(brain.paths.config_path, tmp.path().join("config.json"));
        },
    );
}
