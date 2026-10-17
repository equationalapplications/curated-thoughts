//! `ct reindex repair-embed-key --stale` (issue #271, plan Task 12).
//!
//! Spec 2026-10-09-issue271 §6 repair path: re-embeds the STORED CHUNKS of
//! every stale/NULL-key CANDIDATE doc (two_stage::candidate_set) DIRECTLY
//! FROM THE DB — no file read, no status/tier precondition, no reindex
//! stamp required. This reaches exactly the docs the four other tools
//! cannot (deleted files, non-indexed status, failed bootstrap sample).
//!
//! Per-doc atomicity (the heart of the task): ONE IMMEDIATE transaction
//! covers (a) the in-place embeddings UPDATEs from stored chunk text and
//! (b) the `documents.embed_key` write. The doc's chunk-id set is
//! re-checked UNDER the lock against the pre-read (Task-8 swap-race rule —
//! equal sets in any order); a mismatch skips the doc with a printed note
//! and the next run retries, never a partial write.
//!
//! Caller contract (Task 9 §8(c)): `embed_batch` cannot surface a typed
//! `IngestRefused` (refusals are raised only by the §8(c) decision table,
//! which this DB-only path never enters), but any re-embed error is still
//! handled with skip semantics — one informational stderr line per failed
//! doc, the pass continues, exit 0.

use std::collections::HashSet;

use anyhow::{Context, Result};
use rusqlite::TransactionBehavior;

use crate::write;
use tauri_app_lib::embedder::embed_batch;
use tauri_app_lib::retrieval;
use tauri_app_lib::two_stage;
use tauri_app_lib::wisdom_match::gate_model_key;

/// One stale/NULL-key candidate doc to repair.
struct StaleDoc {
    id: i64,
    path: String,
}

/// TEST SEAM ONLY (plan Step-1 test 2): between the pre-read and the
/// IMMEDIATE transaction, drop the doc's HIGHEST-ID chunk and re-insert an
/// equivalent one, so the under-lock chunk-id set differs from the
/// pre-read. (Highest-id, not first: in the fixture the FIRST chunk's
/// content_hash is the evidence edge that makes the doc a candidate at
/// all — deleting it would remove the doc from the candidate set instead
/// of racing it.) Invoked from `repair_embed_key_stale` when
/// `CURATED_REPAIR_TEST_RACE` names a doc id — production never sets it.
fn test_race_mutate_chunks(conn: &rusqlite::Connection, doc_id: i64) {
    conn.execute(
        "DELETE FROM embeddings WHERE chunk_id = (
            SELECT id FROM chunks WHERE doc_id = ?1 ORDER BY id DESC LIMIT 1)",
        rusqlite::params![doc_id],
    )
    .expect("race seam: delete embedding");
    conn.execute(
        "DELETE FROM chunks WHERE id = (
            SELECT id FROM chunks WHERE doc_id = ?1 ORDER BY id DESC LIMIT 1)",
        rusqlite::params![doc_id],
    )
    .expect("race seam: delete chunk");
    let position: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM chunks WHERE doc_id = ?1",
            rusqlite::params![doc_id],
            |r| r.get(0),
        )
        .expect("race seam: next position");
    let mut stmt = conn
        .prepare("SELECT chunk_text FROM chunks WHERE doc_id = ?1 LIMIT 1")
        .expect("race seam: prepare");
    let text: String = stmt
        .query_row(rusqlite::params![doc_id], |r| r.get(0))
        .expect("race seam: chunk text");
    conn.execute(
        "INSERT INTO chunks (doc_id, chunk_text, position, start_line, end_line, symbol_name, strategy)
         VALUES (?1, ?2, ?3, 1, 1, NULL, 'prose')",
        rusqlite::params![doc_id, text, position],
    )
    .expect("race seam: insert chunk");
    let new_id = conn.last_insert_rowid();
    let vec = vec![0_f32; 8];
    let bytes: Vec<u8> = vec.iter().flat_map(|f| f.to_le_bytes()).collect();
    conn.execute(
        "INSERT INTO embeddings (chunk_id, vector) VALUES (?1, ?2)",
        rusqlite::params![new_id, bytes],
    )
    .expect("race seam: insert embedding");
}

