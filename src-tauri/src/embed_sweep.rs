//! Fills `llm_wiki_entries.embedding_blob` for live entries that have none.
//!
//! This is the single mechanism behind two requirements in the design spec:
//! Part B's retry path (a write-time embed that failed leaves NULL, and this
//! picks it up) and Part C's one-time backfill (every pre-existing entry is
//! NULL, and this fills them). Both key on `embedding_blob IS NULL`, so they
//! are the same code — no queue table (YAGNI).
//!
//! Bounded by design: at most `max_batches * SWEEP_BATCH_SIZE` entries per call,
//! mirroring the v1.39.0 watchdog's budget discipline.

use anyhow::{bail, Result};
use rusqlite::{params, Connection};

use crate::embedder::{embed_batch, EmbedProfile};
use crate::wiki_graph::f32_vec_to_blob;

pub const SWEEP_BATCH_SIZE: usize = 64;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// Entries that got a blob this run.
    pub filled: usize,
    /// Entries whose batch failed to embed; they stay NULL for the next run.
    pub failed: usize,
    /// Live entries still NULL after this run (including `failed`).
    pub remaining_null: usize,
}

/// The text an entry embeds to: the WRITE-scheme document text (see
/// [`crate::embed_scheme::doc_text_for_entry`]). Under the `instr1` WRITE
/// scheme (issue #265) the canonical instruction prefix is prepended to the
/// raw `title\n\nbody` prose.
///
/// Both the sweep and the write-time path call this so a re-embed always
/// produces a vector comparable to the original, and write-time parity
/// (`db/commit.rs`) compares against this same function — parity and write
/// must never disagree about what was fed to the provider.
pub fn embed_text_for_entry(title: &str, body: &str) -> String {
    crate::embed_scheme::doc_text_for_entry(title, body)
}

/// SELECT one batch of live entries with `embedding_blob IS NULL`, ordered by
/// id for deterministic processing.
///
/// Exposed so the production sweep loop in `lib::run_embedding_sweep` can
/// fetch a batch under a short-lived lock, drop it, do the blocking network
/// embed call, then re-lock to write — keeping `DbState` available to other
/// Tauri DB commands during the embed round-trip. `sweep_null_embeddings`
/// below uses this same helper for the in-process loop.
pub fn pending_null_batch(
    conn: &Connection,
    limit: usize,
) -> Result<Vec<(String, String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT id, title, body FROM llm_wiki_entries
          WHERE deleted_at IS NULL AND embedding_blob IS NULL
          ORDER BY id
          LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Persist a batch of vectors to `embedding_blob` in a single transaction.
///
/// The `embedding_blob IS NULL` guard keeps a concurrent write-time embed from
/// being overwritten by this slower sweep; the `deleted_at IS NULL` guard
/// aligns the UPDATE with the SELECT that produced the batch, so a row
/// soft-deleted between the two phases is not resurrected with a vector.
/// Returns the number of rows actually filled — `batch.len()` minus any rows
/// that gained a blob or were soft-deleted since the SELECT.
///
/// R6 zip-truncation guard: if the provider returned a different number of
/// vectors than the batch holds, we cannot safely pair them via `zip` — a
/// mis-pairing would silently describe the wrong row. Bail with a length
/// mismatch error so every caller is protected without relying on
/// caller-side checks (CodeRabbit review thread #4, 2026-09-01).
pub fn apply_embeddings(
    conn: &Connection,
    batch: &[(String, String, String)],
    vectors: &[Vec<f32>],
) -> Result<usize> {
    if batch.len() != vectors.len() {
        bail!(
            "apply_embeddings: length mismatch — batch has {} entries, vectors has {}",
            batch.len(),
            vectors.len()
        );
    }
    let tx = conn.unchecked_transaction()?;
    let mut filled = 0usize;
    for ((id, _, _), vector) in batch.iter().zip(vectors.iter()) {
        let blob = f32_vec_to_blob(vector);
        // Issue #265: the stamp rides the same statement as the blob — a
        // vector must never exist under a stale or unknown scheme. The
        // sweep always embeds under the WRITE scheme.
        let updated = tx.execute(
            "UPDATE llm_wiki_entries
                SET embedding_blob = ?1, embed_scheme = ?2
              WHERE id = ?3 AND embedding_blob IS NULL AND deleted_at IS NULL",
            params![blob, crate::embed_scheme::WRITE_SCHEME, id],
        )?;
        filled += updated;
    }
    tx.commit()?;
    Ok(filled)
}

/// Count live entries still missing `embedding_blob`. Used to populate
/// `SweepReport::remaining_null` after a sweep run.
pub fn count_null_entries(conn: &Connection) -> Result<usize> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM llm_wiki_entries
          WHERE deleted_at IS NULL AND embedding_blob IS NULL",
        [],
        |r| r.get::<_, i64>(0),
    )? as usize)
}

