//! Task 6 E2E (spec 2026-11-14-areas §6): the REAL librarian pipeline —
//! deposit file with hand-written `hub:` frontmatter → documents row →
//! chunks → hub resolution at ingest → per-hub synthesis → auto-approve →
//! `commit_fact_add` — binds the model's `area` onto the V30
//! `llm_wiki_entries.area` COLUMN (Layer-3 readback). Prompt-form, parse,
//! force-include, and drop unit semantics live in `synthesis.rs mod tests`
//! (VISIBILITY); this file keeps ONLY ingest-through-file assertions.
//! The outbox PAYLOAD half is Task 7 — `wiki_fact_outbox_payload`'s
//! signature is untouched here.

// `write_config` is the legacy merge path retained for backward compatibility;
// this test pins the same setup shape as tests/librarian_evidence_e2e.rs.
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
use tauri_app_lib::librarian::{deposit_hub_unresolved_count, generate_summary};

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

/// Seeds the brain config beside the temp brain dir with the generation block
/// (mockito LLM, legacy merge path). m6: despite the old misleading name it
/// NEVER wrote a wisdom block — the tests patch the wisdom block in
/// themselves; the name now says what it does.
fn write_generation_config(app: &TestApp) {
    write_config(
        app.tmp.path(),
        &LlmConfig {
            generation: GenerationConfig {
                provider: GenerationProviderKind::External,
                model_path: None,
                model_name: Some("test-model".to_string()),
                external_url: None, // patched below with the mockito URL
                api_key: None,
                timeout_secs: None,
            },
            embedding: Default::default(),
        },
    )
    .unwrap();
}

fn seed_deposit(
    conn: &rusqlite::Connection,
    vault: &std::path::Path,
    file_name: &str,
    frontmatter: &str,
    body: &str,
) -> (i64, String) {
    let source_path = vault.join("immutable-source-files").join(file_name);
    std::fs::create_dir_all(source_path.parent().unwrap()).unwrap();
    std::fs::write(&source_path, format!("{frontmatter}\n{body}\n")).unwrap();
    let source_str = source_path.to_string_lossy().to_string();

    let doc_id = upsert_document(conn, &source_str, "hash").unwrap();
    // m6: real ingest STRIPS the frontmatter (`split_leading_frontmatter`)
    // before chunking — the chunk text is the body only, never the fence.
    let chunk_text = format!("{body}\n");
    let chunk = Chunk {
        text: chunk_text.clone(),
        start_line: 1,
        end_line: 6,
        symbol_name: None,
        defined_symbol: None,
        strategy: ChunkStrategyTag::Prose,
    };
    let _chunk_id = insert_chunk(conn, doc_id, &chunk, 0, "tier_fact", "").unwrap();
    let hash = compute_chunk_hash(&chunk_text, &source_str, 0);
    conn.execute(
        "UPDATE chunks SET content_hash = ?1 WHERE doc_id = ?2",
        rusqlite::params![hash, doc_id],
    )
    .unwrap();

    // Auto-approve the source folder so the proposal commits instead of
    // landing in the pending queue.
    conn.execute(
        "INSERT INTO folder_rules (folder_path, librarian_mode, auto_approve) VALUES (?1, 'synthesize', 1)",
        [source_path
            .parent()
            .unwrap()
            .to_string_lossy()
            .to_string()],
    )
    .unwrap();
    (doc_id, source_str)
}

fn areas_column(conn: &rusqlite::Connection, entry_id: &str) -> Option<String> {
    conn.query_row(
        "SELECT area FROM llm_wiki_entries WHERE id = ?1",
        [entry_id],
        |r| r.get::<_, Option<String>>(0),
    )
    .unwrap()
}

