//! `ct wisdom scheme status|activate` contract tests (issue #265, plan
//! Task 4). The brain is a real migrated scratch DB (the shared fixture runs
//! the AppDb migration ladder); the cutover pre-condition rows are seeded via
//! SQL. No network, no real embedder.

mod common;

use common::{run_ct, with_seeded_brain};
use rusqlite::{params, Connection};

fn brain_db() -> Connection {
    let dir = std::env::var("CURATED_BRAIN_DIR").expect("with_seeded_brain sets it");
    Connection::open(std::path::Path::new(&dir).join("brain.db")).unwrap()
}

/// Seed a live non-null row stamped `scheme` (the cutover precondition class).
fn seed_stamped(id: &str, scheme: &str) {
    brain_db()
        .execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                source_hash, source_ref, created_at, updated_at, last_accessed_at,
                access_count, deleted_at, embedding_blob, embed_scheme, embedding
             ) VALUES (?1, 'ent_t', ?2, 'Body', '[]', 'inferred',
                       'librarian_inferred', NULL, NULL, 100, 100, NULL, 0,
                       NULL, x'00000000', ?3, NULL)",
            params![id, format!("Title {id}"), scheme],
        )
        .unwrap();
}

fn assert_status_counts(raw: i64, instr1: i64) {
    let out = run_ct(&["wisdom", "scheme", "status", "--json"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("stdout is JSON");
    assert_eq!(v["raw"], raw);
    assert_eq!(v["instr1"], instr1);
    assert_eq!(v["other"], 0);
    assert_eq!(v["active_scheme"], "raw", "the window starts on raw");
}

#[test]
fn status_reflects_the_true_counts_before_and_after_the_sweep() {
    with_seeded_brain(|| {
        assert_status_counts(0, 0);

        seed_stamped("fact_raw_ct", "raw");
        seed_stamped("fact_instr_ct", "instr1");
        assert_status_counts(1, 1);

        // Scheme-sweep the raw row (the same library fn the operator run uses),
        // then status must show the workset empty.
        temp_env::with_vars([("CURATED_EMBED_STUB", Some("constant8"))], || {
            let conn = brain_db();
            let report =
                tauri_app_lib::embed_sweep::sweep_scheme_embeddings(&conn, &Default::default(), 10)
                    .unwrap();
            assert_eq!(report.reembedded, 1);
        });
        assert_status_counts(0, 2);
    });
}

#[test]
fn activate_refuses_then_succeeds_and_is_idempotent() {
    with_seeded_brain(|| {
        seed_stamped("fact_raw_ct", "raw");

        // Refusal: non-zero exit, outstanding count on stderr, meta untouched.
        let out = run_ct(&["wisdom", "scheme", "activate", "instr1"]);
        assert_eq!(
            out.status.code(),
            Some(1),
            "must refuse while raw rows remain"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("refusing"), "stderr: {stderr}");
        let outstanding = "refusing: 1 live non-null row(s) not stamped 'instr1'";
        assert!(
            stderr.contains(outstanding),
            "outstanding count must be printed: {stderr}"
        );
        let v: String = brain_db()
            .query_row(
                "SELECT value FROM llm_wiki_meta WHERE key = 'wisdom_active_scheme'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v, "raw");

        // Sweep the raw row away, then activate: exit 0, meta flipped.
        temp_env::with_vars([("CURATED_EMBED_STUB", Some("constant8"))], || {
            let conn = brain_db();
            tauri_app_lib::embed_sweep::sweep_scheme_embeddings(&conn, &Default::default(), 10)
                .unwrap();
        });
        let out = run_ct(&["wisdom", "scheme", "activate", "instr1"]);
        assert_eq!(
            out.status.code(),
            Some(0),
            "stderr={}",
            String::from_utf8_lossy(&out.stderr)
        );
        let v: String = brain_db()
            .query_row(
                "SELECT value FROM llm_wiki_meta WHERE key = 'wisdom_active_scheme'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v, "instr1");

        // Idempotent: activating again is a 0-exit no-op.
        let out = run_ct(&["wisdom", "scheme", "activate", "instr1"]);
        assert_eq!(out.status.code(), Some(0));
        assert!(String::from_utf8_lossy(&out.stdout).contains("no-op"));
    });
}

#[test]
fn activate_rejects_non_instr1_targets() {
    with_seeded_brain(|| {
        for target in ["raw", "some_future_scheme", "INSTR1"] {
            let out = run_ct(&["wisdom", "scheme", "activate", target]);
            assert_eq!(
                out.status.code(),
                Some(1),
                "target {target} must fail closed"
            );
            assert!(
                String::from_utf8_lossy(&out.stderr).contains("fail-closed"),
                "target {target}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        let v: String = brain_db()
            .query_row(
                "SELECT value FROM llm_wiki_meta WHERE key = 'wisdom_active_scheme'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v, "raw", "nothing was flipped");
    });
}

#[test]
fn status_text_mode_prints_the_active_scheme_and_counts() {
    with_seeded_brain(|| {
        seed_stamped("fact_raw_ct", "raw");
        let out = run_ct(&["wisdom", "scheme", "status"]);
        assert_eq!(
            out.status.code(),
            Some(0),
            "stderr={}",
            String::from_utf8_lossy(&out.stderr)
        );
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("active read scheme: raw"), "{text}");
        assert!(text.contains("raw:    1"), "{text}");
        assert!(text.contains("instr1: 0"), "{text}");
    });
}
