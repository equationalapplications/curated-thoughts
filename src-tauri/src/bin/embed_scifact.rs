//! One-time script: embed the SciFact corpus with AllMiniLML6V2 and write gzipped JSON
//! under `tests/fixtures/scifact/` (see `tauri_app_lib::scifact_fixture` for filenames).
//!
//! ```text
//! cargo run --example embed_scifact --features dev-tools -- fulltext-single   # one vector per doc, full title+text
//! cargo run --example embed_scifact --features dev-tools -- sentence-chunk    # sentence chunking + neighbor padding (default)
//! ```

use flate2::{write::GzEncoder, Compression};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use tauri_app_lib::chunker::chunk_text;
use tauri_app_lib::embedder::Embedder;
use tauri_app_lib::scifact_fixture::{
    FULLTEXT_SINGLE_EMBEDDINGS_GZIP_FILENAME, SENTENCE_CHUNK_MULTICHUNK_EMBEDDINGS_GZIP_FILENAME,
};

/// Cross-doc batch size (fastembed amortizes work per call).
const EMBED_BATCH: usize = 128;

#[derive(Clone, Copy)]
enum Preset {
    FulltextSingle,
    SentenceChunk,
}

fn parse_preset(arg: Option<String>) -> Preset {
    match arg.as_deref() {
        None | Some("sentence-chunk") | Some("v2") | Some("multichunk") => Preset::SentenceChunk,
        Some("fulltext-single") | Some("fulltext") | Some("v1") | Some("single-vec") => {
            Preset::FulltextSingle
        }
        Some(other) => {
            eprintln!(
                "Unknown preset {other:?}.\n\
                 Usage: cargo run --example embed_scifact --features dev-tools -- [fulltext-single|sentence-chunk]\n\
                 \n\
                 Presets correspond to filenames in scifact_fixture:\n\
                 - fulltext-single — {}\n\
                 - sentence-chunk — {}",
                FULLTEXT_SINGLE_EMBEDDINGS_GZIP_FILENAME,
                SENTENCE_CHUNK_MULTICHUNK_EMBEDDINGS_GZIP_FILENAME
            );
            std::process::exit(2);
        }
    }
}

fn gzip_name(p: Preset) -> &'static str {
    match p {
        Preset::FulltextSingle => FULLTEXT_SINGLE_EMBEDDINGS_GZIP_FILENAME,
        Preset::SentenceChunk => SENTENCE_CHUNK_MULTICHUNK_EMBEDDINGS_GZIP_FILENAME,
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

fn embed_sentence_chunk(docs: Vec<(String, String)>, embedder: &Embedder, out_path: &str) {
    println!("Preset sentence-chunk: chunk_text() groups, TARGET_WORDS neighbors (see chunker).");

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

    let mut chunks_out: HashMap<String, Vec<Vec<f32>>> = HashMap::with_capacity(doc_chunks.len());
    for (id, chunks) in &doc_chunks {
        chunks_out.insert(id.clone(), vec![Vec::new(); chunks.len()]);
    }

    let mut pending_text: Vec<String> = Vec::with_capacity(EMBED_BATCH);
    let mut pending_meta: Vec<(String, usize)> = Vec::with_capacity(EMBED_BATCH);

    let total_chunks: usize = doc_chunks.iter().map(|(_, c)| c.len()).sum();
    let mut embedded_chunks_done: usize = 0;

    let mut flush = |pending_text: &mut Vec<String>, pending_meta: &mut Vec<(String, usize)>| {
        let n = flush_embedding_batch(embedder, pending_text, pending_meta, &mut chunks_out);
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
            println!(
                "  embedded chunk streams for {}/{} docs",
                i + 1,
                doc_chunks.len()
            );
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

    println!("Embedded {} docs. Writing {}…", embeddings.len(), out_path);
    write_gz_json(out_path, embeddings);
}

fn embed_fulltext_single(docs: Vec<(String, String)>, embedder: &Embedder, out_path: &str) {
    println!("Preset fulltext-single: one embedding per doc (combined title + text).");

    let mut embeddings: Map<String, Value> = Map::new();

    let batch_size = 64usize;
    for batch_idx in 0..docs.len().div_ceil(batch_size) {
        let chunk = &docs[batch_idx * batch_size..((batch_idx + 1) * batch_size).min(docs.len())];
        let texts: Vec<String> = chunk.iter().map(|(_, t)| t.clone()).collect();
        let vecs = embedder.embed(texts).expect("embed batch");
        for ((id, _), vec) in chunk.iter().zip(vecs.iter()) {
            let arr: Vec<Value> = vec.iter().map(|&f| Value::from(f as f64)).collect();
            embeddings.insert(id.clone(), Value::Array(arr));
        }
        if batch_idx % 10 == 0 {
            println!(
                "  batch {}/{chunks}",
                batch_idx + 1,
                chunks = docs.len().div_ceil(batch_size)
            );
        }
    }

    println!("Embedded {} docs. Writing {}…", embeddings.len(), out_path);
    write_gz_json(out_path, embeddings);
}

fn write_gz_json(out_path: &str, embeddings: Map<String, Value>) {
    let out_file = std::fs::File::create(out_path).expect("create output file");
    let mut gz = GzEncoder::new(out_file, Compression::default());
    let json = serde_json::to_string(&embeddings).expect("serialize");
    gz.write_all(json.as_bytes()).expect("write gzip");
    gz.finish().expect("finish gzip");
    println!("Done. Output: {out_path}");
}

fn main() {
    let preset = parse_preset(std::env::args().nth(1));
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let fixtures_dir = format!("{manifest_dir}/tests/fixtures/scifact");
    let corpus_path = format!("{fixtures_dir}/corpus.jsonl");
    let gzip = gzip_name(preset);
    let out_path = format!("{fixtures_dir}/{gzip}");

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

    println!("Loaded {} corpus lines.", docs.len());

    let embedder = Embedder::new().expect("embedder init");

    match preset {
        Preset::SentenceChunk => embed_sentence_chunk(docs, &embedder, &out_path),
        Preset::FulltextSingle => embed_fulltext_single(docs, &embedder, &out_path),
    }
}
