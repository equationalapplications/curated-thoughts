// `write_config` is the legacy merge path retained for backward compatibility;
// these tests pin its behavior alongside the new `BrainConfig::write` path.
#![allow(deprecated)]

mod helpers;
use helpers::TestApp;
use serde_json::json;
use tauri_app_lib::inference::config::{
    write_config, GenerationConfig, GenerationProviderKind, LlmConfig,
};
use tauri_app_lib::librarian::generate_summary;

/// Serializes the env-mutating synthesis tests (same pattern as
/// cli_doctor.rs): `CURATED_BRAIN_DIR` is process-wide, so two tests running
/// in parallel race — one's `set_var` lands inside the other's live-brain
/// guard window and trips the issue-#178 TEST BUG panic.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct EnvVarGuard {
    key: String,
    previous: Option<std::ffi::OsString>,
}

impl EnvVarGuard {
    fn new(key: &str) -> Self {
        EnvVarGuard {
            key: key.to_string(),
            previous: std::env::var_os(key),
        }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(previous) = &self.previous {
            std::env::set_var(&self.key, previous);
        } else {
            std::env::remove_var(&self.key);
        }
    }
}

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

    let mut conn = app.open_db();
    // Insert folder rule: mode=index for the documents directory
    conn.execute(
        "INSERT INTO folder_rules (folder_path, librarian_mode, auto_approve) VALUES ('/vault/documents', 'index', 0)",
        [],
    ).unwrap();

    // Should return Ok without calling Ollama (mode=index returns early)
    let result = generate_summary(&mut conn, source_path, "test-model", false);
    assert!(result.is_ok(), "generate_summary failed: {:?}", result);

    // No proposal should have been created
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM curated_proposals", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0, "no proposal should be created for index mode");
}

#[test]
fn auto_approve_commits_proposal_via_resolve_path() {
    let _env_lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let app = TestApp::new();
    let vault = app.tmp.path().join("vault");
    // v2 layout: source tier is immutable-source-files/ (documents/ is v1 and
    // set_vault_path migrates it away).
    std::fs::create_dir_all(vault.join("immutable-source-files")).unwrap();
    app.invoke::<()>("set_vault_path", json!({ "path": vault }));

    let source_path = vault.join("immutable-source-files").join("auto.md");
    let source_str = source_path.to_string_lossy().to_string();
    std::fs::write(&source_path, "Auto-approve test content.").unwrap();
    seed_chunks(&app, &source_str);

    let db_conn = app.open_db();
    db_conn
        .execute(
            "INSERT INTO folder_rules (folder_path, librarian_mode, auto_approve) VALUES (?1, 'synthesize', 1)",
            [&vault.join("immutable-source-files").to_string_lossy().to_string()],
        )
        .unwrap();

    db_conn
        .execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
             VALUES ('ent-auto', 'Auto Entity', 'concept', 'Summary', 100, 100)",
            [],
        )
        .unwrap();

    let mut server = mockito::Server::new();
    let llm_json = serde_json::json!({
        "proposals": [{
            "target": { "existing_id": "ent-auto" },
            "reasoning": "Auto commit test.",
            "summary_update": null,
            "facts": [{
                "op": "add",
                "body": "Auto Wiki Generated content.",
                "tags": [],
                "confidence": "inferred",
                "evidence": ["C1"]
            }],
            "edges": [],
            "tasks": []
        }]
    });
    let _mock = server
        .mock("POST", "/v1/chat/completions")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            "{{\"choices\":[{{\"message\":{{\"content\":{}}}}}]}}",
            serde_json::to_string(&llm_json.to_string()).unwrap()
        ))
        .create();

    let _brain_dir_guard = EnvVarGuard::new("CURATED_BRAIN_DIR");
    std::env::set_var(
        "CURATED_BRAIN_DIR",
        app.tmp.path().to_string_lossy().to_string(),
    );
    write_config(
        app.tmp.path(),
        &LlmConfig {
            generation: GenerationConfig {
                provider: GenerationProviderKind::External,
                model_path: None,
                model_name: Some("test-model".to_string()),
                external_url: Some(server.url()),
                api_key: None,
                timeout_secs: None,
            },
            embedding: Default::default(),
        },
    )
    .unwrap();

    let mut conn = db_conn;
    let result = generate_summary(&mut conn, &source_str, "test-model", false);
    assert!(result.is_ok(), "generate_summary failed: {:?}", result);

    // Proposal auto-approved — no pending queue
    let pending: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM curated_proposals WHERE status = 'pending'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(pending, 0, "auto-approved proposal should not stay pending");

    let fact_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM llm_wiki_entries WHERE source_type = 'librarian_inferred'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        fact_count, 1,
        "auto-approve should commit fact to llm_wiki_entries"
    );

    // Legacy review queue still uses wiki_pages until Task 9 shims
    let queue: Vec<serde_json::Value> = app.invoke("get_review_queue", json!({}));
    assert!(
        queue.is_empty(),
        "auto-approved work should not appear in legacy review queue"
    );
}

