//! Completeness + behavior tests for the working-memory age-out filter
//! (spec 2026-11-13-working-memory-age, plan Task 4).
//!
//! Every recall entry point is exercised on a FRESHLY OPENED raw
//! connection: if the consumer forgot `register_ct_functions` the query
//! fails with "no such function: ct_folder_age_days" and the test fails;
//! if the registration is present but the fragment is wrong, the
//! aged-excluded / unaged-kept assertions fail.

mod ct_age_fixture;

use temp_env::with_vars;
use tempfile::tempdir;

use ct_age_fixture::seed_aged_brain;
use tauri_app_lib::retrieval::{self, open_brain_readonly, semantic_search_chunks};
use tauri_app_lib::search;
use tauri_app_lib::tool_dispatch::{
    CuratedRecallContextParams, CuratedSearchCodeParams, ToolDispatchContext,
};

fn doc_paths(hits: &[search::SearchResult]) -> Vec<String> {
    let mut v: Vec<String> = hits.iter().map(|h| h.doc_path.clone()).collect();
    v.sort();
    v
}

fn ctx_for(paths: &retrieval::BrainPaths) -> ToolDispatchContext {
    let conn = open_brain_readonly(&paths.db_path).unwrap();
    let profile = retrieval::load_embed_profile(&paths.config_path).unwrap();
    ToolDispatchContext {
        conn: std::sync::Arc::new(std::sync::Mutex::new(conn)),
        profile,
        vault_dir: None,
        db_path: paths.db_path.clone(),
        rw_conn: Default::default(),
        client: "ct-test".into(),
    }
}

#[test]
fn every_recall_entry_point_carries_the_age_function_on_a_fresh_connection() {
    let brain = tempdir().unwrap();
    let brain_path = brain.path().to_path_buf();

    with_vars(
        [
            ("CURATED_BRAIN_DIR", Some(brain_path.to_str().unwrap())),
            ("CURATED_EMBED_STUB", Some("constant8")),
        ],
        || {
            let (aged_doc, fresh_doc) = seed_aged_brain(&brain_path);
            let paths = retrieval::resolve_brain_paths();

            // 1. semantic_search (the app `search_vault` command path) —
            //    fresh read-only connection; registration must be inside
            //    the function.
            {
                let conn = open_brain_readonly(&paths.db_path).unwrap();
                let profile = retrieval::load_embed_profile(&paths.config_path).unwrap();
                let hits = semantic_search_chunks(&conn, &profile, "fixture", 10).expect(
                    "semantic_search_chunks: `no such function` would mean a missing registration",
                );
                assert_eq!(doc_paths(&hits), vec![fresh_doc.clone()]);
            }

            // 2. search::semantic_search directly (vector-input arm).
            {
                let conn = open_brain_readonly(&paths.db_path).unwrap();
                let qvec = vec![0.0f32; 8];
                let hits = search::semantic_search(&conn, &qvec, 10)
                    .expect("search::semantic_search: missing registration");
                assert_eq!(doc_paths(&hits), vec![fresh_doc.clone()]);
            }

            // 3. related_chunks (the app `get_related_chunks` command path).
            {
                let conn = open_brain_readonly(&paths.db_path).unwrap();
                let hits = search::related_chunks(&conn, &fresh_doc, 10)
                    .expect("search::related_chunks: missing registration");
                assert!(
                    hits.iter().all(|h| h.doc_path != aged_doc),
                    "aged doc leaked into related_chunks"
                );
            }

            // 4. retrieval::related_chunks_facade (same SQL, facade wrapper).
            {
                let conn = open_brain_readonly(&paths.db_path).unwrap();
                let hits = retrieval::related_chunks_facade(&conn, &fresh_doc, 10)
                    .expect("related_chunks_facade: missing registration");
                assert!(hits.iter().all(|h| h.doc_path != aged_doc));
            }

            // 5. tool_dispatch curated_recall_context leg.
            let rt = tokio::runtime::Runtime::new().unwrap();
            let recall = rt
                .block_on(
                    tauri_app_lib::tool_dispatch::dispatch_curated_recall_context(
                        &ctx_for(&paths),
                        CuratedRecallContextParams {
                            query: "fixture".into(),
                            limit_wiki: Some(5),
                            limit_code: Some(10),
                        },
                    ),
                )
                .expect("dispatch_curated_recall_context: missing registration");
            let code = recall["code_chunks"].as_array().expect("code_chunks array");
            assert!(
                code.iter().all(|c| c["doc_path"] != aged_doc),
                "aged doc leaked into curated_recall_context: {recall}"
            );
            assert!(
                code.iter().any(|c| c["doc_path"] == fresh_doc),
                "unaged doc missing from curated_recall_context"
            );

            // 6. tool_dispatch curated_search_code leg (fresh connection).
            let code = rt
                .block_on(tauri_app_lib::tool_dispatch::dispatch_curated_search_code(
                    &ctx_for(&paths),
                    CuratedSearchCodeParams {
                        query: "fixture".into(),
                        limit: Some(10),
                        symbol: None,
                    },
                ))
                .expect("dispatch_curated_search_code: missing registration");
            let rows = code["code_chunks"].as_array().unwrap();
            assert!(rows.iter().all(|c| c["doc_path"] != aged_doc));
            assert!(rows.iter().any(|c| c["doc_path"] == fresh_doc));
        },
    );
}

