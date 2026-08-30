//! Micro-benchmark for [`tauri_app_lib::search::semantic_search`]: scans every indexed embedding,
//! computes cosine similarity in Rust, sorts, and takes `limit` hits (**O(#chunks)** per query).
//!
//! Default row count **3000**; override with first CLI arg. Uses **`CURATED_EMBED_STUB=constant8`**
//! so embeddings need no network (same as MCP integration tests).
//!
//! ```text
//! CURATED_EMBED_STUB=constant8 cargo run --manifest-path tools/Cargo.toml --release --bin semantic_search_profile -- 8000
//! ```

use std::path::PathBuf;
use std::time::Instant;

use tauri_app_lib::chunker::{Chunk, ChunkStrategyTag};
use tauri_app_lib::db::{
    insert_chunk, insert_embedding, mark_document_indexed, upsert_document, AppDb,
};
use tauri_app_lib::embedder::{embed_one, EmbedProfile};
use tauri_app_lib::retrieval::resolve_brain_paths;
use tauri_app_lib::search::semantic_search;

fn temp_db_path() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("ct_semantic_profile_{nanos}.db"))
}

fn main() {
    std::env::set_var("CURATED_EMBED_STUB", "constant8");

    let n: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(3000);

    let db_path = temp_db_path();
    let paths = resolve_brain_paths();
    let db = AppDb::open_with_config(&db_path, &paths.config_path).expect("open db");
    let conn = &db.0;

    let profile = EmbedProfile::default();
    let query_vec = embed_one(&profile, "bench query".into()).expect("query embed");

    let doc_id = upsert_document(conn, "/bench/doc.rs", "profile-hash").expect("upsert doc");
    let proto = Chunk {
        text: "synthetic semantic_search profile chunk".into(),
        start_line: 1,
        end_line: 4,
        symbol_name: Some("profile_sym".into()),
        strategy: ChunkStrategyTag::AstSymbolRust,
        defined_symbol: None,
    };

    for i in 0..n {
        let cid = insert_chunk(
            conn,
            doc_id,
            &proto,
            i,
            "tier_working",
            &format!("bench-hash-{i}"),
        )
        .expect("chunk");
        insert_embedding(conn, cid, &query_vec).expect("embedding");
    }
    mark_document_indexed(conn, doc_id).expect("indexed");

    let _warm = semantic_search(conn, &query_vec, 10).expect("warmup");

    let rounds = 5u64;
    let t0 = Instant::now();
    for _ in 0..rounds {
        let _ = semantic_search(conn, &query_vec, 10).expect("query");
    }
    let elapsed = t0.elapsed();
    let ms = elapsed.as_secs_f64() * 1000.0 / rounds as f64;

    drop(db);
    let _ = std::fs::remove_file(&db_path);

    eprintln!(
        "semantic_search: {n} chunks (all indexed), top_k=10, {rounds} rounds, {:.2} ms/query mean.",
        ms
    );
    eprintln!(
        "Scaling: latency grows ~linearly with chunk count (full scan). When hot paths justify it, migrate to approximate search (sqlite-vec, USearch, sqlite-vss, external ANN)."
    );
}
