mod helpers;
use helpers::TestApp;
use serde_json::json;

fn seed_pending_page(app: &TestApp, filename: &str, content: &str) -> i64 {
    // Write proposed content to tmp/.brain/proposed/
    let proposed_dir = app.tmp.path().join(".brain").join("proposed");
    std::fs::create_dir_all(&proposed_dir).unwrap();
    std::fs::write(proposed_dir.join(filename), content).unwrap();

    // Seed wiki_pages row with status='pending_review'
    let conn = app.open_db();
    conn.execute(
        "INSERT INTO wiki_pages (path, source_doc_ids, generated_by, status)
         VALUES (?1, '[]', 'test-model', 'pending_review')",
        [filename],
    )
    .unwrap();
    conn.last_insert_rowid()
}

#[test]
fn get_review_queue_returns_pending_pages() {
    let app = TestApp::new();
    seed_pending_page(&app, "note.md", "# Note");

    let queue: Vec<serde_json::Value> = app.invoke("get_review_queue", json!({}));
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0]["path"], "note.md");
    assert_eq!(queue[0]["generated_by"], "test-model");
}

#[test]
fn get_proposed_content_returns_file_contents() {
    let app = TestApp::new();
    let id = seed_pending_page(&app, "doc.md", "# Generated Content");

    // set_vault_path needed so get_proposed_content can find the file
    app.invoke::<()>("set_vault_path", json!({ "path": app.tmp.path() }));

    // get_proposed_content reads from {vault}/.brain/proposed/
    // Copy to vault's .brain/proposed/ since set_vault_path changed config
    let vault_proposed = app.tmp.path().join(".brain").join("proposed");
    std::fs::create_dir_all(&vault_proposed).unwrap();
    std::fs::write(vault_proposed.join("doc.md"), "# Generated Content").unwrap();

    let content: String = app.invoke("get_proposed_content", json!({ "pageId": id }));
    assert!(
        content.contains("Generated Content"),
        "expected content, got: {content}"
    );
}

#[test]
fn approve_wiki_page_writes_file_and_marks_approved() {
    let app = TestApp::new();
    let vault = app.tmp.path().join("vault");
    std::fs::create_dir_all(&vault).unwrap();
    app.invoke::<()>("set_vault_path", json!({ "path": vault }));

    let id = seed_pending_page(&app, "page.md", "# Wiki");
    let content = "# Approved Wiki Page\n\nContent.";

    app.invoke::<()>(
        "approve_wiki_page",
        json!({
            "id": id,
            "content": content
        }),
    );

    // File written to vault/wiki/
    let wiki_file = vault.join("wiki").join("page.md");
    assert!(wiki_file.exists(), "wiki file not written");
    assert_eq!(std::fs::read_to_string(&wiki_file).unwrap(), content);

    // Status updated; no longer in queue
    let queue: Vec<serde_json::Value> = app.invoke("get_review_queue", json!({}));
    assert!(queue.is_empty(), "page still in queue after approve");

    let conn = app.open_db();
    let status: String = conn
        .query_row("SELECT status FROM wiki_pages WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(status, "approved");
}

#[test]
fn approve_wiki_page_accepts_backslash_wiki_path() {
    let app = TestApp::new();
    let vault = app.tmp.path().join("vault");
    std::fs::create_dir_all(&vault).unwrap();
    app.invoke::<()>("set_vault_path", json!({ "path": vault }));

    // Simulate a DB row whose path uses backslash separators (possible on Windows).
    // After normalization: "wiki/bs-approved.md" — must not double-prefix.
    let id = seed_pending_page(&app, "wiki\\bs-approved.md", "# Wiki");
    let content = "# Approved";

    app.invoke::<()>(
        "approve_wiki_page",
        json!({
            "id": id,
            "content": content
        }),
    );

    let wiki_file = vault.join("wiki").join("bs-approved.md");
    assert!(
        wiki_file.exists(),
        "wiki file not written at normalized path"
    );
    assert_eq!(std::fs::read_to_string(&wiki_file).unwrap(), content);
}

#[test]
fn reject_wiki_page_does_not_write_file_and_marks_rejected() {
    let app = TestApp::new();
    let vault = app.tmp.path().join("vault");
    std::fs::create_dir_all(vault.join("wiki")).unwrap();
    app.invoke::<()>("set_vault_path", json!({ "path": vault }));

    let id = seed_pending_page(&app, "reject.md", "# Draft");

    app.invoke::<()>("reject_wiki_page", json!({ "id": id }));

    // No file written
    assert!(
        !vault.join("wiki").join("reject.md").exists(),
        "file should not exist after reject"
    );

    // No longer in queue
    let queue: Vec<serde_json::Value> = app.invoke("get_review_queue", json!({}));
    assert!(queue.is_empty(), "page still in queue after reject");

    let conn = app.open_db();
    let status: String = conn
        .query_row("SELECT status FROM wiki_pages WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(status, "rejected");
}
