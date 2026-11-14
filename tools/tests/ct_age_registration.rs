//! Task 4 completeness for the tools-crate consumers: `recall_chunks`
//! (library + `ct recall`) and the SQL of the two `curated_thoughts_mcp`
//! consumers (`curated_recall_context` / `curated_search_code`). The bin
//! itself can't be linked from a test, so we run the EXACT statement it
//! prepares (`format!("{RECALL_CHUNKS_SQL_BASE}{RECALL_CHUNKS_AST_FILTER}")`
//! via `fetch_ranked_chunks`) on a freshly opened raw connection after
//! `register_ct_functions` — the same registration the bin performs before
//! each prepare (added in Task 4).

mod common;

use temp_env::with_vars;
use tempfile::tempdir;

use common::ct_age::{doc_paths, seed_aged_brain};
use curated_thoughts_tools::cli_common::{
    RECALL_CHUNKS_AST_FILTER, RECALL_CHUNKS_SQL_BASE, fetch_ranked_chunks, recall_chunks,
};
use tauri_app_lib::retrieval;

#[test]
fn recall_chunks_and_mcp_bin_sql_filter_aged_docs_on_fresh_connections() {
    let brain = tempdir().unwrap();
    let brain_path = brain.path().to_path_buf();

    with_vars(
        [
            ("CURATED_BRAIN_DIR", Some(brain_path.to_str().unwrap())),
            ("CURATED_EMBED_STUB", Some("constant8")),
        ],
        || {
            seed_aged_brain(&brain_path);
            let (aged_doc, fresh_doc) = doc_paths(&brain_path);
            let paths = retrieval::resolve_brain_paths();
            let profile = retrieval::load_embed_profile(&paths.config_path).unwrap();

            // recall_chunks on a freshly opened raw connection.
            {
                let conn = rusqlite::Connection::open(&paths.db_path).unwrap();
                let hits = recall_chunks(&conn, &profile, "fixture", 10, false)
                    .expect("recall_chunks: `no such function` would mean a missing registration");
                let got: Vec<&str> = hits.iter().map(|h| h.doc_path.as_str()).collect();
                assert!(
                    !got.contains(&aged_doc.as_str()),
                    "aged doc leaked: {got:?}"
                );
                assert!(
                    got.contains(&fresh_doc.as_str()),
                    "unaged doc missing: {got:?}"
                );
            }

            // The MCP bin's curated_recall_context / curated_search_code
            // statement, fresh connection, register-then-prepare like the bin.
            {
                let conn = rusqlite::Connection::open(&paths.db_path).unwrap();
                tauri_app_lib::register_ct_functions(&conn);
                let sql = format!("{RECALL_CHUNKS_SQL_BASE}{RECALL_CHUNKS_AST_FILTER}");
                let qvec = [0.0f32; 8];
                let rows = fetch_ranked_chunks(&conn, &sql, &[], &qvec, 10)
                    .expect("bin SQL: missing registration");
                let got: Vec<&str> = rows.iter().map(|r| r.doc_path.as_str()).collect();
                assert!(
                    !got.contains(&aged_doc.as_str()),
                    "aged doc leaked: {got:?}"
                );
                assert!(
                    got.contains(&fresh_doc.as_str()),
                    "unaged doc missing: {got:?}"
                );
            }
        },
    );
}
