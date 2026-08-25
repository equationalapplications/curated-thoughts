//! Integration tests for the shared query helpers extracted from the MCP
//! sidecar into `cli_common` (Task 2): recall_chunks + resolve_symbol.

use std::fs;

use temp_env::with_vars;
use tempfile::tempdir;

use curated_thoughts_tools::cli_common::{recall_chunks, resolve_symbol};
use tauri_app_lib::chunker::{Chunk, ChunkStrategyTag};
use tauri_app_lib::embedder::embed_one;
use tauri_app_lib::retrieval::{
    self, insert_chunk, insert_embedding, mark_document_indexed, upsert_document, AppDb,
};

/// Seed a temp brain vault with one ast-strategy chunk defining `my_fn`
/// (with embedding) and one prose chunk (also with an embedding).
fn seed_vault(brain_path: &std::path::Path) -> i64 {
    fs::write(brain_path.join("config.json"), b"{}\n").unwrap();
    let paths = retrieval::resolve_brain_paths();
    let db = AppDb::open(&paths.db_path).expect("writable brain db open");
    let doc_id = upsert_document(&db.0, "/vault/code.rs", "h_fixture").unwrap();

    let ast_chunk = Chunk {
        text: "fn my_fn() { todo!() }".into(),
        start_line: 1,
        end_line: 1,
        symbol_name: Some("my_fn".into()),
        defined_symbol: Some("my_fn".into()),
        strategy: ChunkStrategyTag::AstSymbolRust,
    };
    let chunk_id = insert_chunk(&db.0, doc_id, &ast_chunk, 0, "ent_fixture", "chash1").unwrap();

    let prose_chunk = Chunk {
        text: "plain prose notes about my_fn usage".into(),
        start_line: 2,
        end_line: 2,
        symbol_name: None,
        defined_symbol: None,
        strategy: ChunkStrategyTag::Prose,
    };
    let _prose_id = insert_chunk(&db.0, doc_id, &prose_chunk, 1, "ent_fixture", "chash2").unwrap();

    let profile = retrieval::load_embed_profile(&paths.config_path).unwrap();
    for (idx, text) in [ast_chunk.text.clone(), prose_chunk.text.clone()]
        .into_iter()
        .enumerate()
    {
        let v = embed_one(&profile, text.clone()).unwrap();
        insert_embedding(&db.0, chunk_id + idx as i64, &v).unwrap();
    }
    mark_document_indexed(&db.0, doc_id).unwrap();
    chunk_id
}

#[test]
fn recall_chunks_ast_only_and_resolve_symbol_case_insensitive() {
    let brain = tempdir().unwrap();
    let brain_path = brain.path().to_path_buf();

    with_vars(
        [
            ("CURATED_BRAIN_DIR", Some(brain_path.to_str().unwrap())),
            ("CURATED_EMBED_STUB", Some("constant8")),
        ],
        || {
            let ast_chunk_id = seed_vault(&brain_path);
            let paths = retrieval::resolve_brain_paths();
            let conn = retrieval::open_brain_readonly(&paths.db_path).expect("readonly open");
            let profile = retrieval::load_embed_profile(&paths.config_path).unwrap();

            // ast_only=true must return only the ast-strategy chunk.
            let hits = recall_chunks(&conn, &profile, "my_fn", 10, true).expect("recall_chunks");
            assert_eq!(hits.len(), 1, "ast_only leg should exclude the prose chunk");
            assert_eq!(hits[0].chunk_text, "fn my_fn() { todo!() }");
            assert_eq!(hits[0].doc_path, "/vault/code.rs");
            assert_eq!(hits[0].symbol_name.as_deref(), Some("my_fn"));
            assert_eq!(hits[0].entity_id, "ent_fixture");
            assert!(hits[0].score.is_finite());

            // Without the ast filter both chunks come back.
            let all = recall_chunks(&conn, &profile, "my_fn", 10, false).expect("recall_chunks");
            assert_eq!(all.len(), 2);

            // Symbol resolution is case-insensitive (lowercase+trim normalize)
            // and prefers the defined_symbol row.
            let resolved = resolve_symbol(&conn, "  MY_FN ").expect("resolve_symbol");
            assert_eq!(resolved, Some((ast_chunk_id, "ent_fixture".to_string())));

            // Unknown symbol resolves to None without error.
            let missing = resolve_symbol(&conn, "nope").expect("resolve_symbol miss");
            assert_eq!(missing, None);
        },
    );
}
