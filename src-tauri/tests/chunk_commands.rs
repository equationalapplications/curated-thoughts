mod helpers;
use helpers::TestApp;
use serde_json::json;
use tauri_app_lib::chunker::{Chunk, ChunkStrategyTag};
use tauri_app_lib::db::{insert_chunk, mark_document_indexed, upsert_document};

/// Regression test for the v1.19.0 wire-name bug: the frontend invokes
/// "resolve_chunk_overlay" (src/lib/tauri.ts), and Tauri v2 registers
/// commands under the exact fn ident — so the Rust fn must be named
/// `resolve_chunk_overlay`, not `resolve_chunk_overlay_cmd`.
#[test]
fn resolve_chunk_overlay_is_invocable_by_frontend_name() {
    let app = TestApp::new();
    let resolved: Option<serde_json::Value> = app.invoke(
        "resolve_chunk_overlay",
        json!({ "path": "/nope.md", "hash": "h" }),
    );
    assert_eq!(resolved, None);
}

fn seed_doc_with_hashed_chunk(conn: &rusqlite::Connection, path: &str) -> String {
    let doc_id = upsert_document(conn, path, "h").unwrap();
    mark_document_indexed(conn, doc_id).unwrap();
    insert_chunk(
        conn,
        doc_id,
        &Chunk {
            text: "the exact passage".into(),
            start_line: 2,
            end_line: 4,
            symbol_name: None,
            defined_symbol: None,
            strategy: ChunkStrategyTag::Prose,
        },
        0,
        "tier_fact",
        "deadbeef",
    )
    .unwrap();
    "deadbeef".to_string()
}

#[test]
fn fetch_chunk_content_returns_text_by_path_and_hash() {
    let app = TestApp::new();
    let conn = app.open_db();
    let hash = seed_doc_with_hashed_chunk(&conn, "documents/notes.md");
    drop(conn);
    let text: Option<String> = app.invoke(
        "fetch_chunk_content",
        json!({ "path": "documents/notes.md", "hash": hash }),
    );
    assert_eq!(text.as_deref(), Some("the exact passage"));
}

#[test]
fn fetch_chunk_content_returns_null_for_unknown_hash() {
    let app = TestApp::new();
    let conn = app.open_db();
    let _ = seed_doc_with_hashed_chunk(&conn, "documents/notes.md");
    drop(conn);
    let text: Option<String> = app.invoke(
        "fetch_chunk_content",
        json!({ "path": "documents/notes.md", "hash": "missing" }),
    );
    assert_eq!(text, None);
}
