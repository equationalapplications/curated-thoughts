//! V7 OKF data conversion fixture tests.

mod helpers;

use helpers::TestApp;
use rusqlite::Connection;
use tauri_app_lib::db::connection::open_in_memory;
use tauri_app_lib::db::okf_migration::{entity_id_from_wiki_path, run_okf_migration};
use tempfile::TempDir;

fn seed_v6_wiki_page(conn: &Connection, path: &str, status: &str, source_doc_ids: &str) {
    conn.execute(
        "INSERT INTO wiki_pages (path, source_doc_ids, generated_by, status)
         VALUES (?1, ?2, 'test-model', ?3)",
        rusqlite::params![path, source_doc_ids, status],
    )
    .unwrap();
}

fn seed_wiki_tier_document(conn: &Connection, doc_path: &str) -> i64 {
    conn.execute(
        "INSERT INTO documents (path, hash, tier, status) VALUES (?1, 'h', 'wiki', 'indexed')",
        [doc_path],
    )
    .unwrap();
    let doc_id: i64 = conn
        .query_row(
            "SELECT id FROM documents WHERE path = ?1",
            [doc_path],
            |r| r.get(0),
        )
        .unwrap();
    conn.execute(
        "INSERT INTO chunks (doc_id, chunk_text, position, start_line, end_line, strategy, entity_id)
         VALUES (?1, 'chunk', 0, 1, 1, 'prose', 'tier_wisdom')",
        [doc_id],
    )
    .unwrap();
    let chunk_id: i64 = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO embeddings (chunk_id, vector) VALUES (?1, X'00000000')",
        [chunk_id],
    )
    .unwrap();
    doc_id
}

