//! Integration tests for the `--onboard` CLI subcommand.
//!
//! Tests call `create_layout_and_onboard` directly (not stdin-driven `run_onboard`)
//! so they work in any environment.

use std::fs;
use tempfile::TempDir;
use tauri_app_lib::embedder::EmbedProfile;
use tauri_app_lib::inference::config::GenerationConfig;
use tauri_app_lib::onboard::{create_layout_and_onboard, OnboardConfig};

fn make_config(vault: &std::path::Path) -> OnboardConfig {
    OnboardConfig {
        vault_root: vault.to_path_buf(),
        force: false,
        embed_profile: EmbedProfile::Local {
            model: "nomic-embed-code".to_string(),
        },
        generation: GenerationConfig::default(),
    }
}

/// Set up a temp brain dir so tests are fully isolated from ~/.brain.
fn with_temp_brain_dir<F>(temp: &TempDir, f: F)
where
    F: FnOnce(),
{
    let brain_dir = temp.path();
    std::env::set_var("CURATED_BRAIN_DIR", brain_dir);
    // Also set CURATED_BRAIN_CONFIG so we can find the file to verify it.
    let cfg = brain_dir.join("config.json");
    std::env::set_var("CURATED_BRAIN_CONFIG", &cfg);
    // Unset so tests don't interfere with each other.
    // CURATED_BRAIN_DB is not set — defaults to brain_dir/brain.db.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    std::env::remove_var("CURATED_BRAIN_DIR");
    std::env::remove_var("CURATED_BRAIN_CONFIG");
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}

#[test]
fn onboard_creates_vault_layout() {
    let temp = TempDir::new().unwrap();
    let vault = temp.path().join("test-vault");

    with_temp_brain_dir(&temp, || {
        create_layout_and_onboard(make_config(&vault)).expect("onboard succeeds");
    });

    assert!(vault.exists(), "vault directory created");
    assert!(
        vault.join("immutable-source-files").is_dir(),
        "immutable-source-files/ created"
    );
    assert!(vault.join("wiki").is_dir(), "wiki/ created");
    assert!(
        vault
            .join("immutable-source-files")
            .join("agents")
            .is_dir(),
        "immutable-source-files/agents/ created"
    );
    assert!(
        vault.join(".brain/converted").is_dir(),
        ".brain/converted/ created"
    );
}

#[test]
fn onboard_writes_vault_path_into_config() {
    let temp = TempDir::new().unwrap();
    let vault = temp.path().join("my-vault");

    with_temp_brain_dir(&temp, || {
        create_layout_and_onboard(make_config(&vault)).expect("onboard succeeds");
    });

    let cfg = temp.path().join("config.json");
    let text = fs::read_to_string(&cfg).expect("config should be written");
    let parsed: serde_json::Value =
        serde_json::from_str(&text).expect("config should be valid JSON");

    let vp = parsed
        .get("vault_path")
        .and_then(|v| v.as_str())
        .expect("vault_path should be set");
    assert!(
        vp.contains("my-vault") || vp == vault.to_string_lossy().as_ref(),
        "vault_path = {vp}, expected to contain 'my-vault' or {}",
        vault.display()
    );
}

#[test]
fn onboard_force_backs_up_existing_malformed_config() {
    let temp = TempDir::new().unwrap();
    let vault = temp.path().join("force-vault");

    let cfg = temp.path().join("config.json");
    fs::write(&cfg, "{ malformed }").unwrap();

    with_temp_brain_dir(&temp, || {
        let mut c = make_config(&vault);
        c.force = true;
        create_layout_and_onboard(c).expect("onboard --force succeeds");
    });

    let backup = cfg.with_extension("json.bak");
    assert!(backup.exists(), "backup config.json.bak should be created");
    assert_eq!(
        fs::read_to_string(&backup).unwrap(),
        "{ malformed }",
        "backup should contain original malformed content"
    );

    let new_text = fs::read_to_string(&cfg).unwrap();
    let parsed: serde_json::Value =
        serde_json::from_str(&new_text).expect("new config must be valid JSON");
    assert!(
        parsed.get("vault_path").is_some(),
        "new config should have vault_path set"
    );
}

#[test]
fn onboard_merge_preserves_unknown_keys() {
    let temp = TempDir::new().unwrap();
    let vault = temp.path().join("merge-vault");

    let cfg = temp.path().join("config.json");
    let pre_json = serde_json::json!({
        "vault_path": "~/old",
        "custom_field": "preserve_me",
        "generation": {},
        "embedding": {},
        "privacy": {}
    });
    fs::write(&cfg, serde_json::to_string_pretty(&pre_json).unwrap()).unwrap();

    with_temp_brain_dir(&temp, || {
        create_layout_and_onboard(make_config(&vault)).expect("onboard merge succeeds");
    });

    let text = fs::read_to_string(&cfg).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();

    assert_eq!(
        parsed
            .get("custom_field")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
        "preserve_me",
        "unknown key 'custom_field' should survive merge"
    );

    assert!(
        parsed
            .get("vault_path")
            .and_then(|v| v.as_str())
            .map(|v| v.contains("merge-vault"))
            .unwrap_or(false),
        "vault_path should be updated to new value"
    );
}

#[test]
fn onboard_is_idempotent_on_vault_dirs() {
    let temp = TempDir::new().unwrap();
    let vault = temp.path().join("idem-vault");

    with_temp_brain_dir(&temp, || {
        let cfg = make_config(&vault);
        create_layout_and_onboard(cfg.clone()).expect("first onboard");
        create_layout_and_onboard(cfg).expect("second onboard (idempotent)");
    });
}
