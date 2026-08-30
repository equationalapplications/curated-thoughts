//! Task 5 integration tests: `ct graph` traversal and `ct wiki get|list`.
//!
//! Seeds two chunks linked by a CALLS edge (fixture pattern from
//! src-tauri/tests/graph_integration.rs) plus one llm_wiki_entries row, then
//! drives the compiled `ct` binary against the temp brain.

use std::fs;
use std::process::{Command, Output};

use temp_env::with_vars;
use tempfile::tempdir;

use tauri_app_lib::chunker::{Chunk, ChunkStrategyTag};
use tauri_app_lib::db::{
    insert_chunk, insert_relationship, mark_document_indexed, upsert_document, AppDb,
};

const ENTITY_ID: &str = "ent_graph_fixture";

fn make_chunk(name: &str) -> Chunk {
    Chunk {
        text: format!("fn {}() {{}}", name),
        start_line: 1,
        end_line: 3,
        symbol_name: Some(name.to_string()),
        defined_symbol: Some(name.to_lowercase()),
        strategy: ChunkStrategyTag::AstSymbolRust,
    }
}

/// Seed a brain.db with caller_fn -> helper_fn (CALLS) and one wiki row.
fn seed_graph_brain(brain_path: &std::path::Path) -> (i64, i64) {
    fs::write(brain_path.join("config.json"), b"{}\n").unwrap();
    let paths = tauri_app_lib::retrieval::resolve_brain_paths();
    let db = AppDb::open_with_config(&paths.db_path, &paths.config_path)
        .expect("writable brain db open");
    let doc_id = upsert_document(&db.0, "/vault/graph_fixture.rs", "h_graph").unwrap();
    let caller_id =
        insert_chunk(&db.0, doc_id, &make_chunk("caller_fn"), 0, ENTITY_ID, "").unwrap();
    let helper_id =
        insert_chunk(&db.0, doc_id, &make_chunk("helper_fn"), 1, ENTITY_ID, "").unwrap();
    insert_relationship(&db.0, caller_id, helper_id, "CALLS", "helper_fn", ENTITY_ID).unwrap();
    mark_document_indexed(&db.0, doc_id).unwrap();

    db.0
        .execute(
            "INSERT INTO llm_wiki_entries
             (id, entity_id, title, body, confidence, created_at, updated_at)
             VALUES ('wiki-1', ?1, 'Helper facts', 'the full wiki body text', 'verified', 1000, 2000)",
            rusqlite::params![ENTITY_ID],
        )
        .unwrap();
    (caller_id, helper_id)
}

/// Run `f` with a freshly seeded graph/wiki brain as CURATED_BRAIN_DIR.
fn with_graph_brain<F: FnOnce(i64, i64)>(f: F) {
    let brain = tempdir().unwrap();
    let brain_path = brain.path().to_path_buf();
    let brain_path_str = brain_path.to_str().unwrap().to_string();
    with_vars(
        [("CURATED_BRAIN_DIR", Some(brain_path_str.as_str()))],
        move || {
            let (caller_id, helper_id) = seed_graph_brain(&brain_path);
            f(caller_id, helper_id);
        },
    );
}

fn run_ct(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ct"))
        .args(args)
        .output()
        .expect("spawn ct")
}

#[test]
fn graph_callees_json_lists_linked_entity() {
    with_graph_brain(|_caller_id, _helper_id| {
        let out = run_ct(&[
            "graph",
            "caller_fn",
            "--dir",
            "callees",
            "--hops",
            "2",
            "--json",
        ]);
        assert!(
            out.status.success(),
            "ct graph failed: {} stderr={}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert!(v["root"]["chunk_id"].is_i64(), "root.chunk_id missing: {v}");
        assert_eq!(v["root"]["entity_id"], ENTITY_ID);
        let neighbors = v["neighbors"].as_array().expect("neighbors array");
        assert!(
            neighbors.iter().any(|n| n["rel_type"] == "CALLS"
                && n["depth"] == 1
                && n["entity_id"] == ENTITY_ID),
            "callee neighbor missing: {v}"
        );
        assert_eq!(v["truncated"], false);
    });
}

#[test]
fn graph_unknown_symbol_exits_two_with_message() {
    with_graph_brain(|_, _| {
        let out = run_ct(&["graph", "nope_fn", "--json"]);
        assert_eq!(out.status.code(), Some(2), "expected exit 2");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("symbol not found: nope_fn"),
            "stderr missing message"
        );
    });
}

#[test]
fn wiki_list_json_includes_seeded_entry() {
    with_graph_brain(|_, _| {
        let out = run_ct(&["wiki", "list", "--json"]);
        assert!(out.status.success());
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let entries = v.as_array().expect("wiki list json is an array");
        let e = entries
            .iter()
            .find(|e| e["id"] == "wiki-1")
            .expect("seeded entry");
        assert_eq!(e["entity_id"], ENTITY_ID);
        assert_eq!(e["title"], "Helper facts");
        assert_eq!(e["updated_at"], 2000);
    });
}

#[test]
fn wiki_get_by_entity_id_prints_full_row_including_body() {
    with_graph_brain(|_, _| {
        let out = run_ct(&["wiki", "get", ENTITY_ID]);
        assert!(out.status.success());
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("the full wiki body text"), "body missing");
        assert!(stdout.contains("Helper facts"));
    });
}

#[test]
fn wiki_get_unknown_id_exits_two() {
    with_graph_brain(|_, _| {
        let out = run_ct(&["wiki", "get", "missing-entity"]);
        assert_eq!(out.status.code(), Some(2));
    });
}
