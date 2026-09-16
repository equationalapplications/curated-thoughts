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

/// Empty this vault's brain for a switch without backup restore.
///
/// The brain database is global (`~/.brain/brain.db`) but the brain is
/// **per-vault** (issue #213): a vault's knowledge is only what was created
/// while it was active. This clears the knowledge layer and the document
/// layer together, in one transaction, so vault A's entities, facts, edges,
/// tasks and agent memories cannot surface in vault B.
///
/// `llm_wiki_entries` and `llm_wiki_tasks` are replicated, so each doomed row
/// gets an `OutboxOperation::Delete` row — set-based, one statement per table,
/// rather than the per-row `hard_delete_entries` ceremony, which would cost
/// ~3 round trips per fact on the user-facing switch path. The payload is
/// `{"id"}`: this is a hard delete, matching `hard_delete_entries` and
/// `clear_entity_content`, not the archive convention's
/// `{id, entity_id, deleted_at}`.
///
/// Nothing relies on `ON DELETE CASCADE`: the caller opens a raw connection
/// that never sets `PRAGMA foreign_keys`, so cascades do not fire. Every
/// delete is explicit, children before parents.
///
/// Supersedes issue #211 spec D7, which stranded pending proposals here —
/// that decision was deferred to #213 and is now "per-vault", so proposals
/// are cleared with everything else. The destruction is confirmed in the
/// switch UI (spec D5), so it is not a silent side effect.
pub fn clear_vault_tables(conn: &mut Connection, now_ms: i64) -> anyhow::Result<()> {
    let tx = conn.transaction()?;

    // Entries: replica Deletes first (the rows must still exist to select
    // from), then their evidence, then the rows. No `deleted_at IS NULL`
    // filter — an archived fact's Insert may already have drained, so the
    // replica still needs an explicit Delete.
    tx.execute(
        "INSERT INTO llm_wiki_outbox
             (id, entity_id, table_name, record_id, operation, payload, created_at)
         SELECT 'out_' || lower(hex(randomblob(12))), entity_id, 'entries', id,
                'DELETE', json_object('id', id), ?1
         FROM llm_wiki_entries",
        [now_ms],
    )?;
    tx.execute(
        "DELETE FROM librarian_evidence
          WHERE entry_id IN (SELECT id FROM llm_wiki_entries)",
        [],
    )?;
    tx.execute("DELETE FROM llm_wiki_entries", [])?;

    // Tasks, mirrored. No evidence table.
    tx.execute(
        "INSERT INTO llm_wiki_outbox
             (id, entity_id, table_name, record_id, operation, payload, created_at)
         SELECT 'out_' || lower(hex(randomblob(12))), entity_id, 'tasks', id,
                'DELETE', json_object('id', id), ?1
         FROM llm_wiki_tasks",
        [now_ms],
    )?;
    tx.execute("DELETE FROM llm_wiki_tasks", [])?;

    // Everything else per the D2 matrix. Edges are not replicated and every
    // endpoint they could reference is doomed, so one unconditional sweep
    // empties the table. Children before parents throughout.
    tx.execute_batch(
        "DELETE FROM llm_wiki_edges;
         DELETE FROM llm_wiki_events;
         DELETE FROM llm_wiki_source_ref_index;
         DELETE FROM llm_wiki_checkpoints;
         DELETE FROM curated_agent_log;
         DELETE FROM curated_entities;
         DELETE FROM curated_proposal_items;
         DELETE FROM curated_proposal_sources;
         DELETE FROM curated_proposal_deleted_sources;
         DELETE FROM curated_proposals;
         DELETE FROM curated_relationships;
         DELETE FROM embeddings;
         DELETE FROM chunks;
         DELETE FROM ingest_runs;
         DELETE FROM documents;
         DELETE FROM wiki_pages;
         DELETE FROM folder_rules;
         DELETE FROM stall_strikes;",
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
        delete_path, deleted_source_rows, seed_pending_proposal, status_of,
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
            .query_row(
                "SELECT COUNT(*) FROM chunks WHERE doc_id = ?1",
                [doc],
                |r| r.get(0),
            )
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

    const NOW: i64 = 1_726_000_000_000;

    fn count(conn: &Connection, table: &str) -> i64 {
        conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap_or_else(|e| panic!("count {table}: {e}"))
    }

    /// Seed one row in every table the D2 matrix marks "clear", plus the
    /// keep-row tables, plus a pre-existing undrained outbox Insert.
    /// Includes ARCHIVED (soft-deleted) entry and task rows: the ceremony
    /// must NOT filter on `deleted_at IS NULL` — an archived fact's Insert
    /// may have drained long ago, so its replica still needs a Delete.
    fn seed_full_vault(conn: &Connection) -> i64 {
        upsert_document(conn, "/test/doc.md", "abc123").unwrap();
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
        let chunk_id = insert_chunk(conn, doc_id, &chunk, 0, "tier_fact", "").unwrap();
        insert_embedding(conn, chunk_id, &[0.1_f32, 0.2, 0.3]).unwrap();

        conn.execute(
            "INSERT INTO ingest_runs (doc_id, run_at, outcome) VALUES (?1, 1, 'indexed')",
            [doc_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO folder_rules (folder_path, librarian_mode, auto_approve)
             VALUES ('test', 'index', 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO stall_strikes (path, strikes, last_ms) VALUES ('notes/a.md', 2, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO curated_entities (id, name, created_at, updated_at)
             VALUES ('ent_a', 'Entity A', 1, 1)",
            [],
        )
        .unwrap();

        // Two live facts + one ARCHIVED fact, each with evidence.
        for (id, deleted_at) in [
            ("fact_live_1", None::<i64>),
            ("fact_live_2", None),
            ("fact_archived", Some(NOW - 1)),
        ] {
            conn.execute(
                "INSERT INTO llm_wiki_entries
                     (id, entity_id, title, body, created_at, updated_at, deleted_at)
                 VALUES (?1, 'ent_a', 'T', 'B', 1, 1, ?2)",
                rusqlite::params![id, deleted_at],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO librarian_evidence (entry_id, proposal_id, evidence_json, created_at)
                 VALUES (?1, 'prop-1', '{}', 1)",
                [id],
            )
            .unwrap();
        }

        // One live task + one ARCHIVED task.
        for (id, deleted_at) in [("task_live", None::<i64>), ("task_archived", Some(NOW - 1))] {
            conn.execute(
                "INSERT INTO llm_wiki_tasks (id, entity_id, description, created_at, updated_at, deleted_at)
                 VALUES (?1, 'ent_a', 'do a thing', 1, 1, ?2)",
                rusqlite::params![id, deleted_at],
            )
            .unwrap();
        }

        conn.execute(
            // `created_at` is a millisecond value (>= SEC_VS_MS_THRESHOLD) per
            // the edge-integrity spec §2.4: a seconds-scale fixture would mask
            // a unit regression in a production writer.
            "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
             VALUES ('edge_1', 'ent_a', 'fact_live_1', 'fact_live_2', 'relates_to', ?1)",
            [NOW],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_events (id, entity_id, event_type, summary, created_at)
             VALUES ('evt_1', 'ent_a', 'action', 'did a thing', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_source_ref_index (id, entity_id, source_hash, source_ref, created_at)
             VALUES ('sri_1', 'ent_a', 'hash1', 'ref1', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_checkpoints (entity_id, heal_checkpoint, memory_checkpoint)
             VALUES ('ent_a', 5, 5)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO curated_agent_log (client, tool, operation, entity_id, summary)
             VALUES ('claude', 'wiki_add_fact', 'write', 'ent_a', 'agent wrote a fact')",
            [],
        )
        .unwrap();

        use crate::db::proposals::test_support::seed_pending_proposal;
        use crate::db::proposals::ProposalSourceRole::Trigger;
        seed_pending_proposal(conn, "prop-switch-1", &[(doc_id, Trigger)]);

        // Keep-rows.
        conn.execute(
            "INSERT INTO llm_wiki_meta (key, value) VALUES ('okf_migrated_at', '123')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_entity_manifests (entity_id, mode, updated_at)
             VALUES ('tier_fact', 'strict', 1)",
            [],
        )
        .unwrap();

        // A pre-existing undrained outbox Insert: must survive untouched and
        // stay AHEAD of the Deletes this clear pushes.
        conn.execute(
            "INSERT INTO llm_wiki_outbox
                 (id, entity_id, table_name, record_id, operation, payload, created_at)
             VALUES ('out_preexisting000000000001', 'ent_a', 'entries', 'fact_live_1',
                     'INSERT', '{\"id\":\"fact_live_1\"}', 1)",
            [],
        )
        .unwrap();

        doc_id
    }

    #[test]
    fn clear_vault_tables_empties_every_clear_row_in_the_d2_matrix() {
        let mut conn = open_in_memory().unwrap();
        seed_full_vault(&conn);

        clear_vault_tables(&mut conn, NOW).unwrap();

        for table in [
            "documents",
            "chunks",
            "embeddings",
            "curated_relationships",
            "wiki_pages",
            "folder_rules",
            "ingest_runs",
            "stall_strikes",
            "curated_entities",
            "llm_wiki_entries",
            "librarian_evidence",
            "llm_wiki_tasks",
            "llm_wiki_edges",
            "llm_wiki_events",
            "llm_wiki_source_ref_index",
            "llm_wiki_checkpoints",
            "curated_agent_log",
            "curated_proposals",
            "curated_proposal_items",
            "curated_proposal_sources",
            "curated_proposal_deleted_sources",
        ] {
            assert_eq!(
                count(&conn, table),
                0,
                "{table} must be empty after the clear"
            );
        }

        // Keep-rows survive.
        assert_eq!(count(&conn, "llm_wiki_meta"), 1, "meta marker must survive");
        assert_eq!(
            count(&conn, "llm_wiki_entity_manifests"),
            1,
            "ontology manifests must survive"
        );
        assert!(
            count(&conn, "schema_version") > 0,
            "schema watermark must survive"
        );
    }

    #[test]
    fn clear_vault_tables_pushes_one_delete_per_entry_and_task_including_archived() {
        let mut conn = open_in_memory().unwrap();
        seed_full_vault(&conn);

        clear_vault_tables(&mut conn, NOW).unwrap();

        let mut stmt = conn
            .prepare(
                "SELECT id, entity_id, table_name, record_id, operation, payload, created_at
                   FROM llm_wiki_outbox ORDER BY created_at ASC, rowid ASC",
            )
            .unwrap();
        let rows: Vec<(String, String, String, String, String, String, i64)> = stmt
            .query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();

        // Pre-existing undrained Insert stays first and untouched.
        assert_eq!(rows[0].0, "out_preexisting000000000001");
        assert_eq!(rows[0].4, "INSERT");

        let deletes: Vec<_> = rows.iter().skip(1).collect();
        assert_eq!(
            deletes.len(),
            5,
            "3 entries (incl. archived) + 2 tasks (incl. archived)"
        );

        let entry_records: std::collections::BTreeSet<&str> = deletes
            .iter()
            .filter(|r| r.2 == "entries")
            .map(|r| r.3.as_str())
            .collect();
        assert_eq!(
            entry_records,
            ["fact_archived", "fact_live_1", "fact_live_2"]
                .into_iter()
                .collect(),
            "archived entries need Deletes too — their Insert may have drained"
        );

        let task_records: std::collections::BTreeSet<&str> = deletes
            .iter()
            .filter(|r| r.2 == "tasks")
            .map(|r| r.3.as_str())
            .collect();
        assert_eq!(
            task_records,
            ["task_archived", "task_live"].into_iter().collect(),
            "archived tasks need Deletes too"
        );

        for d in &deletes {
            assert_eq!(d.4, "DELETE");
            assert_eq!(d.1, "ent_a", "outbox is keyed on the row's own entity");
            assert_eq!(d.6, NOW, "created_at must be the passed now_ms");
            assert_eq!(
                d.5,
                format!("{{\"id\":\"{}\"}}", d.3),
                "hard-delete payload is {{\"id\"}} only (spec D1.2)"
            );
            // Format must match generate_outbox_id(): out_ + 24 lowercase hex.
            assert!(d.0.starts_with("out_"), "id {} must start with out_", d.0);
            assert_eq!(d.0.len(), 28, "out_ + 24 hex chars");
            assert!(
                d.0[4..]
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
                "id {} must be lowercase hex",
                d.0
            );
        }
    }

    #[test]
    fn clear_vault_tables_rolls_back_leaving_no_outbox_rows() {
        let mut conn = open_in_memory().unwrap();
        seed_full_vault(&conn);
        // Induce a mid-ceremony failure AFTER the outbox inserts: the
        // straight-delete batch names this table, so the statement errors.
        conn.execute_batch("DROP TABLE folder_rules;").unwrap();

        let err = clear_vault_tables(&mut conn, NOW);
        assert!(err.is_err(), "the clear must fail, not silently skip");

        assert_eq!(
            count(&conn, "llm_wiki_outbox"),
            1,
            "rollback must leave only the pre-existing row — zero new Deletes"
        );
        assert_eq!(
            count(&conn, "llm_wiki_entries"),
            3,
            "rollback must restore the entries"
        );
    }

    #[test]
    fn clear_vault_tables_is_idempotent() {
        let mut conn = open_in_memory().unwrap();
        seed_full_vault(&conn);

        clear_vault_tables(&mut conn, NOW).unwrap();
        let after_first = count(&conn, "llm_wiki_outbox");
        clear_vault_tables(&mut conn, NOW).unwrap();

        assert_eq!(
            count(&conn, "llm_wiki_outbox"),
            after_first,
            "second clear has nothing to delete, so pushes no new rows"
        );
        assert_eq!(count(&conn, "llm_wiki_entries"), 0);
    }

    #[test]
    fn clear_vault_tables_on_empty_brain_writes_no_outbox_rows() {
        let mut conn = open_in_memory().unwrap();

        clear_vault_tables(&mut conn, NOW).unwrap();

        assert_eq!(count(&conn, "llm_wiki_outbox"), 0);
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
