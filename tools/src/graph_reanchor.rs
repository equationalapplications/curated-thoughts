//! One-time migration: heal a brain.db whose wiki graph was severed by the
//! Aug 2026 librarian regeneration. See the design spec, Part C.

use anyhow::Result;
use rusqlite::Connection;

/// An edge is an orphan iff BOTH endpoints are absent from EVERY valid home
/// the endpoint id can belong to: `llm_wiki_entries`, `curated_entities`, or
/// `llm_wiki_tasks`. Each table carries a `deleted_at` column, so a row that
/// exists but is soft-deleted counts as absent for this purpose.
///
/// Edge endpoints are heterogeneous (design spec §2 + remediation R1): an
/// endpoint id may live in any of the three tables. An edge is preserved as
/// long as ONE endpoint resolves to a live row in at least one table. This
/// predicate is the symmetric both-endpoints-dead contract — it must match
/// the runtime's `db::edge_purge::purge_dead_edges` so the repair tool and
/// the runtime agree on what an orphan is.
///
/// `NOT EXISTS` rather than `NOT IN`: the `NOT IN` form would treat a
/// soft-deleted endpoint id as alive and trigger the NULL-subquery trap.
const ORPHAN_PREDICATE: &str = "
     NOT (
            EXISTS (SELECT 1 FROM llm_wiki_entries e
                    WHERE e.id = llm_wiki_edges.source_id AND e.deleted_at IS NULL)
         OR EXISTS (SELECT 1 FROM curated_entities ce
                    WHERE ce.id = llm_wiki_edges.source_id AND ce.deleted_at IS NULL)
         OR EXISTS (SELECT 1 FROM llm_wiki_tasks st
                    WHERE st.id = llm_wiki_edges.source_id AND st.deleted_at IS NULL)
     )
  AND NOT (
            EXISTS (SELECT 1 FROM llm_wiki_entries e
                    WHERE e.id = llm_wiki_edges.target_id AND e.deleted_at IS NULL)
         OR EXISTS (SELECT 1 FROM curated_entities ce
                    WHERE ce.id = llm_wiki_edges.target_id AND ce.deleted_at IS NULL)
         OR EXISTS (SELECT 1 FROM llm_wiki_tasks st
                    WHERE st.id = llm_wiki_edges.target_id AND st.deleted_at IS NULL)
     )";

/// Delete every edge with no live endpoint. Returns rows deleted.
/// Idempotent: a second run deletes nothing.
pub fn purge_orphan_edges(conn: &Connection) -> Result<usize> {
    let sql = format!("DELETE FROM llm_wiki_edges WHERE {ORPHAN_PREDICATE}");
    let tx = conn.unchecked_transaction()?;
    let removed = tx.execute(&sql, [])?;
    tx.commit()?;
    Ok(removed)
}

