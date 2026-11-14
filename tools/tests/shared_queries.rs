//! Integration tests for the shared query helpers extracted from the MCP
//! sidecar into `cli_common` (Task 2): recall_chunks + resolve_symbol.
//! Seeding lives in the shared `common` module (factored out in Task 4).

mod common;

use temp_env::with_vars;
use tempfile::tempdir;

use common::{seed_vault, AST_CHUNK_TEXT, DOC_PATH};
use curated_thoughts_tools::cli_common::{recall_chunks, resolve_symbol};

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
            let paths = tauri_app_lib::retrieval::resolve_brain_paths();
            let conn = tauri_app_lib::retrieval::open_brain_readonly(&paths.db_path)
                .expect("readonly open");
            let profile = tauri_app_lib::retrieval::load_embed_profile(&paths.config_path).unwrap();

            // ast_only=true must return only the ast-strategy chunk.
            let hits = recall_chunks(&conn, &profile, "my_fn", 10, true).expect("recall_chunks");
            assert_eq!(hits.len(), 1, "ast_only leg should exclude the prose chunk");
            assert_eq!(hits[0].chunk_text, AST_CHUNK_TEXT);
            assert_eq!(hits[0].doc_path, DOC_PATH);
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

/// Verbatim-twin parity (working-memory-age plan Task 3, Step 2): BOTH
/// crates' RECALL_CHUNKS_SQL_BASE copies must carry the age-out fragment
/// with IDENTICAL text, and the src-tauri copy must byte-equal the tools
/// copy.
#[test]
fn recall_chunks_sql_base_fragment_is_identical_in_both_crates() {
    let tools_copy = curated_thoughts_tools::queries::RECALL_CHUNKS_SQL_BASE;
    let tauri_copy = tauri_app_lib::tool_dispatch::RECALL_CHUNKS_SQL_BASE;
    assert_eq!(
        tools_copy, tauri_copy,
        "the verbatim twins drifted — fix the src-tauri copy, not the test"
    );
    // Both NULL arms mandatory, outcome='indexed' only.
    for needle in [
        "ct_folder_age_days(d.path) IS NULL",
        "(SELECT MIN(ir.run_at) FROM ingest_runs ir\n        WHERE ir.doc_id = c.doc_id AND ir.outcome = 'indexed') IS NULL",
        "(unixepoch()\n       - (SELECT MIN(ir.run_at) FROM ingest_runs ir\n          WHERE ir.doc_id = c.doc_id AND ir.outcome = 'indexed')) / 86400",
        "<= ct_folder_age_days(d.path)",
    ] {
        assert!(
            tools_copy.contains(needle),
            "age fragment missing arm: {needle:?}"
        );
    }
}

/// Empty-config byte identity (spec §2 promise, plan Task 3 Step 3): with
/// NO `folder_ages` config, `semantic_search` returns exactly the rows it
/// returned before the fragment (both seeded chunks, same ordering).
#[test]
fn semantic_search_without_folder_ages_config_is_byte_identical() {
    let brain = tempdir().unwrap();
    let brain_path = brain.path().to_path_buf();

    with_vars(
        [
            ("CURATED_BRAIN_DIR", Some(brain_path.to_str().unwrap())),
            ("CURATED_EMBED_STUB", Some("constant8")),
        ],
        || {
            seed_vault(&brain_path);
            let paths = tauri_app_lib::retrieval::resolve_brain_paths();
            let conn = tauri_app_lib::retrieval::open_brain_readonly(&paths.db_path)
                .expect("readonly open");
            let profile = tauri_app_lib::retrieval::load_embed_profile(&paths.config_path).unwrap();
            let query_vec =
                tauri_app_lib::embedder::embed_one(&profile, AST_CHUNK_TEXT.to_string()).unwrap();

            // Pre-fragment expectation: both chunks, ast chunk first (it is
            // the query), prose second — identical WITH the fragment because
            // no folder_ages config exists in this brain.
            let hits =
                tauri_app_lib::search::semantic_search(&conn, &query_vec, 10).expect("semantic_search");
            assert_eq!(
                hits.len(),
                2,
                "empty folder_ages config must leave row count unchanged"
            );
            assert_eq!(hits[0].chunk_text, AST_CHUNK_TEXT);
            assert!(hits.iter().any(|h| h.chunk_text.contains("plain prose")));
        },
    );
}