/// Behavior: aged records doc is hidden; removing `folder_ages` from the
/// config and re-registering brings the SAME chunk back with NO re-ingest.
#[test]
fn age_out_is_reversible_without_reingest() {
    let brain = tempdir().unwrap();
    let brain_path = brain.path().to_path_buf();

    with_vars(
        [
            ("CURATED_BRAIN_DIR", Some(brain_path.to_str().unwrap())),
            ("CURATED_EMBED_STUB", Some("constant8")),
        ],
        || {
            let (aged_doc, fresh_doc) = seed_aged_brain(&brain_path);
            let paths = retrieval::resolve_brain_paths();

            let profile = retrieval::load_embed_profile(&paths.config_path).unwrap();
            {
                let conn = open_brain_readonly(&paths.db_path).unwrap();
                let hits = semantic_search_chunks(&conn, &profile, "fixture", 10).unwrap();
                assert_eq!(
                    doc_paths(&hits),
                    vec![fresh_doc.clone()],
                    "aged doc must be hidden while folder_ages is configured"
                );
            }

            // Row-count evidence that nothing was re-ingested.
            let chunk_count_before: i64 = {
                let conn = rusqlite::Connection::open(&paths.db_path).unwrap();
                conn.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))
                    .unwrap()
            };

            // Remove the age from the config; the content-keyed policy
            // cache serves the new bytes on the NEXT registration, which
            // is the next query.
            std::fs::write(&paths.config_path, b"{}\n").unwrap();

            {
                let conn = open_brain_readonly(&paths.db_path).unwrap();
                let hits = semantic_search_chunks(&conn, &profile, "fixture", 10).unwrap();
                let found = doc_paths(&hits);
                assert!(
                    found.contains(&aged_doc.clone()),
                    "un-aged chunk must REAPPEAR after the config edit: {found:?}"
                );
                assert!(found.contains(&fresh_doc.clone()));
            }

            let chunk_count_after: i64 = {
                let conn = rusqlite::Connection::open(&paths.db_path).unwrap();
                conn.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))
                    .unwrap()
            };
            assert_eq!(
                chunk_count_before, chunk_count_after,
                "reversibility must not touch ingest: chunk count changed"
            );
        },
    );
}

/// Pre-V10 arm: a document with NO ingest_runs row at all stays visible
/// inside an aged folder (the `MIN(...) IS NULL` arm short-circuits).
#[test]
fn doc_without_ingest_runs_row_stays_visible_in_aged_folder() {
    let brain = tempdir().unwrap();
    let brain_path = brain.path().to_path_buf();

    with_vars(
        [
            ("CURATED_BRAIN_DIR", Some(brain_path.to_str().unwrap())),
            ("CURATED_EMBED_STUB", Some("constant8")),
        ],
        || {
            let (aged_doc, _fresh_doc) = seed_aged_brain(&brain_path);
            let paths = retrieval::resolve_brain_paths();

            // Strip the ingest history the seeding path wrote for the aged
            // doc — exactly what a pre-V10 brain looks like.
            {
                let conn = rusqlite::Connection::open(&paths.db_path).unwrap();
                conn.execute(
                    "DELETE FROM ingest_runs WHERE doc_id IN
                     (SELECT id FROM documents WHERE path = ?1)",
                    rusqlite::params![aged_doc.as_str()],
                )
                .unwrap();
            }

            let conn = open_brain_readonly(&paths.db_path).unwrap();
            let profile = retrieval::load_embed_profile(&paths.config_path).unwrap();
            let hits = semantic_search_chunks(&conn, &profile, "fixture", 10).unwrap();
            assert!(
                doc_paths(&hits).contains(&aged_doc.clone()),
                "pre-V10 doc (no ingest_runs row) must stay visible inside an aged folder",
            );
        },
    );
}
