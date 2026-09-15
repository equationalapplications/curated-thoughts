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
    // Preserve per-doc ingest history (documents.last_indexed is overwritten
    // on every re-ingest; this table keeps one row per attempt).
    let _ = conn.execute(
        "INSERT INTO ingest_runs (doc_id, outcome) VALUES (?1, 'indexed')",
        [doc_id],
    );
    Ok(())
}

pub fn mark_document_error(conn: &Connection, doc_id: i64) -> Result<()> {
    conn.execute(
        "UPDATE documents SET status = 'error' WHERE id = ?1",
        [doc_id],
    )?;
    let _ = conn.execute(
        "INSERT INTO ingest_runs (doc_id, outcome) VALUES (?1, 'error')",
        [doc_id],
    );
    Ok(())
}

/// Record the sources a document deletion is about to cascade away, for
/// PENDING proposals only (issue #211 spec D2/D3). `$filter` is appended to
/// the WHERE clause: `" AND d.path = ?1"` for one document, `""` for all.
///
/// At most one row per `(proposal_id, doc_path)` is selected because
/// `documents.path` is UNIQUE and `curated_proposal_sources` is keyed
/// `(proposal_id, doc_id)`. If either constraint is ever relaxed, this
/// upsert must aggregate first or SQLite rejects the double update.
///
/// A macro, not `format!`, so both statements stay compile-time literals and
/// the path-filtered form keeps using the `documents.path` index.
macro_rules! record_deleted_sources_sql {
    ($filter:literal) => {
        concat!(
            "INSERT INTO curated_proposal_deleted_sources
                 (proposal_id, doc_path, doc_hash, role, deleted_at)
             SELECT s.proposal_id, d.path, d.hash, s.role, unixepoch()
               FROM curated_proposal_sources s
               JOIN documents d         ON d.id = s.doc_id
               JOIN curated_proposals p ON p.id = s.proposal_id
              WHERE p.status = 'pending'",
            $filter,
            "
             ON CONFLICT(proposal_id, doc_path) DO UPDATE SET
                 doc_hash = excluded.doc_hash,
                 role = excluded.role,
                 deleted_at = excluded.deleted_at"
        )
    };
}

/// Delete one document row, first recording it as a deleted source on every
/// pending proposal that cites it (issue #211 spec D3).
///
/// Takes `&Transaction` so the record and the delete commit or roll back
/// together: a record for a document that still exists is a false flag, a
/// delete with no record loses provenance. Returns the number of `documents`
/// rows deleted (0 or 1).
pub fn delete_document(tx: &rusqlite::Transaction<'_>, path: &str) -> Result<usize> {
    tx.execute(record_deleted_sources_sql!(" AND d.path = ?1"), [path])?;
    Ok(tx.execute("DELETE FROM documents WHERE path = ?1", [path])?)
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
pub fn find_chunk_overlay(conn: &Connection, path: &str, hash: &str) -> Result<Option<(u32, u32)>> {
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

/// Fetch the raw text of the chunk matching (`path`, `content_hash`).
/// Returns `Ok(None)` when either the path or hash doesn't resolve —
/// "source moved", which callers surface distinctly from backend errors.
pub fn find_chunk_text(conn: &Connection, path: &str, hash: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT c.chunk_text
             FROM chunks c JOIN documents d ON d.id = c.doc_id
             WHERE d.path = ?1 AND c.content_hash = ?2
             LIMIT 1",
            rusqlite::params![path, hash],
            |r| r.get(0),
        )
        .optional()?)
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
        let mut conn = open_in_memory().unwrap();
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
        let tx = conn.transaction().unwrap();
        delete_document(&tx, "/docs/b.md").unwrap();
        tx.commit().unwrap();

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
        assert_eq!(
            find_chunk_overlay(&conn, "/docs/a.md", "nope").unwrap(),
            None
        );
    }

    #[test]
    fn find_chunk_overlay_returns_none_for_missing_doc() {
        let conn = open_in_memory().unwrap();
        assert_eq!(find_chunk_overlay(&conn, "/nope.md", "abc").unwrap(), None);
    }

    #[test]
    fn find_chunk_text_returns_text_by_path_and_hash() {
        let conn = open_in_memory().unwrap();
        let doc_id = upsert_document(&conn, "/docs/a.md", "h").unwrap();
        let chunk = crate::chunker::Chunk {
            text: "the passage".into(),
            start_line: 7,
            end_line: 12,
            symbol_name: None,
            defined_symbol: None,
            strategy: ChunkStrategyTag::Prose,
        };
        insert_chunk(&conn, doc_id, &chunk, 0, "tier_fact", "abc").unwrap();
        let text = find_chunk_text(&conn, "/docs/a.md", "abc").unwrap();
        assert_eq!(text.as_deref(), Some("the passage"));
    }

    #[test]
    fn find_chunk_text_returns_none_for_unknown_hash() {
        let conn = open_in_memory().unwrap();
        let _ = upsert_document(&conn, "/docs/a.md", "h").unwrap();
        assert_eq!(find_chunk_text(&conn, "/docs/a.md", "nope").unwrap(), None);
    }

    #[test]
    fn find_chunk_text_returns_none_for_missing_doc() {
        let conn = open_in_memory().unwrap();
        assert_eq!(find_chunk_text(&conn, "/nope.md", "abc").unwrap(), None);
    }
}

