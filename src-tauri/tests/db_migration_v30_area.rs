//! V30 migration tests (spec 2026-11-14-areas §5, PR #352 Task 1).
//!
//! `llm_wiki_entries.area TEXT NULL` must exist in EVERY fresh brain and be
//! added by the migration ladder to a brain stamped at V29. NULL means
//! whole-repo; legacy rows keep `area = NULL` (no backfill, spec §2 Q9).

use rusqlite::Connection;
use tauri_app_lib::db::connection::migrate_open_db;
use tauri_app_lib::retrieval::AppDb;
use tempfile::TempDir;

/// A rooted temp brain: `config.json` names the vault so V22 (and everything
/// gated on it) stamps — the ct_age_fixture pattern.
fn rooted_fresh_brain(tmp: &TempDir) -> Connection {
    let vault = tmp.path().join("vault");
    std::fs::create_dir_all(&vault).unwrap();
    std::fs::write(
        tmp.path().join("config.json"),
        format!(r#"{{"vault_path":"{}"}}"#, vault.to_str().unwrap()),
    )
    .unwrap();
    let db = AppDb::open_with_config(&tmp.path().join("brain.db"), &tmp.path().join("config.json"))
        .expect("writable brain db open");
    db.0
}

/// (type, notnull, dflt_value) for one column of `llm_wiki_entries`.
fn column_info(conn: &Connection, name: &str) -> Option<(String, i64, Option<String>)> {
    conn.query_row(
        "SELECT type, \"notnull\", \"dflt_value\"
           FROM pragma_table_info('llm_wiki_entries')
          WHERE name = ?1",
        [name],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .ok()
}

fn has_column(conn: &Connection, table: &str, name: &str) -> bool {
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2",
            [table, name],
            |r| r.get(0),
        )
        .unwrap();
    n > 0
}

fn schema_version(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_version",
        [],
        |r| r.get(0),
    )
    .unwrap()
}

/// §5.1: a FRESH brain carries the column with type TEXT and default NULL.
/// A fresh-brain-only assertion proves nothing about the migration path —
/// see `v30_area_column_added_when_migrating_a_v29_brain`.
#[test]
fn v30_area_column_present_on_fresh_brain() {
    let tmp = TempDir::new().unwrap();
    let conn = rooted_fresh_brain(&tmp);

    let (ctype, notnull, dflt) = column_info(&conn, "area")
        .expect("fresh brain must carry llm_wiki_entries.area");
    assert_eq!(ctype, "TEXT", "area must be TEXT");
    assert_eq!(notnull, 0, "area must be nullable");
    assert_eq!(dflt, None, "area must default to NULL");

    assert!(
        schema_version(&conn) >= 30,
        "a fresh brain's first open must stamp V30"
    );
}

/// §5.1: a MIGRATED brain stamped at V29 — `llm_wiki_entries` WITHOUT `area`
/// (that absence is the point) but WITH the `librarian_evidence.
/// adjudication_hold` column (V29 is TWO-PHASE: column-add early, late stamp;
/// a fixture missing it would test V29 REPAIR, not V30) — gains the column
/// through `migrate_open_db`, and legacy rows keep `area = NULL` (no backfill).
#[test]
fn v30_area_column_added_when_migrating_a_v29_brain() {
    let tmp = TempDir::new().unwrap();
    let conn = rooted_fresh_brain(&tmp);

    // Rewind to the exact V29 state: drop the V30 column, forget the stamp.
    // (The V29 restore-guard precedent, connection.rs unit tests.)
    // Conditional so the RED run fails on the post-migration assertion, not
    // on the fixture build.
    if has_column(&conn, "llm_wiki_entries", "area") {
        conn.execute("ALTER TABLE llm_wiki_entries DROP COLUMN area", [])
            .unwrap();
        conn.execute("DELETE FROM schema_version WHERE version >= 30", [])
            .unwrap();
    }

    // Pre-state pins: we ARE at V29, with V29's two-phase column present and
    // V30's absent.
    assert_eq!(schema_version(&conn), 29, "fixture must be stamped at V29");
    assert!(
        has_column(&conn, "librarian_evidence", "adjudication_hold"),
        "fixture must carry the V29 adjudication_hold column (else this \
         would exercise V29 REPAIR, not V30)"
    );
    assert!(
        !has_column(&conn, "llm_wiki_entries", "area"),
        "fixture llm_wiki_entries must be V29-schema (no area column)"
    );

    // A legacy row written before V30: the INSERT names its columns
    // explicitly, so `area` is absent → stays NULL after migration (no
    // backfill, §2 Q9).
    conn.execute(
        "INSERT INTO llm_wiki_entries (
            id, entity_id, title, body, tags, confidence, source_type,
            source_hash, source_ref, created_at, updated_at, last_accessed_at,
            access_count, deleted_at, embedding_blob, embed_scheme, embedding, tier
         ) VALUES ('legacy-fact', 'e1', 't', 'b', '[]', 0.5, 'doc',
                   'h', 'ref', 1, 1, NULL, 0, NULL, NULL, 'raw', NULL, NULL)",
        [],
    )
    .unwrap();

    migrate_open_db(&conn, tmp.path().parent()).expect("migrate must upgrade the V29 brain");

    let (ctype, notnull, dflt) = column_info(&conn, "area")
        .expect("migration must add llm_wiki_entries.area");
    assert_eq!(ctype, "TEXT", "migrated area must be TEXT");
    assert_eq!(notnull, 0, "migrated area must be nullable");
    assert_eq!(dflt, None, "migrated area must default to NULL");

    assert!(
        schema_version(&conn) >= 30,
        "the migrated brain must stamp V30"
    );

    let legacy_area: Option<String> = conn
        .query_row(
            "SELECT area FROM llm_wiki_entries WHERE id = 'legacy-fact'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        legacy_area, None,
        "legacy rows must keep area = NULL (no backfill)"
    );
}
