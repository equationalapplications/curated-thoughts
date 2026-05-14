use anyhow::Result;
use rusqlite::Connection;

#[derive(Debug, serde::Serialize)]
pub struct NeighborRow {
    pub chunk_id: i64,
    pub depth: i64,
    pub rel_type: String,
}

const CALLEE_CTE: &str = "
WITH RECURSIVE callee_walk(chunk_id, depth, rel_type) AS (
    SELECT to_id, 1, rel_type
    FROM   curated_relationships
    WHERE  from_id   = ?1
      AND  rel_type  IN ('CALLS', 'IMPORTS')
      AND  entity_id = ?2

    UNION ALL

    SELECT r.to_id, cw.depth + 1, r.rel_type
    FROM   curated_relationships r
    JOIN   callee_walk cw ON r.from_id = cw.chunk_id
    WHERE  cw.depth    < ?3
      AND  r.rel_type  IN ('CALLS', 'IMPORTS')
      AND  r.entity_id = ?2
)
,
ranked AS (
    SELECT chunk_id, depth, rel_type,
           ROW_NUMBER() OVER (
               PARTITION BY chunk_id
               ORDER BY depth,
                        CASE rel_type WHEN 'CALLS' THEN 0 WHEN 'IMPORTS' THEN 1 ELSE 2 END,
                        rel_type
           ) AS rn
    FROM callee_walk
    WHERE chunk_id != ?1
)
SELECT chunk_id, depth AS min_depth, rel_type
FROM   ranked
WHERE  rn = 1
ORDER  BY min_depth
";

const CALLER_CTE: &str = "
WITH RECURSIVE caller_walk(chunk_id, depth, rel_type) AS (
    SELECT from_id, 1, rel_type
    FROM   curated_relationships
    WHERE  to_id     = ?1
      AND  rel_type  IN ('CALLS', 'IMPORTS')
      AND  entity_id = ?2

    UNION ALL

    SELECT r.from_id, cw.depth + 1, r.rel_type
    FROM   curated_relationships r
    JOIN   caller_walk cw ON r.to_id = cw.chunk_id
    WHERE  cw.depth    < ?3
      AND  r.rel_type  IN ('CALLS', 'IMPORTS')
      AND  r.entity_id = ?2
)
,
ranked AS (
    SELECT chunk_id, depth, rel_type,
           ROW_NUMBER() OVER (
               PARTITION BY chunk_id
               ORDER BY depth,
                        CASE rel_type WHEN 'CALLS' THEN 0 WHEN 'IMPORTS' THEN 1 ELSE 2 END,
                        rel_type
           ) AS rn
    FROM caller_walk
    WHERE chunk_id != ?1
)
SELECT chunk_id, depth AS min_depth, rel_type
FROM   ranked
WHERE  rn = 1
ORDER  BY min_depth
";

pub fn get_callees(
    conn: &Connection,
    root_chunk_id: i64,
    entity_id: &str,
    max_hops: u32,
) -> Result<Vec<NeighborRow>> {
    run_cte(conn, CALLEE_CTE, root_chunk_id, entity_id, max_hops)
}

pub fn get_callers(
    conn: &Connection,
    root_chunk_id: i64,
    entity_id: &str,
    max_hops: u32,
) -> Result<Vec<NeighborRow>> {
    run_cte(conn, CALLER_CTE, root_chunk_id, entity_id, max_hops)
}

pub fn get_both(
    conn: &Connection,
    root_chunk_id: i64,
    entity_id: &str,
    max_hops: u32,
) -> Result<Vec<NeighborRow>> {
    let mut callees = get_callees(conn, root_chunk_id, entity_id, max_hops)?;
    let callers = get_callers(conn, root_chunk_id, entity_id, max_hops)?;

    for caller in callers {
        if let Some(existing) = callees.iter_mut().find(|c| c.chunk_id == caller.chunk_id) {
            if caller.depth < existing.depth {
                existing.depth = caller.depth;
                existing.rel_type = caller.rel_type;
            }
        } else {
            callees.push(caller);
        }
    }
    callees.sort_by_key(|r| r.depth);
    Ok(callees)
}

