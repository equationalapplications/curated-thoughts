//! Verify no API keys are written to config.json.

use std::fs;
use tauri_app_lib::config::BrainConfig;
use tauri_app_lib::retrieval::BrainPaths;
use tempfile::TempDir;

fn temp_paths() -> (TempDir, BrainPaths) {
    let temp = TempDir::new().unwrap();
    let config_path = temp.path().join("config.json");
    let brain_dir = temp.path().to_path_buf();
    fs::write(
        &config_path,
        r#"{"generation":{},"embedding":{},"privacy":{}}"#,
    )
    .unwrap();
    let paths = BrainPaths {
        brain_dir,
        config_path: config_path.clone(),
        db_path: temp.path().join("brain.db"),
    };
    (temp, paths)
}

#[test]
fn onboard_never_writes_api_key_to_config() {
    use tauri_app_lib::embedder::EmbedProfile;
    use tauri_app_lib::inference::config::{GenerationConfig, GenerationProviderKind};
    use tauri_app_lib::onboard::{create_layout_and_onboard, OnboardConfig};

    let temp = TempDir::new().unwrap();
    let config_path = temp.path().join("config.json");
    fs::write(
        &config_path,
        r#"{"generation":{},"embedding":{},"privacy":{}}"#,
    )
    .unwrap();

    let vault = temp.path().join("vault");
    let cfg = OnboardConfig {
        vault_root: vault.clone(),
        force: false,
        embed_profile: EmbedProfile::Local {
            model: "nomic".to_string(),
        },
        generation: GenerationConfig {
            provider: GenerationProviderKind::External,
            model_path: None,
            model_name: Some("gpt-4".to_string()),
            external_url: Some("https://api.example.com".to_string()),
            api_key: None, // Never set
            timeout_secs: None,
        },
        ontology: tauri_app_lib::ontology_config::OntologySelection::Off,
    };

    // Point CURATED_BRAIN_CONFIG at our temp file via temp_env.
    temp_env::with_var(
        "CURATED_BRAIN_CONFIG",
        Some(config_path.to_string_lossy().as_ref()),
        || {
            create_layout_and_onboard(cfg).expect("onboard succeeds");
        },
    );

    let content = fs::read_to_string(&config_path).unwrap();
    let written: serde_json::Value = serde_json::from_str(&content).unwrap();
    // Verify no api_key field on the on-disk config
    if let Some(gen) = written.get("generation").and_then(|g| g.as_object()) {
        assert!(
            !gen.contains_key("api_key") || gen.get("api_key").map(|v| v.is_null()).unwrap_or(true),
            "api_key should not be written to config.json"
        );
    }
}

#[test]
fn doctor_never_echoes_api_key() {
    use temp_env::with_var;

    let temp = TempDir::new().unwrap();
    let config_path = temp.path().join("config.json");
    fs::write(
        &config_path,
        r#"{"vault_path":"~/v","generation":{},"embedding":{},"privacy":{}}"#,
    )
    .unwrap();

    with_var("GENERATION_API_KEY", Some("super-secret-key-99999"), || {
        with_var(
            "CURATED_BRAIN_CONFIG",
            Some(config_path.to_string_lossy().as_ref()),
            || {
                // Capture the full doctor report so the redaction contract
                // is actually asserted, not just the exit code.
                let mut out: Vec<u8> = Vec::new();
                let exit_code = tauri_app_lib::doctor::run_doctor_to(&mut out).unwrap();
                assert_eq!(exit_code, 0);
                let report = String::from_utf8(out).unwrap();
                assert!(
                    !report.contains("super-secret-key-99999"),
                    "secret should never be echoed by --doctor; got:\n{report}"
                );
                assert!(
                    report.contains("NOTE: generation API key in environment"),
                    "doctor should acknowledge the env key without echoing it; got:\n{report}"
                );
                // config.json on disk must not gain the secret either.
                let content = fs::read_to_string(&config_path).unwrap();
                assert!(
                    !content.contains("super-secret-key-99999"),
                    "secret should never be written to config.json"
                );
            },
        );
    });
}

#[test]
fn brainconfig_default_has_no_api_key_in_json() {
    // Verify that when an OnboardConfig has api_key: None and we serialize,
    // the resulting JSON does NOT carry an api_key field.
    use tauri_app_lib::inference::config::{GenerationConfig, GenerationProviderKind};

    let (temp, paths) = temp_paths();
    let config = BrainConfig {
        generation: GenerationConfig {
            provider: GenerationProviderKind::External,
            model_path: None,
            model_name: Some("gpt-4".to_string()),
            external_url: Some("https://api.example.com".to_string()),
            api_key: None,
            timeout_secs: None,
        },
        ..BrainConfig::default()
    };
    config.write(&paths).expect("write succeeds");

    let content = fs::read_to_string(&paths.config_path).unwrap();
    let value: serde_json::Value = serde_json::from_str(&content).unwrap();
    let gen = value.get("generation").and_then(|g| g.as_object());
    if let Some(gen) = gen {
        assert!(
            !gen.contains_key("api_key") || gen["api_key"].is_null(),
            "api_key (None) should not appear in written JSON: got {:?}",
            gen.get("api_key")
        );
    }

    drop(temp);
}
