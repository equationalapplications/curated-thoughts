use anyhow::Result;
use rusqlite::{Connection, OptionalExtension};

pub struct DocRow {
    pub id: i64,
    pub hash: String,
    pub status: String,
}

pub fn upsert_document(conn: &Connection, path: &str, hash: &str) -> Result<i64> {
    conn.execute(
        "INSERT INTO documents (path, hash, tier, status)
         VALUES (?1, ?2, 'user_doc', 'pending')
         ON CONFLICT(path) DO UPDATE SET hash = ?2, status = 'pending'",
        rusqlite::params![path, hash],
    )?;
    Ok(
        conn.query_row("SELECT id FROM documents WHERE path = ?1", [path], |r| {
            r.get(0)
        })?,
    )
}

pub fn get_document_by_path(conn: &Connection, path: &str) -> Result<Option<DocRow>> {
    let mut stmt = conn.prepare("SELECT id, hash, status FROM documents WHERE path = ?1")?;
    let mut rows = stmt.query([path])?;
    if let Some(row) = rows.next()? {
        Ok(Some(DocRow {
            id: row.get(0)?,
            hash: row.get(1)?,
            status: row.get(2)?,
        }))
    } else {
        Ok(None)
    }
}

pub fn list_indexed_user_doc_paths(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT path FROM documents WHERE tier = 'user_doc' AND status = 'indexed' ORDER BY path",
    )?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(row.get(0)?);
    }
    Ok(out)
}

pub fn delete_document_chunks(conn: &Connection, doc_id: i64) -> Result<()> {
    conn.execute("DELETE FROM chunks WHERE doc_id = ?1", [doc_id])?;
    Ok(())
}

pub fn insert_chunk(
    conn: &Connection,
    doc_id: i64,
    chunk: &crate::chunker::Chunk,
    position: usize,
    entity_id: &str,
    content_hash: &str,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO chunks (doc_id, chunk_text, position, start_line, end_line, symbol_name, strategy, defined_symbol, entity_id, content_hash)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        rusqlite::params![
            doc_id,
            chunk.text,
            position as i64,
            chunk.start_line as i64,
            chunk.end_line as i64,
            chunk.symbol_name,
            chunk.strategy.as_db_str(),
            chunk.defined_symbol,
            entity_id,
            content_hash,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn insert_embedding(conn: &Connection, chunk_id: i64, vector: &[f32]) -> Result<()> {
    let bytes: Vec<u8> = vector.iter().flat_map(|f| f.to_le_bytes()).collect();
    conn.execute(
        "INSERT INTO embeddings (chunk_id, vector) VALUES (?1, ?2)",
        rusqlite::params![chunk_id, bytes],
    )?;
    Ok(())
}

pub fn mark_document_indexed(conn: &Connection, doc_id: i64) -> Result<()> {
    conn.execute(
        "UPDATE documents SET status = 'indexed', last_indexed = unixepoch() WHERE id = ?1",
        [doc_id],
    )?;
    Ok(())
}

pub fn mark_document_error(conn: &Connection, doc_id: i64) -> Result<()> {
    conn.execute(
        "UPDATE documents SET status = 'error' WHERE id = ?1",
        [doc_id],
    )?;
    Ok(())
}

pub fn delete_document(conn: &Connection, path: &str) -> Result<()> {
    conn.execute("DELETE FROM documents WHERE path = ?1", [path])?;
    Ok(())
}

pub fn count_indexed_documents(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM documents WHERE status = 'indexed'",
        [],
        |r| r.get(0),
    )?)
}

pub fn count_pending_documents(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM documents WHERE status = 'pending'",
        [],
        |r| r.get(0),
    )?)
}

pub fn clear_vault_tables(conn: &mut Connection) -> anyhow::Result<()> {
    let tx = conn.transaction()?;
    tx.execute_batch(
        "DELETE FROM curated_relationships;
         DELETE FROM embeddings;
         DELETE FROM chunks;
         DELETE FROM documents;
         DELETE FROM wiki_pages;
         DELETE FROM folder_rules;",
    )?;
    tx.commit()?;
    Ok(())
}

