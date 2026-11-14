//! Hit-rate gate for the working-memory age-out filter (plan Task 5).
//!
//! Stored embeddings only — NO external model. The corpus is built so the
//! truth is known by construction: each query's relevant fresh doc has
//! cosine 0.9 to its query, while five aged `records/` distractor docs sit
//! at cosines 0.91–0.95. Without the filter the distractors occupy the
//! whole top-5 (hit@5 = 0); with the filter active the fresh doc is the
//! only nonzero candidate (hit@5 = 1).
//!
//! Gate (asserted): aged docs excluded when the filter is active; the
//! unaged ranking is byte-identical between the two configurations (the
//! filter never reorders or drops unaged docs).

use std::fs;

use temp_env::with_vars;
use tempfile::tempdir;

use tauri_app_lib::chunker::{Chunk, ChunkStrategyTag};
use tauri_app_lib::retrieval::{
    self, AppDb, insert_chunk, insert_embedding, mark_document_indexed, upsert_document,
};
use tauri_app_lib::search;
use tauri_app_lib::search::cosine_similarity;

const DIM: usize = 16;
const QUERIES: usize = 8;
const AGED_DISTRACTORS_PER_QUERY: usize = 5;
/// similarity ladder for the aged distractors, all above the fresh 0.9
const DISTRACTOR_SIMS: [f32; 5] = [0.95, 0.94, 0.93, 0.92, 0.91];
const FRESH_SIM: f32 = 0.9;
const K: usize = 5;

fn unit_query(j: usize) -> Vec<f32> {
    let mut v = vec![0.0f32; DIM];
    v[j] = 1.0;
    v
}

/// Fresh doc for query j: unit vector with cosine FRESH_SIM to query j.
fn fresh_vec(j: usize) -> Vec<f32> {
    let mut v = vec![0.0f32; DIM];
    v[j] = FRESH_SIM;
    v[(j + 1) % DIM] = (1.0 - FRESH_SIM * FRESH_SIM).sqrt();
    v
}

