//! Integration tests for `ct wiki forget` (issue #163).

use std::fs;
use std::process::{Command, Output};

use temp_env::with_vars;
use tempfile::tempdir;

use tauri_app_lib::db::AppDb;

const ENTITY_ID: &str = "ent_forget_fixture";
const DOOMED_REF: &str = "evidence-doomed";

/// Seed two entries (one doomed, one bystander) plus an edge between them.
fn seed_forget_brain(brain_path: &std::path::Path) {
    fs::write(brain_path.join("config.json"), b"{}\n").unwrap();
    let paths = tauri_app_lib::retrieval::resolve_brain_paths();
    let db = AppDb::open_with_config(&paths.db_path, &paths.config_path)
        .expect("writable brain db open");

    for (id, source_ref) in [("wiki-doomed", Some(DOOMED_REF)), ("wiki-alive", None)] {
        db.0.execute(
            "INSERT INTO llm_wiki_entries
             (id, entity_id, title, body, confidence, created_at, updated_at, source_ref)
             VALUES (?1, ?2, 'T', 'B', 'verified', 1000, 2000, ?3)",
            rusqlite::params![id, ENTITY_ID, source_ref],
        )
        .unwrap();
    }

    db.0.execute(
        "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
         VALUES ('edge-1', ?1, 'wiki-doomed', 'wiki-alive', 'depends_on', 1000)",
        rusqlite::params![ENTITY_ID],
    )
    .unwrap();
}

fn with_forget_brain<F: FnOnce()>(f: F) {
    let brain = tempdir().unwrap();
    let brain_path = brain.path().to_path_buf();
    let brain_path_str = brain_path.to_str().unwrap().to_string();
    with_vars(
        [("CURATED_BRAIN_DIR", Some(brain_path_str.as_str()))],
        move || {
            seed_forget_brain(&brain_path);
            f();
        },
    );
}

fn run_ct(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ct"))
        .args(args)
        .output()
        .expect("spawn ct")
}

/// Open the seeded brain read-only for assertions.
fn count(sql: &str) -> i64 {
    let paths = tauri_app_lib::retrieval::resolve_brain_paths();
    let db = AppDb::open_with_config(&paths.db_path, &paths.config_path).unwrap();
    db.0.query_row(sql, [], |r| r.get(0)).unwrap()
}

/// Open the seeded brain read-only; fetch a single TEXT column.
fn scalar(sql: &str) -> String {
    let paths = tauri_app_lib::retrieval::resolve_brain_paths();
    let db = AppDb::open_with_config(&paths.db_path, &paths.config_path).unwrap();
    db.0.query_row(sql, [], |r| r.get(0)).unwrap()
}

#[test]
fn forget_by_ref_deletes_entry_edges_and_pushes_outbox() {
    with_forget_brain(|| {
        let out = run_ct(&["wiki", "forget", "--ref", DOOMED_REF, "--yes"]);
        assert!(
            out.status.success(),
            "ct wiki forget failed: stderr={}",
            String::from_utf8_lossy(&out.stderr)
        );

        assert_eq!(
            count("SELECT COUNT(*) FROM llm_wiki_entries WHERE id = 'wiki-doomed'"),
            0,
            "the doomed entry must be hard-deleted"
        );
        assert_eq!(
            count("SELECT COUNT(*) FROM llm_wiki_entries WHERE id = 'wiki-alive'"),
            1,
            "the bystander entry must survive"
        );
        assert_eq!(
            count("SELECT COUNT(*) FROM llm_wiki_edges WHERE id = 'edge-1'"),
            0,
            "the edge must be purged when its source is hard-deleted"
        );
        // `push_entries_outbox` writes one row per doomed entry into
        // `llm_wiki_outbox` with `table_name = 'entries'` and
        // `operation = 'DELETE'` (PR #132 contract). This fixture deletes
        // exactly one entry, so there must be exactly ONE outbox DELETE,
        // stamped with that entry's record_id — `>= 1` would silently pass
        // duplicate rows or a DELETE for the wrong record.
        let outbox_deletes: i64 = count(
            "SELECT COUNT(*) FROM llm_wiki_outbox
              WHERE table_name = 'entries' AND operation = 'DELETE'",
        );
        assert_eq!(
            outbox_deletes, 1,
            "exactly one outbox DELETE row for the single doomed entry"
        );
        let deleted_record_id: String = scalar(
            "SELECT record_id FROM llm_wiki_outbox
              WHERE table_name = 'entries' AND operation = 'DELETE'",
        );
        assert_eq!(
            deleted_record_id, "wiki-doomed",
            "the outbox DELETE must carry the doomed entry's record_id"
        );
    });
}

#[test]
fn dry_run_writes_nothing() {
    with_forget_brain(|| {
        let out = run_ct(&["wiki", "forget", "--like", "evidence-", "--dry-run"]);
        assert!(out.status.success(), "dry-run must exit 0");

        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("dry-run"),
            "dry-run must report what it would do, got: {stdout}"
        );
        assert_eq!(
            count("SELECT COUNT(*) FROM llm_wiki_entries WHERE id = 'wiki-doomed'"),
            1,
            "dry-run must not delete"
        );
        assert_eq!(
            count("SELECT COUNT(*) FROM llm_wiki_outbox"),
            0,
            "dry-run must not push outbox rows"
        );
    });
}

#[test]
fn refuses_without_yes() {
    with_forget_brain(|| {
        let out = run_ct(&["wiki", "forget", "--ref", DOOMED_REF]);
        assert_eq!(out.status.code(), Some(1), "must refuse with exit 1");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("refusing"),
            "must explain the refusal"
        );
        assert_eq!(
            count("SELECT COUNT(*) FROM llm_wiki_entries WHERE id = 'wiki-doomed'"),
            1,
            "a refused command must not delete"
        );
    });
}

#[test]
fn rejects_wildcard_in_like() {
    with_forget_brain(|| {
        let out = run_ct(&["wiki", "forget", "--like", "evid%", "--dry-run"]);
        assert!(!out.status.success(), "a `%` in --like must be rejected");
        assert_eq!(
            count("SELECT COUNT(*) FROM llm_wiki_entries WHERE id = 'wiki-doomed'"),
            1
        );
    });
}

#[test]
fn rejects_empty_like_prefix() {
    with_forget_brain(|| {
        // An empty prefix binds LIKE '%' — every non-NULL source_ref. An
        // incident tool that would hard-delete the whole table off an
        // empty flag must refuse instead of widening.
        let out = run_ct(&["wiki", "forget", "--like", "", "--yes"]);
        assert!(
            !out.status.success(),
            "an empty --like prefix must be rejected, got: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            count("SELECT COUNT(*) FROM llm_wiki_entries WHERE id = 'wiki-doomed'"),
            1,
            "nothing may be deleted when --like is empty"
        );
    });
}