pub fn delete_stale_relationships(
    conn: &Connection,
    entity_id: &str,
    since_epoch: i64,
) -> Result<()> {
    // Only purge outgoing (from_id) edges; ON DELETE CASCADE handles to_id cleanup
    // when definition chunks are later removed.
    conn.execute(
        "DELETE FROM curated_relationships
         WHERE entity_id = ?1
           AND from_id IN (
               SELECT c.id FROM chunks c
               JOIN documents d ON d.id = c.doc_id
               WHERE d.last_indexed >= ?2
                 AND c.entity_id = ?1
           )",
        rusqlite::params![entity_id, since_epoch],
    )?;
    Ok(())
}

/// Insert a relationship edge between two chunks.
pub fn insert_relationship(
    conn: &Connection,
    from_id: i64,
    to_id: i64,
    rel_type: &str,
    symbol: &str,
    entity_id: &str,
) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO curated_relationships (from_id, to_id, rel_type, symbol, entity_id)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![from_id, to_id, rel_type, symbol, entity_id],
    )?;
    Ok(())
}

/// Delete relationships where chunk_id appears as from_id or to_id (orphan cleanup for runHeal).
pub fn delete_relationships_for_chunk(conn: &Connection, chunk_id: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM curated_relationships WHERE from_id = ?1 OR to_id = ?1",
        [chunk_id],
    )?;
    Ok(())
}

