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
    conn
}

fn insert_entry(
    conn: &Connection,
    entity_id: &str,
    source_ref: Option<&str>,
    source_type: &str,
) -> i64 {
    let id = format!("entry-{}", conn.last_insert_rowid() + 1);
    conn.execute(
        "INSERT INTO llm_wiki_entries (
            id, entity_id, title, body, tags, confidence, source_type, source_ref,
            created_at, updated_at
         ) VALUES (?1, ?2, ?3, 'body', '[]', 'inferred', ?4, ?5, 1, 1)",
        rusqlite::params![
            id,
            entity_id,
            format!("Title {id}"),
            source_type,
            source_ref
        ],
    )
    .unwrap();
    conn.last_insert_rowid()
}

fn insert_entry_soft_deleted(
    conn: &Connection,
    entity_id: &str,
    source_ref: Option<&str>,
    source_type: &str,
    deleted_at: i64,
) -> i64 {
    let id = format!("entry-{}", conn.last_insert_rowid() + 1);
    conn.execute(
        "INSERT INTO llm_wiki_entries (
            id, entity_id, title, body, tags, confidence, source_type, source_ref,
            created_at, updated_at, deleted_at
         ) VALUES (?1, ?2, ?3, 'body', '[]', 'inferred', ?4, ?5, 1, 1, ?6)",
        rusqlite::params![
            id,
            entity_id,
            format!("Title {id}"),
            source_type,
            source_ref,
            deleted_at
        ],
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
    let entries: Vec<(i64, String, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT e.rowid, e.source_ref, e.entity_id
                 FROM llm_wiki_entries e
                 WHERE e.deleted_at IS NULL AND e.source_ref IS NOT NULL",
            )
            .unwrap();
        let mut rows = stmt.query([]).unwrap();
        let mut v = Vec::new();
        while let Some(row) = rows.next().unwrap() {
            v.push((
                row.get::<_, i64>(0).unwrap(),
                row.get::<_, String>(1).unwrap(),
                row.get::<_, String>(2).unwrap(),
            ));
        }
        v
    };
    let mut healed_by_entity: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for (rowid, source_ref, entity_id) in entries {
        let safe = safe_vault_path(vault_root, &source_ref, &["."], PathMode::MustExist);
        if safe.is_err() {
            conn.execute(
                "UPDATE llm_wiki_entries SET deleted_at = unixepoch() WHERE rowid = ?1",
                [rowid],
            )
            .unwrap();
            *healed_by_entity.entry(entity_id).or_insert(0) += 1;
        }
    }
    // Write healed events
    if !healed_by_entity.is_empty() {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        for (entity_id, n) in healed_by_entity {
            let _ = conn.execute(
                "INSERT INTO llm_wiki_events (id, entity_id, event_type, summary, related_entry_id, created_at)
                 VALUES (?1, ?2, 'healed', ?3, NULL, ?4)",
                rusqlite::params![
                    format!("evt_{}", rand::random::<u64>()),
                    entity_id,
                    format!("Healed {n} invalid source reference(s)"),
                    now_ms,
                ],
            );
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
    let rowid = insert_entry(&conn, "tier_fact", Some("note.md"), "librarian_inferred");

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
    let rowid = insert_entry(&conn, "tier_fact", Some("ghost.md"), "librarian_inferred");

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
    let rowid = insert_entry(
        &conn,
        "tier_fact",
        Some("/etc/passwd"),
        "librarian_inferred",
    );

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
    let rowid = insert_entry(
        &conn,
        "tier_fact",
        Some("../escape.md"),
        "librarian_inferred",
    );

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
    let rowid = insert_entry_soft_deleted(
        &conn,
        "tier_fact",
        Some("ghost.md"),
        "librarian_inferred",
        original_ts,
    );

    run_heal(&conn, &vault);

    assert_eq!(
        deleted_at(&conn, rowid),
        Some(original_ts),
        "heal must not overwrite an already-soft-deleted entry's timestamp"
    );
}

#[test]
fn heal_writes_healed_event() {
    let tmp = TempDir::new().unwrap();
    let vault = tmp.path().join("vault");
    std::fs::create_dir_all(&vault).unwrap();

    let conn = open_migrated_db(&tmp);
    // Insert an entity
    conn.execute(
        "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
         VALUES ('ent-1', 'Test Entity', 'concept', 'Summary', 100, 100)",
        [],
    )
    .unwrap();
    // Insert an entry with invalid source_ref
    let _rowid = insert_entry(&conn, "ent-1", Some("../escape.md"), "librarian_inferred");

    run_heal(&conn, &vault);

    // Verify healed event was written
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM llm_wiki_events WHERE event_type = 'healed' AND entity_id = 'ent-1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
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
    let rowid = insert_entry_soft_deleted(
        &conn,
        "tier_fact",
        Some("stale.md"),
        "librarian_inferred",
        old_ts,
    );

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
    let rowid = insert_entry_soft_deleted(
        &conn,
        "tier_fact",
        Some("recent.md"),
        "librarian_inferred",
        six_days_ago,
    );

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
    let rowid =
        insert_entry_soft_deleted(&conn, "tier_fact", Some("manual.md"), "manual", very_old);

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
    let rowid = insert_entry(&conn, "tier_fact", Some("active.md"), "librarian_inferred");

    run_prune(&conn);

    assert!(
        row_exists(&conn, rowid),
        "librarian_inferred entry with no deleted_at must not be pruned"
    );
}
