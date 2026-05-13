//! Pass 3: Global Resolver — links reference chunks to their definition chunks.
//! Runs as a background job after a batch of file ingest operations completes.

use anyhow::Result;
use rusqlite::Connection;

struct ResolvedEdge {
    ref_chunk_id: i64,
    def_chunk_id: i64,
    rel_type: String,
    symbol: String,
    entity_id: String,
}

const RESOLVER_SQL: &str = "
SELECT ref.id          AS ref_chunk_id,
       ref.symbol_name AS symbol,
       ref.entity_id,
       def.id          AS def_chunk_id,
       CASE ref.strategy
         WHEN 'ast_ref' THEN 'CALLS'
         ELSE 'CALLS'
       END             AS rel_type
FROM   chunks AS ref
JOIN   chunks AS def
       ON  def.defined_symbol = LOWER(TRIM(ref.symbol_name))
       AND def.entity_id      = ref.entity_id
WHERE  ref.defined_symbol IS NULL
  AND  ref.symbol_name    IS NOT NULL
  AND  ref.entity_id      = ?1
";

/// Run the linker for a single entity_id.
/// 1. Deletes stale edges for chunks re-indexed since `since_epoch`
/// 2. Resolves all reference-to-definition pairs and inserts edges
///
/// Entity-scoped: only chunks within the given `entity_id` are linked,
/// preventing cross-vault symbol contamination.
pub fn run_linker(conn: &Connection, entity_id: &str, since_epoch: i64) -> Result<()> {
    crate::db::queries::delete_stale_relationships(conn, entity_id, since_epoch)?;

    let edges: Vec<ResolvedEdge> = {
        let mut stmt = conn.prepare(RESOLVER_SQL)?;
        let mut rows = stmt.query([entity_id])?;
        let mut v = Vec::new();
        while let Some(row) = rows.next()? {
            v.push(ResolvedEdge {
                ref_chunk_id: row.get(0)?,
                symbol: row.get::<_, String>(1)?,
                entity_id: row.get::<_, String>(2)?,
                def_chunk_id: row.get(3)?,
                rel_type: row.get::<_, String>(4)?,
            });
        }
        v
    };

    for edge in &edges {
        crate::db::queries::insert_relationship(
            conn,
            edge.ref_chunk_id,
            edge.def_chunk_id,
            &edge.rel_type,
            &edge.symbol,
            &edge.entity_id,
        )?;
    }

    Ok(())
}

/// Collect all distinct entity_ids that have unresolved reference chunks.
pub fn entity_ids_needing_link(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT entity_id FROM chunks
         WHERE defined_symbol IS NULL
           AND symbol_name IS NOT NULL
           AND entity_id IS NOT NULL",
    )?;
    let mut rows = stmt.query([])?;
    let mut ids = Vec::new();
    while let Some(row) = rows.next()? {
        ids.push(row.get::<_, String>(0)?);
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunker::{Chunk, ChunkStrategyTag};
    use crate::db::connection::open_in_memory;
    use crate::db::queries::{insert_chunk, mark_document_indexed, upsert_document};

    fn def_chunk(name: &str) -> Chunk {
        Chunk {
            text: format!("fn {}() {{}}", name),
            start_line: 1,
            end_line: 3,
            symbol_name: Some(name.to_string()),
            defined_symbol: Some(name.to_lowercase()),
            strategy: ChunkStrategyTag::AstSymbolRust,
        }
    }

    fn ref_chunk(symbol: &str) -> Chunk {
        Chunk {
            text: format!("{}();", symbol),
            start_line: 10,
            end_line: 10,
            symbol_name: Some(symbol.to_lowercase()),
            defined_symbol: None,
            strategy: ChunkStrategyTag::AstRef,
        }
    }

    #[test]
    fn linker_creates_calls_edge() {
        let conn = open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        let doc_id = upsert_document(&conn, "/vault/documents/main.rs", "hash1").unwrap();
        mark_document_indexed(&conn, doc_id).unwrap();

        let def_id = insert_chunk(&conn, doc_id, &def_chunk("init_db"), 0, "tier_fact").unwrap();
        let ref_id = insert_chunk(&conn, doc_id, &ref_chunk("init_db"), 1, "tier_fact").unwrap();

        run_linker(&conn, "tier_fact", 0).unwrap();

        let (from_id, to_id, rel_type, symbol): (i64, i64, String, String) = conn
            .query_row(
                "SELECT from_id, to_id, rel_type, symbol FROM curated_relationships WHERE entity_id = 'tier_fact'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();

        assert_eq!(from_id, ref_id);
        assert_eq!(to_id, def_id);
        assert_eq!(rel_type, "CALLS");
        assert_eq!(symbol, "init_db");
    }

    #[test]
    fn linker_does_not_cross_entity_boundaries() {
        let conn = open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();

        let doc_a = upsert_document(&conn, "/vault_a/documents/a.rs", "ha").unwrap();
        let doc_b = upsert_document(&conn, "/vault_b/documents/b.rs", "hb").unwrap();
        mark_document_indexed(&conn, doc_a).unwrap();
        mark_document_indexed(&conn, doc_b).unwrap();

        insert_chunk(&conn, doc_a, &def_chunk("shared_fn"), 0, "tier_fact").unwrap();
        insert_chunk(&conn, doc_b, &ref_chunk("shared_fn"), 0, "tier_wisdom").unwrap();

        run_linker(&conn, "tier_fact", 0).unwrap();
        run_linker(&conn, "tier_wisdom", 0).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM curated_relationships",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "cross-entity edges must not be created");
    }

    #[test]
    fn linker_stale_cleanup_removes_old_edges() {
        let conn = open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();

        let doc_id = upsert_document(&conn, "/vault/documents/main.rs", "hash1").unwrap();
        mark_document_indexed(&conn, doc_id).unwrap();
        let def_id = insert_chunk(&conn, doc_id, &def_chunk("foo"), 0, "tier_fact").unwrap();
        let ref_id = insert_chunk(&conn, doc_id, &ref_chunk("foo"), 1, "tier_fact").unwrap();

        run_linker(&conn, "tier_fact", 0).unwrap();
        let before: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM curated_relationships",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(before, 1);

        conn.execute(
            "UPDATE documents SET last_indexed = unixepoch() WHERE id = ?1",
            [doc_id],
        )
        .unwrap();

        let since = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        run_linker(&conn, "tier_fact", since).unwrap();

        let after: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM curated_relationships",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(after, 1, "edge count should remain 1 after stale cleanup + re-link");

        let _ = (def_id, ref_id);
    }

    #[test]
    fn entity_ids_needing_link_returns_ids_with_unresolved_refs() {
        let conn = open_in_memory().unwrap();
        let doc_id = upsert_document(&conn, "/vault/documents/a.rs", "h1").unwrap();
        insert_chunk(&conn, doc_id, &ref_chunk("something"), 0, "tier_fact").unwrap();

        let ids = entity_ids_needing_link(&conn).unwrap();
        assert!(ids.contains(&"tier_fact".to_string()));
    }
}
