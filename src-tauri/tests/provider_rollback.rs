use std::sync::Mutex;
use tauri_app_lib::inference::config::{read_config, GenerationConfig, GenerationProviderKind};
use tauri_app_lib::inference::{
    update_provider_with_brain_path, GenerationProvider, InferenceState,
};
use tauri_app_lib::privacy::{self, PrivacyMode};
use tempfile::TempDir;

fn allow_external_generation(brain_path: &std::path::Path) {
    privacy::write_privacy_mode(brain_path, PrivacyMode::Ephemeral, true).expect("privacy mode");
}

#[test]
fn update_provider_rejects_external_in_strict_mode() {
    let tmp = TempDir::new().expect("tempdir");
    let brain_path = tmp.path();
    privacy::write_privacy_mode(brain_path, PrivacyMode::Strict, true).expect("privacy mode");

    let state = InferenceState(Mutex::new(GenerationProvider::Unconfigured));
    let config = GenerationConfig {
        provider: GenerationProviderKind::External,
        model_path: None,
        model_name: Some("gpt-3.5-turbo".to_string()),
        external_url: Some("https://api.openai.com/v1".to_string()),
        api_key: Some("sk-test".to_string()),
    };

    let err = update_provider_with_brain_path(brain_path, config, &state, None).unwrap_err();
    assert!(err.contains("privacy-mode-strict"));
}

#[test]
fn update_provider_rolls_back_to_unconfigured_on_init_failure() {
    let tmp = TempDir::new().expect("tempdir");
    let brain_path = tmp.path();
    // Mark privacy as chosen so resolve_privacy_state skips keyring (unavailable on headless CI).
    privacy::write_privacy_mode(brain_path, PrivacyMode::Strict, true).expect("privacy mode");

    let state = InferenceState(Mutex::new(GenerationProvider::Unconfigured));
    // Missing model_path fails before any runner-specific llama-server layout.
    let config = GenerationConfig {
        provider: GenerationProviderKind::Sidecar,
        model_path: None,
        model_name: None,
        external_url: None,
        api_key: None,
    };

    let err = update_provider_with_brain_path(brain_path, config, &state, None).unwrap_err();
    assert!(err.contains("sidecar requires model_path"));

    assert!(matches!(
        *state.0.lock().unwrap(),
        GenerationProvider::Unconfigured
    ));
    assert_eq!(
        read_config(brain_path).generation.provider,
        GenerationProviderKind::Unconfigured
    );
}

#[cfg(unix)]
#[test]
fn update_provider_rolls_back_to_unconfigured_when_config_write_fails() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new().expect("tempdir");
    let brain_path = tmp.path();
    allow_external_generation(brain_path);

    let state = InferenceState(Mutex::new(GenerationProvider::Unconfigured));
    let config = GenerationConfig {
        provider: GenerationProviderKind::External,
        model_path: None,
        model_name: Some("gpt-3.5-turbo".to_string()),
        external_url: Some("https://api.openai.com/v1".to_string()),
        api_key: Some("sk-test".to_string()),
    };

    let original_perms = std::fs::metadata(brain_path)
        .expect("metadata")
        .permissions();
    std::fs::set_permissions(brain_path, PermissionsExt::from_mode(0o500))
        .expect("make brain dir read-only");

    let err = update_provider_with_brain_path(brain_path, config, &state, None).unwrap_err();

    std::fs::set_permissions(brain_path, original_perms).expect("restore permissions");

    assert!(err.contains("settings could not be saved to disk"));
    assert!(matches!(
        *state.0.lock().unwrap(),
        GenerationProvider::Unconfigured
    ));
}

#[cfg(unix)]
#[test]
fn update_provider_preserves_state_when_config_and_rollback_fail() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new().expect("tempdir");
    let brain_path = tmp.path();
    allow_external_generation(brain_path);

    let state = InferenceState(Mutex::new(GenerationProvider::External {
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: Some("sk-test".to_string()),
        model_name: "gpt-3.5-turbo".to_string(),
    }));
    let config = GenerationConfig {
        provider: GenerationProviderKind::External,
        model_path: None,
        model_name: Some("gpt-3.5-turbo".to_string()),
        external_url: Some("https://api.openai.com/v1".to_string()),
        api_key: Some("sk-test".to_string()),
    };

    let original_perms = std::fs::metadata(brain_path)
        .expect("metadata")
        .permissions();
    std::fs::set_permissions(brain_path, PermissionsExt::from_mode(0o500))
        .expect("make brain dir read-only");

    let err = update_provider_with_brain_path(brain_path, config, &state, None).unwrap_err();

    std::fs::set_permissions(brain_path, original_perms).expect("restore permissions");

    assert!(err.contains("rollback failed"));
    let guard = state.0.lock().unwrap();
    if let GenerationProvider::External {
        base_url,
        api_key,
        model_name,
    } = &*guard
    {
        assert_eq!(base_url, "https://api.openai.com/v1");
        assert_eq!(api_key.as_deref(), Some("sk-test"));
        assert_eq!(model_name, "gpt-3.5-turbo");
    } else {
        panic!("expected state to preserve existing external provider");
    }
}
