//! Integration tests that lock the `deleted_at` timestamp-unit contract for
//! the heal path (Bug C). Pinned by the spec section 4 — the off-by-one-zero
//! regression caught in spec review is regression-locked by U1/U2.
//!
//! These tests run as a separate `cargo test --test timestamp_units` target
//! so the source-ref/MIGRATION_V12 contract can be verified without
//! dragging the full Tauri-app-lib test suite.

use rusqlite::{params, Connection};
use tauri_app_lib::db::commit::{ms_now, source_ref_is_still_grounded};
use tauri_app_lib::db::connection::open_in_memory;
use tauri_app_lib::db::schema::{MIGRATION_V12, SEC_VS_MS_THRESHOLD};

/// Helper: insert an `llm_wiki_entries` row with a caller-supplied
/// `deleted_at` so the MIGRATION_V12 boundary tests can verify upgrade
/// behavior without spinning up the full commit pipeline.
fn insert_entries_row(
    conn: &Connection,
    id: &str,
    source_type: &str,
    source_ref: Option<&str>,
    deleted_at: Option<i64>,
) {
    conn.execute(
        "INSERT INTO llm_wiki_entries
            (id, entity_id, title, body, tags, confidence, source_type,
             source_ref, created_at, updated_at, deleted_at)
         VALUES (?1, 'ent', 't', 'b', '[]', 'inferred', ?2, ?3, 1, 1, ?4)",
        params![id, source_type, source_ref, deleted_at],
    )
    .unwrap();
}

/// Pin the threshold constant at twelve zeros. U1 is the spec review
/// regression lock — see "CRITICAL: 12 zeros, not 11" in the task brief.
#[test]
fn u1_threshold_constant_is_twelve_zeros() {
    assert_eq!(
        SEC_VS_MS_THRESHOLD, 1_000_000_000_000,
        "SEC_VS_MS_THRESHOLD must be twelve zeros (1e12)"
    );
}

/// Pin the off-by-one-zero variant as DIFFERENT from the threshold.
#[test]
fn u2_off_by_one_zero_eleven_zeros_is_below_threshold() {
    let eleven_zeros: i64 = 100_000_000_000;
    assert_ne!(
        eleven_zeros, SEC_VS_MS_THRESHOLD,
        "eleven-zero sentinel must NOT equal the threshold (off-by-one guard)"
    );
    assert!(
        eleven_zeros < SEC_VS_MS_THRESHOLD,
        "eleven-zero sentinel ({eleven_zeros}) must be below the threshold ({}), \
         so MIGRATION_V12 will promote it",
        SEC_VS_MS_THRESHOLD
    );
}

/// `ms_now()` must return a millisecond-precision timestamp that is well
/// past the seconds-vs-ms boundary.
#[test]
fn u3_ms_now_returns_value_above_threshold() {
    let now = ms_now();
    assert!(
        now >= SEC_VS_MS_THRESHOLD,
        "ms_now() must return ms (>= threshold); got {now}"
    );
    // Belt-and-braces: also assert it's clearly not a seconds value.
    // 1.7e9 seconds = ~2023-11; 1.7e12 ms = ~2023-11. The boundary check
    // alone is sufficient, but the magnitude sanity check documents intent.
    assert!(now > 1_700_000_000_000, "ms_now() magnitude looks like ms");
}

/// `source_ref_is_still_grounded` is reachable from an integration test
/// (not just internal unit tests) and reports true for empty strings.
#[test]
fn u4_source_ref_helper_is_pub_crate_and_handles_empty() {
    let conn = open_in_memory().unwrap();
    assert!(source_ref_is_still_grounded(&conn, ""));
    assert!(source_ref_is_still_grounded(&conn, "   "));
}

/// MIGRATION_V12 is exposed as a string constant we can drive manually
/// (the spec requires a fresh-from-migration smoke that doesn't rely on the
/// `migrate()` gate, so this integration test can simulate partial upgrade
/// states).
#[test]
fn u5_migration_v12_constant_is_exposed_and_idempotent_sql() {
    assert!(MIGRATION_V12.contains("llm_wiki_entries"));
    assert!(MIGRATION_V12.contains("deleted_at * 1000"));
    // Idempotency hint: the UPDATE body is wrapped in an INSERT-or-IGNORE
    // schema_version bump so re-running is harmless.
    assert!(MIGRATION_V12.contains("schema_version"));
}