/// Resolve a (path, content_hash) to the matching chunk's line range.
/// Returns `Ok(None)` if either the path or hash doesn't match.
pub fn find_chunk_overlay(
    conn: &Connection,
    path: &str,
    hash: &str,
) -> Result<Option<(u32, u32)>> {
    let row: Option<(i64,)> = conn
        .query_row(
            "SELECT c.id FROM chunks c
             JOIN documents d ON d.id = c.doc_id
             WHERE d.path = ?1 AND c.content_hash = ?2
             LIMIT 1",
            rusqlite::params![path, hash],
            |r| Ok((r.get::<_, i64>(0)?,)),
        )
        .optional()?;
    let Some((chunk_id,)) = row else {
        return Ok(None);
    };
    let (start, end): (i64, i64) = conn.query_row(
        "SELECT start_line, end_line FROM chunks WHERE id = ?1",
        [chunk_id],
        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
    )?;
    Ok(Some((start as u32, end as u32)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunker::ChunkStrategyTag;
    use crate::db::connection::open_in_memory;

    #[test]
    fn test_upsert_document_creates_and_updates() {
        let conn = open_in_memory().unwrap();
        let id1 = upsert_document(&conn, "/docs/note.md", "abc123").unwrap();
        let id2 = upsert_document(&conn, "/docs/note.md", "def456").unwrap();
        assert_eq!(id1, id2, "upsert must return same id");
        let doc = get_document_by_path(&conn, "/docs/note.md")
            .unwrap()
            .unwrap();
        assert_eq!(doc.hash, "def456");
    }

    #[test]
    fn test_insert_chunk_and_embedding() {
        let conn = open_in_memory().unwrap();
        let doc_id = upsert_document(&conn, "/docs/a.md", "hash1").unwrap();
        let chunk = crate::chunker::Chunk {
            text: "hello world".into(),
            start_line: 1,
            end_line: 1,
            symbol_name: None,
            defined_symbol: None,
            strategy: ChunkStrategyTag::Prose,
        };
        let chunk_id = insert_chunk(&conn, doc_id, &chunk, 0, "tier_fact", "").unwrap();
        insert_embedding(&conn, chunk_id, &[0.1_f32, 0.2, 0.3]).unwrap();

        let bytes: Vec<u8> = conn
            .query_row(
                "SELECT vector FROM embeddings WHERE chunk_id = ?1",
                [chunk_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(bytes.len(), 12); // 3 × 4 bytes
    }

    #[test]
    fn test_delete_document_cascades() {
        let conn = open_in_memory().unwrap();
        let doc_id = upsert_document(&conn, "/docs/b.md", "hash2").unwrap();
        let chunk = crate::chunker::Chunk {
            text: "text".into(),
            start_line: 2,
            end_line: 2,
            symbol_name: Some("sym".into()),
            defined_symbol: None,
            strategy: ChunkStrategyTag::Scanner,
        };
        let chunk_id = insert_chunk(&conn, doc_id, &chunk, 0, "tier_fact", "").unwrap();
        insert_embedding(&conn, chunk_id, &[1.0_f32]).unwrap();
        delete_document(&conn, "/docs/b.md").unwrap();

        let doc = get_document_by_path(&conn, "/docs/b.md").unwrap();
        assert!(doc.is_none());
        let emb_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM embeddings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(emb_count, 0);
    }

    #[test]
    fn list_indexed_user_doc_paths_orders_results() {
        let conn = open_in_memory().unwrap();
        let id_a = upsert_document(&conn, "/documents/a.md", "ha").unwrap();
        let id_b = upsert_document(&conn, "/documents/b.md", "hb").unwrap();
        mark_document_indexed(&conn, id_b).unwrap();
        mark_document_indexed(&conn, id_a).unwrap();
        assert_eq!(
            list_indexed_user_doc_paths(&conn).unwrap(),
            vec!["/documents/a.md", "/documents/b.md"],
        );
    }

    #[test]
    fn test_count_documents() {
        let conn = open_in_memory().unwrap();
        let id = upsert_document(&conn, "/docs/c.md", "hash3").unwrap();
        assert_eq!(count_pending_documents(&conn).unwrap(), 1);
        mark_document_indexed(&conn, id).unwrap();
        assert_eq!(count_indexed_documents(&conn).unwrap(), 1);
        assert_eq!(count_pending_documents(&conn).unwrap(), 0);
    }

    #[test]
    fn insert_chunk_persists_metadata_columns() {
        let conn = open_in_memory().unwrap();
        let doc_id = upsert_document(&conn, "/docs/meta.md", "hashM").unwrap();
        let chunk = crate::chunker::Chunk {
            text: "chunk body".into(),
            start_line: 10,
            end_line: 20,
            symbol_name: Some("root_key".into()),
            defined_symbol: None,
            strategy: ChunkStrategyTag::Declarative,
        };
        insert_chunk(&conn, doc_id, &chunk, 2, "tier_fact", "").unwrap();
        let row: (String, i64, i64, i64, Option<String>, String, Option<String>) = conn
            .query_row(
                "SELECT chunk_text, position, start_line, end_line, symbol_name, strategy, entity_id FROM chunks WHERE doc_id = ?1 AND position = 2",
                [doc_id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(row.0, "chunk body");
        assert_eq!(row.1, 2);
        assert_eq!(row.2, 10);
        assert_eq!(row.3, 20);
        assert_eq!(row.4.as_deref(), Some("root_key"));
        assert_eq!(row.5, "declarative");
        assert_eq!(row.6.as_deref(), Some("tier_fact"));
    }

    #[test]
    fn find_chunk_overlay_returns_line_range_by_hash() {
        let conn = open_in_memory().unwrap();
        let doc_id = upsert_document(&conn, "/docs/a.md", "h").unwrap();
        let chunk = crate::chunker::Chunk {
            text: "body".into(),
            start_line: 7,
            end_line: 12,
            symbol_name: None,
            defined_symbol: None,
            strategy: ChunkStrategyTag::Prose,
        };
        insert_chunk(&conn, doc_id, &chunk, 0, "tier_fact", "abc").unwrap();
        let overlay = find_chunk_overlay(&conn, "/docs/a.md", "abc").unwrap();
        assert_eq!(overlay, Some((7, 12)));
    }

    #[test]
    fn find_chunk_overlay_returns_none_for_unknown_hash() {
        let conn = open_in_memory().unwrap();
        let _ = upsert_document(&conn, "/docs/a.md", "h").unwrap();
        assert_eq!(find_chunk_overlay(&conn, "/docs/a.md", "nope").unwrap(), None);
    }

    #[test]
    fn find_chunk_overlay_returns_none_for_missing_doc() {
        let conn = open_in_memory().unwrap();
        assert_eq!(find_chunk_overlay(&conn, "/nope.md", "abc").unwrap(), None);
    }
}

#[cfg(test)]
mod clear_vault_tables_tests {
    use super::*;
    use crate::db::connection::open_in_memory;

    #[test]
    fn clear_vault_tables_empties_all_vault_data() {
        let mut conn = open_in_memory().unwrap();
        upsert_document(&conn, "/test/doc.md", "abc123").unwrap();
        let doc_id: i64 = conn
            .query_row("SELECT id FROM documents LIMIT 1", [], |r| r.get(0))
            .unwrap();
        let chunk = crate::chunker::Chunk {
            text: "hello".into(),
            start_line: 1,
            end_line: 1,
            symbol_name: None,
            defined_symbol: None,
            strategy: crate::chunker::ChunkStrategyTag::Prose,
        };
        let chunk_id = insert_chunk(&conn, doc_id, &chunk, 0, "tier_fact", "").unwrap();
        insert_embedding(&conn, chunk_id, &[0.1_f32, 0.2, 0.3]).unwrap();

        conn.execute(
            "INSERT INTO folder_rules (folder_path, librarian_mode, auto_approve) VALUES ('test', 'index', 0)",
            [],
        )
        .unwrap();

        clear_vault_tables(&mut conn).unwrap();

        let doc_count: i64 = conn
            .query_row("SELECT count(*) FROM documents", [], |r| r.get(0))
            .unwrap();
        let chunk_count: i64 = conn
            .query_row("SELECT count(*) FROM chunks", [], |r| r.get(0))
            .unwrap();
        let embed_count: i64 = conn
            .query_row("SELECT count(*) FROM embeddings", [], |r| r.get(0))
            .unwrap();
        let wiki_count: i64 = conn
            .query_row("SELECT count(*) FROM wiki_pages", [], |r| r.get(0))
            .unwrap();
        let rule_count: i64 = conn
            .query_row("SELECT count(*) FROM folder_rules", [], |r| r.get(0))
            .unwrap();
        let rel_count: i64 = conn
            .query_row("SELECT count(*) FROM curated_relationships", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(doc_count, 0);
        assert_eq!(chunk_count, 0);
        assert_eq!(embed_count, 0);
        assert_eq!(wiki_count, 0);
        assert_eq!(rule_count, 0);
        assert_eq!(rel_count, 0);
    }
}

#[cfg(test)]
mod content_hash_tests {
    use super::*;
    use crate::chunker::ChunkStrategyTag;
    use crate::db::connection::open_in_memory;

    #[test]
    fn insert_chunk_persists_content_hash() {
        let conn = open_in_memory().unwrap();
        let doc_id = upsert_document(&conn, "/docs/h.md", "hashH").unwrap();
        let chunk = crate::chunker::Chunk {
            text: "body".into(),
            start_line: 1,
            end_line: 2,
            symbol_name: None,
            defined_symbol: None,
            strategy: ChunkStrategyTag::Prose,
        };
        insert_chunk(&conn, doc_id, &chunk, 0, "tier_fact", "abc123hash").unwrap();
        let row: (String, i64) = conn
            .query_row(
                "SELECT content_hash, doc_id FROM chunks WHERE doc_id = ?1",
                [doc_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(row.0, "abc123hash");
    }

    #[test]
    fn unique_index_on_doc_id_and_content_hash_rejects_duplicate() {
        let conn = open_in_memory().unwrap();
        let doc_id = upsert_document(&conn, "/docs/dup.md", "hashD").unwrap();
        let chunk = crate::chunker::Chunk {
            text: "x".into(),
            start_line: 1,
            end_line: 1,
            symbol_name: None,
            defined_symbol: None,
            strategy: ChunkStrategyTag::Prose,
        };
        insert_chunk(&conn, doc_id, &chunk, 0, "tier_fact", "dup").unwrap();
        let err = insert_chunk(&conn, doc_id, &chunk, 1, "tier_fact", "dup").unwrap_err();
        assert!(
            err.to_string().contains("UNIQUE") || err.to_string().contains("unique"),
            "expected unique-index violation, got: {err}"
        );
    }
}
