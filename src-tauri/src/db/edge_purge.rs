//! Cascading edge deletion: an edge in `llm_wiki_edges` exists only between two
//! live `llm_wiki_entries` rows. Every site that deletes, archives, or
//! regenerates an entry calls into here **inside its own transaction**, so a
//! crash can never strand an edge.
//!
//! Edges are not replicated (no outbox rows) — `commit_edge_add` and the bundle
//! import path insert them without CDC, so purges match. See the design spec §2.

use anyhow::Result;
use rusqlite::{params, Connection};

/// Delete every edge with `entry_id` at either end.
///
/// Takes `&Connection` rather than `&mut Connection` so it composes inside a
/// caller's open transaction — the entry deletion and this purge must commit or
/// roll back together, otherwise a crash between them mints an orphan.
pub fn purge_edges_for_entry(conn: &Connection, entry_id: &str) -> Result<usize> {
    let removed = conn.execute(
        "DELETE FROM llm_wiki_edges
          WHERE source_id = ?1 OR target_id = ?1",
        params![entry_id],
    )?;
    Ok(removed)
}

/// Purge edges for a set of dying entries, for the predicate-driven deletion
/// sites (`prune_old_librarian_inferred`) that do not have a single id to hand.
///
/// An edge whose two endpoints are both in `entry_ids` is deleted by the first
/// id that reaches it and contributes 1 to the total, not 2.
pub fn purge_edges_for_entries(conn: &Connection, entry_ids: &[String]) -> Result<usize> {
    let mut total = 0;
    for id in entry_ids {
        total += purge_edges_for_entry(conn, id)?;
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::open_in_memory;

    /// Inserts a live entry. `deleted_at` is NULL (live).
    fn seed_entry(conn: &Connection, id: &str, entity_id: &str) {
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                source_hash, source_ref, created_at, updated_at, last_accessed_at,
                access_count, deleted_at, embedding_blob, embedding
             ) VALUES (?1, ?2, 'T', 'B', '[]', 'inferred', 'librarian_inferred',
                       NULL, NULL, 100, 100, NULL, 0, NULL, NULL, NULL)",
            params![id, entity_id],
        )
        .unwrap();
    }

    fn seed_edge(conn: &Connection, id: &str, entity_id: &str, source: &str, target: &str) {
        conn.execute(
            "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
             VALUES (?1, ?2, ?3, ?4, 'related_to', 100)",
            params![id, entity_id, source, target],
        )
        .unwrap();
    }

    fn edge_ids(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT id FROM llm_wiki_edges ORDER BY id")
            .unwrap();
        let rows = stmt.query_map([], |r| r.get(0)).unwrap();
        rows.collect::<rusqlite::Result<Vec<String>>>().unwrap()
    }

    #[test]
    fn purges_edges_in_both_directions() {
        let conn = open_in_memory().unwrap();
        seed_entry(&conn, "fact_a", "ent-1");
        seed_entry(&conn, "fact_b", "ent-1");
        seed_entry(&conn, "fact_c", "ent-1");
        // fact_a as source, fact_a as target, and one edge that must survive.
        seed_edge(&conn, "edge_out", "ent-1", "fact_a", "fact_b");
        seed_edge(&conn, "edge_in", "ent-1", "fact_c", "fact_a");
        seed_edge(&conn, "edge_other", "ent-1", "fact_b", "fact_c");

        let removed = purge_edges_for_entry(&conn, "fact_a").unwrap();

        assert_eq!(removed, 2, "both the inbound and outbound edge must go");
        assert_eq!(edge_ids(&conn), vec!["edge_other".to_string()]);
    }

    #[test]
    fn purging_an_entry_with_no_edges_is_a_no_op() {
        let conn = open_in_memory().unwrap();
        seed_entry(&conn, "fact_a", "ent-1");
        seed_entry(&conn, "fact_b", "ent-1");
        seed_edge(&conn, "edge_other", "ent-1", "fact_a", "fact_b");

        let removed = purge_edges_for_entry(&conn, "fact_unrelated").unwrap();

        assert_eq!(removed, 0);
        assert_eq!(edge_ids(&conn), vec!["edge_other".to_string()]);
    }

    #[test]
    fn purges_a_set_of_entries_and_counts_each_edge_once() {
        let conn = open_in_memory().unwrap();
        seed_entry(&conn, "fact_a", "ent-1");
        seed_entry(&conn, "fact_b", "ent-1");
        seed_entry(&conn, "fact_c", "ent-1");
        // This edge touches BOTH doomed ids — it must be counted once, not twice,
        // because the first per-id DELETE already removed it.
        seed_edge(&conn, "edge_ab", "ent-1", "fact_a", "fact_b");
        seed_edge(&conn, "edge_bc", "ent-1", "fact_b", "fact_c");

        let doomed = vec!["fact_a".to_string(), "fact_b".to_string()];
        let removed = purge_edges_for_entries(&conn, &doomed).unwrap();

        assert_eq!(removed, 2, "edge_ab counted once even though both ends died");
        assert!(edge_ids(&conn).is_empty());
    }

    #[test]
    fn purge_is_visible_to_the_enclosing_transaction_and_rolls_back_with_it() {
        let conn = open_in_memory().unwrap();
        seed_entry(&conn, "fact_a", "ent-1");
        seed_entry(&conn, "fact_b", "ent-1");
        seed_edge(&conn, "edge_ab", "ent-1", "fact_a", "fact_b");

        let tx = conn.unchecked_transaction().unwrap();
        purge_edges_for_entry(&tx, "fact_a").unwrap();
        tx.rollback().unwrap();

        assert_eq!(
            edge_ids(&conn),
            vec!["edge_ab".to_string()],
            "a rolled-back purge must leave the edge in place"
        );
    }
}
