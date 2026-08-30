//! Regression test: VaultConfig setters must not destroy generation/privacy blocks.
//!
//! Bug (Aug 2026): VaultConfig::write() serialized only the typed fields
//! (vault_path, embed_profile, migrated_to_v2) and dropped generation,
//! embedding, and privacy blocks entirely.  Any call to set_vault_path,
//! set_embed_profile, or set_migrated_to_v2 would corrupt the config file.
//!
//! Fix: setters now route through BrainConfig::load() + BrainConfig::write(),
//! which uses raw-document merge so all blocks survive verbatim.

use tauri_app_lib::vault::VaultConfig;
use std::fs;
use tempfile::TempDir;

#[test]
fn set_vault_path_preserves_generation_and_privacy() {
    let temp = TempDir::new().unwrap();
    let config_path = temp.path().join("config.json");

    // Seed config with generation, privacy, and an unknown key.
    // These three sections must survive after set_vault_path is called.
    let json = r#"{
        "vault_path":"~/old",
        "generation":{"provider":"openai","model_name":"gpt-4"},
        "privacy":{"mode":"strict"},
        "unknown_key":"preserve_me"
    }"#;
    fs::write(&config_path, json).unwrap();

    let config = VaultConfig::new(config_path.clone());
    config.set_vault_path("~/new").expect("set_vault_path succeeds");

    // Read back and verify every section survived.
    let written = fs::read_to_string(&config_path).unwrap();
    let value: serde_json::Value = serde_json::from_str(&written).unwrap();

    assert_eq!(value["vault_path"], "~/new", "vault_path was updated");
    assert_eq!(
        value["generation"]["model_name"], "gpt-4",
        "generation block survived"
    );
    assert_eq!(
        value["generation"]["provider"], "openai",
        "generation provider survived"
    );
    assert_eq!(value["privacy"]["mode"], "strict", "privacy block survived");
    assert_eq!(
        value["unknown_key"], "preserve_me",
        "unknown top-level key survived"
    );
}

#[test]
fn set_embed_profile_preserves_generation_and_privacy() {
    let temp = TempDir::new().unwrap();
    let config_path = temp.path().join("config.json");

    let json = r#"{
        "vault_path":"~/vault",
        "embed_profile":{"type":"cloud","provider":"cohere","model":"embed-english-v3.0"},
        "generation":{"provider":"openai","model_name":"gpt-4"},
        "privacy":{"mode":"strict"}
    }"#;
    fs::write(&config_path, json).unwrap();

    let config = VaultConfig::new(config_path.clone());
    let new_profile = tauri_app_lib::embedder::EmbedProfile::Cloud {
        provider: tauri_app_lib::embedder::CloudProvider::Cohere,
        model: "embed-multilingual-v3.0".into(),
        api_key: "ck_new".into(),
    };
    config.set_embed_profile(new_profile).expect("set_embed_profile succeeds");

    let written = fs::read_to_string(&config_path).unwrap();
    let value: serde_json::Value = serde_json::from_str(&written).unwrap();

    assert!(
        value["embed_profile"]["model"].as_str().unwrap().starts_with("embed"),
        "embed_profile was updated"
    );
    assert_eq!(
        value["generation"]["model_name"], "gpt-4",
        "generation block survived after set_embed_profile"
    );
    assert_eq!(
        value["privacy"]["mode"], "strict",
        "privacy block survived after set_embed_profile"
    );
}

#[test]
fn set_migrated_to_v2_preserves_generation_and_privacy() {
    let temp = TempDir::new().unwrap();
    let config_path = temp.path().join("config.json");

    let json = r#"{
        "vault_path":"~/vault",
        "generation":{"provider":"openai","model_name":"gpt-4"},
        "privacy":{"mode":"strict"},
        "unknown_nested":{"inside":"preserve_me_too"}
    }"#;
    fs::write(&config_path, json).unwrap();

    let config = VaultConfig::new(config_path.clone());
    config.set_migrated_to_v2().expect("set_migrated_to_v2 succeeds");

    let written = fs::read_to_string(&config_path).unwrap();
    let value: serde_json::Value = serde_json::from_str(&written).unwrap();

    assert_eq!(value["migrated_to_v2"], true, "migrated_to_v2 was set");
    assert_eq!(
        value["generation"]["model_name"], "gpt-4",
        "generation block survived after set_migrated_to_v2"
    );
    assert_eq!(
        value["privacy"]["mode"], "strict",
        "privacy block survived after set_migrated_to_v2"
    );
    assert_eq!(
        value["unknown_nested"]["inside"], "preserve_me_too",
        "unknown nested key survived after set_migrated_to_v2"
    );
}

#[test]
fn set_embed_profile_local_variant_preserves_all_blocks() {
    let temp = TempDir::new().unwrap();
    let config_path = temp.path().join("config.json");
    let json = r#"{"vault_path":"~/v","generation":{"model":"gpt4"},"privacy":{"mode":"strict"},"custom_key":"val"}"#;
    fs::write(&config_path, json).unwrap();

    let config = VaultConfig::new(config_path.clone());
    config
        .set_embed_profile(tauri_app_lib::embedder::EmbedProfile::Local {
            model: "nomic".to_string(),
        })
        .expect("set_embed_profile succeeds");

    let written: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
    assert_eq!(written["generation"]["model"], "gpt4", "generation survived");
    assert_eq!(written["privacy"]["mode"], "strict", "privacy survived");
    assert_eq!(written["custom_key"], "val", "custom key survived");
}

#[test]
fn set_migrated_to_v2_keeps_all_blocks_and_custom_keys() {
    let temp = TempDir::new().unwrap();
    let config_path = temp.path().join("config.json");
    let json =
        r#"{"vault_path":"~/v","generation":{},"embedding":{},"privacy":{},"custom":"keep"}"#;
    fs::write(&config_path, json).unwrap();

    let config = VaultConfig::new(config_path.clone());
    config.set_migrated_to_v2().expect("set_migrated_to_v2 succeeds");

    let written: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
    assert!(written["migrated_to_v2"].as_bool().unwrap_or(false));
    assert_eq!(written["custom"], "keep");
}
