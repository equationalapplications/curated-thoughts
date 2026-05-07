mod helpers;
use helpers::TestApp;
use serde_json::json;

#[test]
fn set_vault_path_creates_subdirs() {
    let app = TestApp::new();
    let vault = app.tmp.path().join("vault");
    std::fs::create_dir_all(&vault).unwrap();

    app.invoke::<()>("set_vault_path", json!({ "path": vault }));

    assert!(vault.join("documents").is_dir(), "documents/ not created");
    assert!(vault.join("wiki").is_dir(), "wiki/ not created");
    assert!(vault.join(".brain").join("converted").is_dir(), ".brain/converted/ not created");
}

#[test]
fn get_vault_path_round_trips() {
    let app = TestApp::new();
    let vault = app.tmp.path().join("vault");
    std::fs::create_dir_all(&vault).unwrap();

    app.invoke::<()>("set_vault_path", json!({ "path": vault }));

    let path: Option<String> = app.invoke("get_vault_path", json!({}));
    assert_eq!(path, Some(vault.to_string_lossy().to_string()));
}
