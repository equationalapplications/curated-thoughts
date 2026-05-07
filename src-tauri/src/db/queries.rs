use anyhow::Result;
use rusqlite::Connection;

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
    Ok(conn.query_row(
        "SELECT id FROM documents WHERE path = ?1",
        [path],
        |r| r.get(0),
    )?)
}

pub fn get_document_by_path(conn: &Connection, path: &str) -> Result<Option<DocRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, hash, status FROM documents WHERE path = ?1",
    )?;
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

pub fn delete_document_chunks(conn: &Connection, doc_id: i64) -> Result<()> {
    conn.execute("DELETE FROM chunks WHERE doc_id = ?1", [doc_id])?;
    Ok(())
}

pub fn insert_chunk(
    conn: &Connection,
    doc_id: i64,
    chunk: &crate::chunker::Chunk,
    position: usize,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO chunks (doc_id, chunk_text, position, start_line, end_line, symbol_name, strategy)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            doc_id,
            chunk.text,
            position as i64,
            chunk.start_line as i64,
            chunk.end_line as i64,
            chunk.symbol_name,
            chunk.strategy.as_db_str(),
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn insert_embedding(conn: &Connection, chunk_id: i64, vector: &[f32]) -> Result<()> {
    let bytes: Vec<u8> = vector
        .iter()
        .flat_map(|f| f.to_le_bytes())
        .collect();
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
        let doc = get_document_by_path(&conn, "/docs/note.md").unwrap().unwrap();
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
            strategy: ChunkStrategyTag::Prose,
        };
        let chunk_id = insert_chunk(&conn, doc_id, &chunk, 0).unwrap();
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
            strategy: ChunkStrategyTag::Scanner,
        };
        let chunk_id = insert_chunk(&conn, doc_id, &chunk, 0).unwrap();
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
            strategy: ChunkStrategyTag::Declarative,
        };
        insert_chunk(&conn, doc_id, &chunk, 2).unwrap();
        let row: (String, i64, i64, i64, Option<String>, String) = conn
            .query_row(
                "SELECT chunk_text, position, start_line, end_line, symbol_name, strategy FROM chunks WHERE doc_id = ?1 AND position = 2",
                [doc_id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
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
    }
}
