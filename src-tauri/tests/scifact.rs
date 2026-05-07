#![cfg(feature = "slow-tests")]

mod helpers;
use helpers::TestApp;
use flate2::read::GzDecoder;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use tauri_app_lib::scifact_fixture::{
    FULLTEXT_SINGLE_EMBEDDINGS_GZIP_FILENAME, SENTENCE_CHUNK_MULTICHUNK_EMBEDDINGS_GZIP_FILENAME,
};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/scifact");

#[derive(Clone, Copy)]
enum SeedStrategy {
    /// One DB chunk per doc: full combined text — matches `fulltext-single` embed preset.
    FulltextSingle,
    /// Rows from `chunk_text()` — matches `sentence-chunk` embed preset.
    SentenceChunks,
}

struct ScifactFixtures {
    corpus: Vec<(String, String)>, // (doc_id, combined text)
    /// Per-doc: one outer vec per preset; legacy has len 1, sentence-chunk has N chunk vectors.
    embeddings: HashMap<String, Vec<Vec<f32>>>,
    queries: HashMap<String, String>,      // claim_id → query text
    qrels: HashMap<String, Vec<String>>,   // claim_id → [relevant doc_ids]
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

impl ScifactFixtures {
    fn load(embeddings_gzip_filename: &str) -> Self {
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

        let emb_path = format!("{FIXTURES}/{embeddings_gzip_filename}");
        let emb_gz = std::fs::File::open(&emb_path).unwrap_or_else(|_| {
            panic!(
                "missing {emb_path}; regenerate with:\n\
                 cargo run --bin embed_scifact -- fulltext-single\n\
                 cargo run --bin embed_scifact -- sentence-chunk\n"
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

        let q_bytes = std::fs::read(format!("{FIXTURES}/queries.json")).expect("queries.json");
        let queries: HashMap<String, String> = serde_json::from_slice(&q_bytes).expect("queries");

        let qr_bytes = std::fs::read(format!("{FIXTURES}/qrels.json")).expect("qrels.json");
        let qrels: HashMap<String, Vec<String>> = serde_json::from_slice(&qr_bytes).expect("qrels");

        ScifactFixtures { corpus, embeddings, queries, qrels }
    }
}

fn seed_corpus(app: &TestApp, fixtures: &ScifactFixtures, strategy: SeedStrategy) {
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

        let embedding_rows = fixtures
            .embeddings
            .get(doc_id)
            .unwrap_or_else(|| panic!("missing embeddings for doc {doc_id}"));

        match strategy {
            SeedStrategy::FulltextSingle => {
                assert_eq!(
                    embedding_rows.len(),
                    1,
                    "fulltext-single fixture expects one vector per doc {doc_id}"
                );
                let vec = &embedding_rows[0];
                conn.execute(
                    "INSERT INTO chunks (doc_id, chunk_text, position) VALUES (?1, ?2, 0)",
                    rusqlite::params![db_doc_id, text],
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
            SeedStrategy::SentenceChunks => {
                let chunk_texts = tauri_app_lib::chunker::chunk_text(text);
                assert_eq!(
                    chunk_texts.len(),
                    embedding_rows.len(),
                    "fixture chunk count mismatch for {doc_id}"
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
    }
}

fn run_recall_benchmark(
    label: &str,
    embeddings_gzip_filename: &str,
    strategy: SeedStrategy,
) {
    let fixtures = ScifactFixtures::load(embeddings_gzip_filename);
    let app = TestApp::new();
    println!("[{label}] Seeding {} corpus docs…", fixtures.corpus.len());
    seed_corpus(&app, &fixtures, strategy);

    let embedder = tauri_app_lib::embedder::Embedder::new().expect("embedder");

    let total = fixtures.queries.len();
    let mut hits = 0usize;
    let mut misses: Vec<String> = Vec::new();

    for (claim_id, query_text) in &fixtures.queries {
        let relevant: HashSet<&str> = fixtures
            .qrels
            .get(claim_id)
            .map(|ids| ids.iter().map(|s| s.as_str()).collect())
            .unwrap_or_default();

        if relevant.is_empty() {
            continue;
        }

        let query_vec = embedder.embed(vec![query_text.clone()]).expect("embed query")[0].clone();

        let conn = app.open_db();
        let results =
            tauri_app_lib::search::semantic_search(&conn, &query_vec, 10).expect("semantic_search");

        let found = results
            .iter()
            .any(|r| relevant.contains(r.doc_path.as_str()));
        if found {
            hits += 1;
        } else {
            misses.push(format!(
                "claim {claim_id}: '{}'",
                &query_text[..query_text.len().min(60)]
            ));
        }
    }

    let recall = hits as f64 / total as f64;
    println!(
        "[{label}] Recall@10: {:.3} ({hits}/{total}) [{embeddings_gzip_filename}]",
        recall
    );
    if !misses.is_empty() {
        println!("[{label}] Missed {} queries:", misses.len());
        for m in misses.iter().take(10) {
            println!("  {m}");
        }
    }

    assert!(
        recall >= 0.60,
        "[{label}] Recall@10 {:.3} < 0.60 ({hits}/{total})",
        recall
    );
}

/// Baseline SciFact Recall@10: full-document embedding (historical harness).
#[test]
fn scifact_recall_fulltext_single_embedding_benchmark() {
    run_recall_benchmark(
        "fulltext-single",
        FULLTEXT_SINGLE_EMBEDDINGS_GZIP_FILENAME,
        SeedStrategy::FulltextSingle,
    );
}

/// SciFact Recall@10 with sentence-based `chunk_text` + neighbor-padding chunks.
#[test]
fn scifact_recall_sentence_chunk_neighbor_pad_benchmark() {
    run_recall_benchmark(
        "sentence-chunk",
        SENTENCE_CHUNK_MULTICHUNK_EMBEDDINGS_GZIP_FILENAME,
        SeedStrategy::SentenceChunks,
    );
}
