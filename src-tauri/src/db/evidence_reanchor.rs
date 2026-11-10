//! Quote-text re-anchoring for dead evidence hashes (`ct evidence re-anchor`).
//!
//! Root cause this repairs: `compute_chunk_hash` binds an anchor to chunk
//! text + doc path + enumeration position, so ANY edit that shifts chunk
//! boundaries orphans a previously-live hash even when the quoted text still
//! survives verbatim inside a current chunk. The nightly heal would then
//! soft-delete the entry — the correct last resort, but wasteful when the
//! quote is recoverable.
//!
//! For every dead `content_hash` in a live evidence row, this pass searches
//! the CURRENT chunks for the item's `quote` as a verbatim substring:
//!
//! - exactly ONE chunk contains the quote → re-anchor: rewrite the item's
//!   `content_hash` (recomputed via [`compute_chunk_hash`] from the matched
//!   chunk — never trusted from the row), `chunk_id`, `start_line`,
//!   `end_line`;
//! - zero chunks → unresolved (left flagged; the heal owns it);
//! - more than one DISTINCT chunk → ambiguous (left flagged; guessing would
//!   fabricate provenance);
//! - matched chunk's stored hash ≠ recomputed hash → chunker drift; skip.
//!
//! When EVERY item of an entry resolves (its dead ones re-anchored, its live
//! ones still live), `evidence_has_live_chunk` — the same grounding contract
//! the commit gate and regrade use — agrees, and `unanchored` is cleared.
//!
//! Safety posture (modeled on `evidence_regrade`):
//! - soft-deleted entries are NEVER touched (the 90-day prune owns them);
//! - rows are only ever UPDATED, never deleted — there is no destructive
//!   phase, but it is still a brain write, so the CLI gates on `--yes` and
//!   runs a dry-run summary by default;
//! - apply runs in ONE IMMEDIATE transaction; each target row's
//!   `evidence_json` is re-read under the lock and must equal the snapshot
//!   the matching ran against — any concurrent mutation skips that entry
//!   (swap-race rule; re-run to converge);
//! - no breaker interaction: nothing is deleted, so the shared bulk-deletion
//!   budget is untouched.
//!
//! Idempotent: a second pass on a converged brain finds no dead hashes and
//! returns a default report.

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;

use crate::db::chunk_hash::compute_chunk_hash;
use crate::db::commit::evidence_has_live_chunk;

/// Outcome of one `re_anchor` pass.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReanchorReport {
    /// Live flagged evidence rows examined.
    pub entries_examined: usize,
    /// Already-grounded flagged rows (all hashes live) — flag cleared.
    pub entries_already_anchored: usize,
    /// Dead evidence items (hash misses AND no chunk_id fallback) inspected.
    pub items_dead: usize,
    /// Dead items re-anchored to a unique current chunk by quote match.
    pub items_reanchored: usize,
    /// Dead items whose quote matched 2+ distinct chunks — left flagged.
    pub items_ambiguous: usize,
    /// Dead items whose quote matched no current chunk — left flagged.
    pub items_unresolved: usize,
    /// Dead items whose unique match failed the recomputed-hash check.
    pub items_hash_mismatch: usize,
    /// Entries that went from ungrounded to grounded — flag cleared.
    pub entries_regrounded: usize,
    /// Entries that remain ungrounded after the pass.
    pub entries_still_unanchored: usize,
    /// Entries skipped because the row mutated between matching and apply.
    pub entries_race_skipped: usize,
}

