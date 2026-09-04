mod helpers;
use helpers::TestApp;
use serde_json::json;
use tauri_app_lib::db::proposals::{
    insert_proposal, NewProposal, NewProposalItem, NewProposalSource, ProposalKind,
    ProposalSourceRole, StoredEvidenceChunk,
};

fn seed_pending_proposal(
    app: &TestApp,
    proposal_id: &str,
    target_name: &str,
    fact_body: &str,
) -> i64 {
    let conn = app.open_db();
    conn.execute(
        "INSERT INTO documents (path, hash, tier, status) VALUES ('/vault/documents/src.pdf', 'hash1', 'user_doc', 'indexed')",
        [],
    )
    .unwrap();
    let doc_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO chunks (doc_id, chunk_text, position, start_line, end_line) VALUES (?1, 'evidence quote', 0, 1, 3)",
        [doc_id],
    )
    .unwrap();
    let chunk_id = conn.last_insert_rowid();

    insert_proposal(
        &conn,
        &NewProposal {
            id: proposal_id.into(),
            kind: ProposalKind::NewEntity,
            entity_id: None,
            proposed_name: Some(target_name.into()),
            proposed_type: Some("concept".into()),
            reasoning: Some("Librarian reasoning for this proposal.".into()),
            model: "test-model".into(),
        },
        &[NewProposalItem {
            id: format!("{proposal_id}-item"),
            item_type: "fact_add".into(),
            target_id: None,
            payload: serde_json::json!({
                "body": fact_body,
                "tags": [],
                "confidence": "inferred"
            }),
            evidence: vec![StoredEvidenceChunk {
                chunk_id: Some(chunk_id),
                content_hash: String::new(),
                quote: "evidence quote".into(),
                start_line: Some(1),
                end_line: Some(3),
                source_kind: None,
            }],
        }],
        &[NewProposalSource {
            doc_id,
            role: ProposalSourceRole::Trigger,
        }],
    )
    .unwrap();

    conn.query_row(
        "SELECT rowid FROM curated_proposals WHERE id = ?1",
        [proposal_id],
        |r| r.get(0),
    )
    .unwrap()
}

#[test]
fn get_review_queue_returns_pending_pages() {
    let app = TestApp::new();
    seed_pending_proposal(&app, "prop-queue", "note.md", "A fact.");

    let queue: Vec<serde_json::Value> = app.invoke("get_review_queue", json!({}));
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0]["path"], "note.md");
    assert_eq!(queue[0]["generated_by"], "test-model");
    assert_eq!(
        queue[0]["reasoning_summary"],
        "Librarian reasoning for this proposal."
    );
}

#[test]
fn get_proposed_content_returns_formatted_preview() {
    let app = TestApp::new();
    let rowid = seed_pending_proposal(&app, "prop-preview", "doc.md", "Generated Content");

    let content: String = app.invoke("get_proposed_content", json!({ "pageId": rowid }));
    assert!(
        content.contains("Generated Content"),
        "expected fact body in preview, got: {content}"
    );
    assert!(content.contains("# doc.md"));
    assert!(content.contains("Librarian reasoning"));
}

#[test]
fn get_proposed_content_resolves_by_rowid_not_wiki_path() {
    let app = TestApp::new();
    let rowid = seed_pending_proposal(&app, "prop-nested", "nested/win", "From nested");

    let content: String = app.invoke("get_proposed_content", json!({ "pageId": rowid }));
    assert!(
        content.contains("From nested"),
        "expected preview via rowid lookup, got: {content}"
    );
}