/// Fill `embedding_blob` for live entries that have none.
///
/// Does at most `max_batches` provider calls of up to `SWEEP_BATCH_SIZE`
/// entries each. A batch that fails to embed is counted in `failed` and its
/// rows stay NULL for a later run — an embedding is a derived artifact and must
/// never be worth failing a caller over.
///
/// Network I/O happens between transactions, never inside one: each batch is
/// embedded first, then written. The production caller
/// (`lib::run_embedding_sweep`) drives the same per-batch SELECT → embed →
/// UPDATE cadence itself so it can drop the `DbState` mutex between phases;
/// this function exists for tests and any single-connection caller.
pub fn sweep_null_embeddings(
    conn: &Connection,
    profile: &EmbedProfile,
    max_batches: usize,
) -> Result<SweepReport> {
    let mut report = SweepReport::default();

    for _ in 0..max_batches {
        let pending = pending_null_batch(conn, SWEEP_BATCH_SIZE)?;
        if pending.is_empty() {
            break;
        }

        let texts: Vec<String> = pending
            .iter()
            .map(|(_, title, body)| embed_text_for_entry(title, body))
            .collect();

        // Outside any transaction: this is the blocking network call.
        let vectors = match embed_batch(profile, texts) {
            Ok(v) => v,
            Err(e) => {
                eprintln!(
                    "embed_sweep: batch of {} entries failed to embed: {e}",
                    pending.len()
                );
                report.failed += pending.len();
                // Every remaining null row is unreachable this run; stop rather
                // than hammering a provider that is already failing.
                break;
            }
        };

        // R6 zip-truncation guard lives inside `apply_embeddings` so every
        // caller is protected; treat a length-mismatch error as a failed
        // batch and stop the run.
        //
        // Anything else is a DB failure (schema drift, SQLITE_BUSY, disk
        // full) and must propagate: swallowing it here reports real data
        // loss as a recoverable provider hiccup and lets `graph_reanchor`
        // print "done." and exit 0 on a database it never actually touched.
        // Mirrors `lib::run_embedding_sweep`, which already discriminates.
        match apply_embeddings(conn, &pending, &vectors) {
            Ok(n) => report.filled += n,
            Err(e) if e.to_string().contains("length mismatch") => {
                // Static message — the anyhow error chain may carry DB
                // context but not the resolved embed key, and printing
                // `{e}` here invites CodeQL to re-flag the data flow.
                eprintln!(
                    "embed_sweep: provider returned a mismatched number of vectors; \
                     skipping batch to avoid mis-pairing"
                );
                report.failed += pending.len();
                break;
            }
            Err(e) => return Err(e),
        }
    }

    report.remaining_null = count_null_entries(conn)?;

    Ok(report)
}