/// Match (and optionally apply) quote-text re-anchors for the live unanchored
/// stock. `apply = false` is the dry-run: identical matching, zero writes.
///
/// See the module docs for the full contract. Idempotent.
pub fn re_anchor(conn: &Connection, apply: bool) -> Result<ReanchorReport> {
    let mut report = ReanchorReport::default();

    // 1. ALL live evidence rows — the doom condition is a hash that resolves
    //    to no chunk, not the writer-era `unanchored` flag (the live brain's
    //    flag stock is empty; gating on it would scan nothing). Soft-deleted
    //    stock is prune's business and is excluded by the join.
    let mut stmt = conn.prepare(
        "SELECT le.entry_id, le.evidence_json
           FROM librarian_evidence le
           JOIN llm_wiki_entries e ON e.id = le.entry_id
          WHERE e.deleted_at IS NULL",
    )?;
    let flagged: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    report.entries_examined = flagged.len();

    // 2. Match phase (read-only): per dead item, quote-match current chunks.
    //    Each candidate carries its matching-time snapshot json so the apply
    //    transaction can re-verify the row under the lock (swap-race rule).
    let mut candidates: Vec<(String, String, String, bool)> = Vec::new(); // (entry_id, snapshot_json, new_json, would_ground)
    for (entry_id, evidence_json) in flagged {
        let Ok(mut value) = serde_json::from_str::<Value>(&evidence_json) else {
            continue;
        };
        let Some(evidence) = value.get_mut("evidence").and_then(|v| v.as_array_mut()) else {
            continue;
        };
        // Stale flag: the shared grounding contract already finds a live
        // anchor for this row — clear the flag and move on.
        if evidence_has_live_chunk(conn, &evidence_json)? {
            report.entries_already_anchored += 1;
            if apply {
                conn.execute(
                    "UPDATE librarian_evidence SET unanchored = 0 WHERE entry_id = ?1",
                    [&entry_id],
                )?;
            }
            continue;
        }
        // Dead entry (the shared contract finds NO live anchor).

        let mut changed = false;
        for item in evidence.iter_mut() {
            let hash = item
                .get("content_hash")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty());
            let chunk_id = item.get("chunk_id").and_then(|v| v.as_i64());
            // Item is live if hash resolves, or (hash-less) chunk_id resolves.
            if let Some(h) = hash {
                let live: bool = conn
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM chunks WHERE content_hash = ?1)",
                        [h],
                        |r| r.get(0),
                    )?;
                if live {
                    continue;
                }
            } else if let Some(cid) = chunk_id {
                let live: bool = conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM chunks WHERE id = ?1)",
                    [cid],
                    |r| r.get(0),
                )?;
                if live {
                    continue;
                }
            } else {
                // No anchor of either kind: unresolvable item.
                report.items_unresolved += 1;
                continue;
            }

            report.items_dead += 1;
            let quote = item
                .get("quote")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .unwrap_or("");
            if quote.is_empty() {
                report.items_unresolved += 1;
                continue;
            }
            match match_quote(conn, quote)? {
                QuoteMatch::None => {
                    report.items_unresolved += 1;
                }
                QuoteMatch::Ambiguous => {
                    report.items_ambiguous += 1;
                }
                QuoteMatch::HashMismatch => {
                    report.items_hash_mismatch += 1;
                }
                QuoteMatch::One(chunk) => {
                    item["chunk_id"] = Value::from(chunk.chunk_id);
                    item["content_hash"] = Value::from(chunk.content_hash.clone());
                    item["start_line"] = Value::from(chunk.start_line);
                    item["end_line"] = Value::from(chunk.end_line);
                    report.items_reanchored += 1;
                    changed = true;
                }
            }
        }

        if !changed {
            report.entries_still_unanchored += 1;
            continue;
        }
        // Preserve the other top-level keys (proposal_id etc.). Grounding is
        // re-evaluated against the REWRITTEN json via the shared contract.
        let new_json = serde_json::to_string(&value)?;
        let would_ground = evidence_has_live_chunk(conn, &new_json)?;
        if would_ground {
            report.entries_regrounded += 1;
        } else {
            report.entries_still_unanchored += 1;
        }
        candidates.push((entry_id, evidence_json, new_json, would_ground));
    }

    if !apply || candidates.is_empty() {
        return Ok(report);
    }

    // 3. Apply phase: ONE IMMEDIATE transaction; each row is re-read under
    //    the lock and must equal the matching-time snapshot (swap-race rule).
    //    Rows that ground are cleared in the SAME write; ungrounded rows get
    //    the re-anchored hashes only (they stay flagged for the heal).
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    for (entry_id, snapshot_json, new_json, would_ground) in &candidates {
        let current: Option<String> = tx
            .query_row(
                "SELECT evidence_json FROM librarian_evidence WHERE entry_id = ?1",
                [entry_id],
                |r| r.get(0),
            )
            .optional()?;
        match current {
            Some(cur) if &cur == snapshot_json => {
                let unanchored = if *would_ground { 0 } else { 1 };
                tx.execute(
                    "UPDATE librarian_evidence
                        SET evidence_json = ?2, unanchored = ?3
                      WHERE entry_id = ?1",
                    rusqlite::params![entry_id, new_json, unanchored],
                )?;
            }
            _ => {
                report.entries_race_skipped += 1;
                if *would_ground {
                    report.entries_regrounded = report.entries_regrounded.saturating_sub(1);
                    report.entries_still_unanchored += 1;
                }
            }
        }
    }
    tx.commit()?;
    Ok(report)
}

