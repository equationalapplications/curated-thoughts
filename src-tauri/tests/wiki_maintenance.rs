// Phase 4 spec: Database "Heal" and "Prune" Automation
//
// These tests verify the SQL contract that run_wiki_heal and run_wiki_prune
// implement in src-tauri/src/lib.rs. They operate at the DB level (no Tauri
// IPC layer) because the commands are async and their core invariants are SQL.
//
// Heal contract  (run_wiki_heal):
//   - vault-relative source_ref that resolves to an existing file → unchanged
//   - source_ref whose resolved path does not exist              → deleted_at = NOW
//   - absolute paths and parent-traversal refs                   → deleted_at = NOW
//
// Prune contract (run_wiki_prune):
//   - librarian_inferred + soft-deleted > 7 days ago → hard-deleted
//   - librarian_inferred + soft-deleted ≤ 7 days ago → retained
//   - non-librarian_inferred + any soft-delete age   → retained
//   - no deleted_at (not yet soft-deleted)            → retained
//
// CI breakage checklist: if you touch the SQL inside run_wiki_heal or
// run_wiki_prune, update these tests to match.

use rusqlite::Connection;
use tauri_app_lib::vault::{safe_vault_path, PathMode};
use tempfile::TempDir;

fn open_migrated_db(tmp: &TempDir) -> Connection {
    drop(tauri_app_lib::make_test_app(tmp.path()));
    let conn = Connection::open(tmp.path().join("brain.db")).unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    // llm_wiki_entries is owned by core-llm-wiki in production; create a minimal
    // test-only schema matching the columns heal/prune operate on.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS llm_wiki_entries (
            source_ref  TEXT,
            source_type TEXT NOT NULL DEFAULT 'librarian_inferred',
            deleted_at  INTEGER
        );",
    )
    .unwrap();
    conn
}

fn insert_entry(conn: &Connection, source_ref: Option<&str>, source_type: &str) -> i64 {
    conn.execute(
        "INSERT INTO llm_wiki_entries (source_ref, source_type) VALUES (?1, ?2)",
        rusqlite::params![source_ref, source_type],
    )
    .unwrap();
    conn.last_insert_rowid()
}

fn insert_entry_soft_deleted(
    conn: &Connection,
    source_ref: Option<&str>,
    source_type: &str,
    deleted_at: i64,
) -> i64 {
    conn.execute(
        "INSERT INTO llm_wiki_entries (source_ref, source_type, deleted_at) VALUES (?1, ?2, ?3)",
        rusqlite::params![source_ref, source_type, deleted_at],
    )
    .unwrap();
    conn.last_insert_rowid()
}

fn deleted_at(conn: &Connection, rowid: i64) -> Option<i64> {
    conn.query_row(
        "SELECT deleted_at FROM llm_wiki_entries WHERE rowid = ?1",
        [rowid],
        |r| r.get(0),
    )
    .unwrap()
}

fn row_exists(conn: &Connection, rowid: i64) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM llm_wiki_entries WHERE rowid = ?1",
        [rowid],
        |r| r.get::<_, i64>(0),
    )
    .unwrap()
        > 0
}

// ─── heal ─────────────────────────────────────────────────────────────────────

fn run_heal(conn: &Connection, vault_root: &std::path::Path) {
    let entries: Vec<(i64, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT rowid, source_ref FROM llm_wiki_entries
                 WHERE deleted_at IS NULL AND source_ref IS NOT NULL",
            )
            .unwrap();
        let mut rows = stmt.query([]).unwrap();
        let mut v = Vec::new();
        while let Some(row) = rows.next().unwrap() {
            v.push((
                row.get::<_, i64>(0).unwrap(),
                row.get::<_, String>(1).unwrap(),
            ));
        }
        v
    };
    for (rowid, source_ref) in entries {
        let safe = safe_vault_path(vault_root, &source_ref, &["."], PathMode::MustExist);
        if safe.is_err() {
            conn.execute(
                "UPDATE llm_wiki_entries SET deleted_at = unixepoch() WHERE rowid = ?1",
                [rowid],
            )
            .unwrap();
        }
    }
}

#[test]
fn heal_keeps_entry_whose_source_file_exists() {
    let tmp = TempDir::new().unwrap();
    let vault = tmp.path().join("vault");
    std::fs::create_dir_all(&vault).unwrap();
    std::fs::write(vault.join("note.md"), "content").unwrap();

    let conn = open_migrated_db(&tmp);
    let rowid = insert_entry(&conn, Some("note.md"), "librarian_inferred");

    run_heal(&conn, &vault);

    assert!(
        deleted_at(&conn, rowid).is_none(),
        "existing file entry must not be soft-deleted by heal"
    );
}

