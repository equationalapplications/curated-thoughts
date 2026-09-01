//! One-time migration: heal a brain.db whose wiki graph was severed by the
//! Aug 2026 librarian regeneration. See the design spec, Part C.

use anyhow::Result;
use rusqlite::Connection;

/// An edge is an orphan unless BOTH endpoints are present AND not soft-deleted.
///
/// `NOT EXISTS` rather than `NOT IN`: the `NOT IN` form treats a soft-deleted
/// endpoint as alive (its id is still in the table) and sidesteps the classic
/// NULL-subquery trap. This predicate matches the runtime contract in spec §2.
const ORPHAN_PREDICATE: &str = "
     NOT EXISTS (SELECT 1 FROM llm_wiki_entries s
                  WHERE s.id = llm_wiki_edges.source_id AND s.deleted_at IS NULL)
  OR NOT EXISTS (SELECT 1 FROM llm_wiki_entries t
                  WHERE t.id = llm_wiki_edges.target_id AND t.deleted_at IS NULL)";

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
