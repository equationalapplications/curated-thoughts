use std::sync::Mutex;
use tauri_app_lib::inference::config::{read_config, GenerationConfig, GenerationProviderKind};
use tauri_app_lib::inference::{update_provider_with_brain_path, GenerationProvider, InferenceState};
use tempfile::TempDir;

#[test]
fn update_provider_rolls_back_to_unconfigured_on_init_failure() {
    let tmp = TempDir::new().expect("tempdir");
    let brain_path = tmp.path();

    let state = InferenceState(Mutex::new(GenerationProvider::Unconfigured));
    let config = GenerationConfig {
        provider: GenerationProviderKind::Sidecar,
        model_path: Some("models/nonexistent.gguf".to_string()),
        model_name: None,
        external_url: None,
        api_key: None,
    };

    let err = update_provider_with_brain_path(brain_path, config, &state, None).unwrap_err();
    assert!(err.contains("llama-server binary not found") || err.contains("provider init failed"));

    assert!(matches!(*state.0.lock().unwrap(), GenerationProvider::Unconfigured));
    assert_eq!(read_config(brain_path).generation.provider, GenerationProviderKind::Unconfigured);
}

#[cfg(unix)]
#[test]
fn update_provider_rolls_back_to_unconfigured_when_config_write_fails() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new().expect("tempdir");
    let brain_path = tmp.path();

    let state = InferenceState(Mutex::new(GenerationProvider::Unconfigured));
    let config = GenerationConfig {
        provider: GenerationProviderKind::External,
        model_path: None,
        model_name: Some("gpt-3.5-turbo".to_string()),
        external_url: Some("https://api.openai.com/v1".to_string()),
        api_key: Some("sk-test".to_string()),
    };

    let original_perms = std::fs::metadata(brain_path).expect("metadata").permissions();
    std::fs::set_permissions(brain_path, PermissionsExt::from_mode(0o500)).expect("make brain dir read-only");

    let err = update_provider_with_brain_path(brain_path, config, &state, None).unwrap_err();

    std::fs::set_permissions(brain_path, original_perms).expect("restore permissions");

    assert!(err.contains("settings could not be saved to disk"));
    assert!(matches!(*state.0.lock().unwrap(), GenerationProvider::Unconfigured));
}
