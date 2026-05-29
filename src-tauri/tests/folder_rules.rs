mod helpers;
use helpers::TestApp;
use serde_json::json;
use tauri_app_lib::inference::config::{write_config, GenerationConfig, GenerationProviderKind, LlmConfig};
use tauri_app_lib::librarian::generate_summary;

fn seed_chunks(app: &TestApp, source_path: &str) {
    let conn = app.open_db();
    conn.execute(
        "INSERT INTO documents (path, hash, tier, status) VALUES (?1, 'hash1', 'user_doc', 'indexed')",
        [source_path],
    ).unwrap();
    let doc_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO chunks (doc_id, chunk_text, position) VALUES (?1, 'test content for summarization', 0)",
        [doc_id],
    ).unwrap();
}

#[test]
fn set_and_get_folder_rules_round_trip() {
    let app = TestApp::new();

    app.invoke::<()>(
        "set_folder_rule",
        json!({
            "folderPath": "/vault/research",
            "librarianMode": "summarize",
            "autoApprove": false
        }),
    );

    let rules: Vec<serde_json::Value> = app.invoke("get_folder_rules", json!({}));
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0]["folder_path"], "/vault/research");
    assert_eq!(rules[0]["librarian_mode"], "summarize");
    assert_eq!(rules[0]["auto_approve"], false);
}

#[test]
fn delete_folder_rule_removes_row() {
    let app = TestApp::new();

    app.invoke::<()>(
        "set_folder_rule",
        json!({
            "folderPath": "/vault/docs",
            "librarianMode": "index",
            "autoApprove": false
        }),
    );

    let rules: Vec<serde_json::Value> = app.invoke("get_folder_rules", json!({}));
    let id = rules[0]["id"].as_i64().unwrap();

    app.invoke::<()>("delete_folder_rule", json!({ "id": id }));

    let rules: Vec<serde_json::Value> = app.invoke("get_folder_rules", json!({}));
    assert!(rules.is_empty(), "rule not deleted");
}

#[test]
fn index_mode_skips_librarian_without_calling_ollama() {
    // OLLAMA_BASE_URL is not set; if librarian calls Ollama it will fail and error
    let app = TestApp::new();
    let source_path = "/vault/documents/note.md";
    seed_chunks(&app, source_path);

    let conn = app.open_db();
    // Insert folder rule: mode=index for the documents directory
    conn.execute(
        "INSERT INTO folder_rules (folder_path, librarian_mode, auto_approve) VALUES ('/vault/documents', 'index', 0)",
        [],
    ).unwrap();

    // Should return Ok without calling Ollama (mode=index returns early)
    let result = generate_summary(&conn, source_path, "test-model");
    assert!(result.is_ok(), "generate_summary failed: {:?}", result);

    // No wiki_pages row should have been created
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM wiki_pages", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0, "no wiki page should be created for index mode");
}

#[test]
fn auto_approve_writes_directly_to_wiki() {
    let app = TestApp::new();
    let vault = app.tmp.path().join("vault");
    std::fs::create_dir_all(vault.join("documents")).unwrap();
    std::fs::create_dir_all(vault.join("wiki")).unwrap();
    std::fs::create_dir_all(vault.join(".brain").join("proposed")).unwrap();
    app.invoke::<()>("set_vault_path", json!({ "path": vault }));

    let source_path = vault.join("documents").join("auto.md");
    let source_str = source_path.to_string_lossy().to_string();
    std::fs::write(&source_path, "Auto-approve test content.").unwrap();
    seed_chunks(&app, &source_str);

    let db_conn = app.open_db();
    db_conn.execute(
        "INSERT INTO folder_rules (folder_path, librarian_mode, auto_approve) VALUES (?1, 'summarize', 1)",
        [&vault.join("documents").to_string_lossy().to_string()],
    )
    .unwrap();

    let mut server = mockito::Server::new();
    let _mock = server
        .mock("POST", "/v1/chat/completions")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("{\"choices\":[{\"message\":{\"content\":\"Auto Wiki Generated content.\"}}]}")
        .create();

    std::env::set_var("CURATED_BRAIN_DIR", app.tmp.path().to_string_lossy().to_string());
    write_config(
        app.tmp.path(),
        &LlmConfig {
            generation: GenerationConfig {
                provider: GenerationProviderKind::External,
                model_path: None,
                model_name: Some("test-model".to_string()),
                external_url: Some(server.url()),
                api_key: None,
            },
            embedding: Default::default(),
        },
    )
    .unwrap();

    let conn = db_conn;
    let result = generate_summary(&conn, &source_str, "test-model");
    assert!(result.is_ok(), "generate_summary failed: {:?}", result);

    // Wiki page written directly to vault/wiki/ (auto_approve=true)
    let wiki_file = vault.join("wiki").join("auto.md");
    assert!(wiki_file.exists(), "wiki file not written for auto_approve");

    // Status should be approved, not pending_review
    let status: String = conn
        .query_row(
            "SELECT status FROM wiki_pages WHERE path = 'auto.md'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "approved");

    // Should NOT appear in review queue (already approved)
    let queue: Vec<serde_json::Value> = app.invoke("get_review_queue", json!({}));
    assert!(
        queue.is_empty(),
        "auto-approved page should not be in review queue"
    );
}
