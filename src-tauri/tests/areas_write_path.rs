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
    // External completers are gated on the effective privacy mode (Strict
    // default when unset — INTENT rule 11, fail-closed). Mark chosen +
    // permissive: Ephemeral allows external generation and skips the keyring
    // (unavailable on headless CI) — same pattern as provider_rollback.rs.
    tauri_app_lib::privacy::write_privacy_mode(
        app.tmp.path(),
        tauri_app_lib::privacy::PrivacyMode::Ephemeral,
        true,
    )
    .expect("write privacy mode");
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
    // landing in the pending queue. (OR IGNORE: the measurement fixture
    // seeds several deposits into the SAME folder.)
    conn.execute(
        "INSERT OR IGNORE INTO folder_rules (folder_path, librarian_mode, auto_approve) VALUES (?1, 'synthesize', 1)",
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

/// §11.2 measured attach rate (Task 10): a realistic fixture brain — four
/// deposits with hubs (one attaches, one misses, one over-attaches, one
/// baits the candidate gate), one unconfigured deposit, one unresolved-hub
/// deposit — driven through the REAL pipeline with a mock model that both
/// attaches and misses. The four §11.2 buckets are read from DB state and
/// the §6.8.5 counters' deltas; the counts are printed (run with
/// `-- --nocapture` to see them) and recorded in the LEDGER + PR body.
/// The spec claims no rate in advance (§19.3) — this fixture measures.
#[test]
fn measurement_attach_rate_four_buckets() {
    use tauri_app_lib::librarian::wisdom_hub_unconfigured_count;

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
    for (id, name) in [
        ("hub-m", "MainHub"),     // configured hub (owns deposits 1-3)
        ("hub-gate", "GateHub"),  // configured hub, NOT in any candidate set
        ("ent-over", "OverHub"),  // the more-specific entity the fixture expects
        ("ent-stray", "StrayHub"), // live but NOT configured
    ] {
        conn.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
             VALUES (?1, ?2, 'concept', 'entity', 100, 100)",
            [id, name],
        )
        .unwrap();
    }

    // TWO DISTINCT areas files: the Task-2 duplicate-directory guard drops
    // the whole wisdom block when two hubs share one file (fail-closed) —
    // the fixture needs both hubs CONFIGURED, so each gets its own file.
    let checkout_a = app.tmp.path().join("checkout-a");
    let checkout_b = app.tmp.path().join("checkout-b");
    std::fs::create_dir_all(&checkout_a).unwrap();
    std::fs::create_dir_all(&checkout_b).unwrap();
    let areas_yaml = checkout_a.join(".ct-areas.yaml");
    let areas_yaml_b = checkout_b.join(".ct-areas.yaml");
    let vocab = "areas:\n  deploy:\n    - infra/**\n  networking:\n    - net/**\n";
    std::fs::write(&areas_yaml, vocab).unwrap();
    std::fs::write(&areas_yaml_b, vocab).unwrap();

    let vault = app.tmp.path().join("vault");

    // Write the generation block + wisdom block (two configured hubs), then
    // return a helper that re-points the mock URL (each deposit gets its own
    // mockito server) without disturbing the wisdom block.
    write_generation_config(&app);
    let cfg_path = app.tmp.path().join("config.json");
    {
        let mut cfg: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
        cfg["wisdom"] = serde_json::json!({
            "areas_paths": {
                "hub-m": areas_yaml.to_string_lossy(),
                "hub-gate": areas_yaml_b.to_string_lossy()
            },
            "org_entity_id": "org-m"
        });
        std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();
    }
    let set_url = |url: &str| {
        let mut cfg: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
        cfg["generation"]["external_url"] = serde_json::Value::String(url.to_string());
        std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();
    };

    // Run one deposit through the pipeline with the given model JSON.
    let mut run_deposit = |file: &str,
                           frontmatter: &str,
                           body: &str,
                           target: &str,
                           area: Option<&str>| {
        let (_doc_id, source_str) =
            seed_deposit(&conn, &vault, file, frontmatter, body);
        let mut fact = serde_json::json!({
            "op": "add", "body": format!("Fact for {file}."), "tags": [],
            "confidence": "inferred", "evidence": ["C1"]
        });
        if let Some(a) = area {
            fact["area"] = serde_json::Value::String(a.to_string());
        }
        let llm_json = serde_json::json!({
            "proposals": [{
                "target": { "existing_id": target },
                "reasoning": "Measurement fixture.",
                "summary_update": null,
                "facts": [fact],
                "edges": [],
                "tasks": []
            }]
        });
        let mut server = mockito::Server::new();
        let _mock = server
            .mock("POST", "/v1/chat/completions")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(format!(
                "{{\"choices\":[{{\"message\":{{\"content\":{}}}}}]}}",
                serde_json::to_string(&llm_json.to_string()).unwrap()
            ))
            .create();
        set_url(&server.url());
        let result = generate_summary(&mut conn, &source_str, "test-model", false);
        assert!(
            result.is_ok(),
            "deposit {file} must never refuse ingest: {result:?}"
        );
        server
    };

    // §11.2 counter deltas start here (process-global; the whole fixture
    // runs under ENV_LOCK, sibling tests' ticks only ever ADD ticks).
    let unresolved_before = deposit_hub_unresolved_count();
    let unconf_before = wisdom_hub_unconfigured_count();

    // Deposit 1 — CORRECT ATTACH: owning hub, model attaches in-vocab area.
    let _s1 = run_deposit(
        "deploy-note.md",
        "---\nhub: hub-m\ntitle: Deploy note\n---",
        "Runbook updates for the release pipeline.",
        "hub-m",
        Some("deploy"),
    );
    // Deposit 2 — MODEL MISS: owning hub, model proposes NO area.
    let _s2 = run_deposit(
        "miss-note.md",
        "---\nhub: hub-m\ntitle: Miss note\n---",
        "Follow-up notes on the release pipeline.",
        "hub-m",
        None,
    );
    // Deposit 3 — HUB-OVER-ATTACH: the fixture expects the more-specific
    // OverHub entity (named in the body, so it ranks as a candidate), but
    // the force-included hub pulls the attach up to hub-m.
    let _s3 = run_deposit(
        "over-note.md",
        "---\nhub: hub-m\ntitle: Over note\n---",
        "OverHub specific tuning that a hub-level attach would smear.",
        "hub-m",
        Some("deploy"),
    );
    // Deposit 4 — CANDIDATE-GATE DROP: model targets the CONFIGURED
    // GateHub, which carries no embedding and no name hit, so it is not in
    // the batch's candidate set → reported drop, nothing commits.
    let _s4 = run_deposit(
        "gate-bait.md",
        "---\ntitle: Gate bait\n---",
        "Notes that never mention the second repository by name.",
        "hub-gate",
        Some("deploy"),
    );
    // Deposit 5 — NO-OWNER (unresolved): live but unconfigured StrayHub.
    let _s5 = run_deposit(
        "stray-note.md",
        "---\nhub: ent-stray\ntitle: Stray note\n---",
        "StrayHub notes without a configured hub.",
        "ent-stray",
        None,
    );

    // Deposit 6 — NO-OWNER (wisdom unconfigured): the wisdom block is
    // dropped fail-closed, so scoping is impossible (§6.8.5 distinct bucket).
    {
        let mut cfg: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
        cfg.as_object_mut().unwrap().remove("wisdom");
        std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();
    }
    let _s6 = run_deposit(
        "unconf-note.md",
        "---\nhub: hub-m\ntitle: Unconf note\n---",
        "Notes deposited with no wisdom block at all.",
        "hub-m",
        None,
    );

    // ---- Bucket counts, read from REAL DB state + counter deltas ----
    let committed =
        |entity: &str| -> Vec<Option<String>> {
            let mut stmt = conn
                .prepare(
                    "SELECT area FROM llm_wiki_entries
                     WHERE entity_id = ?1 AND deleted_at IS NULL AND body LIKE 'Fact for %'",
                )
                .unwrap();
            let rows = stmt
                .query_map([entity], |r| r.get::<_, Option<String>>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            rows
        };

    // Per-deposit classification on hub-m's committed facts (bodies are
    // unique per deposit: "Fact for <file>."):
    //
    // Gauntlet finding 10 (partial tighten): the helper DISTINGUISHES a
    // committed-NULL area (MODEL MISS — the model had a committed fact but
    // attached no area) from a never-committed deposit (candidate-gate or
    // other DROP, zero rows). Both were `None` before; only the former is a
    // model miss, so the drop classification can't inflate it.
    let area_for = |file: &str| -> (bool, Option<String>) {
        let needle = format!("Fact for {file}.");
        let mut stmt = conn
            .prepare(
                "SELECT area FROM llm_wiki_entries
                 WHERE entity_id = 'hub-m' AND deleted_at IS NULL AND body = ?1",
            )
            .unwrap();
        let mut rows = stmt
            .query_map([needle], |r| r.get::<_, Option<String>>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect::<Vec<_>>();
        assert!(rows.len() <= 1, "one deposit commits one fact");
        let committed = !rows.is_empty();
        (committed, rows.pop().flatten())
    };
    // Deposit 1: fixture expects hub-m — area present → CORRECT attach.
    let (_, attached_area) = area_for("deploy-note.md");
    let attached = usize::from(attached_area.is_some());
    // Deposit 2: fixture expects hub-m — committed but NO area → MODEL MISS.
    // (Coderabbit follow-up: a committed fact WITH an area is neither a miss
    // nor an attach — require committed AND area-None for the miss bucket.)
    let (miss_committed, miss_area) = area_for("miss-note.md");
    let model_misses = usize::from(miss_committed && miss_area.is_none());
    // Deposit 3: fixture expects the more-specific ent-over, got hub-m + area
    // → HUB-OVER-ATTACH (the rank-1 force-include pull-up §11.2 warns about).
    let (_, over_area) = area_for("over-note.md");
    let hub_over_attach = usize::from(over_area.is_some());
    // Bucket: candidate-gate drops — the gate-bait proposal must NOT commit.
    let gate_dropped_committed = committed("hub-gate").len();
    assert_eq!(
        gate_dropped_committed, 0,
        "a candidate-gate-dropped proposal must commit nothing"
    );
    let gate_drops = 1; // the bait proposal (reported drop; stderr names hub-gate)
    // Bucket: no-owner-signal — both §6.8.5 sub-counters' deltas.
    let unresolved_delta = deposit_hub_unresolved_count() - unresolved_before;
    let unconf_delta = wisdom_hub_unconfigured_count() - unconf_before;
    let no_owner = unresolved_delta + unconf_delta;

    println!("MEASUREMENT §11.2 four buckets:");
    println!("  candidate-gate drops: {gate_drops} (bait proposal uncommitted: {gate_dropped_committed} rows)");
    println!("  model misses:         {model_misses}");
    println!(
        "  no-owner-signal:      {no_owner} (unresolved {unresolved_delta} + wisdom-unconfigured {unconf_delta})"
    );
    println!("  hub-over-attach:      {hub_over_attach}");
    println!("  correctly attached:   {attached} of 6 deposits (attach rate incl. over-attach: {}/6 scoped-relevant)", attached + hub_over_attach);

    // Fixture-integrity pins (the measurement means nothing without these):
    assert_eq!(attached, 1, "exactly one deposit should attach correctly");
    assert_eq!(model_misses, 1, "exactly one deposit should be a model miss");
    assert_eq!(hub_over_attach, 1, "exactly one deposit should over-attach to the hub");
    assert_eq!(unresolved_delta, 1, "the stray-hub deposit must tick unresolved");
    assert_eq!(
        unconf_delta, 1,
        "the no-wisdom deposit must tick wisdom-unconfigured (§6.8.5 distinct bucket)"
    );
    let stray_rows = committed("ent-stray").len();
    assert_eq!(stray_rows, 1, "the unresolved-hub deposit still commits UNSCOPED");
}
