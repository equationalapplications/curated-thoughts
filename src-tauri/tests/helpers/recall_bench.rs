//! Shared corpus load / seed / Recall@10 for retrieval benchmarks.

use super::TestApp;
use flate2::read::GzDecoder;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::io::Read;

pub fn fixture_path(fixture_subdir: &str, file: &str) -> String {
    format!(
        "{}/tests/fixtures/{fixture_subdir}/{file}",
        env!("CARGO_MANIFEST_DIR")
    )
}

pub struct RecallFixtures {
    pub corpus: Vec<(String, String)>,
    pub embeddings: HashMap<String, Vec<Vec<f32>>>,
    pub queries: HashMap<String, String>,
    pub qrels: HashMap<String, Vec<String>>,
}

pub fn parse_embedding_entry(v: Value) -> Vec<Vec<f32>> {
    let arr = match v {
        Value::Array(a) => a,
        other => panic!("embedding value must be array, got {:?}", other),
    };
    if arr.first().map_or(false, |x| matches!(x, Value::Array(_))) {
        return arr
            .into_iter()
            .map(|row| {
                row.as_array()
                    .expect("inner chunk embedding must be array")
                    .iter()
                    .map(|x| x.as_f64().expect("vector element") as f32)
                    .collect()
            })
            .collect();
    }

    vec![arr
        .into_iter()
        .map(|x| x.as_f64().expect("vector element") as f32)
        .collect()]
}

impl RecallFixtures {
    pub fn load(fixture_subdir: &str, embeddings_gzip: &str) -> Self {
        let corpus_bytes =
            std::fs::read(fixture_path(fixture_subdir, "corpus.jsonl")).expect("corpus.jsonl");
        let mut corpus = Vec::new();
        for line in std::str::from_utf8(&corpus_bytes).unwrap().lines() {
            if line.trim().is_empty() {
                continue;
            }
            let v: Value = serde_json::from_str(line).expect("corpus line");
            let id = v["_id"].as_str().unwrap_or("").to_string();
            let title = v["title"].as_str().unwrap_or("");
            let text = v["text"].as_str().unwrap_or("");
            corpus.push((id, format!("{title} {text}")));
        }

        let emb_path = fixture_path(fixture_subdir, embeddings_gzip);
        let emb_gz = std::fs::File::open(&emb_path).unwrap_or_else(|_| {
            panic!(
                "missing {emb_path}\nRun: cargo run --bin embed_bench_fixture -- {fixture_subdir}",
            )
        });
        let mut decoder = GzDecoder::new(emb_gz);
        let mut json_str = String::new();
        decoder.read_to_string(&mut json_str).expect("decompress embeddings");
        let raw: HashMap<String, Value> =
            serde_json::from_str(&json_str).expect("parse embeddings");
        let embeddings: HashMap<String, Vec<Vec<f32>>> = raw
            .into_iter()
            .map(|(k, v)| (k, parse_embedding_entry(v)))
            .collect();

        let q_bytes = std::fs::read(fixture_path(fixture_subdir, "queries.json")).expect("queries");
        let queries: HashMap<String, String> = serde_json::from_slice(&q_bytes).expect("queries");

        let qr_bytes = std::fs::read(fixture_path(fixture_subdir, "qrels.json")).expect("qrels");
        let qrels: HashMap<String, Vec<String>> = serde_json::from_slice(&qr_bytes).expect("qrels");

        RecallFixtures {
            corpus,
            embeddings,
            queries,
            qrels,
        }
    }
}

pub fn seed_sentence_chunks(app: &TestApp, fixtures: &RecallFixtures) {
    let conn = app.open_db();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();

    for (doc_id, text) in &fixtures.corpus {
        conn.execute(
            "INSERT OR IGNORE INTO documents (path, hash, tier, status) VALUES (?1, ?1, 'user_doc', 'indexed')",
            [doc_id],
        )
        .unwrap();
        let db_doc_id: i64 = conn
            .query_row(
                "SELECT id FROM documents WHERE path = ?1",
                [doc_id],
                |r| r.get(0),
            )
            .unwrap();

        let chunk_texts = tauri_app_lib::chunker::chunk_text(text);
        let embedding_rows = fixtures
            .embeddings
            .get(doc_id)
            .unwrap_or_else(|| panic!("missing embeddings for doc {doc_id}"));

        assert_eq!(
            chunk_texts.len(),
            embedding_rows.len(),
            "fixture chunk count mismatch for {doc_id}",
        );

        for (position, (chunk_txt, vec)) in chunk_texts
            .iter()
            .zip(embedding_rows.iter())
            .enumerate()
        {
            conn.execute(
                "INSERT INTO chunks (doc_id, chunk_text, position) VALUES (?1, ?2, ?3)",
                rusqlite::params![db_doc_id, chunk_txt, position],
            )
            .unwrap();
            let chunk_id: i64 = conn.last_insert_rowid();
            let bytes: Vec<u8> = vec.iter().flat_map(|f| f.to_le_bytes()).collect();
            conn.execute(
                "INSERT INTO embeddings (chunk_id, vector) VALUES (?1, ?2)",
                rusqlite::params![chunk_id, bytes],
            )
            .unwrap();
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn run_recall_at_k(
    label: &str,
    fixture_subdir: &str,
    embeddings_gzip: &str,
    k: usize,
    min_recall: f64,
) {
    let fixtures = RecallFixtures::load(fixture_subdir, embeddings_gzip);
    let app = TestApp::new();
    println!(
        "[{label}] Seeding {} docs from {fixture_subdir}…",
        fixtures.corpus.len()
    );
    seed_sentence_chunks(&app, &fixtures);

    let embedder = tauri_app_lib::embedder::Embedder::new().expect("embedder");
    let total = fixtures.queries.len();
    let mut hits = 0usize;

    for (claim_id, query_text) in &fixtures.queries {
        let relevant: HashSet<&str> = fixtures
            .qrels
            .get(claim_id)
            .map(|ids| ids.iter().map(|s| s.as_str()).collect())
            .unwrap_or_default();

        if relevant.is_empty() {
            continue;
        }

        let query_vec =
            embedder.embed(vec![query_text.clone()]).expect("embed query")[0].clone();

        let conn = app.open_db();
        let results =
            tauri_app_lib::search::semantic_search(&conn, &query_vec, k).expect("semantic_search");

        let found = results
            .iter()
            .any(|r| relevant.contains(r.doc_path.as_str()));
        if found {
            hits += 1;
        }
    }

    let recall = hits as f64 / total as f64;
    println!(
        "[{label}] Recall@{k}: {:.3} ({hits}/{total})",
        recall
    );

    assert!(
        recall >= min_recall,
        "[{label}] Recall@{k} {:.3} < {:.2} ({hits}/{total})",
        recall,
        min_recall
    );
}