#[test]
fn no_auto_approve_leaves_proposal_pending_with_zero_entries() {
    let _env_lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // hvg Task 5 folder-rule gate: the flip semantics counterpart to
    // `auto_approve_commits_proposal_via_resolve_path` — auto_approve = 0 must
    // NOT commit: the proposal row stays 'pending' (queueing a human review)
    // and llm_wiki_entries gains nothing.
    let app = TestApp::new();
    let vault = app.tmp.path().join("vault");
    // v2 layout: source tier is immutable-source-files/ (documents/ is v1 and
    // set_vault_path migrates it away).
    std::fs::create_dir_all(vault.join("immutable-source-files")).unwrap();
    app.invoke::<()>("set_vault_path", json!({ "path": vault }));

    let source_path = vault.join("immutable-source-files").join("manual.md");
    let source_str = source_path.to_string_lossy().to_string();
    std::fs::write(&source_path, "Manual review test content.").unwrap();
    seed_chunks(&app, &source_str);

    let db_conn = app.open_db();
    db_conn
        .execute(
            "INSERT INTO folder_rules (folder_path, librarian_mode, auto_approve) VALUES (?1, 'synthesize', 0)",
            [&vault.join("immutable-source-files").to_string_lossy().to_string()],
        )
        .unwrap();

    db_conn
        .execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
             VALUES ('ent-manual', 'Manual Entity', 'concept', 'Summary', 100, 100)",
            [],
        )
        .unwrap();

    let mut server = mockito::Server::new();
    let llm_json = serde_json::json!({
        "proposals": [{
            "target": { "existing_id": "ent-manual" },
            "reasoning": "Manual gate test.",
            "summary_update": null,
            "facts": [{
                "op": "add",
                "body": "Manual gate fact.",
                "tags": [],
                "confidence": "inferred",
                "evidence": ["C1"]
            }],
            "edges": [],
            "tasks": []
        }]
    });
    let _mock = server
        .mock("POST", "/v1/chat/completions")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            "{{\"choices\":[{{\"message\":{{\"content\":{}}}}}]}}",
            serde_json::to_string(&llm_json.to_string()).unwrap()
        ))
        .create();

    let _brain_dir_guard = EnvVarGuard::new("CURATED_BRAIN_DIR");
    std::env::set_var(
        "CURATED_BRAIN_DIR",
        app.tmp.path().to_string_lossy().to_string(),
    );
    write_config(
        app.tmp.path(),
        &LlmConfig {
            generation: GenerationConfig {
                provider: GenerationProviderKind::External,
                model_path: None,
                model_name: Some("test-model".to_string()),
                external_url: Some(server.url()),
                api_key: None,
                timeout_secs: None,
            },
            embedding: Default::default(),
        },
    )
    .unwrap();

    let mut conn = db_conn;
    let result = generate_summary(&mut conn, &source_str, "test-model", false);
    assert!(result.is_ok(), "generate_summary failed: {:?}", result);

    // The gate: proposal stays pending, nothing committed.
    let pending: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM curated_proposals WHERE status = 'pending'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(pending, 1, "auto_approve=0 must leave the proposal pending");

    let entry_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM llm_wiki_entries", [], |r| r.get(0))
        .unwrap();
    assert_eq!(entry_count, 0, "no entries may be written without review");
}