/// One current chunk a quote matched.
#[derive(Debug, Clone)]
struct MatchedChunk {
    chunk_id: i64,
    content_hash: String,
    start_line: i64,
    end_line: i64,
}

enum QuoteMatch {
    None,
    Ambiguous,
    HashMismatch,
    One(MatchedChunk),
}

/// Find the current chunk(s) containing `quote` verbatim. Requires the
/// matched chunk's stored hash to equal the recomputed hash (chunker-drift
/// guard); 2+ DISTINCT chunk ids → [`QuoteMatch::Ambiguous`].
fn match_quote(conn: &Connection, quote: &str) -> Result<QuoteMatch> {
    let mut stmt = conn.prepare(
        "SELECT c.id, c.chunk_text, c.position, c.start_line, c.end_line,
                c.content_hash, d.path
           FROM chunks c
           JOIN documents d ON d.id = c.doc_id
          WHERE instr(c.chunk_text, ?1) > 0",
    )?;
    let rows = stmt
        .query_map([quote], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut distinct: Vec<MatchedChunk> = Vec::new();
    let mut distinct_textual_ids: Vec<i64> = Vec::new();
    let mut drift: Vec<i64> = Vec::new();
    for (id, text, position, start_line, end_line, stored_hash, path) in rows {
        let recomputed = compute_chunk_hash(&text, &path, position as usize);
        let recomputed_matches = recomputed == stored_hash;
        if !distinct_textual_ids.contains(&id) {
            distinct_textual_ids.push(id);
            if !recomputed_matches {
                drift.push(id);
            }
        }
        if recomputed_matches && !distinct.iter().any(|c| c.chunk_id == id) {
            distinct.push(MatchedChunk {
                chunk_id: id,
                content_hash: stored_hash,
                start_line,
                end_line,
            });
        }
    }
    // Chunker drift on EVERY textual match — never trust any as an anchor.
    if distinct.is_empty() && !drift.is_empty() {
        return Ok(QuoteMatch::HashMismatch);
    }
    match distinct.len() {
        0 => Ok(QuoteMatch::None),
        1 => Ok(QuoteMatch::One(distinct.pop().unwrap())),
        _ => Ok(QuoteMatch::Ambiguous),
    }
}

// ---------------------------------------------------------------------------
// Tests — in-memory SQLite ONLY (never ~/.brain/brain.db or the live vault).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::chunk_hash::compute_chunk_hash;
    use crate::db::connection::open_in_memory;
    use rusqlite::params;

    const DOC_PATH: &str = "/v/notes.md";

    /// Seed one document + one chunk whose content_hash is computed with the
    /// REAL `compute_chunk_hash` (text || path || position), matching the
    /// stored corpus contract.
    fn seed_chunk(conn: &Connection, text: &str, position: usize, stored_hash: &str) -> i64 {
        // documents.path is UNIQUE — reuse the doc across chunks.
        conn.execute(
            "INSERT OR IGNORE INTO documents (path, hash, tier, status)
             VALUES (?1, 'h', 'user_doc', 'indexed')",
            [DOC_PATH],
        )
        .unwrap();
        let doc_id: i64 = conn
            .query_row("SELECT id FROM documents WHERE path = ?1", [DOC_PATH], |r| r.get(0))
            .unwrap();
        conn.execute(
            "INSERT INTO chunks (doc_id, chunk_text, position, start_line, end_line,
                                 entity_id, content_hash)
             VALUES (?1, ?2, ?3, 1, 99, 'ent', ?4)",
            params![doc_id, text, position as i64, stored_hash],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    /// Seed one live librarian fact + evidence row with the given evidence
    /// json (its `proposal_id` key must be present).
    fn seed_row(
        conn: &Connection,
        entry_id: &str,
        evidence_json: &str,
        deleted_at: Option<i64>,
        unanchored: i64,
    ) {
        conn.execute(
            "INSERT INTO llm_wiki_entries (id, entity_id, title, body, tags, confidence,
                 source_type, source_ref, created_at, updated_at, deleted_at)
             VALUES (?1, 'ent', 't', 'b', '[]', 'inferred', 'librarian_inferred',
                     NULL, 1, 1, ?2)",
            params![entry_id, deleted_at],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO librarian_evidence (entry_id, proposal_id, evidence_json,
                 unanchored, created_at)
             VALUES (?1, 'prop_x', ?2, ?3, 1)",
            params![entry_id, evidence_json, unanchored],
        )
        .unwrap();
    }

    /// Convenience wrapper: flagged row (writer-era `unanchored = 1`).
    fn seed_flagged(conn: &Connection, entry_id: &str, evidence_json: &str, deleted_at: Option<i64>) {
        seed_row(conn, entry_id, evidence_json, deleted_at, 1);
    }

    fn dead_item(dead_hash: &str, quote: &str) -> String {
        format!(
            r#"{{"chunk_id":999,"content_hash":"{dead_hash}","start_line":10,"end_line":20,"quote":"{quote}"}}"#
        )
    }

    fn stored_json(conn: &Connection, entry_id: &str) -> String {
        conn.query_row(
            "SELECT evidence_json FROM librarian_evidence WHERE entry_id = ?1",
            [entry_id],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn stored_unanchored(conn: &Connection, entry_id: &str) -> i64 {
        conn.query_row(
            "SELECT unanchored FROM librarian_evidence WHERE entry_id = ?1",
            [entry_id],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn reanchors_dead_hash_to_unique_quote_match_and_clears_flag() {
        let conn = open_in_memory().unwrap();
        let new_text = "intro line\nthe decisive quote here\ntrailing text";
        let pos = 2usize;
        let live_hash = compute_chunk_hash(new_text, DOC_PATH, pos);
        let chunk_id = seed_chunk(&conn, new_text, pos, &live_hash);
        seed_flagged(
            &conn,
            "fact_recoverable",
            &format!(
                r#"{{"evidence":[{}],"proposal_id":"prop_a"}}"#,
                dead_item("dead0000000000000000000000000000000000000000000000000000000000", "the decisive quote here")
            ),
            None,
        );

        let report = re_anchor(&conn, true).unwrap();
        assert_eq!(report.entries_examined, 1);
        assert_eq!(report.items_dead, 1);
        assert_eq!(report.items_reanchored, 1);
        assert_eq!(report.entries_regrounded, 1);
        assert_eq!(report.entries_still_unanchored, 0);

        // The row now carries the LIVE hash + chunk_id + chunk line bounds.
        let json = stored_json(&conn, "fact_recoverable");
        let v: Value = serde_json::from_str(&json).unwrap();
        let item = &v["evidence"][0];
        assert_eq!(item["content_hash"], Value::from(live_hash.as_str()));
        assert_eq!(item["chunk_id"], Value::from(chunk_id));
        assert_eq!(item["start_line"], Value::from(1));
        assert_eq!(item["end_line"], Value::from(99));
        // proposal_id survived the rewrite.
        assert_eq!(item["quote"], Value::from("the decisive quote here"));
        assert_eq!(v["proposal_id"], Value::from("prop_a"));
        assert_eq!(stored_unanchored(&conn, "fact_recoverable"), 0);
        // The shared grounding contract agrees.
        assert!(evidence_has_live_chunk(&conn, &json).unwrap());

        // Idempotent: a converged brain is a no-op (the row is re-examined —
        // the scan covers ALL live rows — but grounds, so nothing is rewritten).
        let second = re_anchor(&conn, true).unwrap();
        assert_eq!(second.entries_examined, 1);
        assert_eq!(second.entries_already_anchored, 1);
        assert_eq!(second.items_reanchored, 0);
    }

    #[test]
    fn unflagged_row_with_dead_hash_is_still_reanchored() {
        // The live brain's stock: `unanchored = 0` everywhere, doom carried
        // purely by dead hashes. The scan must NOT gate on the flag.
        let conn = open_in_memory().unwrap();
        let new_text = "plain body holds the target quote inside";
        let live_hash = compute_chunk_hash(new_text, DOC_PATH, 0);
        seed_chunk(&conn, new_text, 0, &live_hash);
        seed_row(
            &conn,
            "fact_unflagged",
            &format!(
                r#"{{"evidence":[{}],"proposal_id":"prop_i"}}"#,
                dead_item("dead8888888888888888888888888888888888888888888888888888888888", "holds the target quote")
            ),
            None,
            0, // NOT flagged — the live-brain condition
        );

        let report = re_anchor(&conn, true).unwrap();
        assert_eq!(report.entries_examined, 1);
        assert_eq!(report.items_dead, 1);
        assert_eq!(report.items_reanchored, 1);
        assert_eq!(report.entries_regrounded, 1);
        let json = stored_json(&conn, "fact_unflagged");
        assert!(json.contains(&live_hash));
        assert!(evidence_has_live_chunk(&conn, &json).unwrap());
    }

    #[test]
    fn dry_run_matches_but_writes_nothing() {
        let conn = open_in_memory().unwrap();
        let new_text = "body with the sought quote inside";
        let live_hash = compute_chunk_hash(new_text, DOC_PATH, 0);
        seed_chunk(&conn, new_text, 0, &live_hash);
        seed_flagged(
            &conn,
            "fact_dry",
            &format!(
                r#"{{"evidence":[{}],"proposal_id":"prop_dry"}}"#,
                dead_item("dead1111111111111111111111111111111111111111111111111111111111", "the sought quote")
            ),
            None,
        );

        let report = re_anchor(&conn, false).unwrap();
        assert_eq!(report.items_reanchored, 1);
        assert_eq!(report.entries_regrounded, 1);

        // NOTHING changed on disk: json byte-identical, flag still set.
        let json = stored_json(&conn, "fact_dry");
        assert!(json.contains("dead1111111111111111111111111111111111111111111111111111111111"));
        assert_eq!(stored_unanchored(&conn, "fact_dry"), 1);
    }

    #[test]
    fn ambiguous_quote_is_never_guessed() {
        let conn = open_in_memory().unwrap();
        let text = "shared phrasing appears";
        let h1 = compute_chunk_hash(text, DOC_PATH, 0);
        let h2 = compute_chunk_hash(text, DOC_PATH, 1); // distinct chunk, same text
        seed_chunk(&conn, text, 0, &h1);
        seed_chunk(&conn, text, 1, &h2);
        seed_flagged(
            &conn,
            "fact_amb",
            &format!(
                r#"{{"evidence":[{}],"proposal_id":"prop_b"}}"#,
                dead_item("dead2222222222222222222222222222222222222222222222222222222222", "shared phrasing")
            ),
            None,
        );

        let report = re_anchor(&conn, true).unwrap();
        assert_eq!(report.items_ambiguous, 1);
        assert_eq!(report.items_reanchored, 0);
        assert_eq!(report.entries_still_unanchored, 1);
        assert_eq!(stored_unanchored(&conn, "fact_amb"), 1);
        assert!(stored_json(&conn, "fact_amb").contains("dead2222222222222222222222222222222222222222222222222222222222"));
    }

    #[test]
    fn chunker_drift_chunk_is_never_used_as_anchor() {
        let conn = open_in_memory().unwrap();
        // Chunk's STORED hash disagrees with the recomputed hash — drift.
        seed_chunk(&conn, "drifted body has the quote", 0, "f".repeat(64).as_str());
        seed_flagged(
            &conn,
            "fact_drift",
            &format!(
                r#"{{"evidence":[{}],"proposal_id":"prop_c"}}"#,
                dead_item("dead3333333333333333333333333333333333333333333333333333333333", "has the quote")
            ),
            None,
        );

        let report = re_anchor(&conn, true).unwrap();
        assert_eq!(report.items_hash_mismatch, 1);
        assert_eq!(report.items_reanchored, 0);
        assert_eq!(stored_unanchored(&conn, "fact_drift"), 1);
    }

    #[test]
    fn unresolved_quote_and_soft_deleted_stock_are_untouched() {
        let conn = open_in_memory().unwrap();
        seed_chunk(&conn, "unrelated chunk", 0, &compute_chunk_hash("unrelated chunk", DOC_PATH, 0));
        seed_flagged(
            &conn,
            "fact_lost",
            &format!(
                r#"{{"evidence":[{}],"proposal_id":"prop_d"}}"#,
                dead_item("dead4444444444444444444444444444444444444444444444444444444444", "quote that no longer exists anywhere")
            ),
            None,
        );
        seed_flagged(
            &conn,
            "fact_soft",
            &format!(
                r#"{{"evidence":[{}],"proposal_id":"prop_e"}}"#,
                dead_item("dead5555555555555555555555555555555555555555555555555555555555", "unrelated chunk")
            ),
            Some(90), // soft-deleted: prune's business, never re-anchored
        );

        let report = re_anchor(&conn, true).unwrap();
        assert_eq!(report.entries_examined, 1); // soft-deleted row skipped
        assert_eq!(report.items_unresolved, 1);
        assert_eq!(report.items_reanchored, 0);
        // The soft-deleted row's quote DID match, but must be untouched.
        assert_eq!(stored_unanchored(&conn, "fact_soft"), 1);
        assert!(stored_json(&conn, "fact_soft").contains("dead5555555555555555555555555555555555555555555555555555555555"));
    }

    #[test]
    fn partially_reanchored_row_that_still_does_not_ground_stays_flagged() {
        let conn = open_in_memory().unwrap();
        let live_hash = compute_chunk_hash("recovered chunk text", DOC_PATH, 0);
        seed_chunk(&conn, "recovered chunk text", 0, &live_hash);
        // Item 0 recoverable, item 1 hopeless → row grounds on neither's
        // original anchors; after rewrite item 0 is live → contract satisfied.
        seed_flagged(
            &conn,
            "fact_mixed",
            &format!(
                r#"{{"evidence":[{},{}],"proposal_id":"prop_f"}}"#,
                dead_item("dead6666666666666666666666666666666666666666666666666666666666", "recovered chunk text"),
                dead_item("dead7777777777777777777777777777777777777777777777777777777777", "nowhere to be found")
            ),
            None,
        );

        let report = re_anchor(&conn, true).unwrap();
        assert_eq!(report.items_reanchored, 1);
        assert_eq!(report.items_unresolved, 1);
        // Contract satisfied via the re-anchored item → flag cleared.
        assert_eq!(report.entries_regrounded, 1);
        assert_eq!(stored_unanchored(&conn, "fact_mixed"), 0);
        assert!(stored_json(&conn, "fact_mixed").contains(&live_hash));
    }

    #[test]
    fn stale_flag_on_already_grounded_row_is_cleared() {
        let conn = open_in_memory().unwrap();
        let live_hash = compute_chunk_hash("still alive", DOC_PATH, 0);
        seed_chunk(&conn, "still alive", 0, &live_hash);
        seed_flagged(
            &conn,
            "fact_stale",
            &format!(
                r#"{{"evidence":[{{"chunk_id":1,"content_hash":"{live_hash}","start_line":1,"end_line":2,"quote":"still alive"}}],"proposal_id":"prop_g"}}"#
            ),
            None,
        );

        let report = re_anchor(&conn, true).unwrap();
        assert_eq!(report.entries_already_anchored, 1);
        assert_eq!(report.items_dead, 0);
        assert_eq!(stored_unanchored(&conn, "fact_stale"), 0);
    }

    #[test]
    fn hashless_item_with_live_chunk_id_is_left_alone() {
        let conn = open_in_memory().unwrap();
        let chunk_id = seed_chunk(&conn, "id fallback chunk", 0, &compute_chunk_hash("id fallback chunk", DOC_PATH, 0));
        seed_flagged(
            &conn,
            "fact_hashless",
            r#"{"evidence":[{"chunk_id":PLACEHOLDER,"content_hash":"","start_line":1,"end_line":2,"quote":"id fallback chunk"}],"proposal_id":"prop_h"}"#
                .replace("PLACEHOLDER", &chunk_id.to_string())
                .as_str(),
            None,
        );

        let report = re_anchor(&conn, true).unwrap();
        assert_eq!(report.items_dead, 0);
        assert_eq!(report.entries_already_anchored, 1);
        assert_eq!(stored_unanchored(&conn, "fact_hashless"), 0);
    }
}