fn run_cte(
    conn: &Connection,
    cte: &str,
    root_chunk_id: i64,
    entity_id: &str,
    max_hops: u32,
) -> Result<Vec<NeighborRow>> {
    let mut stmt = conn.prepare(cte)?;
    let mut rows = stmt.query(rusqlite::params![root_chunk_id, entity_id, max_hops])?;
    let mut neighbors = Vec::new();
    while let Some(row) = rows.next()? {
        neighbors.push(NeighborRow {
            chunk_id: row.get(0)?,
            depth: row.get(1)?,
            rel_type: row.get(2)?,
        });
    }
    Ok(neighbors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunker::{Chunk, ChunkStrategyTag};
    use crate::db::connection::open_in_memory;
    use crate::db::queries::{insert_chunk, insert_relationship, mark_document_indexed, upsert_document};

    #[test]
    fn min_depth_rel_type_priority_prefers_calls() {
        let conn = open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        let doc = upsert_document(&conn, "/vault/documents/priority.rs", "h").unwrap();
        mark_document_indexed(&conn, doc).unwrap();
        let root = insert_chunk(&conn, doc, &make_def_chunk("root"), 0, "tier_fact").unwrap();
        let target = insert_chunk(&conn, doc, &make_def_chunk("target"), 1, "tier_fact").unwrap();
        insert_relationship(&conn, root, target, "CALLS", "target", "tier_fact").unwrap();
        insert_relationship(&conn, root, target, "IMPORTS", "target", "tier_fact").unwrap();

        let callees = get_callees(&conn, root, "tier_fact", 5).unwrap();
        let target_rows: Vec<_> = callees.iter().filter(|r| r.chunk_id == target).collect();
        assert_eq!(target_rows.len(), 1, "target chunk should appear once");
        assert_eq!(target_rows[0].rel_type, "CALLS", "CALLS should be preferred over IMPORTS at equal depth");
    }

    fn make_def_chunk(name: &str) -> Chunk {
        Chunk {
            text: format!("fn {}() {{}}", name),
            start_line: 1,
            end_line: 3,
            symbol_name: Some(name.to_string()),
            defined_symbol: Some(name.to_lowercase()),
            strategy: ChunkStrategyTag::AstSymbolRust,
        }
    }

    fn setup_diamond(conn: &rusqlite::Connection) -> (i64, i64, i64, i64) {
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        let doc = upsert_document(conn, "/vault/documents/diamond.rs", "h").unwrap();
        mark_document_indexed(conn, doc).unwrap();
        let a = insert_chunk(conn, doc, &make_def_chunk("a"), 0, "tier_fact").unwrap();
        let b = insert_chunk(conn, doc, &make_def_chunk("b"), 1, "tier_fact").unwrap();
        let c = insert_chunk(conn, doc, &make_def_chunk("c"), 2, "tier_fact").unwrap();
        let d = insert_chunk(conn, doc, &make_def_chunk("d"), 3, "tier_fact").unwrap();
        insert_relationship(conn, a, b, "CALLS", "b", "tier_fact").unwrap();
        insert_relationship(conn, a, c, "CALLS", "c", "tier_fact").unwrap();
        insert_relationship(conn, b, d, "CALLS", "d", "tier_fact").unwrap();
        insert_relationship(conn, c, d, "CALLS", "d", "tier_fact").unwrap();
        (a, b, c, d)
    }

    #[test]
    fn diamond_deduplicates_to_min_depth() {
        let conn = open_in_memory().unwrap();
        let (a, _b, _c, d) = setup_diamond(&conn);
        let callees = get_callees(&conn, a, "tier_fact", 5).unwrap();
        let d_rows: Vec<_> = callees.iter().filter(|r| r.chunk_id == d).collect();
        assert_eq!(d_rows.len(), 1, "D must appear exactly once");
        assert_eq!(d_rows[0].depth, 2, "D must be at min_depth 2");
    }

    #[test]
    fn callers_finds_all_callers_of_d() {
        let conn = open_in_memory().unwrap();
        let (a, b, c, d) = setup_diamond(&conn);
        let callers = get_callers(&conn, d, "tier_fact", 5).unwrap();
        let caller_ids: Vec<i64> = callers.iter().map(|r| r.chunk_id).collect();
        assert!(caller_ids.contains(&b), "B should be a depth-1 caller of D");
        assert!(caller_ids.contains(&c), "C should be a depth-1 caller of D");
        assert!(caller_ids.contains(&a), "A should be a depth-2 caller of D");
    }

    #[test]
    fn max_hops_limits_depth() {
        let conn = open_in_memory().unwrap();
        let (a, _b, _c, d) = setup_diamond(&conn);
        let callees = get_callees(&conn, a, "tier_fact", 1).unwrap();
        let chunk_ids: Vec<i64> = callees.iter().map(|r| r.chunk_id).collect();
        assert!(!chunk_ids.contains(&d), "D is at depth 2, should not appear with max_hops=1");
    }

    #[test]
    fn get_both_merges_and_deduplicates() {
        let conn = open_in_memory().unwrap();
        let (_a, _b, _c, d) = setup_diamond(&conn);
        let doc2 = upsert_document(&conn, "/vault/documents/b.rs", "h2").unwrap();
        let b2 = insert_chunk(&conn, doc2, &make_def_chunk("b_direct"), 0, "tier_fact").unwrap();
        let doc_a2 = upsert_document(&conn, "/vault/documents/a2.rs", "h3").unwrap();
        let a2 = insert_chunk(&conn, doc_a2, &make_def_chunk("a2"), 0, "tier_fact").unwrap();
        insert_relationship(&conn, a2, b2, "CALLS", "b_direct", "tier_fact").unwrap();
        insert_relationship(&conn, b2, d, "CALLS", "d", "tier_fact").unwrap();
        let both = get_both(&conn, b2, "tier_fact", 5).unwrap();
        let ids: Vec<i64> = both.iter().map(|r| r.chunk_id).collect();
        assert!(ids.contains(&d), "callee D should be included");
        assert!(ids.contains(&a2), "caller A2 should be included");
        let unique: std::collections::HashSet<i64> = ids.iter().cloned().collect();
        assert_eq!(unique.len(), ids.len(), "no duplicates in get_both result");
    }
}
