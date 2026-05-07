#![cfg(feature = "slow-tests")]

mod helpers;
use helpers::TestApp;
use tauri_app_lib::scifact_fixture::EMBEDDINGS_GZIP_FILENAME;
use flate2::read::GzDecoder;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::io::Read;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/scifact");

struct ScifactFixtures {
    corpus: Vec<(String, String)>, // (doc_id, combined text)
    /// Per-doc embeddings: outer index is chunk sequence (sentence chunk groups).
    embeddings: HashMap<String, Vec<Vec<f32>>>,
    queries: HashMap<String, String>, // claim_id → query text
    qrels: HashMap<String, Vec<String>>, // claim_id → [relevant doc_ids]
}

fn parse_embedding_entry(v: Value) -> Vec<Vec<f32>> {
    let arr = match v {
        Value::Array(a) => a,
        other => panic!("embedding value must be array, got {:?}", other),
    };
    if arr.first().map_or(false, |x| matches!(x, Value::Array(_))) {
        return arr
            .into_iter()
            .map(|row| {
                let inner = row
                    .as_array()
                    .expect("inner chunk embedding must be array")
                    .iter()
                    .map(|x| x.as_f64().expect("vector element") as f32)
                    .collect();
                inner
            })
            .collect();
    }

    vec![arr
        .into_iter()
        .map(|x| x.as_f64().expect("vector element") as f32)
        .collect()]
}

impl ScifactFixtures {
    fn load() -> Self {
        // Load corpus
        let corpus_bytes = std::fs::read(format!("{FIXTURES}/corpus.jsonl")).expect("corpus.jsonl");
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

        // Load pre-computed embeddings
        let emb_gz = std::fs::File::open(format!(
            "{FIXTURES}/{EMBEDDINGS_GZIP_FILENAME}"
        ))
        .expect("embeddings gz — run `cargo run --bin embed_scifact` first");
        let mut decoder = GzDecoder::new(emb_gz);
        let mut json_str = String::new();
        decoder.read_to_string(&mut json_str).expect("decompress embeddings");
        let raw: HashMap<String, Value> = serde_json::from_str(&json_str).expect("parse embeddings");
        let embeddings: HashMap<String, Vec<Vec<f32>>> = raw
            .into_iter()
            .map(|(k, v)| (k, parse_embedding_entry(v)))
            .collect();

        // Load queries
        let q_bytes = std::fs::read(format!("{FIXTURES}/queries.json")).expect("queries.json");
        let queries: HashMap<String, String> = serde_json::from_slice(&q_bytes).expect("queries");

        // Load qrels
        let qr_bytes = std::fs::read(format!("{FIXTURES}/qrels.json")).expect("qrels.json");
        let qrels: HashMap<String, Vec<String>> = serde_json::from_slice(&qr_bytes).expect("qrels");

        ScifactFixtures { corpus, embeddings, queries, qrels }
    }
}

fn seed_corpus(app: &TestApp, fixtures: &ScifactFixtures) {
    let conn = app.open_db();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();

    for (doc_id, text) in &fixtures.corpus {
        conn.execute(
            "INSERT OR IGNORE INTO documents (path, hash, tier, status) VALUES (?1, ?1, 'user_doc', 'indexed')",
            [doc_id],
        ).unwrap();
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
            "fixture chunk count mismatch for {doc_id}"
        );

        for (position, (chunk_txt, vec)) in chunk_texts.iter().zip(embedding_rows.iter()).enumerate() {
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
            ).unwrap();
        }
    }
}

#[test]
fn scifact_recall_at_10_meets_threshold() {
    let fixtures = ScifactFixtures::load();
    let app = TestApp::new();
    println!("Seeding {} corpus docs...", fixtures.corpus.len());
    seed_corpus(&app, &fixtures);

    let embedder = tauri_app_lib::embedder::Embedder::new().expect("embedder");

    let total = fixtures.queries.len();
    let mut hits = 0usize;
    let mut misses: Vec<String> = Vec::new();

    for (claim_id, query_text) in &fixtures.queries {
        let relevant: HashSet<&str> = fixtures.qrels.get(claim_id).map(|ids| ids.iter().map(|s| s.as_str()).collect()).unwrap_or_default();

        if relevant.is_empty() {
            continue;
        }

        let query_vec = embedder.embed(vec![query_text.clone()]).expect("embed query")[0].clone();

        let conn = app.open_db();
        let results = tauri_app_lib::search::semantic_search(&conn, &query_vec, 10).expect("semantic_search");

        let found = results.iter().any(|r| relevant.contains(r.doc_path.as_str()));
        if found {
            hits += 1;
        } else {
            misses.push(format!("claim {claim_id}: '{}'", &query_text[..query_text.len().min(60)]));
        }
    }

    let recall = hits as f64 / total as f64;
    println!("Recall@10: {:.3} ({hits}/{total})", recall);
    if !misses.is_empty() {
        println!("Missed {} queries:", misses.len());
        for m in misses.iter().take(10) {
            println!("  {m}");
        }
    }

    assert!(
        recall >= 0.60,
        "Recall@10 {:.3} < 0.60 threshold ({hits}/{total} queries found relevant doc in top 10)",
        recall
    );
}