#[test]
fn hit_rate_gate_filter_active_vs_inactive() {
    let brain = tempdir().unwrap();
    let brain_path = brain.path().to_path_buf();

    with_vars(
        [("CURATED_BRAIN_DIR", Some(brain_path.to_str().unwrap()))],
        || {
            let vault = brain_path.join("vault");
            fs::create_dir_all(vault.join("records")).unwrap();
            fs::create_dir_all(vault.join("notes")).unwrap();
            let vault_str = vault.to_str().unwrap();

            let set_config = |json: String| {
                fs::write(brain_path.join("config.json"), json).unwrap();
            };
            set_config(format!(
                r#"{{"vault_path":"{vault_str}","ingest":{{"folder_ages":{{"records":0}}}}}}"#
            ));

            let paths = retrieval::resolve_brain_paths();
            let db = AppDb::open_with_config(&paths.db_path, &paths.config_path)
                .expect("writable brain db open");

            let mut aged_doc_ids = Vec::new();
            let fresh_paths: Vec<String> = (0..QUERIES)
                .map(|j| {
                    vault
                        .join(format!("notes/fresh_{j}.md"))
                        .to_string_lossy()
                        .into_owned()
                })
                .collect();
            for j in 0..QUERIES {
                // 1 relevant fresh doc (unaged folder).
                let doc_id = upsert_document(&db.0, &fresh_paths[j], "h").unwrap();
                let chunk = Chunk {
                    text: format!("fresh relevant doc {j}"),
                    start_line: 1,
                    end_line: 1,
                    symbol_name: None,
                    defined_symbol: None,
                    strategy: ChunkStrategyTag::Prose,
                };
                let cid = insert_chunk(&db.0, doc_id, &chunk, 0, "tier_fact", "").unwrap();
                insert_embedding(&db.0, cid, &fresh_vec(j)).unwrap();
                mark_document_indexed(&db.0, doc_id, "h").unwrap();

                // AGED_DISTRACTORS_PER_QUERY aged distractors in records/.
                for (i, s) in DISTRACTOR_SIMS.iter().enumerate() {
                    let path = vault
                        .join(format!("records/aged_{j}_{i}.md"))
                        .to_string_lossy()
                        .into_owned();
                    let d = upsert_document(&db.0, &path, "h").unwrap();
                    let c = Chunk {
                        text: format!("aged distractor {j} {i}"),
                        start_line: 1,
                        end_line: 1,
                        symbol_name: None,
                        defined_symbol: None,
                        strategy: ChunkStrategyTag::Prose,
                    };
                    let id = insert_chunk(&db.0, d, &c, 0, "tier_fact", "").unwrap();
                    let mut v = vec![0.0f32; DIM];
                    // v[j] alone would have cosine 1.0 to the unit query
                    // (its norm would be s). Add the orthogonal remainder
                    // in an unused dimension (dims QUERIES..DIM carry no
                    // query mass) so cosine(query, v) really is s
                    // (CodeRabbit PR #350).
                    v[j] = *s;
                    v[QUERIES + j] = (1.0 - s * s).sqrt();
                    debug_assert!((v.iter().map(|x| x * x).sum::<f32>() - 1.0).abs() < 1e-5);
                    insert_embedding(&db.0, id, &v).unwrap();
                    mark_document_indexed(&db.0, d, "h").unwrap();
                    aged_doc_ids.push(d);
                }
            }

            // Backdating fixture: push every aged doc's ingest history past
            // its 0-day cutoff (Task 5).
            {
                let conn = rusqlite::Connection::open(&paths.db_path).unwrap();
                for id in &aged_doc_ids {
                    conn.execute(
                        "UPDATE ingest_runs SET run_at = unixepoch() - 400*86400 WHERE doc_id = ?1",
                        rusqlite::params![id],
                    )
                    .unwrap();
                }
            }

            // ---- Filter ACTIVE (folder_ages configured) ----
            const RANK_LIMIT: usize = 50;
            let active_results: Vec<Vec<String>> = (0..QUERIES)
                .map(|j| {
                    let conn = rusqlite::Connection::open(&paths.db_path).unwrap();
                    let hits = search::semantic_search(&conn, &unit_query(j), RANK_LIMIT)
                        .expect("active search");
                    hits.iter().map(|h| h.doc_path.clone()).collect::<Vec<_>>()
                })
                .collect();

            // ---- Filter INACTIVE (age removed from config) ----
            set_config("{}".to_string());
            let inactive_results: Vec<Vec<String>> = (0..QUERIES)
                .map(|j| {
                    let conn = rusqlite::Connection::open(&paths.db_path).unwrap();
                    let hits = search::semantic_search(&conn, &unit_query(j), RANK_LIMIT)
                        .expect("inactive search");
                    hits.iter().map(|h| h.doc_path.clone()).collect::<Vec<_>>()
                })
                .collect();

            // Report (captured by the benchmark doc).
            let hit_at_k = |results: &[Vec<String>]| -> f64 {
                results
                    .iter()
                    .enumerate()
                    .filter(|(j, top)| top[..K.min(top.len())].iter().any(|p| p == &fresh_paths[*j]))
                    .count() as f64
                    / QUERIES as f64
            };
            let aged_in = |results: &[Vec<String>]| -> usize {
                results
                    .iter()
                    .flatten()
                    .filter(|p| p.contains("/records/"))
                    .count()
            };
            // Top-5-restricted counter: aged rows counted only within each
            // query's first K results (the ranking window users see).
            let aged_in_top_k = |results: &[Vec<String>]| -> usize {
                results
                    .iter()
                    .map(|top| top[..K.min(top.len())].iter().filter(|p| p.contains("/records/")).count())
                    .sum()
            };
            // Unaged ranking must be IDENTICAL between configs: filter only
            // removes aged rows, never reorders or drops unaged ones.
            let unaged_only = |top: &Vec<String>| -> Vec<String> {
                top.iter()
                    .filter(|p| !p.contains("/records/"))
                    .cloned()
                    .collect()
            };
            let active_unaged: Vec<Vec<String>> = active_results.iter().map(unaged_only).collect();
            let inactive_unaged: Vec<Vec<String>> =
                inactive_results.iter().map(unaged_only).collect();

            let active_hit = hit_at_k(&active_results);
            let inactive_hit = hit_at_k(&inactive_results);
            let delta = active_hit - inactive_hit;
            println!("WMA-BENCH hit@{K} filter-inactive = {inactive_hit:.3}");
            println!("WMA-BENCH hit@{K} filter-active   = {active_hit:.3}");
            println!("WMA-BENCH hit@{K} delta           = {delta:+.3}");
            println!(
                "WMA-BENCH aged rows across full result sets (≤{RANK_LIMIT}/query): inactive={} active={}",
                aged_in(&inactive_results),
                aged_in(&active_results)
            );
            println!(
                "WMA-BENCH aged rows in top-{K}: inactive={} active={}",
                aged_in_top_k(&inactive_results),
                aged_in_top_k(&active_results)
            );
            println!(
                "WMA-BENCH queries={QUERIES} aged_distractors_per_query={AGED_DISTRACTORS_PER_QUERY} corpus_docs={}",
                QUERIES * (1 + AGED_DISTRACTORS_PER_QUERY)
            );
            let _ = cosine_similarity(&[1.0], &[1.0]); // keep import used
            println!(
                "WMA-BENCH unaged_ranking_identical={}",
                active_unaged == inactive_unaged
            );

            // GATE 1: aged docs excluded when the filter is active.
            assert_eq!(
                aged_in(&active_results),
                0,
                "aged docs leaked through the active filter"
            );
            // GATE 2: unaged hit rate unchanged — the unaged ranking is
            // identical with the filter on and off (delta comes only from
            // aged rows leaving the candidate set).
            assert_eq!(
                active_unaged, inactive_unaged,
                "the filter altered unaged results — it must be subtractive only"
            );
            // GATE 3: the filter RECOVERS the fresh docs the aged
            // distractors crowded out.
            assert!(
                active_hit > inactive_hit,
                "expected hit@{K} to improve with the filter active (active={active_hit}, inactive={inactive_hit})"
            );
            assert_eq!(
                inactive_hit, 0.0,
                "fixture defect: aged distractors did not crowd out the fresh docs"
            );
        },
    );
}
