//! Task 13 (issue #271): `ct reindex verify-scratch` CLI tests.
//!
//! Spec 2026-10-09-issue271 §6 bootstrap: copy the live DB to scratch,
//! rechunk CHUNK-ONLY under the current binary, and write the CHUNKER-ONLY
//! `reindex_stamp` (value `CHUNKER_FINGERPRINT`) to the LIVE db iff every
//! live `librarian_evidence` hash ref still resolves on the scratch rechunk
//! AND every gate-eligible entry stays hop-resolvable (Task 3's hop SQL
//! against the scratch copy) — a chunker bump FAILS (the rechunk must be
//! lossless) and writes no stamp. Sampled backfill: FIRST + LAST chunk per
//! chunk-bearing doc re-embedded from STORED CHUNK TEXT (file existence and
//! document status irrelevant, r25-M1), matched by cosine ≥ 1−1e−3 against
//! the stored vectors; on match `documents.embed_key` is backfilled under a
//! GUARDED per-doc IMMEDIATE transaction (r28-m6: the doc's chunk-id set and
//! current `embed_key` are re-checked against the scratch snapshot — skip on
//! mismatch). `--dry-run` writes NOTHING to either DB (r26 m6).

mod common;

use common::{brain_db, run_ct, run_ct_with_env};

use rusqlite::OptionalExtension;
use temp_env::with_vars;
use tempfile::tempdir;

use tauri_app_lib::chunker::{chunk_autodetect, CHUNKER_FINGERPRINT};
use tauri_app_lib::db::chunk_hash::compute_chunk_hash;
use tauri_app_lib::embedder::embed_one;
use tauri_app_lib::retrieval::{
    self, insert_chunk, insert_embedding, mark_document_indexed, upsert_document, AppDb,
};
use tauri_app_lib::wisdom_match::gate_model_key;

