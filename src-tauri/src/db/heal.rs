//! Heal core (spec 2026-09-24 §5): soft-delete live `librarian_inferred`
//! wiki entries whose `source_ref` is demonstrably ungrounded, purge their
//! edges, and write `healed` events.
//!
//! Connection-only by design: the GUI heal scheduler and the headless
//! `ct heal` CLI must drive the exact same write path, so nothing here may
//! touch Tauri state, `AppDb`, or the vault config. `vault` rides along in
//! the signature for call-site parity (the existence check is purely
//! DB-driven via `source_ref_is_still_grounded`); it is intentionally
//! unused.
//!
//! Callers today: `heal_invalid_sources` (lib.rs, the scheduler thread) and
//! `ct heal` (tools/src/cmds.rs). The GUI "Heal Database" button
//! (`run_wiki_heal`) still calls `heal_lost_librarian_inferred`, NOT this
//! core — consolidating that button onto this path is deliberate follow-up
//! work, not an oversight (m8, Opus review of PR #228).

use std::path::PathBuf;

use anyhow::Result;
use rusqlite::Connection;
use serde::Serialize;

/// Per-run options for [`heal_invalid_sources_conn`].
#[derive(Debug, Clone, Copy, Default)]
pub struct HealOpts {
    /// Skip the bulk-deletion breaker for THIS one run (the pinned
    /// `--allow-bulk` escape, spec §10). Nothing else changes.
    pub allow_bulk: bool,
}

/// Counts from one heal pass over live `librarian_inferred` entries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct HealSummary {
    /// Live `librarian_inferred` rows examined.
    pub evaluated: usize,
    /// Rows soft-deleted (source_ref demonstrably ungrounded).
    pub soft_deleted: usize,
    /// Edges purged by the per-entry cascade (`purge_edges_for_entry`).
    pub edges_purged: usize,
}

/// Heal every live `librarian_inferred` entry on `conn`.
///
/// Selection and grounding policy are unchanged from the original
/// `heal_invalid_sources` (lib.rs, the original GUI-scheduler pass): only
/// rows with
/// `deleted_at IS NULL AND source_ref IS NOT NULL AND source_type =
/// 'librarian_inferred'` are evaluated; a row is soft-deleted only when
/// `source_ref_is_still_grounded` says the reference is *demonstrably*
/// stale (empty / unparseable / DB-error refs stay). Each soft-delete plus
/// its edge purge is atomic per row; `healed` events are written once per
/// affected entity after the loop.
///
/// Task 10 (spec §10): the pass is breaker-governed. The breaker check runs
/// once before the loop — `|doomed|` here is the count of selected rows that
/// are UNGROUNDED, computed per row inside the same IMMEDIATE transaction
/// that soft-deletes it; the pre-loop check pins the batch decision (a refusal
/// deletes NOTHING) while the spend increments ride the per-row transactions.
pub fn heal_invalid_sources_conn(conn: &mut Connection, _vault: PathBuf) -> Result<HealSummary> {
    heal_invalid_sources_conn_opts(conn, _vault, HealOpts::default())
}

