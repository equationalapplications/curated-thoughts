//! One-time migration from rowid-based chunk ids to content-derived hashes.
//!
//! Re-chunks every document and populates `chunks.content_hash`. Rewrites
//! `llm_wiki_entries.source_ref` JSON to carry `content_hash` instead of
//! `chunk_id` in each evidence entry. Preserves `embeddings` and
//! `curated_relationships` rows by remapping their `chunk_id` / `from_id` /
//! `to_id` foreign keys from the legacy rowids to the freshly-issued rowids.
//! Idempotent — re-runs are no-ops once every chunk has a non-empty hash.
//! Wrapped in a single transaction; on failure the corpus is unchanged.

use crate::chunker::{chunk_autodetect, Chunk};
use crate::db::chunk_hash::compute_chunk_hash;
use crate::db::queries::upsert_document;
use crate::pipeline::entity_id_for_path;
use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Copy)]
pub struct MigrationProgress {
    pub current: usize,
    pub total: usize,
    pub phase: &'static str,
}

/// True iff every row in `chunks` has a non-empty `content_hash`.
pub fn chunks_have_content_hash(conn: &Connection) -> Result<bool> {
    let empty_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM chunks WHERE content_hash = '' OR content_hash IS NULL",
            [],
            |r| r.get(0),
        )
        .context("counting un-hashed chunks")?;
    Ok(empty_count == 0)
}

