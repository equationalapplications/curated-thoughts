//! Quote-text re-anchoring for dead evidence hashes (`ct evidence re-anchor`).
//!
//! Root cause this repairs: `compute_chunk_hash` binds an anchor to chunk
//! text + doc path + enumeration position, so ANY edit that shifts chunk
//! boundaries orphans a previously-live hash even when the quoted text still
//! survives verbatim inside a current chunk. The nightly heal would then
//! soft-delete the entry — the correct last resort, but wasteful when the
//! quote is recoverable.
//!
//! For every dead `content_hash` in a live evidence row, this pass matches
//! the item's `quote` against the CURRENT chunks (spec §3, adjudicated
//! 2026-11-10 #2: NORMALIZED PREFIX matching, not verbatim — the librarian
//! quotes the ENTIRE old chunk, median ~1.2k chars, and after a re-split no
//! current chunk contains the full quote, so verbatim scores 0 BY
//! CONSTRUCTION; measured fallback: a normalized 60-char prefix matches 32/40
//! sampled doomed entries). Both sides are normalized (lowercase, whitespace/
//! punctuation runs collapsed to single spaces), then:
//!
//! - the quote's ≥60-char normalized PREFIX matches exactly ONE chunk →
//!   re-anchor: rewrite the item's `content_hash` (recomputed via
//!   [`compute_chunk_hash`] from the matched chunk — never trusted from the
//!   row), `chunk_id`, `start_line`, `end_line`;
//! - several chunks share the prefix → the quote's last-60-chars normalized
//!   SUFFIX narrows them (prefix+suffix agreement is the strong case); still
//!   several (or the suffix matches none of them) → ambiguous (left flagged;
//!   guessing would fabricate provenance);
//! - zero chunks → unresolved (left flagged; the heal owns it);
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
//! Idempotent: a second pass on a converged brain re-examines every live
//! row, reports them as already anchored, and rewrites nothing.

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
    /// Path of the pre-write `VACUUM INTO` snapshot taken on the apply path
    /// (spec §4: before the FIRST write, flag clears included). `None` on
    /// dry runs and in-memory tests.
    pub backup_path: Option<String>,
}