/// Post-condition check: must return 0 after `purge_orphan_edges`.
pub fn count_orphan_edges(conn: &Connection) -> Result<usize> {
    let sql = format!("SELECT COUNT(*) FROM llm_wiki_edges WHERE {ORPHAN_PREDICATE}");
    let n: i64 = conn.query_row(&sql, [], |r| r.get(0))?;
    Ok(n as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    /// Open an in-memory brain.db with the heterogeneous-edge schema.
    /// Mirrors `setupDatabase` + the Rust-owned `curated_*` tables just
    /// enough to exercise `ORPHAN_PREDICATE`.
    fn fresh() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r"
            CREATE TABLE llm_wiki_entries (
                id TEXT PRIMARY KEY,
                entity_id TEXT NOT NULL,
                title TEXT NOT NULL,
                body TEXT NOT NULL,
                tags TEXT NOT NULL DEFAULT '[]',
                confidence TEXT NOT NULL DEFAULT 'inferred',
                source_type TEXT NOT NULL DEFAULT 'librarian_inferred',
                source_hash TEXT,
                source_ref TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                last_accessed_at INTEGER,
                access_count INTEGER NOT NULL DEFAULT 0,
                deleted_at INTEGER,
                embedding TEXT,
                embedding_blob BLOB,
                okf_type TEXT,
                ontology_checked_at INTEGER,
                lifecycle_status TEXT NOT NULL DEFAULT 'stable',
                stale_after INTEGER,
                generated_by TEXT,
                last_verified_at INTEGER,
                last_verified_by TEXT,
                okf_sources TEXT,
                okf_verified TEXT,
                okf_usage_window TEXT
            );
            CREATE TABLE curated_entities (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                entity_type TEXT NOT NULL DEFAULT 'concept',
                summary TEXT NOT NULL DEFAULT '',
                summary_embedding BLOB,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                deleted_at INTEGER
            );
            CREATE TABLE llm_wiki_tasks (
                id TEXT PRIMARY KEY,
                entity_id TEXT NOT NULL,
                description TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                priority INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                resolved_at INTEGER,
                deleted_at INTEGER,
                okf_type TEXT,
                lifecycle_status TEXT NOT NULL DEFAULT 'stable',
                stale_after INTEGER,
                generated_by TEXT,
                last_verified_at INTEGER,
                last_verified_by TEXT,
                okf_sources TEXT,
                okf_verified TEXT,
                okf_usage_window TEXT
            );
            CREATE TABLE llm_wiki_edges (
                id TEXT PRIMARY KEY,
                entity_id TEXT NOT NULL,
                source_id TEXT NOT NULL,
                target_id TEXT NOT NULL,
                edge_type TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );
            ",
        )
        .unwrap();
        conn
    }

    fn seed_entry(conn: &Connection, id: &str, deleted_at: Option<i64>) {
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                source_hash, source_ref, created_at, updated_at, last_accessed_at,
                access_count, deleted_at, embedding_blob, embedding
             ) VALUES (?1, ?2, 'T', 'B', '[]', 'inferred', 'librarian_inferred',
                       NULL, NULL, 1, 1, NULL, 0, ?3, NULL, NULL)",
            params![id, "ent-1", deleted_at],
        )
        .unwrap();
    }

    fn seed_entity(conn: &Connection, id: &str, deleted_at: Option<i64>) {
        conn.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at, deleted_at)
             VALUES (?1, 'n', 'concept', '', 1, 1, ?2)",
            params![id, deleted_at],
        )
        .unwrap();
    }

    fn seed_task(conn: &Connection, id: &str, deleted_at: Option<i64>) {
        conn.execute(
            "INSERT INTO llm_wiki_tasks (id, entity_id, description, status, priority,
                created_at, updated_at, resolved_at, deleted_at)
             VALUES (?1, 'ent-1', 'd', 'pending', 0, 1, 1, NULL, ?2)",
            params![id, deleted_at],
        )
        .unwrap();
    }

    fn seed_edge(conn: &Connection, id: &str, src: &str, tgt: &str) {
        conn.execute(
            "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
             VALUES (?1, 'ent-1', ?2, ?3, 'related_to', 1)",
            params![id, src, tgt],
        )
        .unwrap();
    }

    #[test]
    fn predicate_leaves_edges_with_a_live_endpoint_alone() {
        let conn = fresh();
        // Edge whose source is a curated entity id only (no llm_wiki_entries row).
        seed_entity(&conn, "ce-only", None);
        seed_task(&conn, "task-only", None);
        seed_edge(&conn, "edge_entity_lives", "ce-only", "task-only");
        // Live entry pair — both endpoints live in llm_wiki_entries.
        seed_entry(&conn, "fact_a", None);
        seed_entry(&conn, "fact_b", None);
        seed_edge(&conn, "edge_entries_live", "fact_a", "fact_b");
        // Mixed: one endpoint in entries, other in entities.
        seed_edge(&conn, "edge_mixed", "fact_a", "ce-only");

        assert_eq!(count_orphan_edges(&conn).unwrap(), 0, "nothing is dead yet");

        let removed = purge_orphan_edges(&conn).unwrap();
        assert_eq!(removed, 0, "no edges have dead endpoints");
    }

    #[test]
    fn predicate_purges_only_when_both_endpoints_are_dead_in_every_table() {
        let conn = fresh();
        // Both endpoints are dead: not in entries, not in entities, not in tasks.
        seed_edge(&conn, "edge_truly_dead", "ghost_a", "ghost_b");

        // Source is live in llm_wiki_tasks only; target is dead everywhere.
        // Both-endpoints-dead contract: this edge SURVIVES because the source
        // endpoint is still alive in at least one table.
        seed_task(&conn, "task_alive", None);
        seed_edge(&conn, "edge_one_dead", "task_alive", "ghost_b");

        // Both endpoints soft-deleted everywhere — still dead.
        seed_entity(&conn, "ce_archived", Some(1));
        seed_entry(&conn, "fact_archived", Some(1));
        seed_task(&conn, "task_archived", Some(1));
        seed_edge(&conn, "edge_soft_deleted", "ce_archived", "fact_archived");

        let removed = purge_orphan_edges(&conn).unwrap();
        assert_eq!(
            removed, 2,
            "only edges with BOTH endpoints dead go — half-live edges survive"
        );

        // Re-running is a no-op (idempotent).
        assert_eq!(count_orphan_edges(&conn).unwrap(), 0);
        assert_eq!(purge_orphan_edges(&conn).unwrap(), 0);
    }

    #[test]
    fn predicate_keeps_edges_whose_endpoint_is_only_in_curated_entities() {
        let conn = fresh();
        // The exact bug the spec assumed-away: endpoint id exists only in
        // curated_entities. A naive entry-only purge would have deleted this.
        seed_entity(&conn, "ce_only", None);
        seed_edge(&conn, "edge_with_entity_endpoint", "ce_only", "ce_only");

        assert_eq!(count_orphan_edges(&conn).unwrap(), 0);
        let removed = purge_orphan_edges(&conn).unwrap();
        assert_eq!(
            removed, 0,
            "endpoint is alive in curated_entities — edge must be preserved"
        );
    }
}
