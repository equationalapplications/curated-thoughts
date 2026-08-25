//! Shared test fixtures for tools integration tests: seed a temp brain vault
//! via the real retrieval API (Task 2 pattern from shared_queries.rs).
#![allow(dead_code)]

use std::fs;
use std::process::{Command, Output};

use temp_env::with_vars;
use tempfile::tempdir;

use tauri_app_lib::chunker::{Chunk, ChunkStrategyTag};
use tauri_app_lib::embedder::embed_one;
use tauri_app_lib::retrieval::{
    self, insert_chunk, insert_embedding, mark_document_indexed, upsert_document, AppDb,
};

pub const AST_CHUNK_TEXT: &str = "fn my_fn() { todo!() }";
pub const DOC_PATH: &str = "/vault/code.rs";

/// Seed a temp brain vault with one ast-strategy chunk defining `my_fn`
/// (with embedding) and one prose chunk (also with an embedding).
pub fn seed_vault(brain_path: &std::path::Path) -> i64 {
    fs::write(brain_path.join("config.json"), b"{}\n").unwrap();
    let paths = retrieval::resolve_brain_paths();
    let db = AppDb::open(&paths.db_path).expect("writable brain db open");
    let doc_id = upsert_document(&db.0, DOC_PATH, "h_fixture").unwrap();

    let ast_chunk = Chunk {
        text: AST_CHUNK_TEXT.into(),
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

/// Open the (existing) brain.db schema at `dir` by going through the real
/// AppDb migration path, so seeded rows always match production DDL.
pub fn init_brain_db(brain_dir: &std::path::Path) {
    fs::write(brain_dir.join("config.json"), b"{}\n").unwrap();
    let paths = retrieval::resolve_brain_paths();
    let _db = AppDb::open(&paths.db_path).expect("writable brain db open");
}

/// Insert a pending `new_entity` proposal with `item_count` items directly via
/// SQL matching the real DDL (`src-tauri/src/db/okf_ddl.rs`). Requires
/// `init_brain_db` to have run first.
pub fn insert_pending_proposal(
    brain_dir: &std::path::Path,
    id: &str,
    item_count: usize,
    created_at: i64,
) {
    let conn = rusqlite::Connection::open(brain_dir.join("brain.db")).expect("open brain.db rw");
    conn.execute(
        "INSERT INTO curated_proposals (
            id, kind, entity_id, proposed_name, proposed_type, reasoning, model, status, created_at
         ) VALUES (?1, 'new_entity', NULL, ?2, NULL, NULL, 'fixture-model', 'pending', ?3)",
        rusqlite::params![id, format!("Entity {id}"), created_at],
    )
    .unwrap();
    for i in 0..item_count {
        conn.execute(
            "INSERT INTO curated_proposal_items (
                id, proposal_id, item_type, target_id, payload, evidence, status
             ) VALUES (?1, ?2, 'fact_add', NULL, ?3, '[]', 'pending')",
            rusqlite::params![
                format!("{id}-item-{i}"),
                id,
                format!(r#"{{"body":"fact {i}","tags":[],"confidence":"inferred"}}"#)
            ],
        )
        .unwrap();
    }
}

/// Run `f` with a freshly seeded temp brain as CURATED_BRAIN_DIR and stub
/// embeddings enabled. `run_ct` must be used inside `f`.
pub fn with_seeded_brain<F: FnOnce()>(f: F) {
    let brain = tempdir().unwrap();
    let brain_path = brain.path().to_path_buf();
    let brain_path_str = brain_path.to_str().unwrap().to_string();
    with_vars(
        [
            ("CURATED_BRAIN_DIR", Some(brain_path_str.as_str())),
            ("CURATED_EMBED_STUB", Some("constant8")),
        ],
        move || {
            seed_vault(&brain_path);
            f();
        },
    );
}

/// Invoke the compiled `ct` binary against the ambient (temp) brain.
pub fn run_ct(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ct"))
        .args(args)
        .output()
        .expect("spawn ct")
}
