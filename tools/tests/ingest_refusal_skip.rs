//! Task 9 (issue #271 §8(c) caller contract): a typed `IngestRefused` is
//! NEVER a generic `Err` at the out-of-tree callers.
//!
//! - `bulk_reindex` ⇒ SKIP: one informational stderr line, the pass
//!   CONTINUES, the linker still runs over every processed entity.
//! - `ct ingest` ⇒ success: exit 0, no `failed += 1` ("0 failed" summary),
//!   the refused file's entity stays in the linker set.
//!
//! Refusal fixture (same recipe as Task 8's `forced_drift_refusal_is_typed_
//! and_resets_pending_rows`): seed a doc, rewrite its stored chunk hashes to
//! values a fresh rechunk can never produce (chunker-drift analogue), leave
//! the reindex stamp absent ⇒ a FORCED pure rechunk over unchanged bytes is
//! refused.

use std::process::Command;

use temp_env::with_vars;
use tempfile::tempdir;

mod common;

/// Two `.rs` docs in DIFFERENT tier entities, each SELF-CONTAINED (defines
/// its own symbol and calls it, so each doc's entity resolves one edge):
/// `documents/a.rs` routes to `tier_fact`; `notes/b.rs` routes to the
/// working-tier entity `tier_working::<vault-hash>`. pr283-review finding
/// 13: with the old same-entity fixture the linker pass over the PROCESSED
/// doc's entity recreated the shared edge, so the test could not detect a
/// refused doc's entity missing from the linker set at all. Edge accounting
/// is exact now: seed ⇒ 2 edges; clearing `curated_relationships` and
/// re-running must recreate BOTH (one per entity).
fn write_rs_fixture(vault: &std::path::Path) {
    let docs = vault.join("documents");
    std::fs::create_dir_all(&docs).unwrap();
    let notes = vault.join("notes");
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::write(
        docs.join("a.rs"),
        "pub fn fact_fn() -> u32 { 42 }\npub fn fact_caller() -> u32 { fact_fn() }\n",
    )
    .unwrap();
    std::fs::write(
        notes.join("b.rs"),
        "pub fn work_fn() -> u32 { 7 }\npub fn work_caller() -> u32 { work_fn() }\n",
    )
    .unwrap();
}

fn brain_conn(dir: &std::path::Path) -> rusqlite::Connection {
    rusqlite::Connection::open(dir.join("brain.db")).unwrap()
}

fn edge_count(conn: &rusqlite::Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM curated_relationships", [], |r| {
        r.get(0)
    })
    .unwrap()
}

