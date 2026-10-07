//! Plan Task 4 (issue #265) integration tests: the scheme-filtered sweep and
//! the `instr1` cutover over a scratch DB.
//!
//! STRUCTURAL, per the plan's guardrail: synthetic blobs in a migrated
//! in-memory brain, the `CURATED_EMBED_STUB` embedder for re-embed work, no
//! network, no real embedder, no feature gates. The library functions under
//! test (`embed_sweep::sweep_scheme_embeddings`,
//! `embed_scheme::activate_instr1`) are the exact code the ct surface calls.

use rusqlite::{params, Connection};
use tauri_app_lib::db::connection::open_in_memory;
use tauri_app_lib::embed_scheme::{
    activate_instr1, read_scheme, ActivateOutcome, Scheme, WRITE_SCHEME,
};
use tauri_app_lib::embed_sweep::{count_unstamped_entries, scheme_counts, sweep_scheme_embeddings};

const STUB: Option<&str> = Some("constant8");

/// Seed a live row with a blob under `scheme` (mirrors pre-#265 `raw` rows).
fn seed_with_blob(conn: &Connection, id: &str, scheme: &str) {
    conn.execute(
        "INSERT INTO llm_wiki_entries (
            id, entity_id, title, body, tags, confidence, source_type,
            source_hash, source_ref, created_at, updated_at, last_accessed_at,
            access_count, deleted_at, embedding_blob, embed_scheme, embedding
         ) VALUES (?1, 'ent-1', ?2, 'Body text.', '[]', 'inferred',
                   'librarian_inferred', NULL, NULL, 100, 100, NULL, 0,
                   NULL, x'00000000', ?3, NULL)",
        params![id, format!("Title {id}"), scheme],
    )
    .unwrap();
}

fn scheme_of(conn: &Connection, id: &str) -> String {
    conn.query_row(
        "SELECT embed_scheme FROM llm_wiki_entries WHERE id = ?1",
        [id],
        |r| r.get(0),
    )
    .unwrap()
}

fn active_scheme_value(conn: &Connection) -> String {
    conn.query_row(
        "SELECT value FROM llm_wiki_meta WHERE key = 'wisdom_active_scheme'",
        [],
        |r| r.get(0),
    )
    .unwrap()
}

#[test]
fn sweep_then_activate_end_to_end_over_a_scratch_db() {
    temp_env::with_vars([("CURATED_EMBED_STUB", STUB)], || {
        let conn = open_in_memory().unwrap();
        // Migration-window state: two raw rows, one already instr1 (a
        // write-time embed), one NULL blob (scheme-agnostic).
        seed_with_blob(&conn, "fact_raw_a", "raw");
        seed_with_blob(&conn, "fact_raw_b", "raw");
        seed_with_blob(&conn, "fact_instr", WRITE_SCHEME);
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                source_hash, source_ref, created_at, updated_at, last_accessed_at,
                access_count, deleted_at, embedding_blob, embed_scheme, embedding
             ) VALUES ('fact_null', 'ent-1', 'T', 'B', '[]', 'inferred',
                       'librarian_inferred', NULL, NULL, 100, 100, NULL, 0,
                       NULL, NULL, 'raw', NULL)",
            [],
        )
        .unwrap();

        // Pre-sweep status: the true counts, and activate refuses on them.
        let before = scheme_counts(&conn).unwrap();
        assert_eq!(before.raw, 2);
        assert_eq!(before.instr1, 1);
        assert_eq!(before.other, 0);
        assert_eq!(before.null_blob, 1);
        assert_eq!(count_unstamped_entries(&conn).unwrap(), 2);
        assert_eq!(
            activate_instr1(&conn, WRITE_SCHEME).unwrap(),
            ActivateOutcome::Refused { outstanding: 2 },
            "activate must refuse while raw non-null rows remain"
        );
        assert_eq!(active_scheme_value(&conn), "raw", "refusal is inert");

        // Sweep: exactly the two raw rows are re-embedded and stamped.
        let report = sweep_scheme_embeddings(&conn, &Default::default(), 10).unwrap();
        assert_eq!(report.reembedded, 2);
        assert_eq!(report.failed, 0);
        assert_eq!(report.remaining_raw, 0);
        assert_eq!(scheme_of(&conn, "fact_raw_a"), WRITE_SCHEME);
        assert_eq!(scheme_of(&conn, "fact_raw_b"), WRITE_SCHEME);

        // Second sweep run: pure no-op (idempotent resume proof).
        let second = sweep_scheme_embeddings(&conn, &Default::default(), 10).unwrap();
        assert_eq!(second, Default::default());

        // Post-sweep status reflects the flip in the counts.
        let after = scheme_counts(&conn).unwrap();
        assert_eq!(after.raw, 0);
        assert_eq!(after.instr1, 3);
        assert_eq!(after.null_blob, 1);

        // Cutover now succeeds; a repeat is the idempotent no-op.
        assert_eq!(
            activate_instr1(&conn, WRITE_SCHEME).unwrap(),
            ActivateOutcome::Activated
        );
        assert_eq!(active_scheme_value(&conn), WRITE_SCHEME);
        assert_eq!(read_scheme(&conn).unwrap(), Scheme::Instr1);
        assert_eq!(
            activate_instr1(&conn, WRITE_SCHEME).unwrap(),
            ActivateOutcome::AlreadyActive
        );
    });
}

#[test]
fn activate_rejects_non_instr1_targets_fail_closed() {
    let conn = open_in_memory().unwrap();
    for target in ["raw", "some_future_scheme", "INSTR1", ""] {
        let err =
            activate_instr1(&conn, target).expect_err("only instr1 is accepted (fail-closed)");
        assert!(
            err.to_string().contains("fail-closed"),
            "target {target:?}: {err}"
        );
    }
    assert_eq!(active_scheme_value(&conn), "raw", "nothing was flipped");
}

#[test]
fn activate_refusal_reports_the_true_outstanding_count() {
    let conn = open_in_memory().unwrap();
    for i in 0..7 {
        seed_with_blob(&conn, &format!("fact_{i}"), "raw");
    }
    // Soft-deleted raw rows do not count: the precondition is over LIVE rows.
    seed_with_blob(&conn, "fact_dead", "raw");
    conn.execute(
        "UPDATE llm_wiki_entries SET deleted_at = 1 WHERE id = 'fact_dead'",
        [],
    )
    .unwrap();

    assert_eq!(
        activate_instr1(&conn, WRITE_SCHEME).unwrap(),
        ActivateOutcome::Refused { outstanding: 7 }
    );
}
