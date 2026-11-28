//! Task 4 integration tests: `ct search` / `ct recall` / `ct code` over the
//! shared semantic helpers (recall_chunks + rank_wiki_entries).

mod common;

use common::{brain_db, run_ct, seed_fact, with_seeded_brain, AST_CHUNK_TEXT, DOC_PATH};

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

/// Keys of a JSON object, sorted.
fn sorted_keys(v: &serde_json::Value) -> Vec<String> {
    let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
    k.sort();
    k
}

/// Zero-chunk brain: the chunk leg has no floor (`recall_chunks` ranks every
/// embedded chunk), so a chunk always matches while any embedding exists.
/// Deleting the embeddings is the only way to get zero chunk hits.
fn drop_chunk_embeddings() {
    brain_db().execute("DELETE FROM embeddings", []).unwrap();
}

#[test]
fn recall_runs_wiki_leg_with_zero_chunk_hits() {
    with_seeded_brain(|| {
        drop_chunk_embeddings();
        seed_fact("x");
        let out = run_ct(&["recall", "--json", "Body"]);
        assert_eq!(
            out.status.code(),
            Some(0),
            "stderr={}",
            String::from_utf8_lossy(&out.stderr)
        );
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["results"], serde_json::json!([]));
        let wiki = v["wiki"].as_array().unwrap();
        assert_eq!(wiki.len(), 1);
        assert_eq!(wiki[0]["id"], "x");

        // Text mode prints the wiki section when chunks are empty.
        let out = run_ct(&["recall", "Body"]);
        assert_eq!(out.status.code(), Some(0));
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("--- wiki ---") && text.contains("wiki: Title x"), "{text}");
    });
}

#[test]
fn recall_both_empty_exits_two() {
    with_seeded_brain(|| {
        drop_chunk_embeddings();
        let out = run_ct(&["recall", "--json", "Body"]);
        assert_eq!(out.status.code(), Some(2));
        // search/code keep their exit-2-on-empty contract.
        for cmd in ["search", "code"] {
            let out = run_ct(&[cmd, "--json", "my_fn"]);
            assert_eq!(out.status.code(), Some(2), "{cmd}");
        }
    });
}

/// Exact key sets of the `ct recall --json` surface CTI parses.
#[test]
fn recall_json_shape_is_pinned() {
    with_seeded_brain(|| {
        seed_fact("w1");
        brain_db()
            .execute("UPDATE llm_wiki_entries SET body = 'my_fn wiki body' WHERE id = 'w1'", [])
            .unwrap();
        let out = run_ct(&["recall", "--json", "my_fn"]);
        assert!(out.status.success());
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(sorted_keys(&v), ["results", "wiki"]);
        let result = &v["results"][0];
        assert_eq!(
            sorted_keys(result),
            ["chunk_text", "doc_path", "entity_id", "score", "symbol_name"].map(String::from)
        );
        // Inferred, untyped fact: provenance present; verified and kind absent.
        let wiki = &v["wiki"][0];
        assert_eq!(
            sorted_keys(wiki),
            [
                "confidence", "entity_id", "id", "source_ref", "source_type", "text", "title"
            ]
            .map(String::from)
        );
    });
}

/// Mandatory benchmark gate (spec 6): no superseded fact is ever served,
/// end to end through the real `ct recall` binary.
#[test]
fn recall_never_serves_a_superseded_fact_end_to_end() {
    with_seeded_brain(|| {
        seed_fact("old");
        seed_fact("new");
        brain_db()
            .execute(
                "UPDATE llm_wiki_entries SET superseded_by = 'new', superseded_at = 500 WHERE id = 'old'",
                [],
            )
            .unwrap();
        let out = run_ct(&["recall", "--json", "Body"]);
        assert!(out.status.success());
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let ids: Vec<&str> = v["wiki"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec!["new"]);
    });
}