/// Live rows carrying a blob but not stamped `instr1` — the scheme-filtered
/// sweep's workset (spec §Migration window semantics, mechanism 1). The
/// `embed_scheme !=` filter is what makes crash resume trivial: a run
/// interrupted mid-batch re-selects exactly the rows it did not finish, and
/// a row stamped by a concurrent writer drops out of the workset on its own.
pub fn pending_scheme_batch(
    conn: &Connection,
    limit: usize,
) -> Result<Vec<(String, String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT id, title, body FROM llm_wiki_entries
          WHERE deleted_at IS NULL AND embedding_blob IS NOT NULL
            AND embed_scheme != ?2
          ORDER BY id
          LIMIT ?1",
    )?;
    let rows = stmt.query_map(
        params![limit as i64, crate::embed_scheme::WRITE_SCHEME],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct SchemeSweepReport {
    /// Unstamped rows re-embedded and re-stamped under the WRITE scheme.
    pub reembedded: usize,
    /// Rows whose batch failed to embed; they keep their old stamp for the
    /// next run.
    pub failed: usize,
    /// Live non-null rows still not stamped `instr1` after this run — the
    /// cutover precondition count.
    pub remaining_raw: usize,
}

/// Persist a scheme re-embed batch: blob + stamp in one statement, guarded on
/// `embed_scheme != WRITE_SCHEME` so the UPDATE aligns with the SELECT that
/// produced the batch. Mirrors [`apply_embeddings`] (R6 length guard, one
/// transaction); the differing guard is the resume mechanism, not drift.
pub fn apply_scheme_embeddings(
    conn: &Connection,
    batch: &[(String, String, String)],
    vectors: &[Vec<f32>],
) -> Result<usize> {
    if batch.len() != vectors.len() {
        bail!(
            "apply_scheme_embeddings: length mismatch — batch has {} entries, vectors has {}",
            batch.len(),
            vectors.len()
        );
    }
    let tx = conn.unchecked_transaction()?;
    let mut stamped = 0usize;
    for ((id, _, _), vector) in batch.iter().zip(vectors.iter()) {
        let blob = f32_vec_to_blob(vector);
        // Same-statement stamping (GLM N2): a vector may never exist under a
        // stale or unknown scheme. The stamp always comes from the WRITE
        // constant.
        let updated = tx.execute(
            "UPDATE llm_wiki_entries
                SET embedding_blob = ?1, embed_scheme = ?2
              WHERE id = ?3 AND embedding_blob IS NOT NULL
                AND embed_scheme != ?4 AND deleted_at IS NULL",
            params![
                blob,
                crate::embed_scheme::WRITE_SCHEME,
                id,
                crate::embed_scheme::WRITE_SCHEME
            ],
        )?;
        stamped += updated;
    }
    tx.commit()?;
    Ok(stamped)
}

/// Live rows with a non-null blob that are NOT stamped `instr1` — the count
/// `ct wisdom scheme activate` refuses on. Any non-WRITE stamp (raw or a
/// future value) blocks cutover; NULL-blob rows are scheme-agnostic and do
/// not block (spec §Scope decision 2).
pub fn count_unstamped_entries(conn: &Connection) -> Result<usize> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM llm_wiki_entries
          WHERE deleted_at IS NULL AND embedding_blob IS NOT NULL
            AND embed_scheme != ?1",
        [crate::embed_scheme::WRITE_SCHEME],
        |r| r.get::<_, i64>(0),
    )? as usize)
}

/// Per-scheme counts over live rows — what `ct wisdom scheme status` reports.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SchemeCounts {
    /// Live non-null rows stamped `raw` (the pre-#265 scheme).
    pub raw: usize,
    /// Live non-null rows stamped `instr1`.
    pub instr1: usize,
    /// Live non-null rows stamped neither (fail-closed visibility: any value
    /// here also blocks cutover via [`count_unstamped_entries`]).
    pub other: usize,
    /// Live rows with no blob — scheme-agnostic, invisible to both readers,
    /// picked up by the normal NULL sweep.
    pub null_blob: usize,
}

