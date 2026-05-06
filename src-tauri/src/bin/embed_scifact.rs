//! One-time script: embed the SciFact corpus with AllMiniLML6V2 and write
//! scifact-embeddings.json.gz to tests/fixtures/scifact/.
//! Run: cargo run --bin embed_scifact

use flate2::{write::GzEncoder, Compression};
use std::io::{BufRead, Write};
use tauri_app_lib::embedder::Embedder;

fn main() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let fixtures = format!("{manifest_dir}/tests/fixtures/scifact");
    let corpus_path = format!("{fixtures}/corpus.jsonl");
    let out_path = format!("{fixtures}/scifact-embeddings.json.gz");

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

    println!("Loaded {} docs. Initializing embedder (downloads model on first run)...", docs.len());
    let embedder = Embedder::new().expect("embedder init");

    let batch_size = 64;
    let mut embeddings: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();

    for (batch_idx, chunk) in docs.chunks(batch_size).enumerate() {
        let texts: Vec<String> = chunk.iter().map(|(_, t)| t.clone()).collect();
        let vecs = embedder.embed(texts).expect("embed batch");
        for ((id, _), vec) in chunk.iter().zip(vecs.iter()) {
            let arr: Vec<serde_json::Value> = vec.iter()
                .map(|&f| serde_json::Value::from(f))
                .collect();
            embeddings.insert(id.clone(), serde_json::Value::Array(arr));
        }
        if batch_idx % 10 == 0 {
            println!("  batch {}/{}", batch_idx + 1, (docs.len() + batch_size - 1) / batch_size);
        }
    }

    println!("Embedded {} docs. Writing {}...", embeddings.len(), out_path);

    let out_file = std::fs::File::create(&out_path).expect("create output file");
    let mut gz = GzEncoder::new(out_file, Compression::default());
    let json = serde_json::to_string(&embeddings).expect("serialize");
    gz.write_all(json.as_bytes()).expect("write gzip");
    gz.finish().expect("finish gzip");

    println!("Done. Output: {out_path}");
}