#[test]
fn approved_page_with_h1_becomes_entity_and_event() {
    let tmp = TempDir::new().unwrap();
    let vault = tmp.path();
    std::fs::create_dir_all(vault.join("wiki")).unwrap();
    std::fs::write(vault.join("wiki/foo.md"), "# My Entity\n\nFull body.").unwrap();

    let conn = open_in_memory().unwrap();
    seed_v6_wiki_page(&conn, "foo.md", "approved", "[]");

    run_okf_migration(&conn, vault).unwrap();

    let entity_id = entity_id_from_wiki_path("foo.md");
    let (name, summary): (String, String) = conn
        .query_row(
            "SELECT name, summary FROM curated_entities WHERE id = ?1",
            [&entity_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(name, "My Entity");
    assert!(summary.contains("Full body."));

    let event_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM llm_wiki_events WHERE entity_id = ?1 AND event_type = 'imported'",
            [&entity_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(event_count, 1);
}

#[test]
fn approved_page_missing_file_uses_empty_summary_and_stem_name() {
    let tmp = TempDir::new().unwrap();
    let vault = tmp.path();
    std::fs::create_dir_all(vault.join("wiki")).unwrap();

    let conn = open_in_memory().unwrap();
    seed_v6_wiki_page(&conn, "missing.md", "approved", "[]");

    run_okf_migration(&conn, vault).unwrap();

    let entity_id = entity_id_from_wiki_path("missing.md");
    let (name, summary): (String, String) = conn
        .query_row(
            "SELECT name, summary FROM curated_entities WHERE id = ?1",
            [&entity_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(name, "missing");
    assert_eq!(summary, "");
}

#[test]
fn pending_proposals_orphaned_and_sources_requeued() {
    let tmp = TempDir::new().unwrap();
    let vault = tmp.path();
    let proposed = vault.join(".brain").join("proposed");
    std::fs::create_dir_all(&proposed).unwrap();
    std::fs::write(proposed.join("draft.md"), "# Draft").unwrap();

    let conn = open_in_memory().unwrap();
    conn.execute(
        "INSERT INTO documents (path, hash, tier, status) VALUES ('/v/documents/a.pdf', 'h', 'user_doc', 'indexed')",
        [],
    )
    .unwrap();
    let doc_id: i64 = conn.last_insert_rowid();
    let sources = format!("[{doc_id}]");
    seed_v6_wiki_page(&conn, "draft.md", "pending_review", &sources);

    run_okf_migration(&conn, vault).unwrap();

    let status: String = conn
        .query_row(
            "SELECT status FROM wiki_pages WHERE path = 'draft.md'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "orphaned");
    assert!(!proposed.join("draft.md").exists());

    let doc_status: String = conn
        .query_row(
            "SELECT status FROM documents WHERE id = ?1",
            [doc_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(doc_status, "pending");
}

#[test]
fn wiki_tier_documents_and_chunks_purged() {
    let tmp = TempDir::new().unwrap();
    let vault = tmp.path();

    let conn = open_in_memory().unwrap();
    seed_wiki_tier_document(&conn, "/vault/wiki/old-page.md");

    run_okf_migration(&conn, vault).unwrap();

    let wiki_docs: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM documents WHERE tier = 'wiki'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let wisdom_chunks: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM chunks WHERE entity_id = 'tier_wisdom'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(wiki_docs, 0);
    assert_eq!(wisdom_chunks, 0);
}

#[test]
fn migration_idempotent_no_duplicate_entities() {
    let tmp = TempDir::new().unwrap();
    let vault = tmp.path();
    std::fs::create_dir_all(vault.join("wiki")).unwrap();
    std::fs::write(vault.join("wiki/x.md"), "# X\n").unwrap();

    let conn = open_in_memory().unwrap();
    seed_v6_wiki_page(&conn, "x.md", "approved", "[]");

    run_okf_migration(&conn, vault).unwrap();
    run_okf_migration(&conn, vault).unwrap();

    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM curated_entities", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn test_app_open_runs_v7_schema() {
    let app = TestApp::new();
    let conn = app.open_db();
    let max_version: i64 = conn
        .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
        .unwrap();
    // Bumped from 11 to 12 by MIGRATION_V12 (see
    // docs/superpowers/specs/2026-08-26-fix-run-wiki-heal-source-ref-contract.md
    // §4 / §6 — V12 idempotently multiplies seconds-valued
    // `llm_wiki_entries.deleted_at` by 1000 to lock the timestamp-unit
    // contract for the heal writers).
    //
    // Bumped from 12 to 14 by the ingest drain-stall watchdog (see
    // docs/superpowers/specs/2026-08-31-ingest-drain-stall-watchdog-design.md):
    // V13 adds `pipeline_heartbeat`, the `pipeline_stalls` trip journal and the
    // per-path `stall_strikes` ledger (§2.4/§3/§4.2); V14 adds the single-row
    // `system_strikes` ledger for unattributed shared-dependency stalls (§4.2).
    //
    // Bumped from 14 to 15 by MIGRATION_V15, which rebuilds `documents` to
    // widen the status CHECK so the deferred-reindex staging writes
    // ('pending_reindex') are actually accepted.
    //
    // Bumped from 15 to 16 by MIGRATION_V16, which adds `llm_wiki_entries.tier`
    // with a CHECK restricting values to fact/wisdom/NULL (spec §3.1).
    //
    // Bumped from 16 to 17 by the V17 gate, which adds the core-llm-wiki@7.x
    // embedding-failure marker columns (`embedding_failed_at`,
    // `embedding_failure_kind`, `embedding_attempts`) before the startup
    // schema guard runs — the JS package migration otherwise only runs after
    // the frontend boots, too late for the guard.
    // Bumped from 17 to 18 by MIGRATION_V18, which adds the CT-owned
    // `librarian_evidence` table (issue #186 spec §2.1) and runs the one-shot
    // evidence repair. See docs/superpowers/specs/2026-09-06-issue186-*.md.
    assert_eq!(max_version, 18);
}

#[test]
fn v18_creates_librarian_evidence_with_json_check() {
    let conn = open_in_memory().unwrap();

    // Table exists with the expected columns.
    let cols: Vec<String> = {
        let mut stmt = conn
            .prepare("PRAGMA table_info(librarian_evidence)")
            .unwrap();
        let rows = stmt.query_map([], |r| r.get::<_, String>(1)).unwrap();
        rows.filter_map(Result::ok).collect()
    };
    for expected in [
        "entry_id",
        "proposal_id",
        "evidence_json",
        "unanchored",
        "created_at",
    ] {
        assert!(
            cols.iter().any(|c| c == expected),
            "missing column {expected}"
        );
    }

    // The json_valid CHECK must reject a mangled payload loudly.
    conn.execute(
        "INSERT INTO llm_wiki_entries (id, entity_id, title, body, tags, confidence,
             source_type, source_ref, created_at, updated_at, access_count)
         VALUES ('fact_x','ent','t','b','[]','inferred','librarian_inferred',
                 'librarian-00000000000000000000000000000000', 1, 1, 0)",
        [],
    )
    .unwrap();
    let err = conn.execute(
        "INSERT INTO librarian_evidence (entry_id, proposal_id, evidence_json, unanchored, created_at)
         VALUES ('fact_x','prop_1','evidencechunk_id1',0,1)",
        [],
    );
    assert!(
        err.is_err(),
        "json_valid CHECK must reject non-JSON evidence"
    );

    // A valid payload is accepted.
    conn.execute(
        "INSERT INTO librarian_evidence (entry_id, proposal_id, evidence_json, unanchored, created_at)
         VALUES ('fact_x','prop_1','{\"proposal_id\":\"prop_1\",\"evidence\":[]}',0,1)",
        [],
    )
    .unwrap();
}
