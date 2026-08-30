#[test]
fn brain_config_serializes_and_deserializes_round_trip() {
    use tauri_app_lib::config::BrainConfig;
    use tauri_app_lib::embedder::EmbedProfile;

    let config = BrainConfig {
        vault_path: Some("~/my-vault".to_string()),
        embed_profile: Some(EmbedProfile::Local {
            model: "nomic-embed-text".to_string(),
        }),
        migrated_to_v2: true,
        ..Default::default()
    };

    let json = serde_json::to_string(&config).expect("serialize");
    let loaded: BrainConfig = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(loaded.vault_path, Some("~/my-vault".to_string()));
    assert!(matches!(
        loaded.embed_profile,
        Some(EmbedProfile::Local { .. })
    ));
    assert!(loaded.migrated_to_v2);
}

#[test]
fn load_report_tracks_missing_blocks() {
    use tauri_app_lib::config::LoadReport;

    let report = LoadReport {
        config: Default::default(),
        diagnostics: vec!["generation block missing".to_string()],
        generation_missing: true,
        embedding_missing: false,
        vault_path_missing: false,
        privacy_missing: false,
    };

    assert!(report.generation_missing);
    assert!(!report.embedding_missing);
    assert_eq!(report.diagnostics.len(), 1);
}
