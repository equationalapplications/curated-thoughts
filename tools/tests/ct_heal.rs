//! `ct heal` — write-gated heal subcommand (spec 2026-09-24 §6 + Tests).
//!
//! Contract:
//! - Without `--yes`: exit 1, stderr names the target db path, no mutation.
//! - With `--yes`: the heal core soft-deletes the seeded ungrounded row,
//!   purges its dead-partner edges, writes a `healed` event, and prints
//!   {"evaluated":N,"soft_deleted":N,"edges_purged":N} on stdout.

use std::process::{Command, Output};

use temp_env::with_vars;
use tempfile::tempdir;

mod common;

use common::init_brain_db;

/// Seed one live ungrounded `librarian_inferred` entry (legacy path ref to a
/// document that does not exist in `documents`), one pre-soft-deleted dead
/// partner row, and two edges on the ungrounded row: one to the dead partner
/// (`edge_doomed`, purgeable per the partner-alive retention rule) and one
/// self-edge (`edge_self`, purged too — the heal's own soft-delete makes that
/// endpoint dead within the same transaction).
fn seed_heal_fixture(dir: &std::path::Path) {
    let conn = rusqlite::Connection::open(dir.join("brain.db")).unwrap();
    conn.execute(
        "INSERT INTO llm_wiki_entries (
            id, entity_id, title, body, tags, confidence, source_type,
            source_ref, created_at, updated_at, deleted_at
         ) VALUES ('lost', 'ent-1', 'Title lost', 'body', '[]', 'inferred',
                   'librarian_inferred', 'documents/vanished.md', 1, 1, NULL)",
        [],
    )
    .unwrap();
    // Dead partner: pre-soft-deleted, so edges touching it are purgeable.
    conn.execute(
        "INSERT INTO llm_wiki_entries (
            id, entity_id, title, body, tags, confidence, source_type,
            source_ref, created_at, updated_at, deleted_at
         ) VALUES ('dead', 'ent-1', 'Title dead', 'body', '[]', 'inferred',
                   'librarian_inferred', NULL, 1, 1, 100)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
         VALUES ('edge_doomed', 'ent-1', 'lost', 'dead', 'related_to', 1)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
         VALUES ('edge_self', 'ent-1', 'lost', 'lost', 'related_to', 1)",
        [],
    )
    .unwrap();
}

fn run_ct(dir: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ct"))
        .env("CURATED_BRAIN_DIR", dir)
        .env_remove("CURATED_BRAIN_DB")
        .env_remove("CURATED_BRAIN_CONFIG")
        .args(args)
        .output()
        .unwrap()
}

fn with_seeded_heal_brain<F: FnOnce(&std::path::Path)>(f: F) {
    let brain = tempdir().unwrap();
    let dir = brain.path().to_path_buf();
    let dir_str = dir.to_str().unwrap().to_string();
    with_vars([("CURATED_BRAIN_DIR", Some(dir_str.as_str()))], move || {
        init_brain_db(&dir);
        seed_heal_fixture(&dir);
        f(&dir);
    });
}

#[test]
fn heal_without_yes_exits_one_naming_db_and_does_not_mutate() {
    with_seeded_heal_brain(|dir| {
        let out = run_ct(dir, &["heal"]);
        assert_eq!(
            out.status.code(),
            Some(1),
            "refusal must exit 1, got {} stderr={}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("--yes"), "refusal must point at --yes: {err}");
        assert!(
            err.contains("brain.db"),
            "refusal must name the target db path: {err}"
        );
        // Nothing was actually healed.
        let conn = rusqlite::Connection::open(dir.join("brain.db")).unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_entries WHERE id = 'lost' AND deleted_at IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "refusal must not mutate");
    });
}

#[test]
fn heal_with_yes_heals_purges_and_prints_summary_json() {
    with_seeded_heal_brain(|dir| {
        let out = run_ct(dir, &["heal", "--yes"]);
        assert!(
            out.status.success(),
            "--yes heal failed: {} stderr={}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        let summary: serde_json::Value = serde_json::from_str(stdout.trim())
            .unwrap_or_else(|e| panic!("stdout must be a single JSON object ({e}): {stdout}"));
        assert_eq!(
            summary["evaluated"], 1,
            "only the live row is evaluated: {summary}"
        );
        assert_eq!(
            summary["soft_deleted"], 1,
            "the ungrounded row heals: {summary}"
        );
        assert_eq!(
            summary["edges_purged"], 2,
            "both edges purge: the dead partner AND the self-edge whose other endpoint is the healed row: {summary}"
        );

        let conn = rusqlite::Connection::open(dir.join("brain.db")).unwrap();
        let lost_deleted: Option<i64> = conn
            .query_row(
                "SELECT deleted_at FROM llm_wiki_entries WHERE id = 'lost'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            lost_deleted.is_some(),
            "ungrounded row must be soft-deleted"
        );

        let doomed: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_edges WHERE id = 'edge_doomed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(doomed, 0, "dead-partner edge must be purged");
        let kept: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_edges WHERE id = 'edge_self'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kept, 0, "the self-edge purges once its row is healed");

        let events: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_events WHERE event_type = 'healed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(events, 1, "one healed event for the affected entity");
    });
}

#[test]
fn heal_with_yes_on_clean_brain_exits_zero_with_zero_summary() {
    // M4 (Opus review of PR #228): the nothing-to-do case must be exit 0
    // with a zeroed summary — a no-op heal is a SUCCESS, not an error.
    let brain = tempdir().unwrap();
    let dir = brain.path().to_path_buf();
    let dir_str = dir.to_str().unwrap().to_string();
    with_vars([("CURATED_BRAIN_DIR", Some(dir_str.as_str()))], move || {
        init_brain_db(&dir);
        let out = run_ct(&dir, &["heal", "--yes"]);
        assert!(
            out.status.success(),
            "clean-brain heal must exit 0, got {} stderr={}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        let summary: serde_json::Value = serde_json::from_str(stdout.trim())
            .unwrap_or_else(|e| panic!("stdout must be a single JSON object ({e}): {stdout}"));
        assert_eq!(summary["evaluated"], 0, "{summary}");
        assert_eq!(summary["soft_deleted"], 0, "{summary}");
        assert_eq!(summary["edges_purged"], 0, "{summary}");
    });
}

#[test]
fn heal_refusal_reports_the_live_row_count_it_would_evaluate() {
    // Spec §6: the refusal names the db path AND the live-row count (m1).
    with_seeded_heal_brain(|dir| {
        let out = run_ct(dir, &["heal"]);
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            err.contains("1 live librarian_inferred row"),
            "refusal must include the evaluated-row count: {err}"
        );
    });
}
