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
        "SELECT e.vector, c.chunk_text, c.position, c.start_line, c.end_line, \
         COALESCE(c.symbol_name, '') as symbol_name, c.strategy, d.path
         FROM embeddings e
         JOIN chunks c ON c.id = e.chunk_id
         JOIN documents d ON d.id = c.doc_id
         WHERE d.status = 'indexed'",
    )?;

    let mut results: Vec<(f32, SearchResult)> = Vec::new();
    let mut rows = stmt.query([])?;

    while let Some(row) = rows.next()? {
        let bytes: Vec<u8> = row.get(0)?;
        let chunk_text: String = row.get(1)?;
        let chunk_position: i64 = row.get(2)?;
        let start_line: i64 = row.get(3)?;
        let end_line: i64 = row.get(4)?;
        let symbol_str: String = row.get(5)?;
        let strategy: String = row.get(6)?;
        let doc_path: String = row.get(7)?;
        let vec = bytes_to_f32(&bytes);
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
        "SELECT e.vector, c.chunk_text, c.position, c.start_line, c.end_line, \
         COALESCE(c.symbol_name, '') as symbol_name, c.strategy, d.path
         FROM embeddings e
         JOIN chunks c ON c.id = e.chunk_id
         JOIN documents d ON d.id = c.doc_id
         WHERE d.path != ?1 AND d.status = 'indexed'",
    )?;

    let mut results: Vec<(f32, SearchResult)> = Vec::new();
    let mut rows = stmt.query([doc_path])?;

    while let Some(row) = rows.next()? {
        let bytes: Vec<u8> = row.get(0)?;
        let chunk_text: String = row.get(1)?;
        let chunk_position: i64 = row.get(2)?;
        let start_line: i64 = row.get(3)?;
        let end_line: i64 = row.get(4)?;
        let symbol_str: String = row.get(5)?;
        let strategy: String = row.get(6)?;
        let doc_path_r: String = row.get(7)?;
        let vec = bytes_to_f32(&bytes);
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
            },
        ));
    }

    results.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    Ok(results.into_iter().take(limit).map(|(_, r)| r).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let back: SearchResult =
            serde_json::from_value(v).expect("round-trip SearchResult serde");
        assert_eq!(back.doc_path, "/a.md");
        assert_eq!(back.symbol_name.as_deref(), Some("foo"));
        assert_eq!(back.strategy, "prose");
    }
}
