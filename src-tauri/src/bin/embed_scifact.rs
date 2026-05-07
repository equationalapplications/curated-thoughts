//! One-time script: embed the SciFact corpus with AllMiniLML6V2 and write
//! gzipped JSON (`scifact_fixture::EMBEDDINGS_GZIP_FILENAME`) to tests/fixtures/scifact/.
//! Run: cargo run --bin embed_scifact

use flate2::{write::GzEncoder, Compression};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use tauri_app_lib::chunker::chunk_text;
use tauri_app_lib::embedder::Embedder;
use tauri_app_lib::scifact_fixture::EMBEDDINGS_GZIP_FILENAME;

/// Cross-doc batch size (fastembed amortizes work per call).
const EMBED_BATCH: usize = 128;

fn flush_embedding_batch(
    embedder: &Embedder,
    pending_text: &mut Vec<String>,
    pending_meta: &mut Vec<(String, usize)>,
    chunks_out: &mut HashMap<String, Vec<Vec<f32>>>,
) -> usize {
    if pending_text.is_empty() {
        return 0;
    }
    let n = pending_text.len();
    let texts = std::mem::take(pending_text);
    let meta = std::mem::take(pending_meta);
    let vecs = embedder.embed(texts).expect("embed batch");
    for ((doc_id, ci), emb) in meta.into_iter().zip(vecs) {
        chunks_out.get_mut(&doc_id).expect("doc slot")[ci] = emb;
    }
    n
}

fn main() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let fixtures = format!("{manifest_dir}/tests/fixtures/scifact");
    let corpus_path = format!("{fixtures}/corpus.jsonl");
    let out_path = format!("{fixtures}/{EMBEDDINGS_GZIP_FILENAME}");

    println!("Loading corpus from {corpus_path}");

    let file = std::fs::File::open(&corpus_path).expect("corpus.jsonl not found");
    let reader = std::io::BufReader::new(file);

    let mut docs: Vec<(String, String)> = Vec::new();
    for line in reader.lines() {
        let line = line.unwrap();
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(&line).expect("invalid JSON");
        let id = v["_id"].as_str().unwrap_or("").to_string();
        let title = v["title"].as_str().unwrap_or("");
        let text = v["text"].as_str().unwrap_or("");
        let combined = format!("{title} {text}");
        docs.push((id, combined));
    }

    println!(
        "Loaded {} lines. Chunking sentence-aware for embedding…",
        docs.len()
    );
    let embedder = Embedder::new().expect("embedder init");

    let doc_chunks: Vec<(String, Vec<String>)> = docs
        .into_iter()
        .filter_map(|(id, combined)| {
            let chunks = chunk_text(&combined);
            (!chunks.is_empty()).then_some((id, chunks))
        })
        .collect();

    println!(
        "Prepared {} docs with chunks (batch size {EMBED_BATCH})…",
        doc_chunks.len()
    );

    let mut chunks_out: HashMap<String, Vec<Vec<f32>>> =
        HashMap::with_capacity(doc_chunks.len());
    for (id, chunks) in &doc_chunks {
        chunks_out.insert(id.clone(), vec![Vec::new(); chunks.len()]);
    }

    let mut pending_text: Vec<String> = Vec::with_capacity(EMBED_BATCH);
    let mut pending_meta: Vec<(String, usize)> = Vec::with_capacity(EMBED_BATCH);

    let total_chunks: usize = doc_chunks.iter().map(|(_, c)| c.len()).sum();

    let mut embedded_chunks_done: usize = 0;

    let mut flush = |pending_text: &mut Vec<String>, pending_meta: &mut Vec<(String, usize)>| {
        let n = flush_embedding_batch(&embedder, pending_text, pending_meta, &mut chunks_out);
        if n == 0 {
            return;
        }
        embedded_chunks_done += n;
        if embedded_chunks_done % 4096 <= n || embedded_chunks_done >= total_chunks {
            println!("  … {embedded_chunks_done}/{total_chunks} chunks embedded",);
        }
    };

    for (i, (id, chunks)) in doc_chunks.iter().enumerate() {
        for (ci, part) in chunks.iter().enumerate() {
            pending_meta.push((id.clone(), ci));
            pending_text.push(part.clone());
            if pending_text.len() >= EMBED_BATCH {
                flush(&mut pending_text, &mut pending_meta);
            }
        }
        if i % 500 == 0 {
            println!("  embedded chunk streams for {}/{} docs", i + 1, doc_chunks.len());
        }
    }
    flush(&mut pending_text, &mut pending_meta);

    let mut embeddings: Map<String, Value> = Map::new();
    for (id, vecs) in chunks_out {
        let rows: Vec<Value> = vecs
            .into_iter()
            .map(|v| {
                Value::Array(
                    v.into_iter()
                        .map(|f| Value::from(f as f64))
                        .collect(),
                )
            })
            .collect();
        embeddings.insert(id, Value::Array(rows));
    }

    println!("Embedded {} docs. Writing {}…", embeddings.len(), out_path);

    let out_file = std::fs::File::create(&out_path).expect("create output file");
    let mut gz = GzEncoder::new(out_file, Compression::default());
    let json = serde_json::to_string(&embeddings).expect("serialize");
    gz.write_all(json.as_bytes()).expect("write gzip");
    gz.finish().expect("finish gzip");

    println!("Done. Output: {out_path}");
}