/// Match (and optionally apply) quote-text re-anchors for the live unanchored
/// stock. `apply = false` is the dry-run: identical matching, zero writes.
///
/// See the module docs for the full contract. Idempotent.
pub fn re_anchor(
    conn: &Connection,
    apply: bool,
    db_path: Option<&std::path::Path>,
) -> Result<ReanchorReport> {
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
    //    The normalized-text chunk index is built once for the whole pass.
    let chunk_index = load_chunk_index(conn)?;
    let mut candidates: Vec<(String, String, String, bool)> = Vec::new(); // (entry_id, snapshot_json, new_json, would_ground)
    // Grounded rows carrying a stale `unanchored` flag: cleared inside the
    // apply transaction behind the SAME snapshot compare as hash rewrites
    // (review: flag writes must not bypass the swap-race rule).
    let mut flag_clears: Vec<(String, String)> = Vec::new();
    for (entry_id, evidence_json) in flagged {
        let Ok(mut value) = serde_json::from_str::<Value>(&evidence_json) else {
            continue;
        };
        let Some(evidence) = value.get_mut("evidence").and_then(|v| v.as_array_mut()) else {
            continue;
        };
        // The shared grounding contract only needs ONE live item — so a
        // grounded entry can still carry dead items whose quote survives.
        // Those are repaired TOO (spec §3: recovery is per ITEM; a live
        // sibling must not orphan recoverable provenance), and only a fully
        // resolved entry has its `unanchored` flag semantics touched.
        let grounded = evidence_has_live_chunk(conn, &evidence_json)?;

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
            match match_quote_prefix(conn, &chunk_index, quote)? {
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

        // Preserve the other top-level keys (proposal_id etc.). Grounding is
        // re-evaluated against the REWRITTEN json via the shared contract.
        let new_json = serde_json::to_string(&value)?;
        let would_ground = if changed {
            evidence_has_live_chunk(conn, &new_json)?
        } else {
            grounded
        };
        if !changed {
            if grounded {
                report.entries_already_anchored += 1;
                flag_clears.push((entry_id, evidence_json));
            } else {
                report.entries_still_unanchored += 1;
            }
            continue;
        }
        if grounded {
            // Already grounded; this pass only improved item provenance.
            report.entries_already_anchored += 1;
        } else if would_ground {
            report.entries_regrounded += 1;
        } else {
            report.entries_still_unanchored += 1;
        }
        candidates.push((entry_id, evidence_json, new_json, would_ground));
    }

    if !apply {
        return Ok(report);
    }
    let has_writes = !candidates.is_empty() || !flag_clears.is_empty();
    if !has_writes {
        // Converged brain: nothing to write, nothing to snapshot.
        return Ok(report);
    }

    // 3. Apply phase: §4 hard gate FIRST — the crash-consistent snapshot
    //    lands before the FIRST write, flag clears included; a backup failure
    //    aborts with zero rows touched.
    if let Some(p) = db_path {
        report.backup_path = Some(
            backup_before_apply(conn, p)?
                .to_string_lossy()
                .into_owned(),
        );
    }
    //    ONE IMMEDIATE transaction; each target row is re-read under the lock
    //    and must equal the matching-time snapshot (swap-race rule).
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    for (entry_id, snapshot_json) in &flag_clears {
        let current: Option<String> = tx
            .query_row(
                "SELECT evidence_json FROM librarian_evidence WHERE entry_id = ?1",
                [entry_id],
                |r| r.get(0),
            )
            .optional()?;
        if current.as_deref() == Some(snapshot_json.as_str()) {
            tx.execute(
                "UPDATE librarian_evidence SET unanchored = 0 WHERE entry_id = ?1",
                [entry_id],
            )?;
        } else {
            report.entries_race_skipped += 1;
        }
    }
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

/// Crash-consistent pre-write snapshot (spec §4, adjudicated 2026-11-10:
/// backup KEPT even though the pass is UPDATE-only — a mass brain write takes
/// a `VACUUM INTO` snapshot first). Called on the apply path only, BEFORE the
/// first UPDATE; a failure here aborts the apply with zero rows touched.
/// Returns the backup path.
pub fn backup_before_apply(
    conn: &Connection,
    db_path: &std::path::Path,
) -> Result<std::path::PathBuf> {
    let dir = db_path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let stem = db_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("brain.db");
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let backup = dir.join(format!("{stem}.bak-pre-reanchor-{ts}"));
    conn.execute("VACUUM INTO ?1", [backup.to_string_lossy().as_ref()])?;
    Ok(backup)
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

/// Normalized text for quote matching: lowercase, every non-alphanumeric run
/// collapsed to a single space, trimmed. Line wrapping and punctuation
/// spacing change across re-chunks; the normalized form is the stable surface.
fn normalize_for_match(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pending_space = false;
    for ch in s.chars() {
        if ch.is_alphanumeric() {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.extend(ch.to_lowercase());
        } else {
            pending_space = true;
        }
    }
    out
}

/// Minimum normalized-prefix length for a quote match (handoff 2026-11-10:
/// measured 32/40 recovery on sampled doomed entries at this length).
const MATCH_PREFIX_CHARS: usize = 60;
/// Suffix length for the prefix+suffix agreement strong case.
const MATCH_SUFFIX_CHARS: usize = 60;

fn normalized_prefix(qn: &str) -> String {
    qn.chars().take(MATCH_PREFIX_CHARS).collect()
}

/// `None` when the normalized quote is too short for a suffix to add signal.
fn normalized_suffix(qn: &str) -> Option<String> {
    let count = qn.chars().count();
    if count > MATCH_PREFIX_CHARS + MATCH_SUFFIX_CHARS {
        Some(qn.chars().skip(count - MATCH_SUFFIX_CHARS).collect())
    } else {
        None
    }
}

/// In-memory index of the current chunks, built ONCE per pass (the matcher is
/// normalized-text search — not expressible as a SQL `instr` against raw
/// text), so each dead item consults memory, not the DB.
struct ChunkIndex {
    rows: Vec<ChunkIndexRow>,
}

struct ChunkIndexRow {
    id: i64,
    stored_hash: String,
    norm: String,
}

fn load_chunk_index(conn: &Connection) -> Result<ChunkIndex> {
    let mut stmt = conn.prepare(
        "SELECT c.id, c.chunk_text, c.content_hash
           FROM chunks c
           JOIN documents d ON d.id = c.doc_id",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(ChunkIndex {
        rows: rows
            .into_iter()
            .map(|(id, text, stored_hash)| ChunkIndexRow {
                id,
                stored_hash,
                norm: normalize_for_match(&text),
            })
            .collect(),
    })
}

/// Raw chunk text + path + position for the drift guard — fetched only for
/// the few textual candidates of one item, not the whole index.
fn chunk_hash_inputs(
    conn: &Connection,
    id: i64,
) -> Result<Option<(String, i64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT c.chunk_text, c.position, d.path
           FROM chunks c
           JOIN documents d ON d.id = c.doc_id
          WHERE c.id = ?1",
    )?;
    stmt.query_row([id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?))
    })
    .optional()
    .map_err(Into::into)
}

