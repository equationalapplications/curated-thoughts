//! Semantic retrieval over chunked vault notes: cosine similarity versus every stored embedding.
//!
//! For large vaults, **[`semantic_search`]** and document-centroid paths in **[`related_chunks`]**
//! scan all indexed vectors (cosine in application code). That is simple and exact but **O(#chunks)**
//! per query. If profiling shows regressions (see `semantic_search_profile` binary), typical
//! upgrades are: **sqlite-vec** / **sqlite-vss** (SQLite loadable ANN), **[USearch](https://github.com/unum-cloud/usearch)** in-process,
//! or an external vector DB—after validating dimensionality matches the active embedder.

use anyhow::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};

const VECTOR_CACHE_CAPACITY_PER_ENTITY: usize = 500;
const VECTOR_CACHE_CAPACITY_ENTITY_IDS: usize = 64;

struct EntityVectorCache {
    order: VecDeque<i64>,
    vectors: HashMap<i64, Arc<[f32]>>,
}

impl EntityVectorCache {
    fn new() -> Self {
        Self {
            order: VecDeque::new(),
            vectors: HashMap::new(),
        }
    }

    fn get(&mut self, chunk_id: i64) -> Option<Arc<[f32]>> {
        let result = self.vectors.get(&chunk_id).cloned();
        if result.is_some() {
            self.order.retain(|id| *id != chunk_id);
            self.order.push_back(chunk_id);
        }
        result
    }

    fn insert(&mut self, chunk_id: i64, vector: Arc<[f32]>) {
        if self.vectors.contains_key(&chunk_id) {
            return;
        }
        self.order.push_back(chunk_id);
        self.vectors.insert(chunk_id, vector);
        while self.order.len() > VECTOR_CACHE_CAPACITY_PER_ENTITY {
            if let Some(old_id) = self.order.pop_front() {
                self.vectors.remove(&old_id);
            }
        }
    }
}

struct EntityVectorCacheStore {
    order: VecDeque<String>,
    entities: HashMap<String, EntityVectorCache>,
}

impl EntityVectorCacheStore {
    fn new() -> Self {
        Self {
            order: VecDeque::new(),
            entities: HashMap::new(),
        }
    }

    fn get(&mut self, entity_id: &str, chunk_id: i64) -> Option<Arc<[f32]>> {
        if let Some(entity) = self.entities.get_mut(entity_id) {
            let result = entity.get(chunk_id);
            if result.is_some() {
                self.order.retain(|id| id.as_str() != entity_id);
                self.order.push_back(entity_id.to_string());
            }
            result
        } else {
            None
        }
    }

    fn insert(&mut self, entity_id: &str, chunk_id: i64, vector: Arc<[f32]>) {
        if let Some(entity_cache) = self.entities.get_mut(entity_id) {
            entity_cache.insert(chunk_id, vector);
            self.order.retain(|id| id.as_str() != entity_id);
            self.order.push_back(entity_id.to_string());
            return;
        }

        if self.entities.len() >= VECTOR_CACHE_CAPACITY_ENTITY_IDS {
            if let Some(old_entity_id) = self.order.pop_front() {
                self.entities.remove(&old_entity_id);
            }
        }

        self.order.push_back(entity_id.to_string());
        let mut entity_cache = EntityVectorCache::new();
        entity_cache.insert(chunk_id, vector);
        self.entities.insert(entity_id.to_string(), entity_cache);
    }
}

static VECTOR_CACHE: OnceLock<Mutex<EntityVectorCacheStore>> = OnceLock::new();

fn acquire_cache_lock() -> std::sync::MutexGuard<'static, EntityVectorCacheStore> {
    let cache = VECTOR_CACHE.get_or_init(|| Mutex::new(EntityVectorCacheStore::new()));
    cache.lock().unwrap_or_else(|e| e.into_inner())
}

fn get_cached_embedding(entity_id: &str, chunk_id: i64) -> Option<Arc<[f32]>> {
    let mut cache = acquire_cache_lock();
    cache.get(entity_id, chunk_id)
}

fn insert_cached_embedding(entity_id: &str, chunk_id: i64, vector: Vec<f32>) {
    let mut cache = acquire_cache_lock();
    cache.insert(entity_id, chunk_id, Arc::from(vector));
}

