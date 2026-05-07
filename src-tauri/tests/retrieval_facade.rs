//! Integration smoke test for retrieval path resolution + read-only semantic search façade.

use std::fs;

use temp_env::with_vars;
use tempfile::tempdir;

use tauri_app_lib::chunker::{Chunk, ChunkStrategyTag};
use tauri_app_lib::embedder::embed_one;
use tauri_app_lib::retrieval::{
    self, insert_chunk, insert_embedding, mark_document_indexed, upsert_document, AppDb,
};

#[test]
fn retrieval_facade_semantic_search_readonly_stub() {
    let brain = tempdir().unwrap();
    let brain_path = brain.path().to_path_buf();
    fs::write(brain_path.join("config.json"), b"{}\n").unwrap();

    with_vars(
        [
            ("CURATED_BRAIN_DIR", Some(brain_path.to_str().unwrap())),
            ("CURATED_EMBED_STUB", Some("constant8")),
        ],
        || {
            let paths = retrieval::resolve_brain_paths();
            assert_eq!(paths.brain_dir, brain_path);
            assert_eq!(paths.config_path, brain_path.join("config.json"));
            assert_eq!(paths.db_path, brain_path.join("brain.db"));

            {
                let db = AppDb::open(&paths.db_path).expect("writable brain db open");
                let doc_id = upsert_document(&db.0, "/vault/x.md", "h_fixture").unwrap();
                let chunk = Chunk {
                    text: "fixture chunk for search".into(),
                    start_line: 1,
                    end_line: 2,
                    symbol_name: Some("foo".into()),
                    strategy: ChunkStrategyTag::Prose,
                };
                let chunk_id = insert_chunk(&db.0, doc_id, &chunk, 0).unwrap();
                let profile = retrieval::load_embed_profile(&paths.config_path).unwrap();
                let v = embed_one(&profile, chunk.text.clone()).unwrap();
                insert_embedding(&db.0, chunk_id, &v).unwrap();
                mark_document_indexed(&db.0, doc_id).unwrap();
            }

            let conn = retrieval::open_brain_readonly(&paths.db_path).expect("readonly open");

            let hits = retrieval::semantic_search_chunks(&conn, &paths.config_path, "q".into(), 10)
                .expect("semantic_search_chunks");

            assert_eq!(hits.len(), 1);
            assert_eq!(hits[0].symbol_name.as_deref(), Some("foo"));
            assert_eq!(hits[0].chunk_text, "fixture chunk for search");
        },
    );
}