/// The stale/NULL-key candidate docs: the two-stage candidate set filtered
/// to docs whose `embed_key` differs from the current gate key (NULL counts
/// as stale). Read on a plain connection — the under-lock re-check below
/// makes any staleness between this read and the repair harmless.
fn stale_candidates(
    conn: &rusqlite::Connection,
    brain: &write::Brain,
    key: &str,
    now_ms: i64,
) -> Result<Vec<StaleDoc>> {
    let cands = two_stage::candidate_set(conn, now_ms, None)?
        .context("two-stage candidate probe unavailable (schema skew)")?;
    // TEST SEAM ONLY (review fix round): between candidate-set discovery and
    // the per-doc read, delete a doc on a SECOND (rw) connection. The
    // discovery query and the per-doc read are two separate statements on
    // the WAL read conn — no shared snapshot — so the per-doc read then hits
    // no rows exactly as it would when a concurrent writer removes the doc.
    // Invoked when `CURATED_REPAIR_TEST_DELETE_DOC` names a doc id —
    // production never sets it.
    if let Ok(del) = std::env::var("CURATED_REPAIR_TEST_DELETE_DOC") {
        if let Ok(del_id) = del.trim().parse::<i64>() {
            let rw = write::open_rw(brain).context("race seam: open rw")?;
            test_race_delete_doc(&rw, del_id);
        }
    }
    if cands.doc_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for (i, id) in cands.doc_ids.iter().enumerate() {
        let (path, embed_key): (String, Option<String>) = match conn.query_row(
            "SELECT path, embed_key FROM documents WHERE id = ?1",
            rusqlite::params![id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        ) {
            Ok(row) => row,
            // Discovery-time race (fix round): the doc was deleted between
            // candidate-set discovery and this read. Skip it with a note —
            // never abort the whole pass (exit-0/skip contract).
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                eprintln!(
                    "[{}/{}] skip (doc deleted since candidate discovery): doc_id={id}",
                    i + 1,
                    cands.doc_ids.len()
                );
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        if embed_key.as_deref() != Some(key) {
            out.push(StaleDoc { id: *id, path });
        }
    }
    Ok(out)
}

/// TEST SEAM ONLY: delete one doc row (see `stale_candidates` for why this
/// models the discovery-time race). `chunks.doc_id`'s `ON DELETE CASCADE`
/// never fires here — CT connections never enable `PRAGMA foreign_keys` —
/// which is fine: the doc is gone; its orphaned chunks are simply unreachable.
fn test_race_delete_doc(conn: &rusqlite::Connection, doc_id: i64) {
    conn.execute(
        "DELETE FROM documents WHERE id = ?1",
        rusqlite::params![doc_id],
    )
    .expect("race seam: delete doc");
}

/// Stored chunks of one doc: (chunk_id, chunk_text), id order — the id set
/// here is BOTH the re-embed source and the swap-race pre-read.
fn stored_chunks(conn: &rusqlite::Connection, doc_id: i64) -> Result<Vec<(i64, String)>> {
    let mut stmt =
        conn.prepare("SELECT id, chunk_text FROM chunks WHERE doc_id = ?1 ORDER BY id")?;
    let rows = stmt.query_map(rusqlite::params![doc_id], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// The swap-race rule (Task 8): the under-lock chunk-id set must EQUAL the
/// pre-read set (any order). A mismatch means a concurrent writer moved the
/// doc — skip, never a partial write.
fn chunk_ids_equal(pre: &[(i64, String)], now: &[(i64, String)]) -> bool {
    let pre_ids: HashSet<i64> = pre.iter().map(|(id, _)| *id).collect();
    let now_ids: HashSet<i64> = now.iter().map(|(id, _)| *id).collect();
    pre_ids == now_ids
}

/// Run the repair pass. Exit contract: always `Ok(0)` — per-doc failures
/// are skip semantics (stderr note, pass continues); the final coverage
/// line goes to stdout either way.
pub fn repair_embed_key_stale() -> Result<i32> {
    let brain = write::resolve().context("resolve brain")?;
    let paths_b = retrieval::resolve_brain_paths();
    let profile =
        retrieval::load_embed_profile(&paths_b.config_path).context("read embed profile")?;
    let stub = std::env::var("CURATED_EMBED_STUB").ok();
    let key = gate_model_key(&profile, stub.as_deref());

    let conn = write::open_ro(&brain).context("open brain read-only")?;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let docs = stale_candidates(&conn, &brain, &key, now_ms)?;

    if docs.is_empty() {
        println!("repair-embed-key: no stale/NULL-key candidate docs.");
        print_coverage(&conn, &key, now_ms)?;
        return Ok(0);
    }

    let mut rw = write::open_rw(&brain).context("open brain read-write")?;
    let mut repaired = 0usize;
    let mut skipped = 0usize;
    // `doc_pass` label: the per-chunk rows_affected guard rolls back and
    // skips to the next doc from inside the chunk loop.
    'doc_pass: for (i, doc) in docs.iter().enumerate() {
        // Pre-read (outside the txn): the chunks we intend to re-embed.
        let pre = match stored_chunks(&conn, doc.id) {
            Ok(p) => p,
            Err(e) => {
                eprintln!(
                    "[{}/{}] skip (read error): {}: {e}",
                    i + 1,
                    docs.len(),
                    doc.path
                );
                skipped += 1;
                continue;
            }
        };
        if pre.is_empty() {
            // Candidate coverage never counts chunk-less docs, so this doc
            // is not in the stale population at all — nothing to repair.
            eprintln!(
                "[{}/{}] skip (no stored chunks): {}",
                i + 1,
                docs.len(),
                doc.path
            );
            skipped += 1;
            continue;
        }

        // TEST SEAM (plan Step-1 test 2): mutate the chunk-id set inside
        // the pre-read → txn window. Production never sets this var.
        if let Ok(race_id) = std::env::var("CURATED_REPAIR_TEST_RACE") {
            if race_id == doc.id.to_string() {
                test_race_mutate_chunks(&rw, doc.id);
            }
        }

        let texts: Vec<String> = pre.iter().map(|(_, t)| t.clone()).collect();
        // Re-embed OUTSIDE the transaction: embed_batch can be slow (real
        // provider round-trip) and must never hold the IMMEDIATE write
        // lock. Atomicity is per-doc WRITE (vectors + key under one lock),
        // not per-embed.
        let vectors = match embed_batch(&profile, texts) {
            Ok(v) => v,
            Err(e) => {
                // Skip semantics (Task 9 §8(c) shape): one informational
                // line, the pass continues. `IngestRefused` can never reach
                // this DB-only path, so a plain skip note is the contract.
                eprintln!(
                    "[{}/{}] skip (embed error): {}: {e}",
                    i + 1,
                    docs.len(),
                    doc.path
                );
                skipped += 1;
                continue;
            }
        };
        if vectors.len() != pre.len() {
            eprintln!(
                "[{}/{}] skip (embedder returned {} vectors for {} chunks): {}",
                i + 1,
                docs.len(),
                vectors.len(),
                pre.len(),
                doc.path
            );
            skipped += 1;
            continue;
        }

        // ONE IMMEDIATE txn per doc: under-lock re-read + swap-race check +
        // in-place UPDATEs + key write.
        let tx = rw
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .with_context(|| format!("begin txn for {}", doc.path))?;
        let now = match stored_chunks(&tx, doc.id) {
            Ok(n) => n,
            Err(e) => {
                let _ = tx.rollback();
                eprintln!(
                    "[{}/{}] skip (read error): {}: {e}",
                    i + 1,
                    docs.len(),
                    doc.path
                );
                skipped += 1;
                continue;
            }
        };
        if !chunk_ids_equal(&pre, &now) {
            // Swap-race rule: the doc moved under us. Skip with a printed
            // note; the next run retries. No partial write (nothing was
            // written in this txn).
            tx.rollback().ok();
            eprintln!(
                "[{}/{}] skip (chunk set changed concurrently for {}; retrying next run)",
                i + 1,
                docs.len(),
                doc.path
            );
            skipped += 1;
            continue;
        }
        let mut wrote = false;
        for ((chunk_id, _), vec) in pre.iter().zip(vectors.iter()) {
            let bytes: Vec<u8> = vec.iter().flat_map(|f| f.to_le_bytes()).collect();
            // rows_affected guard (review fix round): every pre-read chunk
            // must have its embeddings row updated EXACTLY once — a 0 here
            // would silently no-op while the doc still got stamped repaired.
            // Roll back and skip (consistent with skip semantics).
            let updated = tx
                .execute(
                    "UPDATE embeddings SET vector = ?1 WHERE chunk_id = ?2",
                    rusqlite::params![bytes, chunk_id],
                )
                .with_context(|| format!("update embedding for chunk {chunk_id}"))?;
            if updated != 1 {
                tx.rollback().ok();
                eprintln!(
                    "[{}/{}] skip (embeddings UPDATE hit {} rows for chunk {chunk_id}, expected 1): {}",
                    i + 1,
                    docs.len(),
                    updated,
                    doc.path
                );
                skipped += 1;
                continue 'doc_pass;
            }
            wrote = true;
        }
        if !wrote {
            // Unreachable given the pre.is_empty() guard above, but a
            // no-op doc must not be counted as repaired.
            tx.rollback().ok();
            eprintln!(
                "[{}/{}] skip (no stored chunks): {}",
                i + 1,
                docs.len(),
                doc.path
            );
            skipped += 1;
            continue;
        }
        tx.execute(
            "UPDATE documents SET embed_key = ?1 WHERE id = ?2",
            rusqlite::params![key, doc.id],
        )
        .with_context(|| format!("write embed_key for {}", doc.path))?;
        tx.commit()
            .with_context(|| format!("commit txn for {}", doc.path))?;
        repaired += 1;
        println!(
            "[{}/{}] repaired: {} (chunks={} key={key})",
            i + 1,
            docs.len(),
            doc.path,
            pre.len()
        );
    }

    let _ = rw; // keep the rw conn alive until the pass is done
    println!("repair-embed-key: {repaired} repaired, {skipped} skipped.");

    // Final coverage over a fresh read connection (post-commit state).
    drop(rw);
    let ro = write::open_ro(&brain)?;
    print_coverage(&ro, &key, now_ms)?;
    Ok(0)
}

/// Final §6 coverage line: `coverage: <covered>/<candidates>` — the same
/// population/covered semantics as `two_stage::coverage`. A probe miss
/// (schema skew) prints the miss note instead of failing: exit 0.
fn print_coverage(conn: &rusqlite::Connection, key: &str, _now_ms: i64) -> Result<()> {
    let now_ms = now_ms_at();
    match two_stage::coverage(conn, key, now_ms)? {
        Some((n, cov)) => println!("coverage: {cov}/{n}"),
        None => println!("coverage: probe unavailable (pre-V28 brain); skipped."),
    }
    Ok(())
}

fn now_ms_at() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
