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

use anyhow::Result;
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

/// The text an entry embeds to. Title and body joined by a blank line — the
/// prose the librarian actually curated, same convention as chunk text.
///
/// Both the sweep and the write-time path call this so a re-embed always
/// produces a vector comparable to the original.
pub fn embed_text_for_entry(title: &str, body: &str) -> String {
    format!("{title}\n\n{body}")
}

/// Fill `embedding_blob` for live entries that have none.
///
/// Does at most `max_batches` provider calls of up to `SWEEP_BATCH_SIZE`
/// entries each. A batch that fails to embed is counted in `failed` and its
/// rows stay NULL for a later run — an embedding is a derived artifact and must
/// never be worth failing a caller over.
///
/// Network I/O happens between transactions, never inside one: each batch is
/// embedded first, then written.
pub fn sweep_null_embeddings(
    conn: &Connection,
    profile: &EmbedProfile,
    max_batches: usize,
) -> Result<SweepReport> {
    let mut report = SweepReport::default();

    for _ in 0..max_batches {
        let pending: Vec<(String, String, String)> = {
            let mut stmt = conn.prepare(
                "SELECT id, title, body FROM llm_wiki_entries
                  WHERE deleted_at IS NULL AND embedding_blob IS NULL
                  ORDER BY id
                  LIMIT ?1",
            )?;
            let rows = stmt.query_map([SWEEP_BATCH_SIZE as i64], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };

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

        let tx = conn.unchecked_transaction()?;
        for ((id, _, _), vector) in pending.iter().zip(vectors.iter()) {
            let blob = f32_vec_to_blob(vector);
            // The `IS NULL` guard makes a concurrent write-time embed win
            // rather than being overwritten by this slower sweep.
            let updated = tx.execute(
                "UPDATE llm_wiki_entries
                    SET embedding_blob = ?1
                  WHERE id = ?2 AND embedding_blob IS NULL",
                params![blob, id],
            )?;
            report.filled += updated;
        }
        tx.commit()?;
    }

    report.remaining_null = conn.query_row(
        "SELECT COUNT(*) FROM llm_wiki_entries
          WHERE deleted_at IS NULL AND embedding_blob IS NULL",
        [],
        |r| r.get::<_, i64>(0),
    )? as usize;

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
    fn embed_text_joins_title_and_body() {
        assert_eq!(
            embed_text_for_entry("A title", "A body."),
            "A title\n\nA body."
        );
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
}