fn get_or_insert_cached_embedding(entity_id: &str, chunk_id: i64, bytes: &[u8]) -> Arc<[f32]> {
    let mut cache = acquire_cache_lock();
    if let Some(cached) = cache.get(entity_id, chunk_id) {
        return cached;
    }
    let decoded: Arc<[f32]> = bytes_to_f32(bytes).into();
    cache.insert(entity_id, chunk_id, Arc::clone(&decoded));
    decoded
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SearchResult {
    pub doc_path: String,
    pub chunk_text: String,
    pub chunk_position: i64,
    pub score: f32,
    pub start_line: i64,
    pub end_line: i64,
    pub symbol_name: Option<String>,
    pub strategy: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structural: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rel_type: Option<String>,
    /// Tier label from `chunks.entity_id`: `tier_fact`, `tier_wisdom`, or `tier_working`.
    /// Authoritative source for frontend tier styling/weighting — avoids path heuristics.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity_id: Option<String>,
}

pub fn bytes_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
}

pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    (dot / (norm_a * norm_b)).clamp(-1.0, 1.0)
}

/// Ranks every indexed chunk by cosine similarity to `query_vec` (full scan).
///
/// Cost grows linearly with embedding row count; see module docs for ANN migration notes.
pub fn semantic_search(
    conn: &Connection,
    query_vec: &[f32],
    limit: usize,
) -> Result<Vec<SearchResult>> {
    let mut stmt = conn.prepare(
        "SELECT e.chunk_id, e.vector, c.chunk_text, c.position, c.start_line, c.end_line, \
         COALESCE(c.symbol_name, '') as symbol_name, c.strategy, c.entity_id, d.path
         FROM embeddings e
         JOIN chunks c ON c.id = e.chunk_id
         JOIN documents d ON d.id = c.doc_id
         WHERE d.status = 'indexed'",
    )?;

    let mut results: Vec<(f32, SearchResult)> = Vec::new();
    let mut rows = stmt.query([])?;

    while let Some(row) = rows.next()? {
        let chunk_id: i64 = row.get(0)?;
        let bytes: Vec<u8> = row.get(1)?;
        let chunk_text: String = row.get(2)?;
        let chunk_position: i64 = row.get(3)?;
        let start_line: i64 = row.get(4)?;
        let end_line: i64 = row.get(5)?;
        let symbol_str: String = row.get(6)?;
        let strategy: String = row.get(7)?;
        let entity_id: Option<String> = row.get(8)?;
        let doc_path: String = row.get(9)?;
        let cache_key = entity_id.as_deref().unwrap_or("unknown");
        let vec = get_or_insert_cached_embedding(cache_key, chunk_id, &bytes);
        let score = cosine_similarity(query_vec, &vec);
        let symbol_name = if symbol_str.is_empty() {
            None
        } else {
            Some(symbol_str)
        };
        results.push((
            score,
            SearchResult {
                doc_path,
                chunk_text,
                chunk_position,
                score,
                start_line,
                end_line,
                symbol_name,
                strategy,
                structural: None,
                rel_type: None,
                entity_id,
            },
        ));
    }

    results.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    Ok(results.into_iter().take(limit).map(|(_, r)| r).collect())
}

pub fn related_chunks(
    conn: &Connection,
    doc_path: &str,
    limit: usize,
) -> Result<Vec<SearchResult>> {
    let doc_vecs: Vec<Vec<f32>> = {
        let mut stmt = conn.prepare(
            "SELECT e.vector FROM embeddings e
             JOIN chunks c ON c.id = e.chunk_id
             JOIN documents d ON d.id = c.doc_id
             WHERE d.path = ?1 AND d.status = 'indexed'",
        )?;
        let mut rows = stmt.query([doc_path])?;
        let mut vecs = Vec::new();
        while let Some(row) = rows.next()? {
            let bytes: Vec<u8> = row.get(0)?;
            vecs.push(bytes_to_f32(&bytes));
        }
        vecs
    };

    if doc_vecs.is_empty() {
        return Ok(vec![]);
    }

    let dim = doc_vecs[0].len();
    let mut avg = vec![0.0_f32; dim];
    for v in &doc_vecs {
        for (a, b) in avg.iter_mut().zip(v) {
            *a += b;
        }
    }
    let n = doc_vecs.len() as f32;
    avg.iter_mut().for_each(|x| *x /= n);

    let mut stmt = conn.prepare(
        "SELECT e.chunk_id, e.vector, c.chunk_text, c.position, c.start_line, c.end_line, \
         COALESCE(c.symbol_name, '') as symbol_name, c.strategy, c.entity_id, d.path
         FROM embeddings e
         JOIN chunks c ON c.id = e.chunk_id
         JOIN documents d ON d.id = c.doc_id
         WHERE d.path != ?1 AND d.status = 'indexed'",
    )?;

    let mut results: Vec<(f32, SearchResult)> = Vec::new();
    let mut rows = stmt.query([doc_path])?;

    while let Some(row) = rows.next()? {
        let chunk_id: i64 = row.get(0)?;
        let bytes: Vec<u8> = row.get(1)?;
        let chunk_text: String = row.get(2)?;
        let chunk_position: i64 = row.get(3)?;
        let start_line: i64 = row.get(4)?;
        let end_line: i64 = row.get(5)?;
        let symbol_str: String = row.get(6)?;
        let strategy: String = row.get(7)?;
        let entity_id: Option<String> = row.get(8)?;
        let doc_path_r: String = row.get(9)?;
        let cache_key = entity_id.as_deref().unwrap_or("unknown");
        let vec = get_or_insert_cached_embedding(cache_key, chunk_id, &bytes);
        let score = cosine_similarity(&avg, &vec);
        let symbol_name = if symbol_str.is_empty() {
            None
        } else {
            Some(symbol_str)
        };
        results.push((
            score,
            SearchResult {
                doc_path: doc_path_r,
                chunk_text,
                chunk_position,
                score,
                start_line,
                end_line,
                symbol_name,
                strategy,
                structural: None,
                rel_type: None,
                entity_id,
            },
        ));
    }

    results.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    Ok(results.into_iter().take(limit).map(|(_, r)| r).collect())
}

