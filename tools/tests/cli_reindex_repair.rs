//! Task 12 (issue #271): `ct reindex repair-embed-key --stale` CLI tests.
//!
//! Spec 2026-10-09-issue271 ("`ct reindex repair-embed-key --stale`"): the
//! command re-embeds the STORED CHUNKS of every stale/NULL-key candidate doc
//! DIRECTLY FROM THE DB — no file read, no status/tier precondition, no
//! stamp — writing `documents.embed_key` on success. Per doc ONE IMMEDIATE
//! transaction covers the in-place embeddings UPDATEs and the key write;
//! the doc's chunk-id set is re-checked UNDER the lock against the pre-read
//! (Task-8 swap-race rule: equal sets in any order); a mismatch skips the
//! doc with a printed note and the next run retries. Exit 0 always; per-doc
//! verdicts + final coverage on stdout.

mod common;

use common::{brain_db, run_ct, run_ct_with_env, with_seeded_brain, DOC_PATH};

use tauri_app_lib::retrieval;
use tauri_app_lib::two_stage;
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

/// Wire the seeded fixture doc into the two-stage candidate set: one
/// eligible entry (non-null `embedding_blob`, live, librarian_inferred —
/// the shared Task-2 predicate) carrying an evidence row whose
/// `content_hash` matches the doc's first stored chunk (`chash1`, per
/// `common::seed_vault`). After this the doc is reachable through the
/// evidence-hash hop (Task 3's `candidate_set` leg 1).
fn wire_doc_into_candidate_set() {
    let conn = brain_db();
    // Any non-null blob satisfies the eligibility predicate (coverage does
    // not score the entry, it only needs the edge).
    let blob = [0u8; 8];
    conn.execute(
        "INSERT INTO llm_wiki_entries
           (id, entity_id, title, body, source_type, created_at, updated_at, embedding_blob)
         VALUES ('f_fix', 'ent_fix', 'T', 'B', 'librarian_inferred', 0, 0, ?1)",
        rusqlite::params![blob],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO librarian_evidence
           (entry_id, proposal_id, evidence_json, created_at)
         VALUES ('f_fix', 'p_fix', '{\"evidence\":[{\"content_hash\":\"chash1\"}]}', 0)",
        [],
    )
    .unwrap();
}

fn doc_id() -> i64 {
    brain_db()
        .query_row("SELECT id FROM documents LIMIT 1", [], |r| r.get(0))
        .unwrap()
}

fn embed_key(conn: &rusqlite::Connection) -> Option<String> {
    conn.query_row("SELECT embed_key FROM documents LIMIT 1", [], |r| r.get(0))
        .unwrap()
}

/// (chunk_id, vector blob) pairs for the fixture doc, id order — the
/// in-place-repair fingerprint: same ids, one embedding row per chunk.
fn vectors(conn: &rusqlite::Connection) -> Vec<(i64, Vec<u8>)> {
    let mut stmt = conn
        .prepare(
            "SELECT c.id, e.vector FROM chunks c
               JOIN embeddings e ON e.chunk_id = c.id
              WHERE c.doc_id = (SELECT id FROM documents LIMIT 1)
              ORDER BY c.id",
        )
        .unwrap();
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?)))
        .unwrap();
    rows.map(|r| r.unwrap()).collect()
}

fn chunk_ids(conn: &rusqlite::Connection) -> Vec<i64> {
    let mut stmt = conn
        .prepare(
            "SELECT id FROM chunks WHERE doc_id = (SELECT id FROM documents LIMIT 1) ORDER BY id",
        )
        .unwrap();
    let rows = stmt.query_map([], |r| r.get::<_, i64>(0)).unwrap();
    rows.map(|r| r.unwrap()).collect()
}

/// Step-1 test 1 (brief): a candidate doc with a NULL `embed_key` and NO
/// source file on disk is repaired purely from stored chunk text — key
/// stamped with the computed gate key, vectors updated in place (same chunk
/// ids), coverage back to 100%, exit 0, per-doc verdict + final coverage
/// printed.
#[test]
fn repair_embed_key_stale_fixes_unreachable_doc() {
    with_seeded_brain(|| {
        wire_doc_into_candidate_set();
        let key = expected_key();

        // The candidate doc's source file does not exist — the repair must
        // never read it (the four coverage tools cannot reach this doc).
        assert!(
            !std::path::Path::new(DOC_PATH).exists(),
            "fixture precondition: {DOC_PATH} must be absent from disk"
        );
        assert_eq!(embed_key(&brain_db()), None, "fixture: key NULL");

        // Pre-repair coverage: the candidate set is stale (1 candidate, 0
        // covered).
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        assert_eq!(
            two_stage::coverage(&brain_db(), &key, now_ms).unwrap(),
            Some((1, 0)),
            "fixture: one stale candidate doc before the repair"
        );

        let ids_before = chunk_ids(&brain_db());
        assert_eq!(ids_before.len(), 2, "fixture: two scorable chunks");
        let vecs_before = vectors(&brain_db());

        let out = run_ct(&["reindex", "repair-embed-key", "--stale"]);
        assert_eq!(
            out.status.code(),
            Some(0),
            "repair must exit 0; stderr={}",
            stderr(&out)
        );

        // The stored chunks carried the repair — no file read, no stamp.
        let conn = brain_db();
        assert_eq!(
            embed_key(&conn),
            Some(key.clone()),
            "embed_key must be gate_model_key(profile, stub)"
        );
        assert_eq!(
            chunk_ids(&conn),
            ids_before,
            "vectors updated IN PLACE: chunk ids unchanged"
        );
        assert_eq!(
            vectors(&conn).len(),
            vecs_before.len(),
            "one embedding row per chunk, updated in place"
        );

        // Coverage back to 100%.
        assert_eq!(
            two_stage::coverage(&conn, &key, now_ms).unwrap(),
            Some((1, 1)),
            "post-repair coverage must be 100%"
        );

        // Per-doc verdict + final coverage on stdout.
        let so = stdout(&out);
        assert!(
            so.contains(DOC_PATH),
            "per-doc verdict must name the doc; stdout={so}"
        );
        assert!(
            so.contains("coverage: 1/1"),
            "final coverage line must print 1/1; stdout={so}"
        );
    });
}