/// Compute [`SchemeCounts`] in one pass over live rows.
pub fn scheme_counts(conn: &Connection) -> Result<SchemeCounts> {
    conn.query_row(
        "SELECT
            COALESCE(SUM(CASE WHEN embedding_blob IS NOT NULL AND embed_scheme = ?1
                              THEN 1 ELSE 0 END), 0),
            COALESCE(SUM(CASE WHEN embedding_blob IS NOT NULL AND embed_scheme = ?2
                              THEN 1 ELSE 0 END), 0),
            COALESCE(SUM(CASE WHEN embedding_blob IS NOT NULL AND embed_scheme != ?1
                               AND embed_scheme != ?2 THEN 1 ELSE 0 END), 0),
            COALESCE(SUM(CASE WHEN embedding_blob IS NULL THEN 1 ELSE 0 END), 0)
          FROM llm_wiki_entries WHERE deleted_at IS NULL",
        params![
            crate::embed_scheme::SCHEME_RAW,
            crate::embed_scheme::WRITE_SCHEME
        ],
        |r| {
            Ok(SchemeCounts {
                raw: r.get::<_, i64>(0)? as usize,
                instr1: r.get::<_, i64>(1)? as usize,
                other: r.get::<_, i64>(2)? as usize,
                null_blob: r.get::<_, i64>(3)? as usize,
            })
        },
    )
    .map_err(Into::into)
}