#[test]
fn approve_wiki_page_commits_proposal_and_clears_queue() {
    // Hermetic embeddings: `approve_wiki_page` / `resolve_proposal_cmd` embed at
    // write time and then run the post-commit sweep, and the sweep resolves its
    // profile from the real `CURATED_BRAIN_*` config rather than the app state.
    // Without the stub this test reads the developer's ~/.brain/config.json and
    // may make live Ollama calls — passing either way, but slow and machine-
    // dependent. `embed_batch` checks this var before the profile.
    // Bind the TempDir: `TempDir::new().path()` would drop the handle at the
    // end of the statement and delete the directory before it is ever used,
    // leaving CURATED_BRAIN_DIR pointing at a path that does not exist.
    let brain_tmp = tempfile::TempDir::new().unwrap();
    let brain = brain_tmp.path().to_string_lossy().into_owned();
    temp_env::with_vars(
        [
            ("CURATED_EMBED_STUB", Some("constant8")),
            // Issue #178: the post-commit sweep resolves config from
            // CURATED_BRAIN_*; keep it off the live ~/.brain.
            ("CURATED_BRAIN_DIR", Some(brain.as_str())),
        ],
        || {
            let app = TestApp::new();
            let rowid =
                seed_pending_proposal(&app, "prop-approve", "page.md", "Approved fact body.");
            let content = "# Ignored markdown\n\nShim ignores content param.";

            app.invoke::<()>(
                "approve_wiki_page",
                json!({
                    "id": rowid,
                    "content": content
                }),
            );

            let queue: Vec<serde_json::Value> = app.invoke("get_review_queue", json!({}));
            assert!(queue.is_empty(), "proposal still in queue after approve");

            let conn = app.open_db();
            let status: String = conn
                .query_row(
                    "SELECT status FROM curated_proposals WHERE id = 'prop-approve'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(status, "approved");

            let fact_count: i64 = conn
                .query_row("SELECT COUNT(*) FROM llm_wiki_entries", [], |r| r.get(0))
                .unwrap();
            assert_eq!(fact_count, 1);

            let body: String = conn
                .query_row("SELECT body FROM llm_wiki_entries LIMIT 1", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(body, "Approved fact body.");
        },
    );
}

#[test]
fn approve_wiki_page_ignores_content_parameter() {
    // Hermetic embeddings: `approve_wiki_page` / `resolve_proposal_cmd` embed at
    // write time and then run the post-commit sweep, and the sweep resolves its
    // profile from the real `CURATED_BRAIN_*` config rather than the app state.
    // Without the stub this test reads the developer's ~/.brain/config.json and
    // may make live Ollama calls — passing either way, but slow and machine-
    // dependent. `embed_batch` checks this var before the profile.
    // Bind the TempDir: `TempDir::new().path()` would drop the handle at the
    // end of the statement and delete the directory before it is ever used,
    // leaving CURATED_BRAIN_DIR pointing at a path that does not exist.
    let brain_tmp = tempfile::TempDir::new().unwrap();
    let brain = brain_tmp.path().to_string_lossy().into_owned();
    temp_env::with_vars(
        [
            ("CURATED_EMBED_STUB", Some("constant8")),
            // Issue #178: the post-commit sweep resolves config from
            // CURATED_BRAIN_*; keep it off the live ~/.brain.
            ("CURATED_BRAIN_DIR", Some(brain.as_str())),
        ],
        || {
            let app = TestApp::new();
            let rowid =
                seed_pending_proposal(&app, "prop-ignore", "wiki/bs-approved.md", "Stored fact.");

            app.invoke::<()>(
                "approve_wiki_page",
                json!({
                    "id": rowid,
                    "content": "# Different content that must be ignored"
                }),
            );

            let conn = app.open_db();
            let body: String = conn
                .query_row("SELECT body FROM llm_wiki_entries LIMIT 1", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(body, "Stored fact.");
        },
    );
}

#[test]
fn reject_wiki_page_marks_proposal_rejected() {
    // Bind the TempDir: `TempDir::new().path()` would drop the handle at the
    // end of the statement and delete the directory before it is ever used,
    // leaving CURATED_BRAIN_DIR pointing at a path that does not exist.
    let brain_tmp = tempfile::TempDir::new().unwrap();
    let brain = brain_tmp.path().to_string_lossy().into_owned();
    temp_env::with_vars(
        [
            // Issue #178: the post-commit sweep resolves config from
            // CURATED_BRAIN_*; keep it off the live ~/.brain.
            ("CURATED_BRAIN_DIR", Some(brain.as_str())),
        ],
        || {
            let app = TestApp::new();
            let rowid = seed_pending_proposal(&app, "prop-reject", "reject.md", "Draft fact");

            app.invoke::<()>("reject_wiki_page", json!({ "id": rowid }));

            let queue: Vec<serde_json::Value> = app.invoke("get_review_queue", json!({}));
            assert!(queue.is_empty(), "proposal still in queue after reject");

            let conn = app.open_db();
            let status: String = conn
                .query_row(
                    "SELECT status FROM curated_proposals WHERE id = 'prop-reject'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(status, "rejected");

            let fact_count: i64 = conn
                .query_row("SELECT COUNT(*) FROM llm_wiki_entries", [], |r| r.get(0))
                .unwrap();
            assert_eq!(fact_count, 0);
        },
    );
}

#[test]
fn list_proposals_cmd_returns_pending_summaries() {
    let app = TestApp::new();
    seed_pending_proposal(&app, "prop-list", "Listed Entity", "Fact.");

    let list: Vec<serde_json::Value> = app.invoke("list_proposals_cmd", json!({}));
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["target_name"], "Listed Entity");
    assert_eq!(list[0]["item_counts"]["facts"], 1);
}

#[test]
fn resolve_proposal_cmd_partial_approval() {
    // Hermetic embeddings: `approve_wiki_page` / `resolve_proposal_cmd` embed at
    // write time and then run the post-commit sweep, and the sweep resolves its
    // profile from the real `CURATED_BRAIN_*` config rather than the app state.
    // Without the stub this test reads the developer's ~/.brain/config.json and
    // may make live Ollama calls — passing either way, but slow and machine-
    // dependent. `embed_batch` checks this var before the profile.
    // Bind the TempDir: `TempDir::new().path()` would drop the handle at the
    // end of the statement and delete the directory before it is ever used,
    // leaving CURATED_BRAIN_DIR pointing at a path that does not exist.
    let brain_tmp = tempfile::TempDir::new().unwrap();
    let brain = brain_tmp.path().to_string_lossy().into_owned();
    temp_env::with_vars(
        [
            ("CURATED_EMBED_STUB", Some("constant8")),
            // Issue #178: the post-commit sweep resolves config from
            // CURATED_BRAIN_*; keep it off the live ~/.brain.
            ("CURATED_BRAIN_DIR", Some(brain.as_str())),
        ],
        || {
            let app = TestApp::new();
            let conn = app.open_db();
            conn.execute(
                "INSERT INTO documents (path, hash, tier, status)
                 VALUES ('/vault/documents/a.pdf', 'h', 'user_doc', 'indexed')",
                [],
            )
            .unwrap();
            let doc_id = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO chunks (doc_id, chunk_text, position) VALUES (?1, 'x', 0)",
                [doc_id],
            )
            .unwrap();
            let chunk_id = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO curated_entities
                    (id, name, entity_type, summary, created_at, updated_at)
                 VALUES ('ent-1', 'Existing', 'concept', 'Sum', 100, 100)",
                [],
            )
            .unwrap();

            insert_proposal(
                &conn,
                &NewProposal {
                    id: "prop-partial".into(),
                    kind: ProposalKind::UpdateEntity,
                    entity_id: Some("ent-1".into()),
                    proposed_name: None,
                    proposed_type: None,
                    reasoning: None,
                    model: "test".into(),
                },
                &[
                    NewProposalItem {
                        id: "item-a".into(),
                        item_type: "fact_add".into(),
                        target_id: None,
                        payload: serde_json::json!({ "body": "Keep", "tags": [], "confidence": "inferred" }),
                        evidence: vec![StoredEvidenceChunk {
                            chunk_id: Some(chunk_id),
                            content_hash: String::new(),
                            quote: "x".into(),
                            start_line: Some(1),
                            end_line: Some(1),
                            source_kind: None,
                        }],
                    },
                    NewProposalItem {
                        id: "item-b".into(),
                        item_type: "fact_add".into(),
                        target_id: None,
                        payload: serde_json::json!({ "body": "Drop", "tags": [], "confidence": "inferred" }),
                        evidence: vec![StoredEvidenceChunk {
                            chunk_id: Some(chunk_id),
                            content_hash: String::new(),
                            quote: "x".into(),
                            start_line: Some(1),
                            end_line: Some(1),
                            source_kind: None,
                        }],
                    },
                ],
                &[NewProposalSource {
                    doc_id,
                    role: ProposalSourceRole::Trigger,
                }],
            )
            .unwrap();

            let result: serde_json::Value = app.invoke(
                "resolve_proposal_cmd",
                json!({
                    "proposalId": "prop-partial",
                    "decisions": [
                        { "item_id": "item-a", "decision": "accept" },
                        { "item_id": "item-b", "decision": "reject" }
                    ]
                }),
            );
            assert_eq!(result["proposal_status"], "partial");

            let fact_count: i64 = conn
                .query_row("SELECT COUNT(*) FROM llm_wiki_entries", [], |r| r.get(0))
                .unwrap();
            assert_eq!(fact_count, 1);
        },
    );
}