/// Step-1 test 2 (brief): a chunk id mutated between the pre-read and the
/// transaction ⇒ the doc is SKIPPED with a printed note, no partial write
/// (vectors unchanged, key unchanged); a second run WITHOUT the race
/// retries and repairs.
///
/// The mutation rides the command's own test seam
/// (`CURATED_REPAIR_TEST_RACE=<doc_id>`): the pre-read → txn window drops
/// the doc's HIGHEST-ID chunk and re-inserts an equivalent one, so the id
/// set differs under the lock. (Highest-id, not first: the FIRST chunk's
/// `content_hash` is the evidence edge that makes the doc a candidate —
/// deleting it would remove the doc from the candidate set instead of
/// racing it.)
#[test]
fn repair_rechecks_chunk_ids_under_lock() {
    with_seeded_brain(|| {
        wire_doc_into_candidate_set();
        let key = expected_key();
        let id = doc_id();

        // Stale (not NULL) key so "unchanged" is observable as a value.
        brain_db()
            .execute(
                "UPDATE documents SET embed_key = 'old-model' WHERE id = ?1",
                rusqlite::params![id],
            )
            .unwrap();

        let vecs_before = vectors(&brain_db());

        // Run 1: the race seam fires between pre-read and txn.
        let doc_id_s = id.to_string();
        let out1 = run_ct_with_env(
            &["reindex", "repair-embed-key", "--stale"],
            [("CURATED_REPAIR_TEST_RACE", doc_id_s.as_str())],
        );
        assert_eq!(
            out1.status.code(),
            Some(0),
            "a skipped doc is never an error; stderr={}",
            stderr(&out1)
        );

        let conn = brain_db();
        assert!(
            stderr(&out1).contains("changed concurrently"),
            "skip note must print; stderr={}",
            stderr(&out1)
        );
        assert_eq!(
            embed_key(&conn),
            Some("old-model".to_string()),
            "no partial write: key untouched"
        );
        // No partial write, per chunk: the seam itself replaced the doc's
        // HIGHEST-ID chunk (id 2) with a new chunk 3 carrying an ALL-ZERO
        // placeholder vector, so (a) the surviving ORIGINAL chunk (id 1)
        // must be byte-identical to before, and (b) the seam-created chunk 3
        // must still carry the all-zero placeholder — a stub re-embed write
        // would have stamped 1e-4/2e-4 values on ids 1/3 instead.
        let after = vectors(&conn);
        let orig = after
            .iter()
            .find(|(id, _)| *id == vecs_before[0].0)
            .expect("surviving original chunk id must persist");
        assert_eq!(
            orig, &vecs_before[0],
            "no partial write: surviving original chunk byte-identical"
        );
        let seam_chunk = after
            .iter()
            .find(|(id, _)| *id != vecs_before[0].0 && *id != vecs_before[1].0);
        let seam_vec: Vec<u8> = seam_chunk.expect("seam-created chunk present").1.clone();
        assert!(
            seam_vec.iter().all(|b| *b == 0),
            "no partial write: seam-created chunk still holds the all-zero placeholder, got {seam_vec:?}"
        );

        // Run 2 (no seam): the next run retries and repairs.
        let out2 = run_ct(&["reindex", "repair-embed-key", "--stale"]);
        assert_eq!(out2.status.code(), Some(0), "stderr={}", stderr(&out2));
        assert_eq!(
            embed_key(&brain_db()),
            Some(key.clone()),
            "retry repairs the doc"
        );
        assert_eq!(
            two_stage::coverage(&brain_db(), &key, now_ms()).unwrap(),
            Some((1, 1)),
            "coverage 100% after the retry"
        );
    });
}

/// Review fix round: a doc deleted between candidate-set discovery and the
/// per-doc read (two separate statements on the WAL read conn — no shared
/// snapshot) must SKIP with a printed note and exit 0, never abort the whole
/// pass with an error. Without the fix, `stale_candidates` propagates
/// `QueryReturnedNoRows` and the command exits 1 — an exit-contract
/// violation. The seam (`CURATED_REPAIR_TEST_DELETE_DOC=<doc_id>`) deletes
/// the doc on a second rw connection exactly between the two reads;
/// production never sets it.
#[test]
fn repair_skips_doc_deleted_since_discovery() {
    with_seeded_brain(|| {
        wire_doc_into_candidate_set();
        let id = doc_id();

        let doc_id_s = id.to_string();
        let out = run_ct_with_env(
            ["reindex", "repair-embed-key", "--stale"].as_slice(),
            [("CURATED_REPAIR_TEST_DELETE_DOC", doc_id_s.as_str())],
        );
        assert_eq!(
            out.status.code(),
            Some(0),
            "a discovery-time deletion is a skip, never an error; stderr={}",
            stderr(&out)
        );
        assert!(
            stderr(&out).contains("deleted since candidate discovery"),
            "skip note must print; stderr={}",
            stderr(&out)
        );
        // The doc is gone; nothing was written on top of the race.
        let conn = brain_db();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "the raced doc stays deleted; no partial rewrite");
    });
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}