fn stderr(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

fn stdout(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// The expected gate key, computed exactly the way the command must compute
/// it: `gate_model_key(profile, stub-as-used)` — never hardcoded (plan
/// Global Constraints).
fn expected_key() -> String {
    let paths = retrieval::resolve_brain_paths();
    let profile = retrieval::load_embed_profile(&paths.config_path).expect("embed profile");
    let stub = std::env::var("CURATED_EMBED_STUB").ok();
    gate_model_key(&profile, stub.as_deref())
}

/// Seed a temp brain whose single doc has a REAL source file on disk (the
/// scratch rechunk chunks from it), stored chunks + embeddings derived from
/// actually chunking that file (so a same-chunker rechunk is lossless by
/// construction and hashes match), and an eligible entry whose evidence
/// anchors the FIRST chunk's real content_hash (the candidate-set hop edge).
/// Returns (doc_id, doc_path, chunk hashes in position order).
fn seed_verify_brain(brain_path: &std::path::Path) -> (i64, String, Vec<String>) {
    std::fs::write(brain_path.join("config.json"), b"{}\n").unwrap();
    let paths = retrieval::resolve_brain_paths();
    let db = AppDb::open_with_config(&paths.db_path, &paths.config_path)
        .expect("writable brain db open");
    let profile = retrieval::load_embed_profile(&paths.config_path).unwrap();

    // The doc's source file MUST exist — the scratch rechunk chunks from it.
    let file_path = brain_path.join("doc_under_test.rs");
    let text = "fn alpha() { todo!() }\n\nfn beta() { todo!() }\n";
    std::fs::write(&file_path, text).unwrap();
    let doc_path = file_path.to_str().unwrap().to_string();

    let chunks = chunk_autodetect(std::path::Path::new(&doc_path), text);
    assert!(
        chunks.len() >= 2,
        "fixture needs >= 2 chunks, got {}",
        chunks.len()
    );

    let doc_id = upsert_document(&db.0, &doc_path, "h_verify").unwrap();
    let mut hashes = Vec::new();
    for (i, ch) in chunks.iter().enumerate() {
        let h = compute_chunk_hash(&ch.text, &doc_path, i);
        let chunk_id = insert_chunk(&db.0, doc_id, ch, i, "ent_fixture", &h).unwrap();
        let v = embed_one(&profile, ch.text.clone()).unwrap();
        insert_embedding(&db.0, chunk_id, &v).unwrap();
        hashes.push(h);
    }
    mark_document_indexed(&db.0, doc_id, "seed-h").unwrap();

    // One eligible entry (shared Task-2 predicate: non-null blob, live,
    // librarian_inferred) whose evidence anchors the FIRST chunk's real
    // hash — the evidence-hash hop edge into the candidate set.
    let blob = [0u8; 8];
    db.0.execute(
        "INSERT INTO llm_wiki_entries
               (id, entity_id, title, body, source_type, created_at, updated_at, embedding_blob)
             VALUES ('f_fix', 'ent_fix', 'T', 'B', 'librarian_inferred', 0, 0, ?1)",
        rusqlite::params![blob],
    )
    .unwrap();
    db.0.execute(
        "INSERT INTO librarian_evidence
               (entry_id, proposal_id, evidence_json, created_at)
             VALUES ('f_fix', 'p_fix', ?1, 0)",
        rusqlite::params![format!(
            r#"{{"evidence":[{{"content_hash":"{}"}}]}}"#,
            hashes[0]
        )],
    )
    .unwrap();

    (doc_id, doc_path, hashes)
}

/// Run `f` with a freshly seeded verify-scratch temp brain as
/// CURATED_BRAIN_DIR (stub embeddings enabled).
fn with_verify_brain<F>(f: F)
where
    F: FnOnce(i64, String, Vec<String>),
{
    let brain = tempdir().unwrap();
    let brain_path = brain.path().to_path_buf();
    let brain_path_str = brain_path.to_str().unwrap().to_string();
    with_vars(
        [
            ("CURATED_BRAIN_DIR", Some(brain_path_str.as_str())),
            ("CURATED_EMBED_STUB", Some("constant8")),
        ],
        move || {
            let (doc_id, doc_path, hashes) = seed_verify_brain(&brain_path);
            f(doc_id, doc_path, hashes);
        },
    );
}

fn embed_key_of(doc_id: i64) -> Option<String> {
    brain_db()
        .query_row(
            "SELECT embed_key FROM documents WHERE id = ?1",
            rusqlite::params![doc_id],
            |r| r.get(0),
        )
        .unwrap()
}

fn stamp() -> Option<String> {
    brain_db()
        .query_row(
            "SELECT value FROM llm_wiki_meta WHERE key = 'reindex_stamp'",
            [],
            |r| r.get(0),
        )
        .optional()
        .unwrap()
}

/// Step-1 test 1 (brief): fixture brain with live evidence + scorable chunks
/// ⇒ stamp == CHUNKER_FINGERPRINT written to the LIVE db, embed_key
/// backfilled on the matching doc, per-doc verdict + coverage printed,
/// exit 0.
#[test]
fn verify_scratch_writes_stamp_and_backfills_when_checks_hold() {
    with_verify_brain(|doc_id, _doc_path, _hashes| {
        assert_eq!(embed_key_of(doc_id), None, "fixture: key NULL");
        assert_eq!(stamp(), None, "fixture: no stamp yet");

        let out = run_ct(&["reindex", "verify-scratch"]);
        assert!(
            out.status.success(),
            "exit {:?}\nstdout:\n{}\nstderr:\n{}",
            out.status.code(),
            stdout(&out),
            stderr(&out)
        );
        assert_eq!(
            stamp().as_deref(),
            Some(CHUNKER_FINGERPRINT),
            "the CHUNKER-ONLY stamp must be written with CHUNKER_FINGERPRINT"
        );
        let key = expected_key();
        assert_eq!(
            embed_key_of(doc_id),
            Some(key),
            "embed_key must be backfilled on the doc whose samples matched"
        );
        let out_str = stdout(&out);
        assert!(
            out_str.contains("coverage: 1/1"),
            "final candidate-set coverage must be printed, got: {out_str}"
        );
        assert!(
            out_str.contains("ok:"),
            "per-doc verdict must be printed, got: {out_str}"
        );
    });
}

/// Step-1 test 2 (brief): simulate chunker drift (the scratch rechunk
/// rehashes the doc, orphaning the evidence ref) ⇒ no stamp written, exit
/// non-zero, the message names the lossless requirement.
#[test]
fn verify_scratch_refuses_chunker_bump() {
    with_verify_brain(|_doc_id, doc_path, _hashes| {
        // Drift: rewrite the source file so the scratch rechunk produces a
        // DIFFERENT content-hash set — the live evidence ref then resolves
        // nowhere on the scratch rechunk.
        std::fs::write(&doc_path, "totally different content, no symbols\n").unwrap();

        let out = run_ct(&["reindex", "verify-scratch"]);
        assert!(
            !out.status.success(),
            "a chunker bump must FAIL verify-scratch\nstdout:\n{}\nstderr:\n{}",
            stdout(&out),
            stderr(&out)
        );
        let combined = format!("{}{}", stdout(&out), stderr(&out));
        assert!(
            combined.to_lowercase().contains("lossless"),
            "the refusal message must name the lossless requirement, got:\n{combined}"
        );
        assert_eq!(stamp(), None, "no stamp may be written on refusal");
    });
}

/// Step-1 test 3 (brief): `--dry-run` writes NOTHING to either DB — the live
/// DB file is byte-identical before/after (hash the file), no stamp, no
/// embed_key; the scratch copy is discarded.
#[test]
fn verify_scratch_dry_run_touches_nothing() {
    with_verify_brain(|doc_id, _doc_path, _hashes| {
        let brain_dir = std::env::var("CURATED_BRAIN_DIR").unwrap();
        let db_path = std::path::Path::new(&brain_dir).join("brain.db");
        let before = std::fs::read(&db_path).unwrap();

        let out = run_ct(&["reindex", "verify-scratch", "--dry-run"]);
        assert!(
            out.status.success(),
            "stdout:\n{}\nstderr:\n{}",
            stdout(&out),
            stderr(&out)
        );

        let after = std::fs::read(&db_path).unwrap();
        assert_eq!(
            before, after,
            "--dry-run: the live DB file must be byte-identical"
        );
        assert_eq!(stamp(), None, "--dry-run writes no stamp");
        assert_eq!(embed_key_of(doc_id), None, "--dry-run backfills nothing");

        // Scratch discarded: no scratch artifacts left in the brain dir.
        for entry in std::fs::read_dir(&brain_dir).unwrap() {
            let name = entry.unwrap().file_name().to_string_lossy().to_string();
            assert!(
                !name.contains("scratch"),
                "scratch must be discarded, found {name}"
            );
        }
        let out_str = stdout(&out);
        assert!(
            out_str.contains("dry-run"),
            "dry-run must be visible in the output, got: {out_str}"
        );
        // Fix round: dry-run now runs the SAME analysis, so the would-be
        // per-doc verdict AND the would-be stamp outcome must be printed.
        assert!(
            out_str.contains("ok (dry-run: would backfill embed_key)"),
            "dry-run must print the per-doc would-be verdict, got: {out_str}"
        );
        assert!(
            out_str.contains("stamp WOULD be written"),
            "dry-run must print the would-be stamp outcome, got: {out_str}"
        );
        // The coverage printed is the CURRENT live coverage — under dry-run
        // nothing was backfilled, so the fixture doc stays uncovered (the
        // would-be coverage is implied by the stamp line above).
        assert!(
            out_str.contains("coverage: 0/1"),
            "dry-run must print the final candidate-set coverage, got: {out_str}"
        );
        assert!(
            out_str.contains("would backfill"),
            "dry-run summary must count would-be backfills, got: {out_str}"
        );
    });
}

/// Step-1 test 4 (brief): a counting-stub-seam doc whose sample vectors are
/// garbage ⇒ that doc stays NULL, its FAILED verdict is printed, and the
/// final coverage reflects the uncovered doc.
#[test]
fn failed_sample_leaves_null_and_is_printed() {
    with_verify_brain(|doc_id, _doc_path, _hashes| {
        let out = run_ct_with_env(
            &["reindex", "verify-scratch"],
            [(
                "CURATED_VERIFY_SCRATCH_TEST_GARBAGE_DOC",
                &doc_id.to_string(),
            )],
        );
        assert!(
            out.status.success(),
            "a failed sample is skip semantics, not a run failure\nstdout:\n{}\nstderr:\n{}",
            stdout(&out),
            stderr(&out)
        );
        assert_eq!(
            embed_key_of(doc_id),
            None,
            "a failed sample must leave embed_key NULL"
        );
        let out_str = stdout(&out);
        assert!(
            out_str.contains("FAILED"),
            "the failed-sample verdict must be printed, got: {out_str}"
        );
        assert!(
            out_str.contains("coverage: 0/1"),
            "coverage must reflect the uncovered doc, got: {out_str}"
        );
    });
}

/// Step-1 test 5 (brief): mutate the live doc's chunk set after sampling
/// (env-gated race seam, the adjudicated Task-8/12 pattern) ⇒ the guarded
/// per-doc IMMEDIATE write re-checks the chunk-id set under the lock and
/// SKIPS — no mismatched embed_key stamping (r28-m6).
#[test]
fn guarded_live_write_skips_on_chunk_set_change() {
    with_verify_brain(|doc_id, _doc_path, _hashes| {
        let out = run_ct_with_env(
            &["reindex", "verify-scratch"],
            [("CURATED_VERIFY_SCRATCH_TEST_RACE", &doc_id.to_string())],
        );
        assert!(
            out.status.success(),
            "a guarded skip is not a run failure\nstdout:\n{}\nstderr:\n{}",
            stdout(&out),
            stderr(&out)
        );
        assert_eq!(
            embed_key_of(doc_id),
            None,
            "the guarded write must SKIP on a chunk-id-set change — no mismatched stamping"
        );
        let combined = format!("{}{}", stdout(&out), stderr(&out));
        assert!(
            combined.contains("skip"),
            "the skip must be printed, got:\n{combined}"
        );
    });
}
