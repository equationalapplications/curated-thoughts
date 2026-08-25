//! Task 4 integration tests: `ct search` / `ct recall` / `ct code` over the
//! shared semantic helpers (recall_chunks + rank_wiki_entries).

mod common;

use common::{run_ct, with_seeded_brain, AST_CHUNK_TEXT, DOC_PATH};

#[test]
fn search_json_returns_scored_chunks_with_expected_top_hit() {
    with_seeded_brain(|| {
        let out = run_ct(&["search", "--json", "my_fn"]);
        assert!(
            out.status.success(),
            "exit={:?} stderr={}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let results = v["results"].as_array().expect("results array");
        assert!(!results.is_empty(), "expected non-empty results");
        assert_eq!(results[0]["doc_path"], DOC_PATH);
        assert_eq!(results[0]["chunk_text"], AST_CHUNK_TEXT);
        assert!(results[0]["score"].as_f64().unwrap().is_finite());
        // ScoredChunk shape: no legacy chunk_id/path/text keys.
        assert!(results[0].get("chunk_id").is_none());
        assert!(results[0].get("entity_id").is_some());
    });
}

#[test]
fn code_json_returns_only_ast_chunks() {
    with_seeded_brain(|| {
        let out = run_ct(&["code", "--json", "my_fn"]);
        assert!(
            out.status.success(),
            "stderr={}",
            String::from_utf8_lossy(&out.stderr)
        );
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let results = v["results"].as_array().expect("results array");
        assert_eq!(results.len(), 1, "ast-only leg must exclude prose chunks");
        assert_eq!(results[0]["chunk_text"], AST_CHUNK_TEXT);
    });
}

#[test]
fn recall_json_includes_wiki_key_and_results() {
    with_seeded_brain(|| {
        let out = run_ct(&["recall", "--json", "my_fn"]);
        assert!(
            out.status.success(),
            "stderr={}",
            String::from_utf8_lossy(&out.stderr)
        );
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert!(v.get("wiki").and_then(|w| w.as_array()).is_some());
        assert!(!v["results"].as_array().expect("results array").is_empty());
    });
}

#[test]
fn empty_query_exits_two() {
    with_seeded_brain(|| {
        for cmd in ["search", "recall", "code"] {
            let out = run_ct(&[cmd, "--json", "   "]);
            assert_eq!(out.status.code(), Some(2), "{cmd} empty query");
        }
    });
}

#[test]
fn k_is_clamped_to_1_50() {
    with_seeded_brain(|| {
        // Out-of-range k must not error (clamped, mirroring the facade).
        let out = run_ct(&["search", "--json", "my_fn", "--k", "500"]);
        assert!(
            out.status.success(),
            "stderr={}",
            String::from_utf8_lossy(&out.stderr)
        );
    });
}