/// Try [`related_chunks`] with each candidate `documents.path` key until one returns results.
///
/// Ingestion and the file watcher typically persist **canonical** absolute paths, while the UI
/// often holds **vault-relative** paths. Older rows may use other spellings; exact SQLite
/// equality would otherwise miss the document row.
pub fn related_chunks_try_paths(
    conn: &Connection,
    doc_paths: &[String],
    limit: usize,
) -> Result<Vec<SearchResult>> {
    for p in doc_paths {
        let hits = related_chunks(conn, p, limit)?;
        if !hits.is_empty() {
            return Ok(hits);
        }
    }
    Ok(vec![])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::open_in_memory;


    fn vec_blob2(x: f32, y: f32) -> Vec<u8> {
        [x.to_le_bytes(), y.to_le_bytes()]
            .into_iter()
            .flatten()
            .collect()
    }

    /// Two indexed documents, each with one chunk and one 2-D embedding.
    fn seed_two_doc_fixture() -> rusqlite::Connection {
        let conn = open_in_memory().expect("open in-memory db");
        conn
            .execute(
                "INSERT INTO documents (path, hash, tier, status) VALUES (?1, ?2, 'user_doc', 'indexed')",
                ["stored-a", "ha"],
            )
            .expect("insert doc a");
        let id_a: i64 = conn.last_insert_rowid();
        conn
            .execute(
                "INSERT INTO documents (path, hash, tier, status) VALUES (?1, ?2, 'user_doc', 'indexed')",
                ["stored-b", "hb"],
            )
            .expect("insert doc b");
        let id_b: i64 = conn.last_insert_rowid();

        conn
            .execute(
                "INSERT INTO chunks (doc_id, chunk_text, position, start_line, end_line, strategy) \
                 VALUES (?1, 'chunk-a', 0, 1, 1, 'prose')",
                [id_a],
            )
            .expect("chunk a");
        let chunk_a: i64 = conn.last_insert_rowid();
        conn
            .execute(
                "INSERT INTO chunks (doc_id, chunk_text, position, start_line, end_line, strategy) \
                 VALUES (?1, 'chunk-b', 0, 1, 1, 'prose')",
                [id_b],
            )
            .expect("chunk b");
        let chunk_b: i64 = conn.last_insert_rowid();

        conn
            .execute(
                "INSERT INTO embeddings (chunk_id, vector) VALUES (?1, ?2)",
                rusqlite::params![chunk_a, vec_blob2(1.0, 0.0)],
            )
            .expect("emb a");
        conn
            .execute(
                "INSERT INTO embeddings (chunk_id, vector) VALUES (?1, ?2)",
                rusqlite::params![chunk_b, vec_blob2(0.0, 1.0)],
            )
            .expect("emb b");
        conn
    }

    #[test]
    fn related_chunks_try_paths_skips_empty_candidates() {
        let conn = seed_two_doc_fixture();
        let hits = related_chunks_try_paths(
            &conn,
            &["no-such-doc-path".into(), "stored-a".into()],
            10,
        )
        .expect("try_paths");
        assert!(!hits.is_empty(), "expected fallback to second path");
        assert!(
            hits.iter().all(|h| h.doc_path == "stored-b"),
            "related view for stored-a should only surface other docs; got {hits:?}"
        );
    }

    #[test]
    fn related_chunks_try_paths_returns_empty_when_none_match() {
        let conn = seed_two_doc_fixture();
        let hits = related_chunks_try_paths(
            &conn,
            &["missing-one".into(), "missing-two".into()],
            10,
        )
        .expect("try_paths");
        assert!(hits.is_empty());
    }

    #[test]
    fn related_chunks_try_paths_returns_first_non_empty() {
        let conn = seed_two_doc_fixture();
        let limit = 10;
        let expected = related_chunks(&conn, "stored-a", limit).expect("direct related");
        let merged = related_chunks_try_paths(
            &conn,
            &["stored-a".into(), "stored-b".into()],
            limit,
        )
        .expect("try_paths");
        assert_eq!(merged.len(), expected.len());
        for (m, e) in merged.iter().zip(expected.iter()) {
            assert_eq!(m.doc_path, e.doc_path);
            assert_eq!(m.chunk_text, e.chunk_text);
            assert_eq!(m.chunk_position, e.chunk_position);
            assert!((m.score - e.score).abs() < 1e-5);
        }
    }

    #[test]
    fn vector_cache_respects_capacity_per_entity() {
        let mut cache = EntityVectorCache::new();
        for chunk_id in 1..=501_i64 {
            cache.insert(chunk_id, Arc::from(vec![chunk_id as f32]));
        }
        assert!(cache.get(1).is_none(), "chunk 1 should be evicted");
        assert!(cache.get(2).is_some(), "chunk 2 should be retained");
        assert!(cache.get(501).is_some(), "chunk 501 should be retained");
    }

    #[test]
    fn vector_cache_respects_capacity_entity_ids() {
        let mut store = EntityVectorCacheStore::new();
        for id_index in 0..(VECTOR_CACHE_CAPACITY_ENTITY_IDS + 1) {
            let entity_id = format!("entity_{id_index}");
            store.insert(&entity_id, 1, Arc::from(vec![id_index as f32]));
        }
        assert!(store.get("entity_0", 1).is_none(), "entity_0 should be evicted");
        assert!(store.get("entity_1", 1).is_some(), "entity_1 should be retained");
    }

    #[test]
    fn test_cosine_identical_vectors() {
        let v = vec![1.0_f32, 2.0, 3.0];
        assert!((cosine_similarity(&v, &v) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_orthogonal_vectors() {
        let a = vec![1.0_f32, 0.0];
        let b = vec![0.0_f32, 1.0];
        assert!(cosine_similarity(&a, &b).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_empty() {
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
    }

    #[test]
    fn test_cosine_dimension_mismatch() {
        assert_eq!(cosine_similarity(&[1.0], &[1.0, 2.0]), 0.0);
    }

    #[test]
    fn test_bytes_to_f32_roundtrip() {
        let original = vec![1.5_f32, -2.0, 0.0, 100.0];
        let bytes: Vec<u8> = original.iter().flat_map(|f| f.to_le_bytes()).collect();
        assert_eq!(bytes_to_f32(&bytes), original);
    }

    #[test]
    fn test_bytes_to_f32_empty() {
        assert!(bytes_to_f32(&[]).is_empty());
    }

    #[test]
    fn test_bytes_to_f32_ignores_trailing_incomplete_chunk() {
        let bytes = vec![0x00, 0x00, 0x80, 0x3f, 0xff];
        let result = bytes_to_f32(&bytes);
        assert_eq!(result.len(), 1);
        assert!((result[0] - 1.0_f32).abs() < 1e-6);
    }

    #[test]
    fn search_result_json_matches_frontend_contract() {
        let r = SearchResult {
            doc_path: "/a.md".into(),
            chunk_text: "hello".into(),
            chunk_position: 2,
            score: 0.42_f32,
            start_line: 1,
            end_line: 3,
            symbol_name: Some("foo".into()),
            strategy: "prose".into(),
            structural: None,
            rel_type: None,
            entity_id: Some("tier_fact".into()),
        };
        let v = serde_json::to_value(&r).expect("serialize");
        for key in [
            "doc_path",
            "chunk_text",
            "chunk_position",
            "score",
            "start_line",
            "end_line",
            "symbol_name",
            "strategy",
        ] {
            assert!(
                v.get(key).is_some(),
                "SearchResult JSON must include `{key}` for TS/MCP parity; got {v}"
            );
        }
        let back: SearchResult = serde_json::from_value(v).expect("round-trip SearchResult serde");
        assert_eq!(back.doc_path, "/a.md");
        assert_eq!(back.symbol_name.as_deref(), Some("foo"));
        assert_eq!(back.strategy, "prose");
    }
}
