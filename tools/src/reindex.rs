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
use rusqlite::{OptionalExtension, TransactionBehavior};

use crate::write;
use tauri_app_lib::embedder::{embed_batch, EmbedProfile};
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
    let vec = [0_f32; 8];
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
            Err(_) => {
                // Skip semantics (Task 9 §8(c) shape): one informational
                // line, the pass continues. `IngestRefused` can never reach
                // this DB-only path, so a plain skip note is the contract.
                // pr283-review finding 1 (CodeQL cleartext-logging): the
                // error is tainted by `resolved_api_key()` via the request
                // path, so it is NEVER printed — this line names the fix,
                // not the failure detail.
                eprintln!(
                    "[{}/{}] skip (embed error — check the profile/EMBED_API_KEY): {}",
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

/// Wall-clock `now_ms` for eligibility/coverage probes. `pub` for the Task-14
/// calibration driver (`tools/src/bin/calibrate_two_stage.rs`), which reuses
/// the Task-13 helper instead of re-deriving wall-clock time.
pub fn now_ms_at() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ─────────────────────────────────────────────────────────────────────────────
// `ct reindex verify-scratch` (issue #271, plan Task 13)
// ─────────────────────────────────────────────────────────────────────────────
//
// Spec 2026-10-09-issue271 §6 bootstrap (r22 M3 / r23 m2 / r24 m5 / r25-M1 /
// r25-M3 / r26 m6 / r28-m6): copy the live DB to a SCRATCH file, rechunk
// every doc CHUNK-ONLY under the current binary (the only embeds are the
// sampled backfill embeds), then write the CHUNKER-ONLY stamp to the LIVE DB
// iff (a) every live `librarian_evidence` hash ref resolves on the scratch
// rechunk AND (b) every gate-eligible entry stays hop-resolvable (Task 3's
// hop SQL against the scratch copy). A chunker bump FAILS (the rechunk must
// be LOSSLESS); no stamp is written. Sampled backfill: per chunk-bearing doc
// the FIRST + LAST chunks are re-embedded from STORED CHUNK TEXT (file
// existence and document status irrelevant, r25-M1), matched by cosine
// ≥ 1−1e−3 against the stored vectors; on match `documents.embed_key` is
// backfilled live under a GUARDED per-doc IMMEDIATE transaction (r28-m6:
// the chunk-id set AND the current `embed_key` are re-checked against the
// scratch snapshot under the lock — skip on mismatch; embeds happen OUTSIDE
// any txn). `--dry-run` writes NOTHING to either DB (r26 m6).

/// Pinned sample-match tolerance (plan Global Constraints): cosine ≥ 1−1e−3.
const SAMPLE_COSINE_FLOOR: f32 = 1.0 - 1e-3;

/// One per-doc verdict as computed on the SCRATCH copy (sampled backfill).
struct ScratchDoc {
    doc_id: i64,
    path: String,
    /// The scratch snapshot's chunk-id set (the guarded write re-checks
    /// against THIS under the live lock, r28-m6).
    chunk_ids: Vec<i64>,
    /// Ok(true) = samples matched; Ok(false) = no stored embeddings to
    /// compare against (nothing to backfill); Err(reason) = failed sample
    /// (embed error or cosine miss) — PRINTED, doc stays NULL/unchanged.
    sample: Result<bool, String>,
}

/// TEST SEAM ONLY (plan Step-1 test 4): when
/// `CURATED_VERIFY_SCRATCH_TEST_GARBAGE_DOC` names a doc id, the sampled
/// re-embed for that doc returns a garbage vector (direction-flipped stub)
/// whose cosine against the stored vector is ≈ −1 — modeling a provider
/// returning nonsense. Production never sets it.
fn test_sample_garbage(doc_id: i64) -> bool {
    matches!(
        std::env::var("CURATED_VERIFY_SCRATCH_TEST_GARBAGE_DOC"),
        Ok(v) if v == doc_id.to_string()
    )
}

/// TEST SEAM ONLY (plan Step-1 test 5): when
/// `CURATED_VERIFY_SCRATCH_TEST_RACE` names a doc id, the LIVE doc's
/// chunk-id set is mutated on a second connection INSIDE the
/// sample-compute → guarded-write window, so the under-lock re-check
/// observes a changed set exactly as a concurrent writer would produce.
/// Production never sets it.
fn test_race_mutate_live_chunks(conn: &rusqlite::Connection, doc_id: i64) {
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
    let text: String = conn
        .query_row(
            "SELECT chunk_text FROM chunks WHERE doc_id = ?1 LIMIT 1",
            rusqlite::params![doc_id],
            |r| r.get(0),
        )
        .expect("race seam: chunk text");
    conn.execute(
        "INSERT INTO chunks (doc_id, chunk_text, position, start_line, end_line, symbol_name, strategy)
         VALUES (?1, ?2, ?3, 1, 1, NULL, 'prose')",
        rusqlite::params![doc_id, text, position],
    )
    .expect("race seam: insert chunk");
    let new_id = conn.last_insert_rowid();
    let vec = [0_f32; 8];
    let bytes: Vec<u8> = vec.iter().flat_map(|f| f.to_le_bytes()).collect();
    conn.execute(
        "INSERT INTO embeddings (chunk_id, vector) VALUES (?1, ?2)",
        rusqlite::params![new_id, bytes],
    )
    .expect("race seam: insert embedding");
}

/// All doc ids that have ≥1 stored chunk, id order — the scratch rechunk and
/// backfill population ("every doc on scratch"; "chunk-bearing doc").
fn chunk_bearing_docs(conn: &rusqlite::Connection) -> Result<Vec<(i64, String)>> {
    let mut stmt =
        conn.prepare("SELECT id, path FROM documents WHERE id IN (SELECT DISTINCT doc_id FROM chunks) ORDER BY id")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// Rechunk one doc CHUNK-ONLY on the scratch DB: chunk the file's bytes,
/// compute the fresh content-hash set, and REPLACE the doc's stored chunks
/// (hashes recomputed via `compute_chunk_hash` — the Task-7 hash contract;
/// no embeddings are written: the hash/hop checks need chunking, not
/// vectors, r24 m5). `Ok(None)` = the doc must be skipped for rechunking
/// (unreadable file / un-ingestable extension / empty rechunk) — its stored
/// chunks stay AS-IS, so its evidence refs keep resolving on scratch.
fn scratch_rechunk_doc(
    conn: &rusqlite::Connection,
    doc_id: i64,
    doc_path: &str,
    vault_root: Option<&str>,
) -> Result<Option<Vec<String>>> {
    use std::path::Path;
    let ext = Path::new(doc_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    if !tauri_app_lib::chunker::should_ingest_extension(ext) {
        return Ok(None);
    }
    let read_path = match vault_root {
        Some(root) if doc_path.starts_with(root) => {
            // documents.path is vault-relative form; the bytes live under
            // the vault root.
            let rel = doc_path
                .strip_prefix(root)
                .unwrap_or(doc_path)
                .trim_start_matches('/');
            std::path::PathBuf::from(root).join(rel)
        }
        _ => std::path::PathBuf::from(doc_path),
    };
    let text = match std::fs::read_to_string(&read_path) {
        Ok(t) => t,
        Err(e) => return Err(anyhow::anyhow!("read {}: {e}", read_path.display())),
    };
    // pr283-review finding 16: the rechunk must produce the SAME chunk list
    // a real ingest would — `chunk_autodetect` PLUS the extracted reference
    // chunks for AstSymbol files (`pipeline::chunk_list_for_text`, the
    // canonical construction). Bare `chunk_autodetect` dropped reference
    // chunks, orphaning every evidence ref that pointed at one and refusing
    // the stamp.
    let chunks = tauri_app_lib::chunk_list_for_text(Path::new(doc_path), &text);
    if chunks.is_empty() {
        return Ok(None);
    }
    let hashes: Vec<String> = chunks
        .iter()
        .enumerate()
        .map(|(i, c)| tauri_app_lib::db::chunk_hash::compute_chunk_hash(&c.text, doc_path, i))
        .collect();
    // CHUNK-ONLY replacement: one txn per doc — old chunks out, fresh
    // chunks (with fresh hashes) in. Embeddings are NOT written (their old
    // rows cascade away with the chunks; the backfill's comparisons read
    // vectors snapshotted BEFORE the rechunk).
    conn.execute("BEGIN IMMEDIATE", [])?;
    // Loop-invariant: the entity id derives from the virtual path, not from
    // the chunk — compute it ONCE, not per chunk (fix round 4a).
    let eid = tauri_app_lib::entity_id_for_virtual_path(doc_path, vault_root);
    let res = (|| -> Result<()> {
        conn.execute(
            "DELETE FROM chunks WHERE doc_id = ?1",
            rusqlite::params![doc_id],
        )?;
        for (i, ch) in chunks.iter().enumerate() {
            // The re-exported `entity_id_for_virtual_path` is the same value
            // the original ingest wrote; the evidence/hop SQL never reads it,
            // but keep the row honest.
            tauri_app_lib::retrieval::insert_chunk(conn, doc_id, ch, i, &eid, &hashes[i])?;
        }
        Ok(())
    })();
    match res {
        Ok(()) => {
            conn.execute("COMMIT", [])?;
        }
        Err(e) => {
            conn.execute("ROLLBACK", [])?;
            return Err(e);
        }
    }
    Ok(Some(hashes))
}

/// Sampled backfill verdict for one doc, computed on the SCRATCH snapshot
/// (pre-rechunk vectors + stored chunk text): re-embed the FIRST + LAST
/// chunk's text and match each against the doc's stored vector by cosine
/// ≥ 1−1e−3. Both samples must match for a backfill.
fn sample_doc(
    scratch: &rusqlite::Connection,
    profile: &EmbedProfile,
    doc_id: i64,
    doc_path: &str,
) -> Result<bool, String> {
    // (id, text, stored-vector blob) for the FIRST + LAST chunk in id order
    // — read BEFORE the rechunk so the comparison is against the live DB's
    // current vectors.
    let mut stmt = scratch
        .prepare(
            "SELECT c.id, c.chunk_text, e.vector FROM chunks c
               JOIN embeddings e ON e.chunk_id = c.id
              WHERE c.doc_id = ?1
              ORDER BY c.id",
        )
        .map_err(|e| format!("read chunks for {doc_path}: {e}"))?;
    let rows: Vec<(i64, String, Vec<u8>)> = stmt
        .query_map(rusqlite::params![doc_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Vec<u8>>(2)?,
            ))
        })
        .map_err(|e| format!("read chunks for {doc_path}: {e}"))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| format!("read chunks for {doc_path}: {e}"))?;
    if rows.is_empty() {
        // No stored embeddings ⇒ nothing to compare; the doc has no
        // scorable chunks (the coverage population never counts it).
        return Ok(false);
    }
    let mut picked: Vec<(i64, String, Vec<u8>)> = Vec::with_capacity(2);
    let first = &rows[0];
    picked.push(first.clone());
    if rows.len() > 1 {
        picked.push(rows[rows.len() - 1].clone());
    }
    let mut texts = Vec::with_capacity(picked.len());
    for (_, text, _) in &picked {
        texts.push(text.clone());
    }
    // TEST SEAM (Step-1 test 4): garbage sample for one doc.
    let garbage = test_sample_garbage(doc_id);
    let vectors = if garbage {
        picked
            .iter()
            .map(|_| {
                let mut v = vec![0_f32; 8];
                v[0] = -1.0;
                v
            })
            .collect::<Vec<_>>()
    } else {
        tauri_app_lib::embedder::embed_batch(profile, texts)
            .map_err(|e| format!("embed sample for {doc_path}: {e}"))?
    };
    if vectors.len() != picked.len() {
        return Err(format!(
            "embedder returned {} vectors for {} samples on {doc_path}",
            vectors.len(),
            picked.len()
        ));
    }
    for ((_, _, blob), vec) in picked.iter().zip(vectors.iter()) {
        let stored = tauri_app_lib::search::bytes_to_f32(blob);
        if stored.len() != vec.len() {
            return Err(format!("dimension mismatch on sample for {doc_path}"));
        }
        let sim = tauri_app_lib::search::cosine_similarity(vec, &stored);
        // NaN-conservative: an incomparable similarity is treated as below-floor.
        let below_floor = matches!(
            sim.partial_cmp(&SAMPLE_COSINE_FLOOR),
            None | Some(std::cmp::Ordering::Less)
        );
        if below_floor {
            return Err(format!(
                "sample cosine {sim:.6} below floor {SAMPLE_COSINE_FLOOR} for {doc_path}"
            ));
        }
    }
    Ok(true)
}

/// Every live `librarian_evidence` evidence blob's hash refs must resolve on
/// the scratch rechunk (`evidence_has_live_chunk` against the SCRATCH copy —
/// mirror of `commit.rs:666`, hash-first with the legacy rowid fallback).
/// Returns (resolved_count, total) plus the first failure description.
fn evidence_refs_resolve(scratch: &rusqlite::Connection) -> Result<(usize, usize, Option<String>)> {
    let mut stmt = scratch.prepare("SELECT entry_id, evidence_json FROM librarian_evidence")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let mut ok = 0usize;
    let mut total = 0usize;
    let mut first_fail: Option<String> = None;
    for row in rows {
        let (entry_id, json) = row?;
        total += 1;
        let has = tauri_app_lib::db::commit::evidence_has_live_chunk(scratch, &json)
            .map_err(|e| anyhow::anyhow!("probe evidence for entry {entry_id}: {e}"))?;
        if has {
            ok += 1;
        } else if first_fail.is_none() {
            first_fail = Some(entry_id);
        }
    }
    Ok((ok, total, first_fail))
}

/// Every gate-eligible entry must stay hop-resolvable on the scratch rechunk:
/// Task 3's hop SQL (candidate-set leg 1 — the evidence-hash hop) evaluated
/// against the scratch copy: the eligible entries (shared Task-2 predicate)
/// joined to live chunk hashes through `librarian_evidence` must ALL be
/// reachable. pr283-review finding 17 (ADJUDICATED — documented rebuttal, no
/// redesign): the spec §6 bootstrap pins this as an ABSOLUTE-state check —
/// "every gate-eligible entry stays hop-resolvable (baseline: 365/365)" is
/// the same absolute census as the 406/406 evidence-ref check, and the
/// shipped live run satisfied exactly that shape (365/365, 406/406 on
/// 2026-10-17). It is deliberately stricter than a before/after delta (a
/// pre-existing orphan refuses the stamp too — fail-safe: "can only succeed
/// if the rechunk is actually lossless"), and the eligible population is the
/// spec's own pinned shape (on the live brain every eligible entry anchors
/// evidence, which is what 365/365 asserts). One statement per entry — the
/// populations are small and this runs once. Returns (resolved, total,
/// first_missing_entry).
fn gate_eligible_hops_resolve(
    scratch: &rusqlite::Connection,
    now_ms: i64,
) -> Result<(usize, usize, Option<String>)> {
    // Eligible ids via Task 3's public surface: the complement of the
    // eligible set under an EMPTY candidate set is the whole eligible set.
    let empty = two_stage::CandidateSet {
        doc_ids: Vec::new(),
    };
    let eligible = two_stage::complement_entry_ids(scratch, &empty, "chunk-hop", now_ms, None)?;
    let total = eligible.len();
    let mut ok = 0usize;
    let mut first_missing: Option<String> = None;
    // Hop-resolvable = the entry's evidence hash lands on a scratch chunk
    // (the same edge `candidate_set` leg 1 walks). One statement per entry —
    // the populations are small and this runs once.
    let mut stmt = scratch.prepare(
        "SELECT 1 FROM librarian_evidence le
           JOIN json_each(le.evidence_json, '$.evidence') je
           JOIN chunks c ON c.content_hash = json_extract(je.value, '$.content_hash')
          WHERE le.entry_id = ?1 LIMIT 1",
    )?;
    for id in &eligible {
        let found = stmt
            .query_row(rusqlite::params![id], |r| r.get::<_, i64>(0))
            .optional()?;
        if found.is_some() {
            ok += 1;
        } else if first_missing.is_none() {
            first_missing = Some(id.clone());
        }
    }
    Ok((ok, total, first_missing))
}

/// Run the verify-scratch bootstrap. Exit contract: `Ok(1)` on the chunker-
/// drift refusal (no stamp), `Ok(0)` otherwise.
pub fn verify_scratch(dry_run: bool) -> Result<i32> {
    use std::io::Write;

    let brain = write::resolve().context("resolve brain")?;
    let paths_b = retrieval::resolve_brain_paths();
    let profile =
        retrieval::load_embed_profile(&paths_b.config_path).context("read embed profile")?;
    let stub = std::env::var("CURATED_EMBED_STUB").ok();
    let key = gate_model_key(&profile, stub.as_deref());

    // Vault root (documents.path is stored root-relative form on a
    // root-configured brain): resolve it the way the repair path's V22
    // note describes — config.json's root, else CURATED_VAULT_ROOT.
    let vault_root: Option<String> = std::env::var("CURATED_VAULT_ROOT")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::fs::read_to_string(&paths_b.config_path)
                .ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .and_then(|v| {
                    v.get("vault_root")
                        .or_else(|| v.get("root"))
                        .and_then(|r| r.as_str())
                        .map(|s| s.to_string())
                })
        })
        .filter(|s| !s.is_empty());

    // 1. Copy live DB → scratch. WAL sidecars are checkpointed INTO the
    //    copy: open a ro conn (read-through), run `VACUUM INTO` — a
    //    consistent single-file snapshot that needs no sidecars.
    let scratch_path = paths_b.db_path.with_extension("db.verify-scratch");
    let _ = std::fs::remove_file(&scratch_path);
    {
        let ro = write::open_ro(&brain).context("open brain read-only")?;
        ro.execute(
            "VACUUM INTO ?1",
            rusqlite::params![scratch_path.to_string_lossy().to_string()],
        )
        .with_context(|| format!("VACUUM INTO {}", scratch_path.display()))?;
    }
    let scratch = rusqlite::Connection::open(&scratch_path)
        .with_context(|| format!("open scratch {}", scratch_path.display()))?;
    scratch.busy_timeout(std::time::Duration::from_secs(5))?;
    // The scratch copy is ALWAYS discarded (r26 m6 / test 3) — including the
    // `?` early-return paths below, which is why this is a Drop guard and
    // not an explicit cleanup call: Drop runs on every unwind/return path
    // after construction.
    struct ScratchGuard<'a>(&'a std::path::Path);
    impl Drop for ScratchGuard<'_> {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(self.0);
        }
    }
    let _scratch_guard = ScratchGuard(&scratch_path);

    let now_ms = now_ms_at();
    if dry_run {
        // Print-only (r26 m6), but the ANALYSIS is the real one (fix round):
        // the full pipeline below runs — snapshot+sample, chunk-only scratch
        // rechunk, evidence/hop lossless checks, per-doc verdicts — with
        // ONLY the live writes suppressed (no live rw conn, no race seam,
        // no stamp). The live DB stays byte-identical (pinned test 3).
        println!(
            "verify-scratch (dry-run): running the full analysis on a scratch copy of {} chunk-bearing docs — snapshot+sample, chunk-only rechunk, evidence/hop lossless checks; ALL live writes suppressed.",
            chunk_bearing_docs(&scratch)?.len()
        );
    }

    // Snapshot the backfill comparison data BEFORE the rechunk: stored
    // vectors belong to the LIVE chunk ids; the rechunk replaces those rows.
    // Per doc: (doc_id, path, chunk-id set at snapshot time).
    let docs = chunk_bearing_docs(&scratch)?;
    let mut snapshots: Vec<(i64, String, Vec<i64>)> = Vec::with_capacity(docs.len());
    for (id, path) in &docs {
        let mut stmt = scratch.prepare("SELECT id FROM chunks WHERE doc_id = ?1 ORDER BY id")?;
        let ids = stmt
            .query_map(rusqlite::params![id], |r| r.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        snapshots.push((*id, path.clone(), ids));
    }

    // Sampled backfill verdicts, computed on scratch (pre-rechunk vectors +
    // stored chunk text). Embeds happen OUTSIDE any txn (adjudicated Task
    // 8/12 pattern).
    let mut verdicts: Vec<ScratchDoc> = Vec::with_capacity(snapshots.len());
    for (id, path, ids) in &snapshots {
        let sample = sample_doc(&scratch, &profile, *id, path);
        verdicts.push(ScratchDoc {
            doc_id: *id,
            path: path.clone(),
            chunk_ids: ids.clone(),
            sample,
        });
    }

    // 2. Chunk-only scratch rechunk of every chunk-bearing doc. Skipped
    //    rechunks (unreadable file / un-ingestable extension / empty
    //    rechunk) are COUNTED and surfaced in the summary (fix round 4b):
    //    the artifact record must show them, not just a stderr line.
    let mut rechunk_skipped = 0usize;
    for (id, path) in &docs {
        match scratch_rechunk_doc(&scratch, *id, path, vault_root.as_deref()) {
            Ok(Some(hashes)) => {
                let _ = hashes; // hashes now stored on scratch chunks
            }
            Ok(None) => rechunk_skipped += 1,
            Err(e) => {
                // One unreadable file must not sink the whole check: leave
                // that doc's stored chunks as-is and note it.
                rechunk_skipped += 1;
                eprintln!("verify-scratch: rechunk skipped for {path}: {e}");
            }
        }
    }

    // 3. The two stamp conditions, both against the SCRATCH rechunk.
    let (ev_ok, ev_total, ev_first_fail) = evidence_refs_resolve(&scratch)?;
    let (hop_ok, hop_total, hop_first_missing) = gate_eligible_hops_resolve(&scratch, now_ms)?;
    let ev_holds = ev_first_fail.is_none();
    let hop_holds = hop_first_missing.is_none();

    // 4. Per-doc verdicts + guarded live backfill. The backfill applies
    //    regardless of the stamp outcome (it is evidence-driven per doc);
    //    the stamp decision below gates ONLY the stamp write.
    //    Under --dry-run there is NO live rw conn at all: the same verdict
    //    math runs, but the guarded write and the race seam are skipped
    //    (nothing may touch the live DB, r26 m6).
    let mut live = if dry_run {
        None
    } else {
        Some(write::open_rw(&brain).context("open brain read-write")?)
    };
    let mut backfilled = 0usize;
    let mut failed_samples = 0usize;
    let mut skipped_race = 0usize;
    for (i, v) in verdicts.iter().enumerate() {
        match &v.sample {
            Err(reason) => {
                failed_samples += 1;
                println!(
                    "[{}/{}] FAILED: {} — {reason}",
                    i + 1,
                    verdicts.len(),
                    v.path
                );
            }
            Ok(false) => {
                println!(
                    "[{}/{}] no-scorable-chunks: {}",
                    i + 1,
                    verdicts.len(),
                    v.path
                );
            }
            Ok(true) => {
                if dry_run {
                    backfilled += 1;
                    println!(
                        "[{}/{}] ok (dry-run: would backfill embed_key): {}",
                        i + 1,
                        verdicts.len(),
                        v.path
                    );
                    continue;
                }
                let live = live.as_mut().expect("live conn present when !dry_run");
                // TEST SEAM (Step-1 test 5): mutate the LIVE doc's chunk-id
                // set inside the sample-compute → guarded-write window, so
                // the under-lock re-check observes a changed set exactly as
                // a concurrent writer would produce it. Production never
                // sets this var.
                if let Ok(race_id) = std::env::var("CURATED_VERIFY_SCRATCH_TEST_RACE") {
                    if race_id == v.doc_id.to_string() {
                        test_race_mutate_live_chunks(live, v.doc_id);
                    }
                }
                // GUARDED per-doc write (r28-m6): ONE IMMEDIATE txn
                // re-checking the chunk-id set (equal sets in any order,
                // Task-8 rule) AND the current embed_key against the
                // scratch snapshot; skip on mismatch.
                let tx = live
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .with_context(|| format!("begin guarded txn for {}", v.path))?;
                let mut stmt = tx.prepare("SELECT id FROM chunks WHERE doc_id = ?1 ORDER BY id")?;
                let now_ids = stmt
                    .query_map(rusqlite::params![v.doc_id], |r| r.get::<_, i64>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                drop(stmt);
                let pre: HashSet<i64> = v.chunk_ids.iter().copied().collect();
                let now: HashSet<i64> = now_ids.into_iter().collect();
                let current_key: Option<String> = tx
                    .query_row(
                        "SELECT embed_key FROM documents WHERE id = ?1",
                        rusqlite::params![v.doc_id],
                        |r| r.get::<_, Option<String>>(0),
                    )
                    .optional()?
                    .flatten();
                if pre != now {
                    // True swap-race drift: the doc moved under us since the
                    // scratch snapshot. Next run retries.
                    tx.rollback().ok();
                    skipped_race += 1;
                    println!(
                        "[{}/{}] skip (chunk set changed since sampling): {}",
                        i + 1,
                        verdicts.len(),
                        v.path
                    );
                } else if current_key.as_deref() == Some(key.as_str()) {
                    // Benign idempotent skip: the sample matched but the key
                    // is already correct — nothing to backfill, not a race.
                    tx.rollback().ok();
                    skipped_race += 1;
                    println!(
                        "[{}/{}] skip (already backfilled): {}",
                        i + 1,
                        verdicts.len(),
                        v.path
                    );
                } else {
                    tx.execute(
                        "UPDATE documents SET embed_key = ?1 WHERE id = ?2",
                        rusqlite::params![key, v.doc_id],
                    )
                    .with_context(|| format!("backfill embed_key for {}", v.path))?;
                    tx.commit()
                        .with_context(|| format!("commit guarded txn for {}", v.path))?;
                    backfilled += 1;
                    println!(
                        "[{}/{}] ok: backfilled embed_key on {}",
                        i + 1,
                        verdicts.len(),
                        v.path
                    );
                }
            }
        }
    }

    // 5. The stamp: written ONLY if both lossless conditions hold — and
    //    NEVER under --dry-run (the verdict below is would-be).
    let exit_code = if ev_holds && hop_holds {
        if dry_run {
            println!(
                "verify-scratch (dry-run): stamp WOULD be written ({}); evidence refs {ev_ok}/{ev_total} resolve; gate-eligible hops {hop_ok}/{hop_total} resolve. Nothing written.",
                tauri_app_lib::chunker::CHUNKER_FINGERPRINT
            );
        } else {
            let live = live.as_mut().expect("live conn present when !dry_run");
            live.execute(
                "INSERT OR REPLACE INTO llm_wiki_meta (key, value) VALUES ('reindex_stamp', ?1)",
                rusqlite::params![tauri_app_lib::chunker::CHUNKER_FINGERPRINT],
            )
            .context("write reindex_stamp")?;
            println!(
                "verify-scratch: stamp written ({}); evidence refs {ev_ok}/{ev_total} resolve; gate-eligible hops {hop_ok}/{hop_total} resolve.",
                tauri_app_lib::chunker::CHUNKER_FINGERPRINT
            );
        }
        0
    } else if dry_run {
        let mut msg = String::from(
            "verify-scratch (dry-run) WOULD REFUSE: the scratch rechunk is not lossless —",
        );
        if let Some(entry) = &ev_first_fail {
            msg.push_str(&format!(
                " evidence refs orphaned ({ev_ok}/{ev_total} resolve; first missing: entry {entry});"
            ));
        }
        if let Some(entry) = &hop_first_missing {
            msg.push_str(&format!(
                " gate-eligible entries lost their hop ({hop_ok}/{hop_total} resolve; first missing: entry {entry});"
            ));
        }
        msg.push_str(
            " a chunker bump must NOT rechunk until the rechunk is LOSSLESS (every evidence hash ref and gate-eligible hop still resolves on the rechunk). Nothing written.",
        );
        println!("{msg}");
        1
    } else {
        let mut msg = String::from("verify-scratch REFUSED: the scratch rechunk is not lossless —");
        if let Some(entry) = &ev_first_fail {
            msg.push_str(&format!(
                " evidence refs orphaned ({ev_ok}/{ev_total} resolve; first missing: entry {entry});"
            ));
        }
        if let Some(entry) = &hop_first_missing {
            msg.push_str(&format!(
                " gate-eligible entries lost their hop ({hop_ok}/{hop_total} resolve; first missing: entry {entry});"
            ));
        }
        msg.push_str(
            " a chunker bump must NOT rechunk until the rechunk is LOSSLESS (every evidence hash ref and gate-eligible hop still resolves on the rechunk). No stamp written.",
        );
        println!("{msg}");
        1
    };

    let summary = if dry_run {
        format!(
            "verify-scratch (dry-run): {backfilled} would backfill, {failed_samples} failed samples, {skipped_race} skipped, {rechunk_skipped} rechunks skipped. Nothing written."
        )
    } else {
        format!(
            "verify-scratch: {backfilled} backfilled, {failed_samples} failed samples, {skipped_race} skipped, {rechunk_skipped} rechunks skipped."
        )
    };
    println!("{summary}");

    // Final candidate-set coverage over a fresh read conn (post-commit).
    drop(live);
    // Scratch discarded by the Drop guard.
    let ro = write::open_ro(&brain)?;
    print_coverage(&ro, &key, now_ms)?;
    let _ = std::io::stdout().flush();
    Ok(exit_code)
}