/// The fixture precondition: the two docs really landed in DIFFERENT
/// entities (the whole point of the finding-13 fixture).
fn assert_two_distinct_entities(conn: &rusqlite::Connection) {
    let n: i64 = conn
        .query_row("SELECT COUNT(DISTINCT entity_id) FROM chunks", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(
        n, 2,
        "fixture: documents/a.rs (tier_fact) and notes/b.rs (tier_working) must map to 2 distinct entities"
    );
}

/// Force the chunker-drift refusal on `a.rs`: same bytes on disk, stored
/// hashes rewritten to never match a fresh rechunk; the reindex stamp stays
/// absent (no verify-scratch in tests).
fn drift_first_doc(conn: &rusqlite::Connection) {
    let doc_a: i64 = conn
        .query_row(
            "SELECT id FROM documents WHERE path LIKE '%/a.rs'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    conn.execute(
        "UPDATE chunks SET content_hash = 'drifted' || printf('%028d', position) \
         WHERE doc_id = ?1",
        [doc_a],
    )
    .unwrap();
}

/// Seed both docs via `ct ingest --yes` (bulk_reindex only revisits
/// ALREADY-INDEXED docs — it is not an ingester), then return.
fn seed_via_ct_ingest() {
    let seed = Command::new(env!("CARGO_BIN_EXE_ct"))
        .args(["ingest", "--yes"])
        .output()
        .expect("spawn ct ingest");
    assert!(
        seed.status.success(),
        "seed ingest failed: {}",
        String::from_utf8_lossy(&seed.stderr)
    );
}

#[test]
fn bulk_reindex_counts_refusal_as_skip_and_continues() {
    let brain = tempdir().unwrap();
    let vault = tempdir().unwrap();
    let dir = brain.path().to_path_buf();
    let dir_str = dir.to_str().unwrap().to_string();
    let vault_path = vault.path().to_path_buf();
    let vault_str = vault_path.to_str().unwrap().to_string();

    with_vars(
        [
            ("CURATED_BRAIN_DIR", Some(dir_str.as_str())),
            ("CURATED_EMBED_STUB", Some("constant8")),
        ],
        move || {
            common::init_brain_db(&dir);
            write_rs_fixture(&vault_path);
            std::fs::write(
                dir.join("config.json"),
                serde_json::json!({ "vault_path": vault_str }).to_string(),
            )
            .unwrap();

            // Pass 1: both docs index and the linker resolves the shared_fn
            // edge (fixture precondition). bulk_reindex only revisits
            // already-indexed docs, so seed via `ct ingest` first.
            seed_via_ct_ingest();
            let seed = Command::new(env!("CARGO_BIN_EXE_bulk_reindex"))
                .output()
                .expect("spawn bulk_reindex");
            assert!(
                seed.status.success(),
                "seed reindex failed: {}",
                String::from_utf8_lossy(&seed.stderr)
            );
            {
                let conn = brain_conn(&dir);
                assert_two_distinct_entities(&conn);
                assert_eq!(
                    edge_count(&conn),
                    2,
                    "fixture precondition: one resolved edge per entity"
                );
            }

            drift_first_doc(&brain_conn(&dir));
            // pr283-review finding 13: exact accounting — wipe all edges, so
            // pass 2's edge count can ONLY come from entities the linker
            // actually ran over.
            brain_conn(&dir)
                .execute("DELETE FROM curated_relationships", [])
                .unwrap();

            // Pass 2: the FIRST path (sorted ORDER BY path) is refused ⇒ SKIP
            // (one informational stderr line, pass CONTINUES, exit 0); the
            // second doc is still processed.
            let out = Command::new(env!("CARGO_BIN_EXE_bulk_reindex"))
                .output()
                .expect("spawn bulk_reindex");
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert!(
                out.status.success(),
                "bulk_reindex must exit 0 despite the refusal, stderr: {stderr}"
            );
            assert_eq!(
                stderr.matches("refused").count(),
                1,
                "exactly one informational skip line expected, stderr: {stderr}"
            );

            let conn = brain_conn(&dir);
            // The refused doc's entity STILL got its linker pass: BOTH edges
            // are recreated (one per entity). The old same-entity fixture
            // could not see this — b.rs's entity pass masked the gap.
            assert_eq!(
                edge_count(&conn),
                2,
                "linker must run over the refused doc's entity too (both edges recreated)"
            );
            // ...and the pass continued: the second doc was reprocessed (the
            // "[2/2] done …" progress line comes AFTER the loop arm ran).
            let stderr_tail = stderr.lines().filter(|l| l.contains("done …")).count();
            assert_eq!(
                stderr_tail, 1,
                "pass must reach the end-of-loop progress line, stderr: {stderr}"
            );
            let b_status: String = conn
                .query_row(
                    "SELECT status FROM documents WHERE path LIKE '%/b.rs'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(b_status, "indexed", "second doc must stay indexed");
        },
    );
}

#[test]
fn ct_ingest_treats_refusal_as_skip() {
    let brain = tempdir().unwrap();
    let vault = tempdir().unwrap();
    let dir = brain.path().to_path_buf();
    let dir_str = dir.to_str().unwrap().to_string();
    let vault_path = vault.path().to_path_buf();
    let vault_str = vault_path.to_str().unwrap().to_string();

    with_vars(
        [
            ("CURATED_BRAIN_DIR", Some(dir_str.as_str())),
            ("CURATED_EMBED_STUB", Some("constant8")),
        ],
        move || {
            common::init_brain_db(&dir);
            write_rs_fixture(&vault_path);
            std::fs::write(
                dir.join("config.json"),
                serde_json::json!({ "vault_path": vault_str }).to_string(),
            )
            .unwrap();

            // Seed: both docs index cleanly via `ct ingest --yes`.
            let seed = Command::new(env!("CARGO_BIN_EXE_ct"))
                .args(["ingest", "--yes"])
                .output()
                .expect("spawn ct ingest");
            assert!(
                seed.status.success(),
                "seed ingest failed: {}",
                String::from_utf8_lossy(&seed.stderr)
            );
            {
                let conn = brain_conn(&dir);
                assert_two_distinct_entities(&conn);
                assert_eq!(
                    edge_count(&conn),
                    2,
                    "fixture precondition: one resolved edge per entity"
                );
            }

            drift_first_doc(&brain_conn(&dir));
            // Exact accounting (pr283-review finding 13): wipe all edges so
            // the re-run's count can only come from entities the linker ran
            // over.
            brain_conn(&dir)
                .execute("DELETE FROM curated_relationships", [])
                .unwrap();

            // Second ingest: the drifted doc is refused ⇒ SKIP accounting —
            // exit 0, failed == 0, and the refused file's entity stays in the
            // linker set (BOTH edges are recreated).
            let out = Command::new(env!("CARGO_BIN_EXE_ct"))
                .args(["ingest", "--yes"])
                .output()
                .expect("spawn ct ingest");
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert!(
                out.status.success(),
                "ct ingest must exit 0 despite the refusal, stderr: {stderr}"
            );
            assert!(
                !stderr.contains("FAILED"),
                "refusal must not count as a FAILED entry, stderr: {stderr}"
            );
            let stdout = String::from_utf8_lossy(&out.stdout);
            assert!(
                stdout.contains("0 failed"),
                "loop accounting must report failed == 0, stdout: {stdout}"
            );
            let conn = brain_conn(&dir);
            assert_eq!(
                edge_count(&conn),
                2,
                "refused file's entity must stay in the linker set (both edges recreated)"
            );
        },
    );
}