/// U6 — The migration promotes seconds-valued rows to ms; an already-ms
/// row is left alone.
#[test]
fn u6_v12_promotes_seconds_leaves_ms_alone() {
    let conn = open_in_memory().unwrap();
    insert_entries_row(
        &conn,
        "seconds-row",
        "librarian_inferred",
        Some("documents/gone.md"),
        Some(1_750_000_000),
    );
    insert_entries_row(
        &conn,
        "ms-row",
        "librarian_inferred",
        Some("documents/notes.md"),
        Some(SEC_VS_MS_THRESHOLD + 60_000),
    );

    conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V12))
        .unwrap();

    let read = |id: &str| -> Option<i64> {
        conn.query_row(
            "SELECT deleted_at FROM llm_wiki_entries WHERE id = ?1",
            [id],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(
        read("seconds-row"),
        Some(1_750_000_000 * 1000),
        "seconds-valued row must be promoted"
    );
    assert_eq!(
        read("ms-row"),
        Some(SEC_VS_MS_THRESHOLD + 60_000),
        "already-ms row must NOT be multiplied"
    );
}

/// U7 — Running V12 twice on the same data is a no-op.
#[test]
fn u7_v12_is_idempotent_on_same_data() {
    let conn = open_in_memory().unwrap();
    insert_entries_row(
        &conn,
        "row-1",
        "librarian_inferred",
        Some("documents/a.md"),
        Some(1_700_000_000),
    );
    insert_entries_row(
        &conn,
        "row-2",
        "user_stated",
        Some(r#"{"proposal_id":null,"evidence":[]}"#),
        Some(1_710_000_000),
    );

    conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V12))
        .unwrap();
    let first_snap = read_all_deleted_at(&conn);
    conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V12))
        .unwrap();
    let second_snap = read_all_deleted_at(&conn);

    assert_eq!(
        first_snap, second_snap,
        "second V12 run must not change any deleted_at value"
    );
}

/// U8 — V12 promotes the only-healed-once pattern: a row whose `deleted_at`
/// was written in seconds (the only known buggy unit) is converted to ms;
/// everything else is untouched.
#[test]
fn u8_v12_targeted_at_seconds_only() {
    let conn = open_in_memory().unwrap();
    // Bug C: a row whose deleted_at was the only known bad unit.
    insert_entries_row(
        &conn,
        "buggy-heal-writer",
        "librarian_inferred",
        Some("documents/gone.md"),
        Some(1_787_658_855), // observed on live DB per spec §C
    );
    // Healthy producers (commit.rs:733 / facts.rs:232 — ms).
    insert_entries_row(
        &conn,
        "healthy-commit",
        "user_stated",
        Some(r#"{"proposal_id":null,"evidence":[]}"#),
        Some(SEC_VS_MS_THRESHOLD + 1),
    );
    insert_entries_row(
        &conn,
        "healthy-ms-2",
        "librarian_inferred",
        Some("documents/notes.md"),
        Some(SEC_VS_MS_THRESHOLD * 2),
    );

    conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V12))
        .unwrap();

    assert_eq!(
        read_deleted_at(&conn, "buggy-heal-writer"),
        Some(1_787_658_855 * 1000),
        "Bug C row must be promoted"
    );
    assert_eq!(
        read_deleted_at(&conn, "healthy-commit"),
        Some(SEC_VS_MS_THRESHOLD + 1),
        "healthy ms row must be untouched"
    );
    assert_eq!(
        read_deleted_at(&conn, "healthy-ms-2"),
        Some(SEC_VS_MS_THRESHOLD * 2),
        "healthy ms row must be untouched"
    );
}

/// U9 — After V12, the post-condition `deleted_at >= SEC_VS_MS_THRESHOLD`
/// holds for every non-NULL row whose pre-migration value was a realistic
/// seconds-valued soft-delete (i.e. a unixepoch()-sized integer).
#[test]
fn u9_v12_enforces_minimum_threshold_for_all_rows() {
    let conn = open_in_memory().unwrap();
    // Seed a realistic mix of seconds-valued rows (the only class V12
    // actually promotes) and ms-valued rows (left alone).
    for (i, val) in [
        1_750_000_000i64,        // ~2025 in seconds — must be promoted
        SEC_VS_MS_THRESHOLD - 1, // just below threshold in ms — must be promoted
        SEC_VS_MS_THRESHOLD + 1, // just above threshold in ms — must stay
    ]
    .iter()
    .enumerate()
    {
        insert_entries_row(
            &conn,
            &format!("row-{i}"),
            "librarian_inferred",
            Some("documents/x.md"),
            Some(*val),
        );
    }

    conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V12))
        .unwrap();

    let below: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM llm_wiki_entries
             WHERE deleted_at IS NOT NULL
               AND deleted_at < ?1",
            [SEC_VS_MS_THRESHOLD],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        below, 0,
        "no row may remain below SEC_VS_MS_THRESHOLD after V12"
    );
}

/// U10 — NULL `deleted_at` rows survive V12 with NULL intact.
#[test]
fn u10_v12_leaves_null_deleted_at_alone() {
    let conn = open_in_memory().unwrap();
    insert_entries_row(
        &conn,
        "live",
        "librarian_inferred",
        Some("documents/notes.md"),
        None,
    );
    conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V12))
        .unwrap();
    assert_eq!(read_deleted_at(&conn, "live"), None);
}

