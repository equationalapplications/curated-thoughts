//! The test PR #169 lacked: drives real librarian synthesis through the real
//! commit path and asserts the evidence contract end to end.

// `write_config` is the legacy merge path retained for backward compatibility;
// this test pins the same setup shape as tests/folder_rules.rs.
#![allow(deprecated)]

mod helpers;

use helpers::TestApp;
use tauri_app_lib::chunker::{Chunk, ChunkStrategyTag};
use tauri_app_lib::db::chunk_hash::compute_chunk_hash;
use tauri_app_lib::db::connection::open_in_memory;
use tauri_app_lib::db::queries::{insert_chunk, upsert_document};
use tauri_app_lib::inference::config::{
    write_config, GenerationConfig, GenerationProviderKind, LlmConfig,
};
use tauri_app_lib::librarian::generate_summary;

struct EnvVarGuard {
    key: String,
    previous: Option<std::ffi::OsString>,
}

impl EnvVarGuard {
    fn new(key: &str) -> Self {
        EnvVarGuard {
            key: key.to_string(),
            previous: std::env::var_os(key),
        }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(previous) = &self.previous {
            std::env::set_var(&self.key, previous);
        } else {
            std::env::remove_var(&self.key);
        }
    }
}

fn entry_ids(conn: &rusqlite::Connection) -> Vec<String> {
    let mut stmt = conn
        .prepare("SELECT id FROM llm_wiki_entries WHERE source_type = 'librarian_inferred'")
        .unwrap();
    stmt.query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .filter_map(Result::ok)
        .collect()
}

/// Re-implementation of the engine's five-predicate selector
/// (dist/index.js:1454-1467) and `normalizeSourceRef` (dist:4082). Supplemental
/// to the Task 12 acceptance gate, which runs the real engine.
fn engine_would_rewrite(source_ref: &str) -> bool {
    let selected = source_ref.trim() != source_ref
        || source_ref.contains('/')
        || source_ref.contains('\\')
        || source_ref.contains('\0')
        || source_ref
            .chars()
            .any(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ' ')));
    if !selected {
        return false;
    }
    let normalized: String = source_ref
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ' '))
        .collect::<String>()
        .trim()
        .chars()
        .take(255)
        .collect();
    normalized != source_ref
}