/// The tests below drive the env-var brain-dir redirect; the harness runs
/// them in parallel threads of ONE process, so they serialize on this lock
/// to keep `CURATED_BRAIN_DIR` from being dropped mid-test by a sibling.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// §6 happy path end-to-end: `hub: hub-e2e` frontmatter resolves through the
/// Task 3 survivor rule, the hub's areas file reaches the prompt via the
/// cached loader, and the model's in-vocabulary `area` lands in the V30
/// column through the commit payload.
#[test]
fn e2e_deposit_hub_area_binds_the_v30_column() {
    let _env_lock = ENV_LOCK.lock().unwrap();
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
         VALUES ('hub-e2e','RepoHub','concept','A repo hub',100,100)",
        [],
    )
    .unwrap();

    // The checkout + its .ct-areas.yaml (§3.1); the wisdom block binds it to
    // the hub entity id.
    let checkout = app.tmp.path().join("checkout");
    std::fs::create_dir_all(&checkout).unwrap();
    let areas_yaml = checkout.join(".ct-areas.yaml");
    std::fs::write(&areas_yaml, "areas:\n  deploy:\n    - infra/**\n").unwrap();

    let vault = app.tmp.path().join("vault");
    let (doc_id, source_str) = seed_deposit(
        &conn,
        &vault,
        "repo-note.md",
        "---\nhub: hub-e2e\ntitle: Repo note\n---",
        "RepoHub deploy notes about the runbook.",
    );

    let mut server = mockito::Server::new();
    let llm_json = serde_json::json!({
        "proposals": [{
            "target": { "existing_id": "hub-e2e" },
            "reasoning": "E2E areas write path.",
            "summary_update": null,
            "facts": [
                { "op": "add", "body": "Scoped fact.", "tags": [],
                  "confidence": "inferred", "evidence": ["C1"], "area": "deploy" }
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

    // Generation block first (legacy merge path), then patch in the wisdom
    // block and the mockito endpoint on the same config.json.
    write_generation_config(&app);
    let cfg_path = app.tmp.path().join("config.json");
    let mut cfg: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    cfg["generation"]["external_url"] = serde_json::Value::String(server.url());
    cfg["wisdom"] = serde_json::json!({
        "areas_paths": { "hub-e2e": areas_yaml.to_string_lossy() },
        "org_entity_id": "org-e2e"
    });
    std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();

    let result = generate_summary(&mut conn, &source_str, "test-model", false);
    assert!(result.is_ok(), "generate_summary failed: {result:?}");

    // Ingest-through-file: the deposit produced its documents row and chunk.
    let docs: i64 = conn
        .query_row("SELECT COUNT(*) FROM documents WHERE path = ?1", [&source_str], |r| r.get(0))
        .unwrap();
    assert_eq!(docs, 1, "deposit document row must exist");
    let chunks: i64 = conn
        .query_row("SELECT COUNT(*) FROM chunks WHERE doc_id = ?1", [doc_id], |r| r.get(0))
        .unwrap();
    assert!(chunks >= 1, "deposit must have at least one chunk");

    // Layer-3 COLUMN half: the committed entry carries the area.
    let entry_id: String = conn
        .query_row(
            "SELECT id FROM llm_wiki_entries WHERE entity_id = 'hub-e2e' AND deleted_at IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        areas_column(&conn, &entry_id),
        Some("deploy".to_string()),
        "the model's area must land in the V30 column"
    );

    // Delivery initiated: an outbox row exists for the entry (payload shape
    // is Task 7 — only presence is asserted here).
    let outbox: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM llm_wiki_outbox WHERE record_id = ?1",
            [&entry_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(outbox, 1, "the entry must be queued for delivery");
}

/// §6.8.4 negative through the real pipeline: the deposit names a LIVE
/// entity that is NOT a configured hub — resolution degrades to `absent`
/// (UNSCOPED synthesis, ingest never refuses) and the written fact carries
/// a NULL area.
#[test]
fn e2e_unresolved_hub_degrades_to_null_area() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let app = TestApp::new();
    let _brain_dir_guard = EnvVarGuard::new("CURATED_BRAIN_DIR");
    std::env::set_var(
        "CURATED_BRAIN_DIR",
        app.tmp.path().to_string_lossy().to_string(),
    );
    let _embed_guard = EnvVarGuard::new("CURATED_EMBED_STUB");
    std::env::set_var("CURATED_EMBED_STUB", "constant8");

    let mut conn = open_in_memory().unwrap();
    // Live entity, but NOT in the (otherwise present) wisdom block.
    conn.execute(
        "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
         VALUES ('ent-stray','StrayRepo','concept','Not a configured hub',100,100)",
        [],
    )
    .unwrap();

    let checkout = app.tmp.path().join("checkout");
    std::fs::create_dir_all(&checkout).unwrap();
    let areas_yaml = checkout.join(".ct-areas.yaml");
    std::fs::write(&areas_yaml, "areas:\n  deploy:\n    - infra/**\n").unwrap();

    let vault = app.tmp.path().join("vault");
    let (_doc_id, source_str) = seed_deposit(
        &conn,
        &vault,
        "stray-note.md",
        "---\nhub: ent-stray\ntitle: Stray note\n---",
        "StrayRepo notes without a configured hub.",
    );

    let mut server = mockito::Server::new();
    let llm_json = serde_json::json!({
        "proposals": [{
            "target": { "existing_id": "ent-stray" },
            "reasoning": "E2E unresolved hub degrade.",
            "summary_update": null,
            "facts": [
                { "op": "add", "body": "Unscoped fact.", "tags": [],
                  "confidence": "inferred", "evidence": ["C1"] }
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

    write_generation_config(&app);
    let cfg_path = app.tmp.path().join("config.json");
    let mut cfg: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    cfg["generation"]["external_url"] = serde_json::Value::String(server.url());
    cfg["wisdom"] = serde_json::json!({
        "areas_paths": { "hub-somewhere-else": areas_yaml.to_string_lossy() },
        "org_entity_id": "org-e2e"
    });
    std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();

    let result = generate_summary(&mut conn, &source_str, "test-model", false);
    assert!(
        result.is_ok(),
        "an unresolved hub must degrade, never refuse ingest: {result:?}"
    );

    // M3 ruling: the §11.2 unresolved bucket ticks from the REAL hub-read
    // path — a DELTA over this deposit's generate_summary, not a hand tick.
    assert!(
        deposit_hub_unresolved_count() >= 1,
        "the unresolved deposit must tick the §11.2 bucket via read_deposit_hub"
    );

    let entry_id: String = conn
        .query_row(
            "SELECT id FROM llm_wiki_entries WHERE entity_id = 'ent-stray' AND deleted_at IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        areas_column(&conn, &entry_id),
        None,
        "an unresolved hub must leave the area column NULL (UNSCOPED)"
    );
}