/// [`heal_invalid_sources_conn`] with explicit [`HealOpts`] (the `ct heal`
/// `--allow-bulk` path).
pub fn heal_invalid_sources_conn_opts(
    conn: &mut Connection,
    _vault: PathBuf,
    opts: HealOpts,
) -> Result<HealSummary> {
    let entries: Vec<(i64, String, String, String)> = {
        let mut stmt = conn.prepare(
            "SELECT e.rowid, e.source_ref, e.entity_id, e.id
             FROM llm_wiki_entries e
             WHERE e.deleted_at IS NULL
               AND e.source_ref IS NOT NULL
               AND e.source_type = 'librarian_inferred'",
        )?;
        let mut rows = stmt.query([])?;
        let mut v = Vec::new();
        while let Some(row) = rows.next()? {
            v.push((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?));
        }
        v
    };

    // Task 10 (spec §10): ONE pre-loop breaker decision for the whole pass.
    // `|doomed|` is the count of selected rows whose source_ref is
    // demonstrably stale — the exact set the loop would soft-delete. On a
    // refusal the pass soft-deletes NOTHING (all-or-nothing) and spends
    // nothing; the stderr line names the threshold that was hit.
    if !opts.allow_bulk {
        let l = crate::db::breaker::heal_denominator(conn)?;
        let threshold = crate::db::breaker::heal_threshold(conn)?;
        let mut doomed = 0usize;
        for (_, source_ref, _, _) in &entries {
            if !crate::db::commit::source_ref_is_still_grounded(conn, source_ref) {
                doomed += 1;
            }
        }
        let tx =
            rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
        let verdict = crate::db::breaker::breaker_check(
            &tx,
            doomed,
            l,
            threshold,
            crate::db::breaker::BREAKER_L_HEAL,
        )?;
        tx.commit()?;
        if let crate::db::breaker::BreakerVerdict::Refuse { reason } = verdict {
            eprintln!("refusing heal pass: {reason}");
            return Ok(HealSummary {
                evaluated: entries.len(),
                ..HealSummary::default()
            });
        }
    }

    let mut summary = HealSummary {
        evaluated: entries.len(),
        ..HealSummary::default()
    };
    let mut healed_by_entity: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for (rowid, source_ref, entity_id, entry_id) in entries {
        // Atomicity fix (/fix-pr PRRT_kwDOSVmXas6lvgTv): wrap the grounding
        // check, soft-delete, and edge cascade in a single BEGIN IMMEDIATE
        // transaction. The Immediate mode acquires the RESERVED lock up-front
        // so a concurrent writer (e.g. an `ingest` deleting the underlying
        // document) can't race the read-then-write window, and a concurrent
        // heal (GUI scheduler thread + headless `ct heal`) can't both decide
        // to soft-delete the same row. The UPDATE is conditional on the row
        // still being live AND still carrying the same source_ref we just
        // read, so a row that was already soft-deleted (or whose source_ref
        // changed) is skipped without double-counting.
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        // Shared consumer helper (`source_ref_is_still_grounded`): handles
        // both the legacy vault-relative path shape and the JSON
        // evidence-blob shape; treats empty / unparseable / DB-error values
        // as still-grounded (defensive — see its docs).
        if !crate::db::commit::source_ref_is_still_grounded(&tx, &source_ref) {
            let changed = tx.execute(
                "UPDATE llm_wiki_entries
                 SET deleted_at = ?1
                 WHERE rowid = ?2
                   AND deleted_at IS NULL
                   AND source_ref = ?3",
                rusqlite::params![crate::db::commit::ms_now(), rowid, source_ref],
            )?;
            if changed == 0 {
                // Row was already soft-deleted (or its source_ref changed)
                // between the SELECT above and the tx open — nothing to
                // cascade, nothing to count. Commit the empty tx and move on.
                tx.commit()?;
                continue;
            }
            // Capture the cascade count so the summary reports what the
            // heal actually purged (spec §5 — previously discarded).
            summary.edges_purged += crate::db::edge_purge::purge_edges_for_entry(&tx, &entry_id)?;
            // Task 10 (spec §10): the soft-delete spends the SHARED counter
            // INSIDE this per-row IMMEDIATE transaction — scheduler, `ct heal`
            // and the GUI button share one budget across processes.
            crate::db::breaker::breaker_spend(&tx, changed as i64)?;
            tx.commit()?;
            *healed_by_entity.entry(entity_id).or_insert(0) += 1;
            summary.soft_deleted += 1;
        } else {
            // Grounded: release the IMMEDIATE lock so the next iteration can
            // open its own tx without contention.
            tx.commit()?;
        }
    }

    // Write healed events for entities that had entries repaired
    if !healed_by_entity.is_empty() {
        let (_, now_ms) = crate::db::commit::now_timestamps();
        for (entity_id, n) in healed_by_entity {
            conn.execute(
                "INSERT INTO llm_wiki_events (id, entity_id, event_type, summary, related_entry_id, created_at)
                 VALUES (?1, ?2, 'healed', ?3, NULL, ?4)",
                rusqlite::params![
                    crate::db::commit::generate_llm_id("evt_"),
                    entity_id,
                    format!("Healed {n} invalid source reference(s)"),
                    now_ms,
                ],
            )?;
        }
    }

    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::open_in_memory;

    fn insert_entry(conn: &Connection, id: &str, source_ref: &str) {
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                source_ref, created_at, updated_at, deleted_at
             ) VALUES (?1, 'tier_fact', ?2, 'body', '[]', 'inferred',
                       'librarian_inferred', ?3, 1, 1, NULL)",
            rusqlite::params![id, format!("Title {id}"), source_ref],
        )
        .unwrap();
    }

    fn insert_edge(conn: &Connection, id: &str, source_id: &str, target_id: &str) {
        conn.execute(
            "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
             VALUES (?1, 'tier_fact', ?2, ?3, 'related_to', 1757000000000)",
            rusqlite::params![id, source_id, target_id],
        )
        .unwrap();
    }

    fn grounded_document(conn: &Connection, path: &str) {
        conn.execute(
            "INSERT INTO documents (path, hash, tier, status)
             VALUES (?1, 'h', 'user_doc', 'indexed')",
            [path],
        )
        .unwrap();
    }

    #[test]
    fn heals_ungrounded_row_purges_edges_and_counts_everything() {
        let mut conn = open_in_memory().unwrap();
        // Ungrounded: legacy path shape pointing at a document that does
        // not exist in `documents` (demonstrably stale).
        insert_entry(&conn, "lost", "documents/vanished.md");
        // Grounded: the referenced document exists and is indexed.
        grounded_document(&conn, "documents/a.md");
        insert_entry(&conn, "live", "documents/a.md");
        // Task 10: a grounded companion keeps the batch PARTIAL — a lone
        // ungrounded row would be the ENTIRE selection (L=1) and the rev-34
        // breaker refuses it before the edge-purge assertions could ever be
        // exercised. The companion matches the heal selection, so it counts
        // toward L but is grounded and stays live.
        insert_entry(&conn, "companion", "documents/a.md");
        insert_edge(&conn, "edge_lost", "lost", "live");
        insert_edge(&conn, "edge_live", "live", "lost");

        // Pre-soft-delete the partner row (same fixture shape as the
        // `heal_lost_librarian_inferred_purges_edges_of_soft_deleted_entries`
        // remediation test): `purge_edges_for_entry` keeps an edge while any
        // partner endpoint is still alive, so an edge whose partner is live
        // survives the heal by design — only edges with dead partners purge.
        conn.execute(
            "UPDATE llm_wiki_entries SET deleted_at = 100 WHERE id = 'live'",
            [],
        )
        .unwrap();

        let summary = heal_invalid_sources_conn(&mut conn, PathBuf::from("/vault")).unwrap();
        assert_eq!(
            summary.evaluated, 2,
            "only live rows are evaluated (the pre-soft-deleted partner is excluded; \
             the Task-10 grounded companion is live and therefore evaluated)"
        );
        assert_eq!(summary.soft_deleted, 1, "only the ungrounded row heals");
        assert_eq!(
            summary.edges_purged, 2,
            "both edges of the healed entry purge once their partners are dead"
        );

        let deleted_at: Option<i64> = conn
            .query_row(
                "SELECT deleted_at FROM llm_wiki_entries WHERE id = 'lost'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(deleted_at.is_some(), "ungrounded row must be soft-deleted");
        let purged: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!(purged, 0, "all dead-partner edges must be purged");

        let events: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_events WHERE event_type = 'healed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(events, 1, "one healed event for the one affected entity");
    }

    #[test]
    fn grounded_row_stays_live_and_its_edges_survive() {
        let mut conn = open_in_memory().unwrap();
        grounded_document(&conn, "documents/a.md");
        insert_entry(&conn, "live", "documents/a.md");
        insert_edge(&conn, "edge_live", "live", "live");

        let summary = heal_invalid_sources_conn(&mut conn, PathBuf::from("/vault")).unwrap();
        assert_eq!(summary.evaluated, 1);
        assert_eq!(summary.soft_deleted, 0, "grounded row must stay live");
        assert_eq!(summary.edges_purged, 0);
        let kept: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_edges WHERE id = 'edge_live'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kept, 1, "the live entry's edges must survive");
    }

    #[test]
    fn empty_brain_yields_zero_summary_and_writes_no_events() {
        let mut conn = open_in_memory().unwrap();
        let summary = heal_invalid_sources_conn(&mut conn, PathBuf::from("/vault")).unwrap();
        assert_eq!(summary, HealSummary::default());
        let events: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(events, 0);
    }

    #[test]
    fn skipped_rows_are_not_double_evaluated_on_second_pass() {
        let mut conn = open_in_memory().unwrap();
        insert_entry(&conn, "lost", "documents/vanished.md");
        // Task 10: a grounded companion keeps the batch PARTIAL — a lone
        // ungrounded row would be the ENTIRE selection (L=1) and the rev-34
        // breaker refuses it. Because a proceeding batch can never heal its
        // whole selection, pass 2 always still selects the live grounded
        // companion (evaluated == 1); the soft-deleted row is what must
        // disappear from the count.
        grounded_document(&conn, "documents/a.md");
        insert_entry(&conn, "companion", "documents/a.md");
        let first = heal_invalid_sources_conn(&mut conn, PathBuf::from("/vault")).unwrap();
        assert_eq!(first.soft_deleted, 1);
        // Second pass: the healed row is soft-deleted, so the selection
        // (`deleted_at IS NULL`) skips it.
        let second = heal_invalid_sources_conn(&mut conn, PathBuf::from("/vault")).unwrap();
        assert_eq!(
            second.evaluated, 1,
            "soft-deleted rows are not re-evaluated (only the live grounded companion is selected)"
        );
        assert_eq!(second.soft_deleted, 0);
    }

    // ── Task 10 (spec §10): the shared cross-process breaker on this writer ──

    fn breaker_spent(conn: &Connection) -> i64 {
        conn.query_row(
            "SELECT CAST(value AS INTEGER) FROM llm_wiki_meta WHERE key = 'breaker_spent'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0)
    }

    /// Task 10 (spec §10): a batch that would remove the ENTIRE selected
    /// population is refused outright (rev-34 `|doomed| >= L` rule), the
    /// breaker stays unspent, and the row survives. `ct heal` and the
    /// scheduler both route through this writer, so this is the refusal arm
    /// for BOTH of those surfaces.
    #[test]
    fn heal_invalid_sources_conn_refuses_when_doomed_equals_selection() {
        let mut conn = open_in_memory().unwrap();
        // 3 live librarian_inferred rows, ALL of them ungrounded: L=3,
        // |doomed|=3 ⇒ refuse even though 3 ≤ threshold 10 (Small-L rule).
        insert_entry(&conn, "lost0", "documents/vanished0.md");
        insert_entry(&conn, "lost1", "documents/vanished1.md");
        insert_entry(&conn, "lost2", "documents/vanished2.md");

        let summary = heal_invalid_sources_conn(&mut conn, PathBuf::from("/vault")).unwrap();
        assert_eq!(
            summary.soft_deleted, 0,
            "a full-population batch must soft-delete NOTHING"
        );
        let alive: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_entries WHERE deleted_at IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(alive, 3, "all selected rows must survive the refusal");
        assert_eq!(
            breaker_spent(&conn),
            0,
            "a refused pass must not spend the shared budget"
        );
    }

    /// Task 10 (spec §10): a PARTIAL batch within the budget proceeds, and
    /// every successful soft-delete spends the SHARED `breaker_spent` counter
    /// inside its per-row IMMEDIATE transaction.
    #[test]
    fn heal_invalid_sources_conn_spends_shared_budget_per_soft_deleted_row() {
        let mut conn = open_in_memory().unwrap();
        // Grounded partner keeps the selection live so the batch is partial.
        grounded_document(&conn, "documents/a.md");
        insert_entry(&conn, "live", "documents/a.md");
        insert_entry(&conn, "lost", "documents/vanished.md");
        // L=2 → threshold max(⌈0.1⌉, 10) = 10; |doomed|=1 < L ⇒ budget-governed.

        let summary = heal_invalid_sources_conn(&mut conn, PathBuf::from("/vault")).unwrap();
        assert_eq!(summary.soft_deleted, 1, "partial batch proceeds");
        assert_eq!(
            breaker_spent(&conn),
            1,
            "each soft-deleted row increments the shared counter"
        );
    }
}
