//! `ct wisdom match` contract tests (issue #265). Stub embeddings
//! (`CURATED_EMBED_STUB=constant8`) give every text the same direction, so
//! a fact seeded with blob [1,0,…] scores cosine 1 and clears the
//! `stub:constant8` floor (0.5); the gate logic itself is unit-tested in
//! src-tauri/src/wisdom_match.rs.

mod common;

use common::{run_ct, with_seeded_brain};
use rusqlite::params;

fn brain_db() -> rusqlite::Connection {
    let dir = std::env::var("CURATED_BRAIN_DIR").expect("with_seeded_brain sets it");
    rusqlite::Connection::open(std::path::Path::new(&dir).join("brain.db")).unwrap()
}

fn seed_fact(id: &str) {
    let mut v = [0f32; 8];
    v[0] = 1.0;
    let blob: Vec<u8> = v.iter().flat_map(|f| f.to_le_bytes()).collect();
    brain_db()
        .execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                source_hash, source_ref, created_at, updated_at, last_accessed_at,
                access_count, deleted_at, embedding_blob, embedding
             ) VALUES (?1, 'ent_t', ?2, ?3, '[]', 'inferred', 'librarian_inferred',
                       NULL, NULL, 100, 100, NULL, 0, NULL, ?4, NULL)",
            params![id, format!("Title {id}"), format!("Body {id}"), blob],
        )
        .unwrap();
}

fn json_of(out: &std::process::Output) -> serde_json::Value {
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("stdout is JSON")
}

#[test]
fn match_returns_contract_shape() {
    with_seeded_brain(|| {
        seed_fact("fact_a");
        let v = json_of(&run_ct(&[
            "wisdom",
            "match",
            "--json",
            "--",
            "how do I deploy",
        ]));
        assert_eq!(v["schema"], 1);
        assert_eq!(v["gate"], "semantic-v1:stub:constant8");
        let e = &v["entries"][0];
        assert_eq!(e["id"], "fact_a");
        assert_eq!(e["title"], "Title fact_a");
        assert_eq!(e["text"], "Body fact_a");
        assert!(e["score"].as_f64().unwrap() > 0.99);
        assert_eq!(e["supersedes"], serde_json::json!([]));
        assert_eq!(e["provenance"], "librarian_inferred");
        assert_eq!(v["corrections"], serde_json::json!([]));
    });
}

#[test]
fn zero_matches_is_success() {
    with_seeded_brain(|| {
        let v = json_of(&run_ct(&["wisdom", "match", "--json", "--", "anything"]));
        assert_eq!(v["entries"], serde_json::json!([]));
    });
}

#[test]
fn exclude_and_correction() {
    with_seeded_brain(|| {
        seed_fact("fact_old");
        seed_fact("fact_new");
        brain_db()
            .execute(
                "UPDATE llm_wiki_entries SET superseded_by = 'fact_new', valid_to = 1
                 WHERE id = 'fact_old'",
                [],
            )
            .unwrap();
        let v = json_of(&run_ct(&[
            "wisdom",
            "match",
            "--json",
            "--max",
            "0",
            "--exclude=fact_old",
            "--",
            "q",
        ]));
        assert_eq!(v["entries"], serde_json::json!([]));
        assert_eq!(v["corrections"][0]["id"], "fact_new");
        assert_eq!(
            v["corrections"][0]["supersedes"],
            serde_json::json!(["fact_old"])
        );
        assert!(v["corrections"][0]["score"].is_null());
        // excluding the head too: nothing to correct, nothing to match
        let v = json_of(&run_ct(&[
            "wisdom",
            "match",
            "--json",
            "--exclude=fact_old",
            "--exclude=fact_new",
            "--",
            "q",
        ]));
        assert_eq!(v["entries"], serde_json::json!([]));
        assert_eq!(v["corrections"], serde_json::json!([]));
    });
}

#[test]
fn text_requires_double_dash_and_may_start_with_dash() {
    with_seeded_brain(|| {
        let out = run_ct(&["wisdom", "match", "--json", "deploy"]);
        assert_eq!(out.status.code(), Some(1));
        json_of(&run_ct(&[
            "wisdom",
            "match",
            "--json",
            "--",
            "-weird --text",
        ]));
    });
}

#[test]
fn bad_or_too_many_excludes_exit_1() {
    with_seeded_brain(|| {
        let out = run_ct(&["wisdom", "match", "--json", "--exclude=bad id", "--", "q"]);
        assert_eq!(out.status.code(), Some(1));
        let many: Vec<String> = (0..1025).map(|i| format!("--exclude=f{i}")).collect();
        let mut args: Vec<&str> = vec!["wisdom", "match", "--json"];
        args.extend(many.iter().map(String::as_str));
        args.extend(["--", "q"]);
        assert_eq!(run_ct(&args).status.code(), Some(1));
    });
}

#[test]
fn whitespace_text_is_empty_success() {
    with_seeded_brain(|| {
        seed_fact("fact_a");
        let v = json_of(&run_ct(&["wisdom", "match", "--json", "--", "   "]));
        assert_eq!(v["entries"], serde_json::json!([]));
    });
}

#[test]
fn help_exits_zero() {
    let out = run_ct(&["wisdom", "match", "--help"]);
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn unresolvable_brain_exits_1() {
    let empty = tempfile::tempdir().unwrap();
    temp_env::with_vars(
        [
            ("CURATED_BRAIN_DIR", Some(empty.path().to_str().unwrap())),
            ("CURATED_EMBED_STUB", Some("constant8")),
        ],
        || {
            let out = run_ct(&["wisdom", "match", "--json", "--", "q"]);
            assert_eq!(out.status.code(), Some(1));
            assert!(!out.stderr.is_empty());
        },
    );
}