#[test]
fn heal_soft_deletes_entry_whose_source_file_is_missing() {
    let tmp = TempDir::new().unwrap();
    let vault = tmp.path().join("vault");
    std::fs::create_dir_all(&vault).unwrap();

    let conn = open_migrated_db(&tmp);
    let rowid = insert_entry(&conn, Some("ghost.md"), "librarian_inferred");

    run_heal(&conn, &vault);

    assert!(
        deleted_at(&conn, rowid).is_some(),
        "missing file entry must be soft-deleted by heal"
    );
}

#[test]
fn heal_soft_deletes_absolute_source_ref() {
    let tmp = TempDir::new().unwrap();
    let vault = tmp.path().join("vault");
    std::fs::create_dir_all(&vault).unwrap();

    let conn = open_migrated_db(&tmp);
    let rowid = insert_entry(&conn, Some("/etc/passwd"), "librarian_inferred");

    run_heal(&conn, &vault);

    assert!(
        deleted_at(&conn, rowid).is_some(),
        "absolute source_ref must be treated as missing by heal"
    );
}

#[test]
fn heal_soft_deletes_traversal_source_ref() {
    let tmp = TempDir::new().unwrap();
    let vault = tmp.path().join("vault");
    std::fs::create_dir_all(&vault).unwrap();

    let conn = open_migrated_db(&tmp);
    let rowid = insert_entry(&conn, Some("../escape.md"), "librarian_inferred");

    run_heal(&conn, &vault);

    assert!(
        deleted_at(&conn, rowid).is_some(),
        "parent-traversal source_ref must be treated as missing by heal"
    );
}

#[test]
fn heal_ignores_already_soft_deleted_entries() {
    let tmp = TempDir::new().unwrap();
    let vault = tmp.path().join("vault");
    std::fs::create_dir_all(&vault).unwrap();

    let conn = open_migrated_db(&tmp);
    let original_ts: i64 = 1_000_000;
    let rowid = insert_entry_soft_deleted(&conn, Some("ghost.md"), "librarian_inferred", original_ts);

    run_heal(&conn, &vault);

    assert_eq!(
        deleted_at(&conn, rowid),
        Some(original_ts),
        "heal must not overwrite an already-soft-deleted entry's timestamp"
    );
}

// ─── prune ────────────────────────────────────────────────────────────────────

fn run_prune(conn: &Connection) {
    conn.execute(
        "DELETE FROM llm_wiki_entries
         WHERE source_type = 'librarian_inferred'
           AND deleted_at IS NOT NULL
           AND deleted_at < (unixepoch() - 7 * 86400)",
        [],
    )
    .unwrap();
}

#[test]
fn prune_hard_deletes_old_librarian_inferred_entry() {
    let tmp = TempDir::new().unwrap();
    let conn = open_migrated_db(&tmp);
    let old_ts: i64 = {
        let ts: i64 = conn
            .query_row("SELECT unixepoch()", [], |r| r.get(0))
            .unwrap();
        ts - 8 * 86400
    };
    let rowid = insert_entry_soft_deleted(&conn, Some("stale.md"), "librarian_inferred", old_ts);

    run_prune(&conn);

    assert!(
        !row_exists(&conn, rowid),
        "librarian_inferred entry soft-deleted 8 days ago must be hard-deleted by prune"
    );
}

#[test]
fn prune_retains_recently_soft_deleted_librarian_inferred_entry() {
    let tmp = TempDir::new().unwrap();
    let conn = open_migrated_db(&tmp);
    let six_days_ago: i64 = {
        let ts: i64 = conn
            .query_row("SELECT unixepoch()", [], |r| r.get(0))
            .unwrap();
        ts - 6 * 86400
    };
    let rowid = insert_entry_soft_deleted(&conn, Some("recent.md"), "librarian_inferred", six_days_ago);

    run_prune(&conn);

    assert!(
        row_exists(&conn, rowid),
        "librarian_inferred entry soft-deleted only 6 days ago must be retained by prune"
    );
}

#[test]
fn prune_retains_non_librarian_inferred_entries_regardless_of_age() {
    let tmp = TempDir::new().unwrap();
    let conn = open_migrated_db(&tmp);
    let very_old: i64 = 1_000_000;
    let rowid = insert_entry_soft_deleted(&conn, Some("manual.md"), "manual", very_old);

    run_prune(&conn);

    assert!(
        row_exists(&conn, rowid),
        "non-librarian_inferred entry must not be pruned regardless of age"
    );
}

#[test]
fn prune_retains_entries_not_yet_soft_deleted() {
    let tmp = TempDir::new().unwrap();
    let conn = open_migrated_db(&tmp);
    let rowid = insert_entry(&conn, Some("active.md"), "librarian_inferred");

    run_prune(&conn);

    assert!(
        row_exists(&conn, rowid),
        "librarian_inferred entry with no deleted_at must not be pruned"
    );
}
