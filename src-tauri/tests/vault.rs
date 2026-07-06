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
    assert!(
        vault.join(".brain").join("converted").is_dir(),
        ".brain/converted/ not created"
    );
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

#[test]
fn list_vault_files_returns_forward_slash_relative_paths() {
    let app = TestApp::new();
    let vault = app.tmp.path().join("vault");
    std::fs::create_dir_all(vault.join("wiki")).unwrap();
    std::fs::create_dir_all(vault.join("documents")).unwrap();
    std::fs::write(vault.join("wiki").join("page.md"), "# page").unwrap();
    std::fs::write(vault.join("documents").join("note.md"), "# note").unwrap();

    app.invoke::<()>("set_vault_path", json!({ "path": vault }));

    let files: Vec<serde_json::Value> = app.invoke("list_vault_files", json!({}));
    let doc = files
        .iter()
        .find(|f| f["name"] == "note.md" && f["tier"] == "user_doc")
        .and_then(|f| f["path"].as_str())
        .expect("documents note path not found");
    assert_eq!(doc, "documents/note.md");
    assert!(
        !files.iter().any(|f| f["name"] == "page.md"),
        "wiki/ is archive-only post-V7 and must not appear in list_vault_files"
    );
    assert!(!doc.contains('\\'));
}

#[test]
fn save_wiki_page_accepts_backslash_separators() {
    let app = TestApp::new();
    let vault = app.tmp.path().join("vault");
    std::fs::create_dir_all(&vault).unwrap();
    app.invoke::<()>("set_vault_path", json!({ "path": vault }));

    // Simulate a caller that supplies the path with Windows backslash separators.
    // After normalization: "wiki/backslash.md" — should write correctly.
    app.invoke::<()>(
        "save_wiki_page",
        json!({ "path": "wiki\\backslash.md", "content": "# ok" }),
    );

    let written = app
        .tmp
        .path()
        .join("vault")
        .join("wiki")
        .join("backslash.md");
    assert!(
        written.exists(),
        "expected normalized wiki path to be written"
    );
    assert_eq!(std::fs::read_to_string(&written).unwrap(), "# ok");
}
