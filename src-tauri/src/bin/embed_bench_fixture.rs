//! Precompute sentence-chunk embeddings for a recall bench fixture directory.
//! Usage: `cargo run --example embed_bench_fixture -- <subdir>`
//!
//! Writes the **canonical gzip JSON basename** from `recall_bench_fixture`; commit that file next
//! to `corpus.jsonl` after regenerating so others can reuse embeddings without FastEmbed.
//!
//! `subdir` must be one of: `yaml-bench-synthetic`, `yaml-bench-k8s-curated`,
//! `code-bench-synthetic`, `code-bench-curated`.

use flate2::{write::GzEncoder, Compression};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use tauri_app_lib::chunker::chunk_text;
use tauri_app_lib::embedder::Embedder;
use tauri_app_lib::recall_bench_fixture::{
    CODE_CURATED_EMBEDDINGS_GZIP, CODE_SYNTHETIC_EMBEDDINGS_GZIP, YAML_K8S_CURATED_EMBEDDINGS_GZIP,
    YAML_SYNTHETIC_EMBEDDINGS_GZIP,
};

const EMBED_BATCH: usize = 128;

fn resolve_out_name(subdir: &str) -> &'static str {
    match subdir {
        "yaml-bench-synthetic" => YAML_SYNTHETIC_EMBEDDINGS_GZIP,
        "yaml-bench-k8s-curated" => YAML_K8S_CURATED_EMBEDDINGS_GZIP,
        "code-bench-synthetic" => CODE_SYNTHETIC_EMBEDDINGS_GZIP,
        "code-bench-curated" => CODE_CURATED_EMBEDDINGS_GZIP,
        other => {
            eprintln!(
                "Unknown subdir {other:?}. Use: yaml-bench-synthetic | yaml-bench-k8s-curated | code-bench-synthetic | code-bench-curated"
            );
            std::process::exit(2);
        }
    }
}

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
    let subdir = std::env::args()
        .nth(1)
        .expect("usage: embed_bench_fixture SUBDIR (e.g. yaml-bench-synthetic)");
    let out_name = resolve_out_name(&subdir);

    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fix = manifest.join("tests/fixtures").join(&subdir);
    let corpus_path = fix.join("corpus.jsonl");
    let out_path = fix.join(out_name);

    println!("Reading {}", corpus_path.display());
    let file = std::fs::File::open(&corpus_path).expect("corpus.jsonl");
    let reader = std::io::BufReader::new(file);

    let mut docs: Vec<(String, String)> = Vec::new();
    for line in reader.lines() {
        let line = line.unwrap();
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(&line).expect("jsonl");
        let id = v["_id"].as_str().unwrap_or("").to_string();
        let title = v["title"].as_str().unwrap_or("");
        let text = v["text"].as_str().unwrap_or("");
        docs.push((id, format!("{title} {text}")));
    }

    println!(
        "Embedding {} docs with chunk_text() (batch {})…",
        docs.len(),
        EMBED_BATCH
    );
    let embedder = Embedder::new().expect("embedder");

    let doc_chunks: Vec<(String, Vec<String>)> = docs
        .into_iter()
        .filter_map(|(id, combined)| {
            let chunks = chunk_text(&combined);
            (!chunks.is_empty()).then_some((id, chunks))
        })
        .collect();

    let mut chunks_out: HashMap<String, Vec<Vec<f32>>> = HashMap::with_capacity(doc_chunks.len());
    for (id, chunks) in &doc_chunks {
        chunks_out.insert(id.clone(), vec![Vec::new(); chunks.len()]);
    }

    let total_chunks: usize = doc_chunks.iter().map(|(_, c)| c.len()).sum();
    let mut pending_text: Vec<String> = Vec::with_capacity(EMBED_BATCH);
    let mut pending_meta: Vec<(String, usize)> = Vec::with_capacity(EMBED_BATCH);
    let mut done: usize = 0;

    let mut flush = |pt: &mut Vec<String>, pm: &mut Vec<(String, usize)>| {
        let n = flush_embedding_batch(&embedder, pt, pm, &mut chunks_out);
        if n > 0 {
            done += n;
            if done % 256 < n || done >= total_chunks {
                println!("  … {done}/{total_chunks} chunks");
            }
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
        if i % 50 == 0 {
            println!("  doc {}/{}", i + 1, doc_chunks.len());
        }
    }
    flush(&mut pending_text, &mut pending_meta);

    let mut embeddings: Map<String, Value> = Map::new();
    for (id, vecs) in chunks_out {
        let rows: Vec<Value> = vecs
            .into_iter()
            .map(|v| Value::Array(v.into_iter().map(|f| Value::from(f as f64)).collect()))
            .collect();
        embeddings.insert(id, Value::Array(rows));
    }

    println!("Writing {}", out_path.display());
    let out_file = std::fs::File::create(&out_path).expect("create gz");
    let mut gz = GzEncoder::new(out_file, Compression::default());
    let json = serde_json::to_string(&embeddings).expect("serialize");
    gz.write_all(json.as_bytes()).expect("write");
    gz.finish().expect("finish");
    println!("Done.");
}