/// U11 — Mixed-unit healing: a column that already has both units (one
/// healthy ms row, one buggy seconds row) is fully normalised by V12.
#[test]
fn u11_v12_normalises_mixed_unit_column() {
    let conn = open_in_memory().unwrap();
    insert_entries_row(
        &conn,
        "healthy",
        "user_stated",
        Some(r#"{"proposal_id":null,"evidence":[]}"#),
        Some(SEC_VS_MS_THRESHOLD + 5),
    );
    insert_entries_row(
        &conn,
        "buggy",
        "librarian_inferred",
        Some("documents/gone.md"),
        Some(1_715_000_000),
    );
    conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V12))
        .unwrap();

    let healthy = read_deleted_at(&conn, "healthy").unwrap();
    let buggy = read_deleted_at(&conn, "buggy").unwrap();
    assert!(healthy >= SEC_VS_MS_THRESHOLD);
    assert!(buggy >= SEC_VS_MS_THRESHOLD);
    assert!(
        healthy < buggy,
        "millisecond-stamped healthy row must be older (smaller) than a freshly-healed buggy row"
    );
}

/// U12 — `source_ref_is_still_grounded` is reachable from outside the
/// crate's binary (`pub(crate)`) — pinning visibility.
#[test]
fn u12_source_ref_helper_is_visibility_pub_crate() {
    // This test compiles iff `source_ref_is_still_grounded` is reachable
    // from the integration test crate. The function lives in
    // `crate::db::commit` so a path `curated_thoughts::db::commit::*`
    // import (see top of file) is what makes this work.
    let conn = open_in_memory().unwrap();
    let _ = source_ref_is_still_grounded(&conn, "anything");
}

/// U13 — Boundary exactness: a `deleted_at` exactly equal to the threshold
/// is treated as already-ms (not promoted). Anything strictly less than the
/// threshold is promoted.
#[test]
fn u13_v12_boundary_exact_threshold_not_promoted() {
    let conn = open_in_memory().unwrap();
    insert_entries_row(
        &conn,
        "exact-threshold",
        "librarian_inferred",
        Some("documents/x.md"),
        Some(SEC_VS_MS_THRESHOLD),
    );
    conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V12))
        .unwrap();
    assert_eq!(
        read_deleted_at(&conn, "exact-threshold"),
        Some(SEC_VS_MS_THRESHOLD),
        "value == threshold must be left alone (>= boundary)"
    );
}

/// U14 — Heuristic safety: a value just below the threshold (like
/// `SEC_VS_MS_THRESHOLD - 1`) is promoted to `1000 * (threshold - 1)`,
/// not something spurious like `(threshold - 1) * 1000 = threshold * 1000 - 1000`.
/// This is mostly belt-and-braces for SQL operator precedence.
#[test]
fn u14_v12_multiplication_is_value_times_1000() {
    let conn = open_in_memory().unwrap();
    let v = SEC_VS_MS_THRESHOLD - 1;
    insert_entries_row(
        &conn,
        "just-below",
        "librarian_inferred",
        Some("documents/x.md"),
        Some(v),
    );
    conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V12))
        .unwrap();
    assert_eq!(
        read_deleted_at(&conn, "just-below"),
        Some(v * 1000),
        "V12 multiplies deleted_at by 1000 — not (deleted_at * 1000 + offset)"
    );
}

/// U15 — Migration-version row gets bumped; running V12 twice keeps
/// `schema_version` at the latest value.
#[test]
fn u15_schema_version_bumped_to_12_and_idempotent() {
    // `open_in_memory` applies every migration, so MAX(version) tracks the
    // latest schema, not V12. Assert what this test is actually about: V12
    // records its own version exactly once and re-running it is a no-op.
    let conn = open_in_memory().unwrap();

    let rows_for_12 = |conn: &Connection| -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM schema_version WHERE version = 12",
            [],
            |r| r.get(0),
        )
        .unwrap()
    };
    let max_version = |conn: &Connection| -> i64 {
        conn.query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .unwrap()
    };

    assert_eq!(rows_for_12(&conn), 1, "V12 must record schema_version=12");
    let before = max_version(&conn);

    conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V12))
        .unwrap();
    assert_eq!(
        rows_for_12(&conn),
        1,
        "V12 must remain idempotent on schema_version"
    );
    conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V12))
        .unwrap();
    assert_eq!(
        rows_for_12(&conn),
        1,
        "V12 must remain idempotent on schema_version"
    );
    assert_eq!(
        max_version(&conn),
        before,
        "re-running V12 must not move the schema watermark"
    );
}

fn read_deleted_at(conn: &Connection, id: &str) -> Option<i64> {
    conn.query_row(
        "SELECT deleted_at FROM llm_wiki_entries WHERE id = ?1",
        [id],
        |r| r.get(0),
    )
    .unwrap()
}

fn read_all_deleted_at(conn: &Connection) -> std::collections::BTreeMap<String, Option<i64>> {
    let mut stmt = conn
        .prepare("SELECT id, deleted_at FROM llm_wiki_entries")
        .unwrap();
    let mut out = std::collections::BTreeMap::new();
    let rows = stmt
        .query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?))
        })
        .unwrap();
    for row in rows {
        let (id, deleted_at) = row.unwrap();
        out.insert(id, deleted_at);
    }
    out
}