/// Re-embed live rows whose blob exists but whose stamp is not the WRITE
/// scheme, stamping as it goes (issue #265 cutover sweep). Same cadence and
/// failure semantics as [`sweep_null_embeddings`]: provider failures are
/// reported, not propagated; DB failures propagate. Idempotent — the
/// `embed_scheme != WRITE_SCHEME` filter is naturally resumable, so a second
/// run finds an empty workset.
pub fn sweep_scheme_embeddings(
    conn: &Connection,
    profile: &EmbedProfile,
    max_batches: usize,
) -> Result<SchemeSweepReport> {
    let mut report = SchemeSweepReport::default();

    for _ in 0..max_batches {
        let pending = pending_scheme_batch(conn, SWEEP_BATCH_SIZE)?;
        if pending.is_empty() {
            break;
        }

        let texts: Vec<String> = pending
            .iter()
            .map(|(_, title, body)| embed_text_for_entry(title, body))
            .collect();

        // Outside any transaction: this is the blocking network call.
        let vectors = match embed_batch(profile, texts) {
            Ok(v) => v,
            Err(e) => {
                eprintln!(
                    "embed_sweep: scheme batch of {} entries failed to embed: {e}",
                    pending.len()
                );
                report.failed += pending.len();
                break;
            }
        };

        // Same discriminator as `sweep_null_embeddings`: the R6 length guard
        // is a soft (reportable) failure, anything else is a real DB failure
        // and must propagate.
        match apply_scheme_embeddings(conn, &pending, &vectors) {
            Ok(n) => report.reembedded += n,
            Err(e) if e.to_string().contains("length mismatch") => {
                eprintln!(
                    "embed_sweep: provider returned a mismatched number of vectors; \
                     skipping batch to avoid mis-pairing"
                );
                report.failed += pending.len();
                break;
            }
            Err(e) => return Err(e),
        }
    }

    report.remaining_raw = count_unstamped_entries(conn)?;

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::open_in_memory;

    fn seed_entry(conn: &Connection, id: &str, deleted_at_ms: Option<i64>, blob: Option<Vec<u8>>) {
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                source_hash, source_ref, created_at, updated_at, last_accessed_at,
                access_count, deleted_at, embedding_blob, embedding
             ) VALUES (?1, 'ent-1', ?2, 'Body text.', '[]', 'inferred',
                       'librarian_inferred', NULL, NULL, 100, 100, NULL, 0, ?3, ?4, NULL)",
            params![id, format!("Title {id}"), deleted_at_ms, blob],
        )
        .unwrap();
    }

    fn blob_of(conn: &Connection, id: &str) -> Option<Vec<u8>> {
        conn.query_row(
            "SELECT embedding_blob FROM llm_wiki_entries WHERE id = ?1",
            [id],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn a_db_failure_during_apply_propagates_instead_of_counting_as_a_provider_hiccup() {
        // `graph_reanchor` drives this function. If a DB failure were folded
        // into `report.failed` the tool would print "done." and exit 0 on a
        // database it never actually wrote to, and tell the operator to
        // re-run — forever. Only the length-mismatch guard is a soft failure.
        temp_env::with_vars([("CURATED_EMBED_STUB", Some("constant8"))], || {
            let conn = open_in_memory().unwrap();
            seed_entry(&conn, "fact_a", None, None);
            // Real DB failure inside `apply_embeddings`: the UPDATE target is
            // gone. The SELECT that builds the batch runs against a view so
            // the batch is non-empty before the table disappears.
            conn.execute_batch(
                "CREATE TABLE entries_backup AS SELECT * FROM llm_wiki_entries;
                 DROP TABLE llm_wiki_entries;
                 CREATE VIEW llm_wiki_entries AS SELECT * FROM entries_backup;",
            )
            .unwrap();

            let err = sweep_null_embeddings(&conn, &EmbedProfile::default(), 4)
                .expect_err("a DB failure must propagate, not be reported as a failed batch");
            assert!(
                !err.to_string().contains("length mismatch"),
                "the soft path is only for provider cardinality errors, got: {err}"
            );
        });
    }

    #[test]
    fn embed_text_joins_title_and_body() {
        // Under the instr1 WRITE scheme (issue #265) the canonical instruction
        // is prepended to the raw prose — same shape the query side uses.
        assert_eq!(
            embed_text_for_entry("A title", "A body."),
            format!(
                "{}A title\n\nA body.",
                crate::embed_scheme::QUERY_INSTRUCTION_PREFIX
            )
        );
    }

    /// Test (c), spec §4 (write path): the sweep's `apply_embeddings` — the
    /// single writer behind both the runtime sweep and the `graph_reanchor`
    /// migration bin — stamps `embed_scheme = WRITE_SCHEME` in the same
    /// UPDATE as the blob.
    #[test]
    fn apply_embeddings_stamps_embed_scheme() {
        temp_env::with_vars([("CURATED_EMBED_STUB", Some("constant8"))], || {
            let conn = open_in_memory().unwrap();
            seed_entry(&conn, "fact_stamp_a", None, None);

            let report = sweep_null_embeddings(&conn, &EmbedProfile::default(), 1).unwrap();
            assert_eq!(report.filled, 1);

            let scheme: String = conn
                .query_row(
                    "SELECT embed_scheme FROM llm_wiki_entries WHERE id = 'fact_stamp_a'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(scheme, crate::embed_scheme::WRITE_SCHEME);

            // The migration-bin path uses `apply_embeddings` directly; prove
            // the stamp rides there too (same statement as the blob write).
            seed_entry(&conn, "fact_stamp_b", None, None);
            let batch = vec![("fact_stamp_b".to_string(), "t".to_string(), "b".to_string())];
            let filled = apply_embeddings(&conn, &batch, &[vec![0.5_f32; 8]]).unwrap();
            assert_eq!(filled, 1);
            let scheme_b: String = conn
                .query_row(
                    "SELECT embed_scheme FROM llm_wiki_entries WHERE id = 'fact_stamp_b'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(scheme_b, crate::embed_scheme::WRITE_SCHEME);
        });
    }

    #[test]
    fn fills_null_blobs_for_live_entries_only() {
        temp_env::with_vars([("CURATED_EMBED_STUB", Some("constant8"))], || {
            let conn = open_in_memory().unwrap();
            seed_entry(&conn, "fact_null_a", None, None);
            seed_entry(&conn, "fact_null_b", None, None);
            seed_entry(&conn, "fact_deleted", Some(200_000), None);
            seed_entry(&conn, "fact_already", None, Some(vec![0u8; 32]));

            let profile = EmbedProfile::default();
            let report = sweep_null_embeddings(&conn, &profile, 10).unwrap();

            assert_eq!(report.filled, 2);
            assert_eq!(report.failed, 0);
            assert_eq!(report.remaining_null, 0);

            // constant8 yields 8-dimension vectors -> 32 bytes.
            assert_eq!(blob_of(&conn, "fact_null_a").map(|b| b.len()), Some(32));
            assert_eq!(blob_of(&conn, "fact_null_b").map(|b| b.len()), Some(32));
            // Soft-deleted entries are not embedded.
            assert_eq!(blob_of(&conn, "fact_deleted"), None);
            // An entry that already had a blob is left exactly as it was.
            assert_eq!(blob_of(&conn, "fact_already"), Some(vec![0u8; 32]));
        });
    }

    #[test]
    fn sweep_is_idempotent() {
        temp_env::with_vars([("CURATED_EMBED_STUB", Some("constant8"))], || {
            let conn = open_in_memory().unwrap();
            seed_entry(&conn, "fact_a", None, None);
            let profile = EmbedProfile::default();

            let first = sweep_null_embeddings(&conn, &profile, 10).unwrap();
            assert_eq!(first.filled, 1);
            let blob_after_first = blob_of(&conn, "fact_a");

            let second = sweep_null_embeddings(&conn, &profile, 10).unwrap();
            assert_eq!(second.filled, 0, "nothing left to do");
            assert_eq!(second.remaining_null, 0);
            assert_eq!(blob_of(&conn, "fact_a"), blob_after_first);
        });
    }

    #[test]
    fn sweep_on_a_clean_db_is_a_cheap_no_op() {
        temp_env::with_vars([("CURATED_EMBED_STUB", Some("constant8"))], || {
            let conn = open_in_memory().unwrap();
            let profile = EmbedProfile::default();
            let report = sweep_null_embeddings(&conn, &profile, 10).unwrap();
            assert_eq!(report, SweepReport::default());
        });
    }

    #[test]
    fn max_batches_bounds_the_work() {
        temp_env::with_vars([("CURATED_EMBED_STUB", Some("constant8"))], || {
            let conn = open_in_memory().unwrap();
            for i in 0..(SWEEP_BATCH_SIZE + 5) {
                seed_entry(&conn, &format!("fact_{i}"), None, None);
            }
            let profile = EmbedProfile::default();

            // One batch only: exactly SWEEP_BATCH_SIZE filled, the rest left.
            let report = sweep_null_embeddings(&conn, &profile, 1).unwrap();
            assert_eq!(report.filled, SWEEP_BATCH_SIZE);
            assert_eq!(report.remaining_null, 5);

            // A follow-up run finishes the job.
            let report2 = sweep_null_embeddings(&conn, &profile, 10).unwrap();
            assert_eq!(report2.filled, 5);
            assert_eq!(report2.remaining_null, 0);
        });
    }

    #[test]
    fn a_failing_provider_leaves_rows_null_and_reports_them() {
        // No CURATED_EMBED_STUB set, and a Cloud profile whose backend is not
        // implemented -> embed_batch returns Err. The sweep must not propagate
        // the error; it reports the failure and leaves the rows NULL for the
        // next run.
        temp_env::with_vars([("CURATED_EMBED_STUB", None::<&str>)], || {
            let conn = open_in_memory().unwrap();
            seed_entry(&conn, "fact_a", None, None);
            let profile = EmbedProfile::Cloud {
                provider: crate::embedder::CloudProvider::OpenAi,
                model: "unreachable".into(),
                api_key: String::new(),
            };

            let report = sweep_null_embeddings(&conn, &profile, 10).unwrap();

            assert_eq!(report.filled, 0);
            assert_eq!(report.failed, 1);
            assert_eq!(report.remaining_null, 1);
            assert_eq!(blob_of(&conn, "fact_a"), None);
        });
    }

    #[test]
    fn a_provider_that_returns_too_few_vectors_skips_the_batch() {
        // R6 length-mismatch guard: `constant8_short` returns N-1 vectors for
        // an N-text batch. The sweep must not pair them; it must report every
        // entry as `failed` and leave all rows NULL for a later run.
        temp_env::with_vars([("CURATED_EMBED_STUB", Some("constant8_short"))], || {
            let conn = open_in_memory().unwrap();
            seed_entry(&conn, "fact_a", None, None);
            seed_entry(&conn, "fact_b", None, None);
            let profile = EmbedProfile::default();

            let report = sweep_null_embeddings(&conn, &profile, 10).unwrap();

            assert_eq!(report.filled, 0, "no rows may be paired");
            assert_eq!(report.failed, 2, "the whole batch is reported failed");
            assert_eq!(report.remaining_null, 2);
            assert_eq!(blob_of(&conn, "fact_a"), None);
            assert_eq!(blob_of(&conn, "fact_b"), None);
        });
    }

    // -------------------------------------------------------------------------
    // Scheme-filtered sweep (issue #265, plan Task 4). The stub embedder makes
    // these structural: no network, no real provider.
    // -------------------------------------------------------------------------

    /// Seed a live row carrying a blob under a given stamp. `raw` fixtures
    /// emulate pre-#265 rows; `instr1` fixtures emulate already-migrated rows.
    fn seed_stamped(conn: &Connection, id: &str, blob: Option<Vec<u8>>, scheme: Option<&str>) {
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                source_hash, source_ref, created_at, updated_at, last_accessed_at,
                access_count, deleted_at, embedding_blob, embed_scheme, embedding
             ) VALUES (?1, 'ent-1', ?2, 'Body text.', '[]', 'inferred',
                       'librarian_inferred', NULL, NULL, 100, 100, NULL, 0,
                       NULL, ?3, ?4, NULL)",
            params![
                id,
                format!("Title {id}"),
                blob,
                scheme.unwrap_or(crate::embed_scheme::SCHEME_RAW)
            ],
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

    #[test]
    fn scheme_sweep_reembeds_and_stamps_exactly_the_unstamped_rows() {
        temp_env::with_vars([("CURATED_EMBED_STUB", Some("constant8"))], || {
            let conn = open_in_memory().unwrap();
            seed_stamped(&conn, "fact_raw_a", Some(vec![0u8; 32]), Some("raw"));
            seed_stamped(&conn, "fact_raw_b", Some(vec![1u8; 32]), Some("raw"));
            seed_stamped(
                &conn,
                "fact_done",
                Some(vec![2u8; 32]),
                Some(crate::embed_scheme::WRITE_SCHEME),
            );
            seed_entry(&conn, "fact_null", None, None); // scheme-agnostic
            let raw_blob_before = blob_of(&conn, "fact_done");

            let report = sweep_scheme_embeddings(&conn, &EmbedProfile::default(), 10).unwrap();

            assert_eq!(report.reembedded, 2, "only the raw-stamped rows");
            assert_eq!(report.failed, 0);
            assert_eq!(report.remaining_raw, 0);
            assert_eq!(
                scheme_of(&conn, "fact_raw_a"),
                crate::embed_scheme::WRITE_SCHEME
            );
            assert_eq!(
                scheme_of(&conn, "fact_raw_b"),
                crate::embed_scheme::WRITE_SCHEME
            );
            // constant8 yields 8-dim vectors -> 32 bytes: the blob was rewritten.
            assert_eq!(blob_of(&conn, "fact_raw_a").map(|b| b.len()), Some(32));
            // Already-stamped and NULL rows are untouched.
            assert_eq!(blob_of(&conn, "fact_done"), raw_blob_before);
            assert_eq!(blob_of(&conn, "fact_null"), None);
        });
    }

    #[test]
    fn scheme_sweep_is_idempotent_and_resumes() {
        temp_env::with_vars([("CURATED_EMBED_STUB", Some("constant8"))], || {
            let conn = open_in_memory().unwrap();
            // More than one batch: a bounded first run must stop mid-workset,
            // and the second run picks up exactly the remainder (resume proof).
            for i in 0..(SWEEP_BATCH_SIZE + 5) {
                seed_stamped(
                    &conn,
                    &format!("fact_{i}"),
                    Some(vec![0u8; 32]),
                    Some("raw"),
                );
            }
            let profile = EmbedProfile::default();

            let first = sweep_scheme_embeddings(&conn, &profile, 1).unwrap();
            assert_eq!(first.reembedded, SWEEP_BATCH_SIZE);
            assert_eq!(first.remaining_raw, 5);

            let second = sweep_scheme_embeddings(&conn, &profile, 10).unwrap();
            assert_eq!(second.reembedded, 5);
            assert_eq!(second.remaining_raw, 0);

            // A third run finds an empty workset: the interrupted-mid-batch
            // case after completion — pure no-op.
            let third = sweep_scheme_embeddings(&conn, &profile, 10).unwrap();
            assert_eq!(third, SchemeSweepReport::default());
        });
    }

    #[test]
    fn scheme_sweep_counts_a_foreign_stamp_as_unstamped() {
        // Deploy-skew rows (spec §Scope decision 5): a stamp that is neither
        // raw nor instr1 still blocks cutover and must be re-embedded.
        temp_env::with_vars([("CURATED_EMBED_STUB", Some("constant8"))], || {
            let conn = open_in_memory().unwrap();
            seed_stamped(&conn, "fact_skew", Some(vec![0u8; 32]), Some("raw_old_bin"));

            let counts = scheme_counts(&conn).unwrap();
            assert_eq!(counts.other, 1);
            assert_eq!(count_unstamped_entries(&conn).unwrap(), 1);

            let report = sweep_scheme_embeddings(&conn, &EmbedProfile::default(), 10).unwrap();
            assert_eq!(report.reembedded, 1);
            assert_eq!(report.remaining_raw, 0);
            assert_eq!(
                scheme_of(&conn, "fact_skew"),
                crate::embed_scheme::WRITE_SCHEME
            );
        });
    }

    #[test]
    fn apply_scheme_embeddings_refuses_mismatched_vectors() {
        let conn = open_in_memory().unwrap();
        seed_stamped(&conn, "fact_a", Some(vec![0u8; 32]), Some("raw"));
        let batch = vec![("fact_a".to_string(), "t".to_string(), "b".to_string())];
        let err = apply_scheme_embeddings(&conn, &batch, &[]).unwrap_err();
        assert!(err.to_string().contains("length mismatch"), "{err}");
        // Refusal leaves the row exactly as it was.
        assert_eq!(scheme_of(&conn, "fact_a"), "raw");
    }

    #[test]
    fn scheme_counts_report_each_class() {
        let conn = open_in_memory().unwrap();
        seed_stamped(&conn, "c_raw", Some(vec![0u8; 32]), Some("raw"));
        seed_stamped(
            &conn,
            "c_instr",
            Some(vec![0u8; 32]),
            Some(crate::embed_scheme::WRITE_SCHEME),
        );
        seed_stamped(&conn, "c_other", Some(vec![0u8; 32]), Some("weird"));
        seed_entry(&conn, "c_null", None, None);
        seed_stamped(&conn, "c_deleted", Some(vec![0u8; 32]), Some("raw"));
        conn.execute(
            "UPDATE llm_wiki_entries SET deleted_at = 1 WHERE id = 'c_deleted'",
            [],
        )
        .unwrap();

        let counts = scheme_counts(&conn).unwrap();
        assert_eq!(counts.raw, 1);
        assert_eq!(counts.instr1, 1);
        assert_eq!(counts.other, 1);
        assert_eq!(counts.null_blob, 1, "soft-deleted rows are excluded");
        // The cutover precondition counts every non-WRITE non-null row.
        assert_eq!(count_unstamped_entries(&conn).unwrap(), 2);
    }
}