/// One-time content_hash migration. See module docs.
pub fn run_chunk_hash_migration(
    conn: &mut Connection,
    emit: impl Fn(MigrationProgress),
) -> Result<()> {
    let tx = conn.transaction()?;

    // Phase 0: snapshot every legacy chunk rowid, every embedding, and every
    // curated_relationship edge that touches those chunks. The FK CASCADE on
    // `chunks.id` (see schema.rs:53,106-107) will DELETE these rows when we
    // DELETE chunks per-doc; we must capture them first and re-INSERT after
    // the new chunks are issued.
    let all_legacy_rowids: Vec<i64> = {
        let mut stmt = tx.prepare("SELECT id FROM chunks")?;
        let rows = stmt
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };

    let legacy_embeddings: Vec<(i64, Vec<u8>)> = capture_embeddings(&tx, &all_legacy_rowids)?;
    let legacy_relationships: Vec<(i64, i64, String, String, String, i64)> =
        capture_relationships(&tx, &all_legacy_rowids)?;

    // Phase 1: enumerate documents.
    let docs: Vec<(i64, String)> = {
        let mut stmt = tx.prepare("SELECT id, path FROM documents ORDER BY id")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    let total = docs.len();
    emit(MigrationProgress { current: 0, total, phase: "rechunk" });

    // Map legacy chunk_id (rowid) -> new chunk rowid (re-issued by SQLite).
    // Built up per-doc as we INSERT the new chunks.
    let mut legacy_rowid_to_new_rowid: HashMap<i64, i64> = HashMap::new();
    // Map legacy chunk_id (rowid) -> content_hash, used by the source_ref
    // rewrite phase. Built up alongside the rowid map.
    let mut legacy_rowid_to_hash: HashMap<i64, String> = HashMap::new();

    for (idx, (doc_id, path)) in docs.iter().enumerate() {
        let text = std::fs::read_to_string(Path::new(path))
            .with_context(|| format!("read {path} for chunk-hash migration"))?;
        let chunks: Vec<Chunk> = chunk_autodetect(Path::new(path), &text);

        // Capture legacy rowids in insertion (position) order before
        // deleting the doc's chunks.
        let legacy_rowids_by_position: Vec<i64> = {
            let mut stmt = tx.prepare(
                "SELECT id FROM chunks WHERE doc_id = ?1 ORDER BY position",
            )?;
            let collected = stmt
                .query_map([doc_id], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            drop(stmt);
            collected
        };

        tx.execute("DELETE FROM chunks WHERE doc_id = ?1", [doc_id])?;
        for (i, chunk) in chunks.iter().enumerate() {
            let hash = compute_chunk_hash(&chunk.text, path, i);
            tx.execute(
                "INSERT INTO chunks (doc_id, chunk_text, position, start_line, end_line,
                                     symbol_name, strategy, defined_symbol, entity_id,
                                     content_hash)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    doc_id,
                    chunk.text,
                    i as i64,
                    chunk.start_line as i64,
                    chunk.end_line as i64,
                    chunk.symbol_name,
                    chunk.strategy.as_db_str(),
                    chunk.defined_symbol,
                    entity_id_for_path(path, None),
                    hash,
                ],
            )?;
            let new_chunk_id = tx.last_insert_rowid();
            // Legacy rowid at position i maps to the new rowid + content_hash.
            // If the legacy chunk count differs from the new count, the
            // tail of legacy rowids has no mapping and the rewrite phase
            // drops the orphan evidence entries.
            if let Some(&legacy_rowid) = legacy_rowids_by_position.get(i) {
                legacy_rowid_to_new_rowid.insert(legacy_rowid, new_chunk_id);
                legacy_rowid_to_hash.insert(legacy_rowid, hash);
            }
        }

        emit(MigrationProgress {
            current: idx + 1,
            total,
            phase: "rechunk",
        });
    }

    // Phase 1.5: re-INSERT captured embeddings using the new rowid map.
    // Embeddings whose chunk_id had no mapping are dropped (the corresponding
    // chunk did not survive re-chunking — text changed or was removed).
    for (old_chunk_id, vector) in &legacy_embeddings {
        if let Some(&new_chunk_id) = legacy_rowid_to_new_rowid.get(old_chunk_id) {
            tx.execute(
                "INSERT INTO embeddings (chunk_id, vector) VALUES (?1, ?2)",
                params![new_chunk_id, vector],
            )?;
        }
    }

    // Phase 1.6: re-INSERT captured curated_relationships using the new
    // rowid map. Edges whose from_id OR to_id has no mapping are dropped
    // (chunk did not survive re-chunking).
    for (from_id, to_id, rel_type, symbol, entity_id, created_at) in &legacy_relationships {
        let new_from = legacy_rowid_to_new_rowid.get(from_id).copied();
        let new_to = legacy_rowid_to_new_rowid.get(to_id).copied();
        if let (Some(nf), Some(nt)) = (new_from, new_to) {
            tx.execute(
                "INSERT INTO curated_relationships
                    (from_id, to_id, rel_type, symbol, entity_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![nf, nt, rel_type, symbol, entity_id, created_at],
            )?;
        }
    }

    // Phase 2: rewrite every llm_wiki_entries.source_ref JSON in place.
    emit(MigrationProgress { current: 0, total, phase: "rewrite" });
    let entries: Vec<(String, Option<String>)> = {
        let mut stmt = tx.prepare(
            "SELECT id, source_ref FROM llm_wiki_entries WHERE source_ref IS NOT NULL",
        )?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    for (idx, (entry_id, raw)) in entries.iter().enumerate() {
        let Some(raw) = raw else { continue };
        let Ok(mut value) = serde_json::from_str::<Value>(raw) else { continue };
        let Some(evidence) = value.get_mut("evidence").and_then(|v| v.as_array_mut()) else {
            continue;
        };
        let mut changed = false;
        for entry in evidence.iter_mut() {
            let Some(obj) = entry.as_object_mut() else { continue };
            let Some(chunk_id) = obj.get("chunk_id").and_then(|v| v.as_i64()) else { continue };
            match legacy_rowid_to_hash.get(&chunk_id) {
                Some(hash) if !hash.is_empty() => {
                    obj.insert("content_hash".into(), Value::String(hash.clone()));
                    obj.remove("chunk_id");
                    changed = true;
                }
                _ => {
                    // Legacy chunk id didn't survive the re-chunk
                    // (text changed, or doc deleted between runs).
                    // Write an empty content_hash so the read path
                    // skips the entry rather than dereferencing a
                    // missing row.
                    obj.insert("content_hash".into(), Value::String(String::new()));
                    obj.remove("chunk_id");
                    changed = true;
                }
            }
        }
        if changed {
            tx.execute(
                "UPDATE llm_wiki_entries SET source_ref = ?1 WHERE id = ?2",
                params![serde_json::to_string(&value)?, entry_id],
            )?;
        }
        emit(MigrationProgress {
            current: idx + 1,
            total: entries.len(),
            phase: "rewrite",
        });
    }

    tx.commit()?;
    Ok(())
}

/// Capture `(chunk_id, vector)` for every embedding whose chunk_id is in the
/// given legacy-rowid set. Returns an empty vec when the set is empty.
fn capture_embeddings(
    tx: &rusqlite::Transaction<'_>,
    legacy_rowids: &[i64],
) -> Result<Vec<(i64, Vec<u8>)>> {
    if legacy_rowids.is_empty() {
        return Ok(vec![]);
    }
    let placeholders = std::iter::repeat("?")
        .take(legacy_rowids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT chunk_id, vector FROM embeddings WHERE chunk_id IN ({})",
        placeholders
    );
    let params: Vec<&dyn rusqlite::ToSql> = legacy_rowids
        .iter()
        .map(|x| x as &dyn rusqlite::ToSql)
        .collect();
    let rows = tx
        .prepare(&sql)?
        .query_map(&params[..], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Capture every relationship edge that touches any legacy rowid as either
/// `from_id` or `to_id`. Returns an empty vec when the set is empty.
fn capture_relationships(
    tx: &rusqlite::Transaction<'_>,
    legacy_rowids: &[i64],
) -> Result<Vec<(i64, i64, String, String, String, i64)>> {
    if legacy_rowids.is_empty() {
        return Ok(vec![]);
    }
    let placeholders = std::iter::repeat("?")
        .take(legacy_rowids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT from_id, to_id, rel_type, symbol, entity_id, created_at
         FROM curated_relationships
         WHERE from_id IN ({0}) OR to_id IN ({0})",
        placeholders
    );
    // Each legacy rowid binds twice (once for the from_id IN clause, once
    // for the to_id IN clause).
    let mut params: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(legacy_rowids.len() * 2);
    for id in legacy_rowids {
        params.push(id as &dyn rusqlite::ToSql);
        params.push(id as &dyn rusqlite::ToSql);
    }
    let rows = tx
        .prepare(&sql)?
        .query_map(&params[..], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, i64>(5)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::open_in_memory;
    use std::sync::{Arc, Mutex};

    /// Collect every progress event into a vec for assertion.
    fn capture_progress() -> (impl Fn(MigrationProgress) + Send + 'static, Arc<Mutex<Vec<MigrationProgress>>>) {
        let log = Arc::new(Mutex::new(Vec::<MigrationProgress>::new()));
        let sink = log.clone();
        let emit = move |p: MigrationProgress| {
            sink.lock().unwrap().push(p);
        };
        (emit, log)
    }

    fn seed_doc(conn: &Connection, path: &str) -> i64 {
        upsert_document(conn, path, "h").unwrap()
    }

    fn seed_chunk_with_legacy_id(
        conn: &Connection,
        doc_id: i64,
        chunk_id: i64,
        text: &str,
        start_line: i64,
        end_line: i64,
    ) {
        conn.execute(
            "INSERT INTO chunks (id, doc_id, chunk_text, position, start_line, end_line, symbol_name, strategy)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, 'prose')",
            params![chunk_id, doc_id, text, 0, start_line, end_line],
        )
        .unwrap();
    }

    fn seed_entry_with_source_ref(conn: &Connection, id: &str, source_ref_json: &str) {
        // Need an entity to satisfy FK; we don't need it for the migration
        // logic but llm_wiki_entries has foreign keys to curated_entities.
        conn.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
             VALUES ('ent-x', 'X', 'concept', '', 100, 100)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_entries
                (id, entity_id, title, body, tags, confidence, source_type, source_ref,
                 created_at, updated_at)
             VALUES (?1, 'ent-x', 'T', 'B', '[]', 'inferred', 'user_confirmed', ?2, 100, 100)",
            params![id, source_ref_json],
        )
        .unwrap();
    }

    #[test]
    fn run_chunk_hash_migration_populates_content_hash_on_all_chunks() {
        let tmp = tempfile::TempDir::new().unwrap();
        let doc_path = tmp.path().join("documents").join("a.md");
        std::fs::create_dir_all(doc_path.parent().unwrap()).unwrap();
        // 21 five-word sentences = 105 words total. The prose chunker groups
        // until acc_words >= TARGET_WORDS (100), so sentence #20 closes the
        // first group; sentence #21 is the second. Result: exactly 2 chunks,
        // matching the 2 seeded legacy chunks.
        let mut long_text = String::new();
        for i in 0..21 {
            long_text.push_str(&format!("S{i:02} w{i:02} x{i:02} y{i:02} z{i:02}. "));
        }
        std::fs::write(&doc_path, &long_text).unwrap();
        let path_str = doc_path.to_string_lossy().to_string();

        let mut conn = open_in_memory().unwrap();
        let doc_id = seed_doc(&conn, &path_str);
        seed_chunk_with_legacy_id(&conn, doc_id, 100, "legacy-a", 1, 1);
        seed_chunk_with_legacy_id(&conn, doc_id, 101, "legacy-b", 3, 3);

        let (emit, log) = capture_progress();
        run_chunk_hash_migration(&mut conn, emit).unwrap();

        let populated: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chunks WHERE content_hash != '' AND content_hash IS NOT NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(populated, 2, "chunker must produce 2 chunks for this text");
        // Each emit fires before mutation; at least one "rechunk" + one "rewrite" event.
        let events = log.lock().unwrap();
        assert!(events.iter().any(|p| p.phase == "rechunk"));
        assert!(events.iter().any(|p| p.phase == "rewrite"));
    }

    #[test]
    fn run_chunk_hash_migration_rewrites_source_ref_json() {
        let tmp = tempfile::TempDir::new().unwrap();
        let doc_path = tmp.path().join("documents").join("b.md");
        std::fs::create_dir_all(doc_path.parent().unwrap()).unwrap();
        std::fs::write(&doc_path, "Single chunk body.").unwrap();
        let path_str = doc_path.to_string_lossy().to_string();

        let mut conn = open_in_memory().unwrap();
        let doc_id = seed_doc(&conn, &path_str);
        seed_chunk_with_legacy_id(&conn, doc_id, 200, "Single chunk body.", 1, 1);

        let source_ref = r#"{"proposal_id":"prop_1","evidence":[{"chunk_id":200,"quote":"q","start_line":1,"end_line":1}]}"#;
        seed_entry_with_source_ref(&conn, "fact-mig", source_ref);

        let (emit, _log) = capture_progress();
        run_chunk_hash_migration(&mut conn, emit).unwrap();

        let new_ref: String = conn
            .query_row(
                "SELECT source_ref FROM llm_wiki_entries WHERE id = 'fact-mig'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let parsed: Value = serde_json::from_str(&new_ref).unwrap();
        let evidence = parsed.get("evidence").unwrap().as_array().unwrap();
        let entry = evidence[0].as_object().unwrap();
        assert!(entry.contains_key("content_hash"), "chunk_id must be replaced by content_hash");
        assert!(!entry.contains_key("chunk_id"), "old chunk_id key must be removed");
        let hash = entry.get("content_hash").unwrap().as_str().unwrap();
        assert_eq!(hash.len(), 32, "rewritten hash must be the 32-char hex");
    }

    #[test]
    fn run_chunk_hash_migration_is_idempotent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let doc_path = tmp.path().join("documents").join("c.md");
        std::fs::create_dir_all(doc_path.parent().unwrap()).unwrap();
        std::fs::write(&doc_path, "Idempotent body.").unwrap();
        let path_str = doc_path.to_string_lossy().to_string();

        let mut conn = open_in_memory().unwrap();
        let doc_id = seed_doc(&conn, &path_str);
        seed_chunk_with_legacy_id(&conn, doc_id, 300, "Idempotent body.", 1, 1);
        seed_entry_with_source_ref(
            &conn,
            "fact-idem",
            r#"{"proposal_id":"p","evidence":[{"chunk_id":300,"quote":"q","start_line":1,"end_line":1}]}"#,
        );

        let (emit1, _) = capture_progress();
        run_chunk_hash_migration(&mut conn, emit1).unwrap();
        let first_hash: String = conn
            .query_row(
                "SELECT source_ref FROM llm_wiki_entries WHERE id = 'fact-idem'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let (emit2, _) = capture_progress();
        run_chunk_hash_migration(&mut conn, emit2).unwrap();
        let second_hash: String = conn
            .query_row(
                "SELECT source_ref FROM llm_wiki_entries WHERE id = 'fact-idem'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(first_hash, second_hash, "second pass must be a no-op");
    }

    #[test]
    fn run_chunk_hash_migration_emits_progress_events_in_order() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut conn = open_in_memory().unwrap();
        for i in 0..3 {
            let p = tmp.path().join("documents").join(format!("d{i}.md"));
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, format!("Doc {i} body.")).unwrap();
            let path_str = p.to_string_lossy().to_string();
            let doc_id = seed_doc(&conn, &path_str);
            seed_chunk_with_legacy_id(&conn, doc_id, 400 + i, &format!("Doc {i} body."), 1, 1);
        }

        let (emit, log) = capture_progress();
        run_chunk_hash_migration(&mut conn, emit).unwrap();
        let events = log.lock().unwrap();
        let rechunk_cur: Vec<usize> = events
            .iter()
            .filter(|p| p.phase == "rechunk")
            .map(|p| p.current)
            .collect();
        for w in rechunk_cur.windows(2) {
            assert!(w[0] <= w[1], "rechunk current must be non-decreasing");
        }
    }

    #[test]
    fn run_chunk_hash_migration_rolls_back_on_failure() {
        // Build a doc that does not exist on disk so chunk_autodetect's
        // read inside run_chunk_hash_migration fails. The transaction
        // must roll back, leaving chunks.content_hash empty.
        let mut conn = open_in_memory().unwrap();
        let doc_id = seed_doc(&conn, "/nonexistent/doc.md");
        seed_chunk_with_legacy_id(&conn, doc_id, 500, "x", 1, 1);

        let (emit, _) = capture_progress();
        let result = run_chunk_hash_migration(&mut conn, emit);
        assert!(result.is_err(), "must error when doc is unreadable");
        let empty_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chunks WHERE content_hash = ''",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(empty_count, 1, "chunks.content_hash must be unchanged after rollback");
    }

    #[test]
    fn chunks_have_content_hash_returns_true_when_populated() {
        let conn = open_in_memory().unwrap();
        let doc_id = seed_doc(&conn, "/x.md");
        conn.execute(
            "INSERT INTO chunks (doc_id, chunk_text, position, start_line, end_line, symbol_name, strategy, content_hash)
             VALUES (?1, 'b', 0, 1, 1, NULL, 'prose', 'aabb')",
            [doc_id],
        ).unwrap();
        assert!(chunks_have_content_hash(&conn).unwrap());
    }

    #[test]
    fn chunks_have_content_hash_returns_false_with_any_empty() {
        let conn = open_in_memory().unwrap();
        let doc_id = seed_doc(&conn, "/y.md");
        conn.execute(
            "INSERT INTO chunks (doc_id, chunk_text, position, start_line, end_line, symbol_name, strategy, content_hash)
             VALUES (?1, 'b', 0, 1, 1, NULL, 'prose', '')",
            [doc_id],
        ).unwrap();
        assert!(!chunks_have_content_hash(&conn).unwrap());
    }

    #[test]
    fn run_chunk_hash_migration_preserves_embeddings() {
        let tmp = tempfile::TempDir::new().unwrap();
        let doc_path = tmp.path().join("documents").join("emb.md");
        std::fs::create_dir_all(doc_path.parent().unwrap()).unwrap();
        // 21 five-word sentences → exactly 2 chunks after migration.
        let mut long_text = String::new();
        for i in 0..21 {
            long_text.push_str(&format!("S{i:02} w{i:02} x{i:02} y{i:02} z{i:02}. "));
        }
        std::fs::write(&doc_path, &long_text).unwrap();
        let path_str = doc_path.to_string_lossy().to_string();

        let mut conn = open_in_memory().unwrap();
        let doc_id = seed_doc(&conn, &path_str);
        seed_chunk_with_legacy_id(&conn, doc_id, 600, "legacy-a", 1, 1);
        seed_chunk_with_legacy_id(&conn, doc_id, 601, "legacy-b", 3, 3);
        // Distinct vectors per legacy chunk so we can verify mapping.
        let vec_a: Vec<f32> = vec![1.0_f32, 2.0, 3.0, 4.0];
        let vec_b: Vec<f32> = vec![5.0_f32, 6.0, 7.0, 8.0];
        let bytes_a: Vec<u8> = vec_a.iter().flat_map(|f| f.to_le_bytes()).collect();
        let bytes_b: Vec<u8> = vec_b.iter().flat_map(|f| f.to_le_bytes()).collect();
        conn.execute(
            "INSERT INTO embeddings (chunk_id, vector) VALUES (?1, ?2)",
            params![600, bytes_a],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO embeddings (chunk_id, vector) VALUES (?1, ?2)",
            params![601, bytes_b],
        )
        .unwrap();

        let (emit, _) = capture_progress();
        run_chunk_hash_migration(&mut conn, emit).unwrap();

        // Every embedding that pointed at legacy rowid 600 or 601 must
        // survive (now pointing at the new rowids). Vector bytes must be
        // identical.
        let legacy_a_rowids: Vec<i64> = conn
            .prepare("SELECT id FROM chunks WHERE doc_id = ?1 ORDER BY position")
            .unwrap()
            .query_map([doc_id], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(legacy_a_rowids.len(), 2, "migration must leave 2 chunks");
        let stored_a: Vec<u8> = conn
            .query_row(
                "SELECT vector FROM embeddings WHERE chunk_id = ?1",
                [legacy_a_rowids[0]],
                |r| r.get(0),
            )
            .unwrap();
        let stored_b: Vec<u8> = conn
            .query_row(
                "SELECT vector FROM embeddings WHERE chunk_id = ?1",
                [legacy_a_rowids[1]],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored_a, bytes_a, "embedding A bytes must be preserved");
        assert_eq!(stored_b, bytes_b, "embedding B bytes must be preserved");

        // Total embedding count is exactly 2 — no orphans, no duplicates.
        let emb_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM embeddings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(emb_count, 2, "no orphaned or duplicated embeddings");
    }

    #[test]
    fn run_chunk_hash_migration_preserves_relationships() {
        let tmp = tempfile::TempDir::new().unwrap();
        let doc_path = tmp.path().join("documents").join("rel.md");
        std::fs::create_dir_all(doc_path.parent().unwrap()).unwrap();
        // 21 five-word sentences → exactly 2 chunks after migration.
        let mut long_text = String::new();
        for i in 0..21 {
            long_text.push_str(&format!("S{i:02} w{i:02} x{i:02} y{i:02} z{i:02}. "));
        }
        std::fs::write(&doc_path, &long_text).unwrap();
        let path_str = doc_path.to_string_lossy().to_string();

        let mut conn = open_in_memory().unwrap();
        let doc_id = seed_doc(&conn, &path_str);
        seed_chunk_with_legacy_id(&conn, doc_id, 700, "legacy-a", 1, 1);
        seed_chunk_with_legacy_id(&conn, doc_id, 701, "legacy-b", 3, 3);
        // Edge from chunk 700 → chunk 701, plus a self-edge on 700 for coverage.
        conn.execute(
            "INSERT INTO curated_relationships (from_id, to_id, rel_type, symbol, entity_id, created_at)
             VALUES (?1, ?2, 'calls', 'foo', 'tier_fact', 1000)",
            params![700, 701],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO curated_relationships (from_id, to_id, rel_type, symbol, entity_id, created_at)
             VALUES (?1, ?2, 'defines', 'bar', 'tier_fact', 1001)",
            params![700, 700],
        )
        .unwrap();

        let (emit, _) = capture_progress();
        run_chunk_hash_migration(&mut conn, emit).unwrap();

        // Both edges must still exist; the from_id/to_id must point at the
        // NEW rowids, not the legacy ones.
        let new_rowids: Vec<i64> = conn
            .prepare("SELECT id FROM chunks WHERE doc_id = ?1 ORDER BY position")
            .unwrap()
            .query_map([doc_id], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(new_rowids.len(), 2);
        let new_a = new_rowids[0];
        let new_b = new_rowids[1];

        let edge_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM curated_relationships", [], |r| r.get(0))
            .unwrap();
        assert_eq!(edge_count, 2, "both edges must survive migration");

        let call_edge: (i64, i64, String) = conn
            .query_row(
                "SELECT from_id, to_id, rel_type FROM curated_relationships
                 WHERE rel_type = 'calls'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(call_edge.0, new_a, "calls edge from_id remapped to new rowid");
        assert_eq!(call_edge.1, new_b, "calls edge to_id remapped to new rowid");
        assert_eq!(call_edge.2, "calls");

        let self_edge: (i64, i64, String) = conn
            .query_row(
                "SELECT from_id, to_id, rel_type FROM curated_relationships
                 WHERE rel_type = 'defines'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(self_edge.0, new_a, "defines self-edge from_id remapped");
        assert_eq!(self_edge.1, new_a, "defines self-edge to_id remapped");
    }
}