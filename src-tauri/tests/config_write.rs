use tauri_app_lib::config::BrainConfig;
use tauri_app_lib::retrieval::BrainPaths;
use std::fs;
use tempfile::TempDir;

#[test]
fn write_preserves_unknown_keys() {
    let temp = TempDir::new().unwrap();
    let config_path = temp.path().join("config.json");

    // Seed with generation + unknown top-level + unknown nested key
    let json = r#"{"vault_path":"~/v","generation":{"model":"gpt4","unknown_field":"preserve_me"},"embedding":{},"privacy":{},"unknown_top":"also_preserve","migrated_to_v2":false}"#;
    fs::write(&config_path, json).unwrap();

    let paths = BrainPaths {
        brain_dir: temp.path().to_path_buf(),
        config_path: config_path.clone(),
        db_path: temp.path().join("brain.db"),
    };

    // Use BrainConfig::load to populate preserved_* fields
    let mut config = BrainConfig::load(&paths).expect("load succeeds");
    config.vault_path = Some("~/new".to_string());
    config.write(&paths).expect("write succeeds");

    let written = fs::read_to_string(&config_path).unwrap();
    let value: serde_json::Value = serde_json::from_str(&written).unwrap();

    assert_eq!(value["vault_path"], "~/new", "vault_path was updated");
    assert_eq!(value["unknown_top"], "also_preserve", "unknown top-level key preserved");
    assert_eq!(value["generation"]["unknown_field"], "preserve_me", "unknown nested key preserved");
}

#[test]
fn write_fails_on_malformed_existing_json() {
    let temp = TempDir::new().unwrap();
    let config_path = temp.path().join("config.json");

    fs::write(&config_path, "{ broken json }").unwrap();

    let paths = BrainPaths {
        brain_dir: temp.path().to_path_buf(),
        config_path: config_path.clone(),
        db_path: temp.path().join("brain.db"),
    };

    let config = BrainConfig::default();
    let result = config.write(&paths);
    assert!(result.is_err(), "write fails on malformed JSON");

    // Verify file was not truncated
    let original = fs::read_to_string(&config_path).unwrap();
    assert_eq!(original, "{ broken json }", "file bytes unchanged after failed write");
}

#[test]
fn write_uses_unique_temp_name() {
    let temp = TempDir::new().unwrap();
    let config_path = temp.path().join("config.json");

    fs::write(&config_path, r#"{"generation":{"provider":"external"},"embedding":{},"privacy":{}}"#).unwrap();

    let paths = BrainPaths {
        brain_dir: temp.path().to_path_buf(),
        config_path: config_path.clone(),
        db_path: temp.path().join("brain.db"),
    };

    // Run sequentially to avoid parallel TempDir interference
    let config = BrainConfig::default();
    config.write(&paths).expect("write succeeds");

    // Assert no .tmp file remains (check extension specifically, not .tmp in path)
    let entries: Vec<_> = fs::read_dir(temp.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map_or(false, |ext| ext == "tmp"))
        .collect();

    assert!(entries.is_empty(), "no .tmp files left after write");
}

#[test]
fn write_on_missing_file_creates_it() {
    let temp = TempDir::new().unwrap();
    let config_path = temp.path().join("config.json");

    assert!(!config_path.exists());

    let paths = BrainPaths {
        brain_dir: temp.path().to_path_buf(),
        config_path: config_path.clone(),
        db_path: temp.path().join("brain.db"),
    };

    let config = BrainConfig {
        vault_path: Some("~/vault".to_string()),
        ..Default::default()
    };

    config.write(&paths).expect("write succeeds on missing file");
    assert!(config_path.exists(), "file created");

    let written: BrainConfig = serde_json::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
    assert_eq!(written.vault_path, Some("~/vault".to_string()));
}

#[test]
fn write_fails_and_preserves_file_on_malformed() {
    let temp = TempDir::new().unwrap();
    let config_path = temp.path().join("config.json");
    let original = "{ broken json }";
    fs::write(&config_path, original).unwrap();

    let paths = BrainPaths {
        brain_dir: temp.path().to_path_buf(),
        config_path: config_path.clone(),
        db_path: temp.path().join("brain.db"),
    };

    let config = BrainConfig::default();
    let result = config.write(&paths);
    assert!(result.is_err(), "write should fail on malformed JSON");
    assert_eq!(
        fs::read_to_string(&config_path).unwrap(),
        original,
        "original bytes preserved"
    );
}

#[test]
fn concurrent_writes_use_different_temp_files() {
    use std::thread;
    use std::sync::{Arc, Barrier};

    let temp = TempDir::new().unwrap();
    let brain_dir = temp.path().to_path_buf();
    let db_path = brain_dir.join("brain.db");
    let config_path = Arc::new(brain_dir.join("config.json"));
    fs::write(
        &*config_path,
        r#"{"generation":{},"embedding":{},"privacy":{}}"#,
    )
    .unwrap();

    let barrier = Arc::new(Barrier::new(2));

    let t1 = {
        let path = Arc::clone(&config_path);
        let b = Arc::clone(&barrier);
        let brain_dir = brain_dir.clone();
        let db_path = db_path.clone();
        thread::spawn(move || {
            let paths = BrainPaths {
                brain_dir,
                config_path: (*path).clone(),
                db_path,
            };
            let mut config = BrainConfig::default();
            config.vault_path = Some("vault1".to_string());
            b.wait();
            config.write(&paths).unwrap();
        })
    };

    let t2 = {
        let path = Arc::clone(&config_path);
        let b = Arc::clone(&barrier);
        let brain_dir = brain_dir.clone();
        let db_path = db_path.clone();
        thread::spawn(move || {
            let paths = BrainPaths {
                brain_dir,
                config_path: (*path).clone(),
                db_path,
            };
            let mut config = BrainConfig::default();
            config.vault_path = Some("vault2".to_string());
            b.wait();
            config.write(&paths).unwrap();
        })
    };

    t1.join().unwrap();
    t2.join().unwrap();

    // No .tmp files left
    let entries: Vec<_> = fs::read_dir(temp.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map_or(false, |ext| ext == "tmp"))
        .collect();
    assert!(entries.is_empty(), "no .tmp files from concurrent writes");
}