#[cfg(test)]
mod deletion_provenance_tests {
    use super::*;
    use crate::chunker::{Chunk, ChunkStrategyTag};
    use crate::db::connection::open_in_memory;
    use crate::db::proposals::test_support::{
        deleted_source_rows, delete_path, seed_pending_proposal, status_of,
    };
    use crate::db::proposals::ProposalSourceRole::{Evidence, Trigger};

    fn live_source_count(conn: &Connection, proposal_id: &str) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM curated_proposal_sources WHERE proposal_id = ?1",
            [proposal_id],
            |r| r.get(0),
        )
        .unwrap()
    }

    /// Spec test 1.
    #[test]
    fn single_source_strand_keeps_pending_and_records_the_trigger() {
        let mut conn = open_in_memory().unwrap();
        let doc = upsert_document(&conn, "/vault/a.md", "h-a").unwrap();
        let chunk = Chunk {
            text: "x".into(),
            start_line: 1,
            end_line: 1,
            symbol_name: None,
            defined_symbol: None,
            strategy: ChunkStrategyTag::Prose,
        };
        insert_chunk(&conn, doc, &chunk, 0, "tier_fact", "").unwrap();
        seed_pending_proposal(&conn, "p1", &[(doc, Trigger)]);

        assert_eq!(delete_path(&mut conn, "/vault/a.md"), 1);

        assert_eq!(status_of(&conn, "p1"), "pending");
        assert_eq!(live_source_count(&conn, "p1"), 0);
        assert_eq!(
            deleted_source_rows(&conn, "p1"),
            vec![("/vault/a.md".into(), "h-a".into(), "trigger".into())]
        );
        let chunks: i64 = conn
            .query_row("SELECT COUNT(*) FROM chunks WHERE doc_id = ?1", [doc], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(chunks, 0, "cascade must still remove chunks");
    }

    /// Spec test 2.
    #[test]
    fn partial_strand_records_only_the_deleted_evidence() {
        let mut conn = open_in_memory().unwrap();
        let trigger = upsert_document(&conn, "/vault/t.md", "h-t").unwrap();
        let evidence = upsert_document(&conn, "/vault/e.md", "h-e").unwrap();
        seed_pending_proposal(&conn, "p2", &[(trigger, Trigger), (evidence, Evidence)]);

        delete_path(&mut conn, "/vault/e.md");

        assert_eq!(live_source_count(&conn, "p2"), 1);
        assert_eq!(
            deleted_source_rows(&conn, "p2"),
            vec![("/vault/e.md".into(), "h-e".into(), "evidence".into())]
        );
    }

    /// Spec test 3.
    #[test]
    fn non_pending_proposals_get_no_record() {
        let mut conn = open_in_memory().unwrap();
        let doc = upsert_document(&conn, "/vault/hist.md", "h-hist").unwrap();
        for (id, status) in [
            ("p-approved", "approved"),
            ("p-rejected", "rejected"),
            ("p-superseded", "superseded"),
        ] {
            seed_pending_proposal(&conn, id, &[(doc, Trigger)]);
            conn.execute(
                "UPDATE curated_proposals SET status = ?1 WHERE id = ?2",
                rusqlite::params![status, id],
            )
            .unwrap();
        }

        delete_path(&mut conn, "/vault/hist.md");

        for id in ["p-approved", "p-rejected", "p-superseded"] {
            assert!(
                deleted_source_rows(&conn, id).is_empty(),
                "{id} is historical and must not be flagged"
            );
        }
    }

    /// Spec test 4.
    #[test]
    fn dropped_transaction_rolls_back_both_statements() {
        let mut conn = open_in_memory().unwrap();
        let doc = upsert_document(&conn, "/vault/atomic.md", "h-atomic").unwrap();
        seed_pending_proposal(&conn, "p4", &[(doc, Trigger)]);

        {
            let tx = conn.transaction().unwrap();
            assert_eq!(delete_document(&tx, "/vault/atomic.md").unwrap(), 1);
            // Dropped without commit: rusqlite rolls back.
        }

        assert!(get_document_by_path(&conn, "/vault/atomic.md")
            .unwrap()
            .is_some());
        assert!(deleted_source_rows(&conn, "p4").is_empty());
    }

    /// Spec test 5.
    #[test]
    fn re_deleting_a_recreated_path_upserts_one_row_with_the_new_hash() {
        let mut conn = open_in_memory().unwrap();
        let first = upsert_document(&conn, "/vault/again.md", "h-1").unwrap();
        seed_pending_proposal(&conn, "p5", &[(first, Trigger)]);
        delete_path(&mut conn, "/vault/again.md");

        let second = upsert_document(&conn, "/vault/again.md", "h-2").unwrap();
        conn.execute(
            "INSERT INTO curated_proposal_sources (proposal_id, doc_id, role)
             VALUES ('p5', ?1, 'trigger')",
            [second],
        )
        .unwrap();
        delete_path(&mut conn, "/vault/again.md");

        assert_eq!(
            deleted_source_rows(&conn, "p5"),
            vec![("/vault/again.md".into(), "h-2".into(), "trigger".into())]
        );
    }

    #[test]
    fn deleting_an_unknown_path_is_a_zero_count_no_op() {
        let mut conn = open_in_memory().unwrap();
        assert_eq!(delete_path(&mut conn, "/vault/never.md"), 0);
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