#[test]
fn synthesis_writes_tokens_and_anchored_evidence() {
    let app = TestApp::new();
    let _brain_dir_guard = EnvVarGuard::new("CURATED_BRAIN_DIR");
    std::env::set_var(
        "CURATED_BRAIN_DIR",
        app.tmp.path().to_string_lossy().to_string(),
    );
    let _embed_guard = EnvVarGuard::new("CURATED_EMBED_STUB");
    std::env::set_var("CURATED_EMBED_STUB", "constant8");

    let mut conn = open_in_memory().unwrap();
    conn.execute(
        "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
         VALUES ('ent-e2e','Notes','concept','Summary',100,100)",
        [],
    )
    .unwrap();

    // v2 layout: source tier is immutable-source-files/. The source path must
    // be real under the temp vault so the folder rule and the chunk join both
    // resolve, mirroring tests/folder_rules.rs.
    let vault = app.tmp.path().join("vault");
    let source_path = vault.join("immutable-source-files").join("notes.md");
    std::fs::create_dir_all(source_path.parent().unwrap()).unwrap();
    let source_str = source_path.to_string_lossy().to_string();

    let doc_id = upsert_document(&conn, &source_str, "hash").unwrap();
    // One live chunk to anchor against, and one fact whose evidence points at
    // a chunk that does not exist — the Phase-1 unanchored case.
    let chunk_text = "E2E evidence anchor content.";
    let chunk = Chunk {
        text: chunk_text.into(),
        start_line: 1,
        end_line: 3,
        symbol_name: None,
        defined_symbol: None,
        strategy: ChunkStrategyTag::Prose,
    };
    let _chunk_id = insert_chunk(&conn, doc_id, &chunk, 0, "tier_fact", "").unwrap();
    // Backfill the post-migration content_hash so resolve_evidence can persist
    // a real hash onto proposal evidence (same pattern as synthesis.rs tests).
    let hash = compute_chunk_hash(chunk_text, &source_str, 0);
    conn.execute(
        "UPDATE chunks SET content_hash = ?1 WHERE doc_id = ?2",
        rusqlite::params![hash, doc_id],
    )
    .unwrap();

    // Auto-approve on the source folder so the proposal commits instead of
    // landing in the pending queue — the commit path is what we are testing.
    conn.execute(
        "INSERT INTO folder_rules (folder_path, librarian_mode, auto_approve) VALUES (?1, 'synthesize', 1)",
        [source_path
            .parent()
            .unwrap()
            .to_string_lossy()
            .to_string()],
    )
    .unwrap();

    let mut server = mockito::Server::new();
    let llm_json = serde_json::json!({
        "proposals": [{
            "target": { "existing_id": "ent-e2e" },
            "reasoning": "E2E evidence contract.",
            "summary_update": null,
            "facts": [
                { "op": "add", "body": "Anchored fact.", "tags": [],
                  "confidence": "inferred", "evidence": ["C1"] },
                { "op": "add", "body": "Unanchored fact.", "tags": [],
                  "confidence": "inferred", "evidence": [] }
            ],
            "edges": [],
            "tasks": []
        }]
    });
    let _mock = server
        .mock("POST", "/v1/chat/completions")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            "{{\"choices\":[{{\"message\":{{\"content\":{}}}}}]}}",
            serde_json::to_string(&llm_json.to_string()).unwrap()
        ))
        .create();

    write_config(
        app.tmp.path(),
        &LlmConfig {
            generation: GenerationConfig {
                provider: GenerationProviderKind::External,
                model_path: None,
                model_name: Some("test-model".to_string()),
                external_url: Some(server.url()),
                api_key: None,
                timeout_secs: None,
            },
            embedding: Default::default(),
        },
    )
    .unwrap();

    // Drive the real synthesis persistence path.
    let result = generate_summary(&mut conn, &source_str, "test-model", false);
    assert!(result.is_ok(), "generate_summary failed: {:?}", result);

    // 1. Every librarian_inferred ref is a fixed point of the normalizer.
    let refs: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT source_ref FROM llm_wiki_entries
                  WHERE source_type = 'librarian_inferred' AND source_ref IS NOT NULL",
            )
            .unwrap();
        stmt.query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .filter_map(Result::ok)
            .collect()
    };
    assert!(!refs.is_empty(), "synthesis must have written facts");
    for r in &refs {
        assert!(r.starts_with("librarian-"), "not a token: {r}");
        assert!(r.len() <= 255, "ref exceeds the 255 cap: {r}");
        assert!(
            !engine_would_rewrite(r),
            "engine selector would rewrite: {r}"
        );
    }

    // 2. Phase-aware evidence assertion. Under Phase-1 write-with-flag a fact
    //    with zero live anchors is a legitimate, expected write — a blanket
    //    ">=1 chunk present" assertion would contradict spec §2.4.
    let mut anchored = 0;
    let mut unanchored = 0;
    for entry_id in entry_ids(&conn) {
        let json = tauri_app_lib::db::commit::evidence_json_for_entry(&conn, &entry_id)
            .unwrap_or_else(|| panic!("every inferred fact needs an evidence row: {entry_id}"));
        serde_json::from_str::<serde_json::Value>(&json)
            .unwrap_or_else(|_| panic!("evidence_json must parse: {entry_id}"));
        let flag: i64 = conn
            .query_row(
                "SELECT unanchored FROM librarian_evidence WHERE entry_id = ?1",
                [&entry_id],
                |r| r.get(0),
            )
            .unwrap();
        if flag == 1 {
            unanchored += 1;
        } else {
            assert!(
                tauri_app_lib::db::commit::evidence_has_live_chunk(&conn, &json).unwrap(),
                "unanchored=0 must mean a live chunk anchor exists: {entry_id}"
            );
            anchored += 1;
        }
    }
    assert!(
        anchored >= 1 && unanchored >= 1,
        "fixture must exercise both paths (anchored={anchored}, unanchored={unanchored})"
    );

    // 3. Engine-simulation pass: zero rows would change. Supplemental — Task 12
    //    is the acceptance gate.
    for r in &refs {
        assert!(!engine_would_rewrite(r));
    }
}
