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

/// Two `.rs` docs whose only chunk set makes the linker resolve one edge:
/// `a.rs` defines `shared_fn`, `b.rs` calls it. Both land in the same
/// working-tier entity, so "the linker ran over the refused doc's entity" is
/// observable as the surviving edge.
fn write_rs_fixture(vault: &std::path::Path) {
    let docs = vault.join("documents");
    std::fs::create_dir_all(&docs).unwrap();
    std::fs::write(docs.join("a.rs"), "pub fn shared_fn() -> u32 { 42 }\n").unwrap();
    std::fs::write(
        docs.join("b.rs"),
        "pub fn caller_fn() -> u32 { shared_fn() }\n",
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
                assert!(
                    edge_count(&conn) > 0,
                    "fixture precondition: linker must resolve the shared_fn edge"
                );
            }

            drift_first_doc(&brain_conn(&dir));

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
            // The refused doc's entity STILL got its linker pass: the edge
            // survives (nothing orphaned by the skip).
            assert!(
                edge_count(&conn) > 0,
                "linker must run over the refused doc's entity too"
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
                assert!(
                    edge_count(&conn) > 0,
                    "fixture precondition: linker must resolve the shared_fn edge"
                );
            }

            drift_first_doc(&brain_conn(&dir));

            // Second ingest: the drifted doc is refused ⇒ SKIP accounting —
            // exit 0, failed == 0, and the refused file's entity stays in the
            // linker set (the shared edge survives).
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
            assert!(
                edge_count(&conn) > 0,
                "refused file's entity must stay in the linker set"
            );
        },
    );
}
