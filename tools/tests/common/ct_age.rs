//! Fixture for the working-memory-age Task 4 tools-side test: mirrors
//! src-tauri/tests/ct_age_fixture.rs (the crates can't share test code).

use std::fs;

use tauri_app_lib::chunker::{Chunk, ChunkStrategyTag};
use tauri_app_lib::embedder::embed_one;
use tauri_app_lib::retrieval::{
    self, AppDb, insert_chunk, insert_embedding, mark_document_indexed, upsert_document,
};

pub const AGED_DOC_REL: &str = "records/aged.md";
pub const FRESH_DOC_REL: &str = "notes/fresh.md";

/// Absolute doc paths under the REAL vault root — the scalar strips the
/// configured vault_path, so stored document paths must live under it.
pub fn doc_paths(brain_path: &std::path::Path) -> (String, String) {
    let vault = brain_path.join("vault");
    (
        vault.join(AGED_DOC_REL).to_string_lossy().into_owned(),
        vault.join(FRESH_DOC_REL).to_string_lossy().into_owned(),
    )
}

/// Seed a temp brain with an aged `records/` doc (cutoff 0 days, ingest
/// backdated 400 days) and an unaged `notes/` doc; returns (aged, fresh).
/// MUST run under `CURATED_BRAIN_DIR=<brain>` and `CURATED_EMBED_STUB=constant8`.
pub fn seed_aged_brain(brain_path: &std::path::Path) -> (String, String) {
    let (aged_path, fresh_path) = doc_paths(brain_path);
    let vault = brain_path.join("vault");
    fs::create_dir_all(vault.join("records")).unwrap();
    fs::create_dir_all(vault.join("notes")).unwrap();
    let vault_str = vault.to_str().unwrap();
    fs::write(
        brain_path.join("config.json"),
        format!(r#"{{"vault_path":"{vault_str}","ingest":{{"folder_ages":{{"records":0}}}}}}"#),
    )
    .unwrap();

    let paths = retrieval::resolve_brain_paths();
    let db = AppDb::open_with_config(&paths.db_path, &paths.config_path)
        .expect("writable brain db open");
    let profile = retrieval::load_embed_profile(&paths.config_path).unwrap();

    let mut doc_ids = Vec::new();
    for (path, text) in [
        (aged_path.as_str(), "aged fixture chunk about wma_fn"),
        (fresh_path.as_str(), "fresh fixture chunk about wma_fn"),
    ] {
        let doc_id = upsert_document(&db.0, path, "h_fixture").unwrap();
        let chunk = Chunk {
            text: text.into(),
            start_line: 1,
            end_line: 1,
            symbol_name: Some("wma_fn".into()),
            defined_symbol: Some("wma_fn".into()),
            strategy: ChunkStrategyTag::AstSymbolRust,
        };
        let chunk_id = insert_chunk(&db.0, doc_id, &chunk, 0, "tier_fact", "").unwrap();
        let v = embed_one(&profile, chunk.text.clone()).unwrap();
        insert_embedding(&db.0, chunk_id, &v).unwrap();
        mark_document_indexed(&db.0, doc_id, "seed-h").unwrap();
        doc_ids.push((path, doc_id));
    }

    // Backdating: push the aged doc's ingest history past its 0-day cutoff.
    let aged_id = doc_ids.iter().find(|(p, _)| *p == aged_path).unwrap().1;
    db.0.execute(
        "UPDATE ingest_runs SET run_at = unixepoch() - 400*86400 WHERE doc_id = ?1",
        rusqlite::params![aged_id],
    )
    .unwrap();
    (aged_path, fresh_path)
}