/// Match `quote` against the current chunks in two tiers (spec §3, both Kurt
/// adjudications harmonized): the FULL normalized quote contained in exactly
/// one chunk anchors at any length (round-1 ruling: verbatim containment of
/// the whole quote is real provenance, never a guess); otherwise a ≥60-char
/// normalized PREFIX, narrowed by the normalized SUFFIX when several chunks
/// share it. Uniqueness counts ALL textual candidates BEFORE the drift gate —
/// a drifted sibling makes a match ambiguous, never silently anchorable. The
/// surviving candidate's stored hash must equal the recomputed hash
/// (chunker-drift guard); the anchor carries the RECOMPUTED hash, never the
/// row's stored value.
fn match_quote_prefix(
    conn: &Connection,
    index: &ChunkIndex,
    quote: &str,
) -> Result<QuoteMatch> {
    let qn = normalize_for_match(quote);
    if qn.is_empty() {
        return Ok(QuoteMatch::None);
    }
    let prefix = normalized_prefix(&qn);
    let suffix = normalized_suffix(&qn);

    // Textual candidates by normalized prefix (distinct chunk ids).
    let mut candidates: Vec<&ChunkIndexRow> = Vec::new();
    for row in &index.rows {
        if row.norm.contains(&prefix) && !candidates.iter().any(|c| c.id == row.id) {
            candidates.push(row);
        }
    }
    if candidates.is_empty() {
        return Ok(QuoteMatch::None);
    }

    // Uniqueness FIRST (drifted siblings count), then the drift gate on the
    // single survivor only.
    let survivor: &ChunkIndexRow = if candidates.len() == 1 {
        candidates[0]
    } else {
        match &suffix {
            Some(suffix) => {
                let narrowed: Vec<&ChunkIndexRow> = candidates
                    .iter()
                    .copied()
                    .filter(|r| r.norm.contains(suffix))
                    .collect();
                if narrowed.len() == 1 {
                    narrowed[0]
                } else {
                    return Ok(QuoteMatch::Ambiguous);
                }
            }
            None => return Ok(QuoteMatch::Ambiguous),
        }
    };
    let Some((text, position, path)) = chunk_hash_inputs(conn, survivor.id)? else {
        return Ok(QuoteMatch::None);
    };
    let recomputed = compute_chunk_hash(&text, &path, position as usize);
    if recomputed != survivor.stored_hash {
        return Ok(QuoteMatch::HashMismatch);
    }
    let mut stmt = conn.prepare("SELECT start_line, end_line FROM chunks WHERE id = ?1")?;
    let (start_line, end_line) = stmt.query_row([survivor.id], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
    })?;
    Ok(QuoteMatch::One(MatchedChunk {
        chunk_id: survivor.id,
        content_hash: recomputed,
        start_line,
        end_line,
    }))
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

        let report = re_anchor(&conn, true, None).unwrap();
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
        let second = re_anchor(&conn, true, None).unwrap();
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

        let report = re_anchor(&conn, true, None).unwrap();
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

        let report = re_anchor(&conn, false, None).unwrap();
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

        let report = re_anchor(&conn, true, None).unwrap();
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

        let report = re_anchor(&conn, true, None).unwrap();
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

        let report = re_anchor(&conn, true, None).unwrap();
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

        let report = re_anchor(&conn, true, None).unwrap();
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

        let report = re_anchor(&conn, true, None).unwrap();
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

        let report = re_anchor(&conn, true, None).unwrap();
        assert_eq!(report.items_dead, 0);
        assert_eq!(report.entries_already_anchored, 1);
        assert_eq!(stored_unanchored(&conn, "fact_hashless"), 0);
    }

    /// Spec §4 (adjudicated: backup KEPT): the apply path snapshots FIRST.
    /// A file-backed db + mutation-after-snapshot proves the backup carries
    /// the pre-write state, not just "a file appeared".
    #[test]
    fn backup_before_apply_snapshots_pre_state() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("brain.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute("CREATE TABLE t(x INTEGER)", []).unwrap();
        conn.execute("INSERT INTO t VALUES (7)", []).unwrap();

        let backup = backup_before_apply(&conn, &db_path).unwrap();
        assert!(backup.exists(), "backup file must exist");
        assert!(backup.starts_with(dir.path()), "backup lands beside the db");
        assert!(
            backup
                .file_name()
                .unwrap()
                .to_string_lossy()
                .contains("bak-pre-reanchor"),
            "backup name follows brain.db.bak-pre-reanchor-<ts>"
        );

        // Mutate AFTER the snapshot; the backup must hold the pre-state.
        conn.execute("INSERT INTO t VALUES (8)", []).unwrap();
        let snap = Connection::open_with_flags(
            &backup,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let n: i64 = snap
            .query_row("SELECT count(*) FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "backup must carry the pre-mutation state");
    }

    /// The live failure (handoff 2026-11-10): the librarian quotes the ENTIRE
    /// OLD chunk (median ~1.2k chars); after a re-split no current chunk
    /// contains the full quote, so a verbatim matcher scores 0 BY
    /// CONSTRUCTION. The normalized 60-char prefix still matches.
    #[test]
    fn reanchors_long_quote_via_normalized_prefix_after_resplit() {
        let conn = open_in_memory().unwrap();
        let mut quote = String::new();
        for i in 0..40 {
            quote.push_str(&format!("Word number {} in the long, decisive quote text. ", i));
        }
        // Current chunk: same words, different case/punctuation/line
        // wrapping, truncated well before the quote's end — verbatim cannot
        // match by construction.
        let mut chunk_text = String::new();
        for i in 0..12 {
            chunk_text.push_str(&format!("WORD  NUMBER {} IN THE long decisive\nquote text,\n", i));
        }
        let live_hash = compute_chunk_hash(&chunk_text, DOC_PATH, 3usize);
        seed_chunk(&conn, &chunk_text, 3, &live_hash);
        seed_flagged(
            &conn,
            "fact_resplit",
            &format!(
                r#"{{"evidence":[{}],"proposal_id":"prop_resplit"}}"#,
                dead_item("dead0000000000000000000000000000000000000000000000000000000000", &quote)
            ),
            None,
        );

        let report = re_anchor(&conn, true, None).unwrap();
        assert_eq!(report.items_dead, 1);
        assert_eq!(
            report.items_reanchored, 1,
            "normalized prefix must recover the resplit quote"
        );
        assert_eq!(report.entries_regrounded, 1);
        assert_eq!(report.items_unresolved, 0);
        // Anchored to the LIVE chunk hash (recomputed), never the dead one.
        assert!(stored_json(&conn, "fact_resplit").contains(&live_hash));
    }

    /// Two current chunks share the normalized prefix and the quote carries
    /// nothing that separates them — refuse and keep the flag (never guess).
    #[test]
    fn ambiguous_when_prefix_matches_multiple_chunks() {
        let conn = open_in_memory().unwrap();
        let mut shared = String::new();
        for i in 0..40 {
            shared.push_str(&format!("word number {} of a shared preamble line. ", i));
        }
        let a = format!("{}alpha ending only here", shared);
        let b = format!("{}beta ending only here", shared);
        let h1 = compute_chunk_hash(&a, DOC_PATH, 1usize);
        let h2 = compute_chunk_hash(&b, DOC_PATH, 2usize);
        seed_chunk(&conn, &a, 1, &h1);
        seed_chunk(&conn, &b, 2, &h2);
        seed_flagged(
            &conn,
            "fact_ambig",
            &format!(
                r#"{{"evidence":[{}],"proposal_id":"prop_ambig"}}"#,
                dead_item("dead1000000000000000000000000000000000000000000000000000000000", &shared)
            ),
            None,
        );

        let report = re_anchor(&conn, true, None).unwrap();
        assert_eq!(
            report.items_ambiguous, 1,
            "shared prefix across 2 chunks must refuse, never guess"
        );
        assert_eq!(report.items_reanchored, 0);
        assert_eq!(stored_unanchored(&conn, "fact_ambig"), 1);
    }

    /// Same shared-prefix setup, but the quote also carries the alpha ending:
    /// exactly one chunk agrees on prefix AND suffix — the strong case anchors.
    #[test]
    fn suffix_agreement_disambiguates_shared_prefix() {
        let conn = open_in_memory().unwrap();
        let mut shared = String::new();
        for i in 0..40 {
            shared.push_str(&format!("word number {} of a shared preamble line. ", i));
        }
        let a = format!("{}alpha ending only here", shared);
        let b = format!("{}beta ending only here", shared);
        let h1 = compute_chunk_hash(&a, DOC_PATH, 1usize);
        let h2 = compute_chunk_hash(&b, DOC_PATH, 2usize);
        seed_chunk(&conn, &a, 1, &h1);
        seed_chunk(&conn, &b, 2, &h2);
        let quote = format!("{}alpha ending only here", shared);
        seed_flagged(
            &conn,
            "fact_suffix",
            &format!(
                r#"{{"evidence":[{}],"proposal_id":"prop_suffix"}}"#,
                dead_item("dead2000000000000000000000000000000000000000000000000000000000", &quote)
            ),
            None,
        );

        let report = re_anchor(&conn, true, None).unwrap();
        assert_eq!(
            report.items_reanchored, 1,
            "prefix+suffix agreement is the strong case"
        );
        assert_eq!(report.items_ambiguous, 0);
        assert!(stored_json(&conn, "fact_suffix").contains(&h1));
    }

    /// Uniqueness counts drifted siblings: two chunks share the prefix (and
    /// the suffix matches both), one with a stale stored hash — the tie is
    /// ambiguous, NEVER anchored to the one valid chunk (review haiku/high).
    #[test]
    fn drifted_sibling_makes_shared_prefix_ambiguous() {
        let conn = open_in_memory().unwrap();
        let mut shared = String::new();
        for i in 0..40 {
            shared.push_str(&format!("word number {} of a shared preamble line. ", i));
        }
        let a = format!("{}alpha ending only here", shared);
        let b = format!("{}alpha ending only here", shared); // same ending!
        let h_good = compute_chunk_hash(&a, DOC_PATH, 1usize);
        seed_chunk(&conn, &a, 1, &h_good);
        // Chunk b stores a hash that does NOT match its text — drifted.
        seed_chunk(&conn, &b, 2, "beef0000000000000000000000000000000000000000000000000000000000");
        let quote = format!("{}alpha ending only here", shared);
        seed_flagged(
            &conn,
            "fact_drift_sibling",
            &format!(
                r#"{{"evidence":[{}],"proposal_id":"prop_ds"}}"#,
                dead_item("dead3000000000000000000000000000000000000000000000000000000000", &quote)
            ),
            None,
        );

        let report = re_anchor(&conn, true, None).unwrap();
        assert_eq!(
            report.items_ambiguous, 1,
            "a drifted sibling must turn the tie ambiguous, not pick the valid one"
        );
        assert_eq!(report.items_reanchored, 0);
        assert_eq!(stored_unanchored(&conn, "fact_drift_sibling"), 1);
    }

    /// A grounded entry can still carry a dead sibling item (the grounding
    /// contract needs ONE live item). The dead item is repaired too — a live
    /// sibling must not orphan recoverable provenance (review haiku/medium).
    #[test]
    fn grounded_entry_dead_sibling_item_is_still_repaired() {
        let conn = open_in_memory().unwrap();
        let live_text = "intro line\nthe live sibling quote\ntrailing";
        let live_hash = compute_chunk_hash(live_text, DOC_PATH, 1usize);
        seed_chunk(&conn, live_text, 1, &live_hash);
        let rec_text = "recovery line\nthe recoverable sibling quote\nmore";
        let rec_hash = compute_chunk_hash(rec_text, DOC_PATH, 2usize);
        seed_chunk(&conn, rec_text, 2, &rec_hash);
        // item0: LIVE hash (grounds the entry); item1: dead but recoverable.
        seed_row(
            &conn,
            "fact_grounded_sibling",
            &format!(
                r#"{{"evidence":[{},{}],"proposal_id":"prop_gs"}}"#,
                dead_item(&live_hash, "the live sibling quote"),
                dead_item("dead4000000000000000000000000000000000000000000000000000000000", "the recoverable sibling quote")
            ),
            None,
            0,
        );

        let report = re_anchor(&conn, true, None).unwrap();
        assert_eq!(report.items_dead, 1);
        assert_eq!(report.items_reanchored, 1, "dead sibling of a live item is repaired");
        assert_eq!(report.entries_already_anchored, 1);
        assert!(stored_json(&conn, "fact_grounded_sibling").contains(&rec_hash));
    }

    /// §4/§6: the APPLY path snapshots before the first write (file-backed db
    /// proves the file lands next to the db); the dry-run path writes none.
    #[test]
    fn apply_snapshots_but_dry_run_writes_no_backup() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("brain.db");
        let conn = Connection::open(&db_path).unwrap();
        crate::db::connection::migrate_open_db(&conn, Some(dir.path())).unwrap();

        let live_text = "intro line\nthe recoverable quote here\ntrailing";
        let live_hash = compute_chunk_hash(live_text, DOC_PATH, 1usize);
        seed_chunk(&conn, live_text, 1, &live_hash);
        seed_flagged(
            &conn,
            "fact_backup",
            &format!(
                r#"{{"evidence":[{}],"proposal_id":"prop_bk"}}"#,
                dead_item("dead5000000000000000000000000000000000000000000000000000000000", "the recoverable quote here")
            ),
            None,
        );

        let backups = || -> Vec<_> {
            std::fs::read_dir(dir.path())
                .unwrap()
                .filter_map(|e| e.ok())
                .filter(|e| e.file_name().to_string_lossy().contains("bak-pre-reanchor"))
                .map(|e| e.file_name())
                .collect::<Vec<_>>()
        };
        assert!(backups().is_empty(), "no backup before any pass");

        let dry = re_anchor(&conn, false, Some(&db_path)).unwrap();
        assert_eq!(dry.items_reanchored, 1);
        assert!(dry.backup_path.is_none(), "dry-run takes no backup");
        assert!(backups().is_empty(), "dry-run must not write a backup");

        let apply = re_anchor(&conn, true, Some(&db_path)).unwrap();
        assert_eq!(apply.items_reanchored, 1);
        let b = apply.backup_path.expect("apply must snapshot first");
        assert!(dir.path().join(&b).exists(), "snapshot lands beside the db");
        assert_eq!(backups().len(), 1, "exactly one snapshot");
    }
}

