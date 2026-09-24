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
pub fn heal_invalid_sources_conn(conn: &mut Connection, _vault: PathBuf) -> Result<HealSummary> {
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

    let mut summary = HealSummary {
        evaluated: entries.len(),
        ..HealSummary::default()
    };
    let mut healed_by_entity: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for (rowid, source_ref, entity_id, entry_id) in entries {
        // Shared consumer helper (`source_ref_is_still_grounded`): handles
        // both the legacy vault-relative path shape and the JSON
        // evidence-blob shape; treats empty / unparseable / DB-error values
        // as still-grounded (defensive — see its docs).
        if !crate::db::commit::source_ref_is_still_grounded(conn, &source_ref) {
            let tx = conn.unchecked_transaction()?;
            tx.execute(
                "UPDATE llm_wiki_entries SET deleted_at = ?1 WHERE rowid = ?2",
                rusqlite::params![crate::db::commit::ms_now(), rowid],
            )?;
            // Capture the cascade count so the summary reports what the
            // heal actually purged (spec §5 — previously discarded).
            summary.edges_purged += crate::db::edge_purge::purge_edges_for_entry(&tx, &entry_id)?;
            tx.commit()?;
            *healed_by_entity.entry(entity_id).or_insert(0) += 1;
            summary.soft_deleted += 1;
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
            summary.evaluated, 1,
            "only live rows are evaluated (the pre-soft-deleted partner is excluded)"
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
        let first = heal_invalid_sources_conn(&mut conn, PathBuf::from("/vault")).unwrap();
        assert_eq!(first.soft_deleted, 1);
        // Second pass: the healed row is soft-deleted, so the selection
        // (`deleted_at IS NULL`) skips it.
        let second = heal_invalid_sources_conn(&mut conn, PathBuf::from("/vault")).unwrap();
        assert_eq!(
            second.evaluated, 0,
            "soft-deleted rows are not re-evaluated"
        );
        assert_eq!(second.soft_deleted, 0);
    }
}
