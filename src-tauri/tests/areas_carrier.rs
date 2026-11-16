//! Task 8 E2E (spec 2026-11-14-areas §6.8): the `repo_path` CARRIER — the
//! deposit door resolves an optional absolute `repo_path` against the
//! configured `wisdom.areas_paths` checkout roots, renders `hub:` LAST in
//! the leading frontmatter block (attribution lines first; BOTH absent →
//! byte-identical to the pre-attribution render, pinned in
//! `wisdom_deposit.rs`), and reports `hub` / `hub_reason` in the reply
//! contract. Status re-reads `hub` from the FILE and never echoes
//! `hub_reason` (spec rev-9 m1). Render unit placement lives in
//! `wisdom_deposit.rs mod tests` (VISIBILITY) — this file keeps
//! ingest-through-door assertions.

#![allow(deprecated)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tauri_app_lib::config::hub_resolver::resolve_deposit_hub;
use tauri_app_lib::db::connection::{open_in_memory, AppDb};
use tauri_app_lib::inference::config::{
    write_config, GenerationConfig, GenerationProviderKind, LlmConfig,
};
use tauri_app_lib::librarian::deposit_hub_unresolved_count;
use tauri_app_lib::tool_dispatch::ToolDispatchContext;
use tauri_app_lib::wisdom_deposit::{
    dispatch_wisdom_deposit_awaiting_kick, dispatch_wisdom_deposit_status, WisdomDepositParams,
};

/// The tests drive the env-var brain-dir redirect; the harness runs them in
/// parallel threads of ONE process, so they serialize on this lock (same
/// discipline as `areas_write_path.rs`).
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn add_entity(conn: &rusqlite::Connection, id: &str) {
    conn.execute(
        "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
         VALUES (?1, ?1, 'concept', 'Summary', 100, 100)",
        [id],
    )
    .unwrap();
}

fn add_redirect(conn: &rusqlite::Connection, from: &str, to: &str) {
    conn.execute(
        "INSERT INTO entity_redirects (entity_id, merged_into, created_at) VALUES (?1, ?2, 100)",
        [from, to],
    )
    .unwrap();
}

/// A migrated brain rooted at `vault` (folder_rules, deposit_kick_state,
/// documents, … all present via the rooted `AppDb::open_with_config` open)
/// with the SAME `ToolDispatchContext` shape production builds.
fn carrier_ctx(vault: &Path) -> ToolDispatchContext {
    std::fs::create_dir_all(vault.join("immutable-source-files/agents")).unwrap();
    let config = serde_json::json!({ "vault_path": vault.to_str().unwrap() });
    std::fs::write(vault.join("config.json"), config.to_string()).unwrap();
    let db = AppDb::open_with_config(&vault.join("brain.db"), vault.join("config.json")).unwrap();
    ToolDispatchContext {
        conn: Arc::new(Mutex::new(db.0)),
        profile: tauri_app_lib::embedder::EmbedProfile::default(),
        vault_dir: Some(vault.to_path_buf()),
        client: "test".into(),
        db_path: vault.join("brain.db"),
        rw_conn: Arc::new(Mutex::new(None)),
    }
}

fn with_brain<F: std::future::Future<Output = ()>>(brain_dir: &str, body: impl FnOnce() -> F) {
    let brain = brain_dir.to_owned();
    temp_env::with_vars(
        [
            ("CURATED_BRAIN_DIR", Some(brain.as_str())),
            ("CURATED_BRAIN_CONFIG", None::<&str>),
            ("CURATED_BRAIN_DB", None::<&str>),
            ("CURATED_EMBED_STUB", Some("constant8")),
        ],
        || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime")
                .block_on(body());
        },
    );
}

/// Seed the brain config with a generation block (mockito LLM patched in by
/// the caller) — legacy merge path, then the caller patches `wisdom` on the
/// SAME config.json.
fn write_generation_config(vault: &Path) {
    write_config(
        vault,
        &LlmConfig {
            generation: GenerationConfig {
                provider: GenerationProviderKind::External,
                model_path: None,
                model_name: Some("test-model".to_string()),
                external_url: None,
                api_key: None,
                timeout_secs: None,
            },
            embedding: Default::default(),
        },
    )
    .unwrap();
}

fn patch_config(vault: &Path, key: &str, value: serde_json::Value) {
    let cfg_path = vault.join("config.json");
    let mut cfg: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    cfg[key] = value;
    std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();
}

fn wisdom_block(areas_yaml: &Path, hub: &str) -> serde_json::Value {
    serde_json::json!({
        "areas_paths": { hub: areas_yaml.to_string_lossy() },
        "org_entity_id": "org-e2e"
    })
}

/// A checkout with `.ct-areas.yaml` binding area `deploy`, under `hub`.
fn make_checkout(root: &Path, hub: &str) -> PathBuf {
    let checkout = root.join(hub);
    std::fs::create_dir_all(&checkout).unwrap();
    std::fs::write(
        checkout.join(".ct-areas.yaml"),
        "areas:\n  deploy:\n    - infra/**\n",
    )
    .unwrap();
    checkout
}

fn deposit_params(path: &str, title: &str, body: &str) -> WisdomDepositParams {
    WisdomDepositParams {
        path: path.to_string(),
        title: title.to_string(),
        body: body.to_string(),
        tags: Some(vec!["alpha".to_string()]),
        attribution: None,
        repo_path: None,
    }
}

const HUB_ONLY_PREFIX: &str = "---\nhub: hub-a\n---\n";
const ATTRIB_PREFIX: &str =
    "---\nattributed_speaker: kurt\nattribution_kind: restated_at_direction\nattribution_source: discord:123\n";
const BASELINE: &str = "# Test title\n\n`alpha`\n\nBeta is written in Rust.\n";

fn read_deposit_file(vault: &Path, rel: &str) -> String {
    std::fs::read_to_string(vault.join(rel)).unwrap()
}

/// Mock LLM returning one fact-add proposal for `target` (with an optional
/// in-vocabulary `area`).
fn mock_llm(server: &mut mockito::ServerGuard, target: &str, area: Option<&str>) -> mockito::Mock {
    let mut fact = serde_json::json!({
        "op": "add", "body": "Scoped fact.", "tags": [],
        "confidence": "inferred", "evidence": ["C1"]
    });
    if let Some(a) = area {
        fact["area"] = serde_json::Value::String(a.to_string());
    }
    let llm_json = serde_json::json!({
        "proposals": [{
            "target": { "existing_id": target },
            "reasoning": "E2E carrier.",
            "summary_update": null,
            "facts": [fact],
            "edges": [],
            "tasks": []
        }]
    });
    server
        .mock("POST", "/v1/chat/completions")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            "{{\"choices\":[{{\"message\":{{\"content\":{}}}}}]}}",
            serde_json::to_string(&llm_json.to_string()).unwrap()
        ))
        .create()
}

// ---------------------------------------------------------------------------
// (i) render + re-parse + full pipeline
// ---------------------------------------------------------------------------

/// (i) hub-only block: `hub:` renders as the LEADING frontmatter when there
/// is no attribution, the ingest pipeline re-parses the file (documents row
/// keyed by the file path, chunk text free of frontmatter tokens), and the
/// Librarian RESOLVES the hub from the FILE (entry committed under hub-a).
#[test]
fn e2e_hub_only_render_reparses_and_librarian_resolves() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    let vault = dir.path().join("vault");
    let checkout = make_checkout(dir.path(), "hub-a");
    let mut server = mockito::Server::new();
    let _mock = mock_llm(&mut server, "hub-a", Some("deploy"));

    let brain_str = vault.to_string_lossy().into_owned();
    with_brain(&brain_str, || async move {
        let ctx = carrier_ctx(&vault);
        write_generation_config(&vault);
        patch_config(&vault, "wisdom", wisdom_block(&checkout, "hub-a"));
        // Keyed patch: replace ONLY external_url, keep provider/model_name.
        {
            let cfg_path = vault.join("config.json");
            let mut cfg: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
            cfg["generation"]["external_url"] = serde_json::Value::String(server.url());
            std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();
        }
        {
            let conn = ctx.conn.lock().unwrap();
            add_entity(&conn, "hub-a");
            // Auto-approve the agents lane so proposals COMMIT (same shape
            // as areas_write_path.rs seed_deposit).
            let folder = std::fs::canonicalize(vault.join("immutable-source-files/agents"))
                .unwrap()
                .to_string_lossy()
                .into_owned();
            conn.execute(
                "INSERT INTO folder_rules (folder_path, librarian_mode, auto_approve) VALUES (?1, 'synthesize', 1)",
                [folder],
            ).unwrap();
        }
        let rel = "immutable-source-files/agents/hub_only.md";
        let v = dispatch_wisdom_deposit_awaiting_kick(
            &ctx,
            WisdomDepositParams {
                repo_path: Some(checkout.join("src").join("f.rs").to_string_lossy().into_owned()),
                ..deposit_params(rel, "Test title", "Beta is written in Rust.")
            },
        )
        .await
        .unwrap();
        assert_eq!(v["hub"], "hub-a", "resolved survivor id in the reply");

        let content = read_deposit_file(&vault, rel);
        assert!(
            content.starts_with(HUB_ONLY_PREFIX),
            "hub-only render must start with the hub frontmatter block: {content:?}"
        );
        // No attribution lines: the block is hub-only.
        assert!(!content.contains("attributed_"), "no attribution lines");

        // Re-parse + Librarian resolution through the REAL ingest leg: the
        // documents row exists for the deposit file, and the committed entry
        // is bound to hub-a (resolved from the file's `hub:` line). The kick
        // stages the row under the canonical/virtual root key form (B1), so
        // match on the deposit file's name under the lane.
        let conn = ctx.conn.lock().unwrap();
        let doc_id: i64 = conn
            .query_row(
                "SELECT id FROM documents WHERE path LIKE '%/immutable-source-files/agents/hub_only.md'",
                [],
                |r| r.get(0),
            )
            .expect("deposit documents row must exist");
        let chunk_texts: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT chunk_text FROM chunks WHERE doc_id = ?1")
                .unwrap();
            let rows = stmt.query_map([doc_id], |r| r.get::<_, String>(0)).unwrap();
            rows.map(|r| r.unwrap()).collect()
        };
        assert!(!chunk_texts.is_empty(), "deposit must have chunks");
        for t in &chunk_texts {
            assert!(
                !t.contains("---") && !t.contains("hub:") && !t.contains("attributed_"),
                "chunk text must be free of frontmatter tokens: {t:?}"
            );
        }
        let hub_entries: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_entries WHERE entity_id = 'hub-a' AND deleted_at IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(hub_entries >= 1, "Librarian must resolve the hub from the file");
    });
}

/// (i) attribution+hub block: attribution lines ride FIRST, `hub:` is LAST
/// in the SAME frontmatter block — one leading block, not two.
#[test]
fn e2e_attribution_then_hub_single_block() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    let vault = dir.path().join("vault");
    let checkout = make_checkout(dir.path(), "hub-a");

    let brain_str = vault.to_string_lossy().into_owned();
    with_brain(&brain_str, || async move {
        let ctx = carrier_ctx(&vault);
        write_generation_config(&vault);
        patch_config(&vault, "wisdom", wisdom_block(&checkout, "hub-a"));
        {
            let conn = ctx.conn.lock().unwrap();
            add_entity(&conn, "hub-a");
        }
        let rel = "immutable-source-files/agents/attrib_hub.md";
        dispatch_wisdom_deposit_awaiting_kick(
            &ctx,
            WisdomDepositParams {
                repo_path: Some(checkout.join("f.rs").to_string_lossy().into_owned()),
                attribution: Some(tauri_app_lib::wisdom_deposit::DepositAttribution {
                    speaker: "kurt".into(),
                    kind: tauri_app_lib::wisdom_deposit::AttributionKind::RestatedAtDirection,
                    source: "discord:123".into(),
                }),
                ..deposit_params(rel, "Test title", "Beta is written in Rust.")
            },
        )
        .await
        .unwrap();

        let content = read_deposit_file(&vault, rel);
        let expected = format!(
            "{ATTRIB_PREFIX}hub: hub-a\n---\n# Test title\n\n`alpha`\n\nBeta is written in Rust.\n"
        );
        assert_eq!(
            content, expected,
            "attribution lines FIRST, hub: LAST, one leading block"
        );
    });
}

// ---------------------------------------------------------------------------
// (ii) Docker no_prefix case (IOC A7) — no area stamped
// ---------------------------------------------------------------------------

/// (ii) `repo_path` outside EVERY configured checkout root → reply
/// `hub: null` / `hub_reason: "no_prefix"`, the file carries NO `hub:` line,
/// and the committed fact carries a NULL area even though the model proposed
/// an in-vocabulary area (Docker case: the vault is outside any checkout).
#[test]
fn e2e_repo_path_outside_every_checkout_is_no_prefix_with_null_area() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    let vault = dir.path().join("vault");
    let checkout = make_checkout(dir.path(), "hub-a");
    let mut server = mockito::Server::new();
    let _mock = mock_llm(&mut server, "hub-a", Some("deploy"));

    let brain_str = vault.to_string_lossy().into_owned();
    with_brain(&brain_str, || async move {
        let ctx = carrier_ctx(&vault);
        write_generation_config(&vault);
        patch_config(&vault, "wisdom", wisdom_block(&checkout, "hub-a"));
        // Keyed patch: replace ONLY external_url, keep provider/model_name.
        {
            let cfg_path = vault.join("config.json");
            let mut cfg: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
            cfg["generation"]["external_url"] = serde_json::Value::String(server.url());
            std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();
        }
        {
            let conn = ctx.conn.lock().unwrap();
            add_entity(&conn, "hub-a");
            // Auto-approve the agents lane so proposals COMMIT (same shape
            // as areas_write_path.rs seed_deposit).
            let folder = std::fs::canonicalize(vault.join("immutable-source-files/agents"))
                .unwrap()
                .to_string_lossy()
                .into_owned();
            conn.execute(
                "INSERT INTO folder_rules (folder_path, librarian_mode, auto_approve) VALUES (?1, 'synthesize', 1)",
                [folder],
            ).unwrap();
        }
        let rel = "immutable-source-files/agents/docker.md";
        let v = dispatch_wisdom_deposit_awaiting_kick(
            &ctx,
            WisdomDepositParams {
                repo_path: Some("/elsewhere/repo/src/f.rs".to_string()),
                ..deposit_params(rel, "Test title", "Beta is written in Rust.")
            },
        )
        .await
        .unwrap();
        assert_eq!(v["hub"], serde_json::Value::Null, "hub must be null");
        assert_eq!(v["hub_reason"], "no_prefix");
        assert!(v.get("area").is_none(), "no area stamped in the reply");

        let content = read_deposit_file(&vault, rel);
        assert!(
            !content.contains("hub:"),
            "no_prefix stamps NO hub line: {content:?}"
        );
        assert!(content.starts_with("# Test title\n"), "no frontmatter block at all");

        // Through the REAL ingest leg: model proposed `deploy`, but without a
        // resolved hub the committed entry's area stays NULL.
        let conn = ctx.conn.lock().unwrap();
        let (kstate, kerr): (String, Option<String>) = conn
            .query_row(
                "SELECT state, error FROM deposit_kick_state WHERE path = ?1",
                [rel],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert!(
            matches!(kstate.as_str(), "chunked" | "ingested"),
            "kick must complete: {kstate} {kerr:?}"
        );
        let all: Vec<String> = {
            let mut stmt = conn.prepare("SELECT entity_id FROM llm_wiki_entries").unwrap();
            let rows = stmt.query_map([], |r| r.get::<_, String>(0)).unwrap();
            rows.map(|r| r.unwrap()).collect()
        };
        let area: Option<String> = conn
            .query_row(
                "SELECT area FROM llm_wiki_entries WHERE entity_id = 'hub-a' AND deleted_at IS NULL",
                [],
                |r| r.get::<_, Option<String>>(0),
            )
            .ok()
            .flatten();
        // §6.8: whatever the unscoped pipeline decided (fact dropped at the
        // §6.6 gate, or committed degraded), the committed fact must never
        // carry a stamped area without a resolved deposit hub.
        assert_eq!(
            area, None,
            "no area may be stamped without a resolved hub (entries: {all:?})"
        );
    });
}

// ---------------------------------------------------------------------------
// (iii) relative repo_path refused at the door
// ---------------------------------------------------------------------------

#[test]
fn relative_repo_path_is_refused_at_the_door() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    let vault = dir.path().join("vault");
    let checkout = make_checkout(dir.path(), "hub-a");

    let brain_str = vault.to_string_lossy().into_owned();
    with_brain(&brain_str, || async move {
        let ctx = carrier_ctx(&vault);
        write_generation_config(&vault);
        patch_config(&vault, "wisdom", wisdom_block(&checkout, "hub-a"));
        let rel = "immutable-source-files/agents/relative.md";
        let err = dispatch_wisdom_deposit_awaiting_kick(
            &ctx,
            WisdomDepositParams {
                repo_path: Some("checkout/f.rs".to_string()),
                ..deposit_params(rel, "Test title", "Beta is written in Rust.")
            },
        )
        .await
        .expect_err("relative repo_path must be refused at the door");
        let msg = format!("{err:#}");
        assert!(
            msg.to_lowercase().contains("absolute"),
            "refusal must say the path must be absolute: {msg}"
        );
        assert!(
            !vault.join(rel).exists(),
            "a refused deposit must write nothing"
        );
    });
}

// ---------------------------------------------------------------------------
// (iv) both-absent render is byte-identical to the pre-attribution pin
// ---------------------------------------------------------------------------

/// (iv) no attribution AND no hub → NO frontmatter block; the file is
/// byte-identical to the pre-attribution baseline (the render-side twin of
/// the unit pin `unattributed_deposit_renders_byte_identical`).
#[test]
fn e2e_both_absent_render_is_byte_identical_to_baseline() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    let vault = dir.path().join("vault");

    let brain_str = vault.to_string_lossy().into_owned();
    with_brain(&brain_str, || async move {
        let ctx = carrier_ctx(&vault);
        write_generation_config(&vault);
        // NO wisdom block: also exercises the door's absent path (no config).
        let rel = "immutable-source-files/agents/plain.md";
        dispatch_wisdom_deposit_awaiting_kick(
            &ctx,
            deposit_params(rel, "Test title", "Beta is written in Rust."),
        )
        .await
        .unwrap();
        assert_eq!(
            read_deposit_file(&vault, rel),
            BASELINE,
            "both-absent render must stay byte-identical"
        );
    });
}

// ---------------------------------------------------------------------------
// (v) reply contract for ALL FOUR reasons
// ---------------------------------------------------------------------------

#[test]
fn reply_contract_covers_all_four_hub_reasons() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    let vault = dir.path().join("vault");
    let checkout = make_checkout(dir.path(), "hub-a");
    let cyc_root = dir.path().join("cyc");

    let brain_str = vault.to_string_lossy().into_owned();
    with_brain(&brain_str, || async move {
        let ctx = carrier_ctx(&vault);
        write_generation_config(&vault);

        // 1. absent: no repo_path param at all (door translates BEFORE the
        // resolver runs — resolver takes &str only).
        let v = dispatch_wisdom_deposit_awaiting_kick(
            &ctx,
            deposit_params(
                "immutable-source-files/agents/r_absent.md",
                "Test title",
                "Beta is written in Rust.",
            ),
        )
        .await
        .unwrap();
        assert_eq!(v["hub"], serde_json::Value::Null);
        assert_eq!(v["hub_reason"], "absent");

        // 2. wisdom_unconfigured: repo_path given, no wisdom block.
        let v = dispatch_wisdom_deposit_awaiting_kick(
            &ctx,
            WisdomDepositParams {
                repo_path: Some(checkout.join("f.rs").to_string_lossy().into_owned()),
                ..deposit_params(
                    "immutable-source-files/agents/r_unconf.md",
                    "Test title",
                    "Beta is written in Rust.",
                )
            },
        )
        .await
        .unwrap();
        assert_eq!(v["hub"], serde_json::Value::Null);
        assert_eq!(v["hub_reason"], "wisdom_unconfigured");

        // 3. no_prefix: wisdom block present, path matches no root.
        patch_config(&vault, "wisdom", wisdom_block(&checkout, "hub-a"));
        let v = dispatch_wisdom_deposit_awaiting_kick(
            &ctx,
            WisdomDepositParams {
                repo_path: Some("/elsewhere/x.rs".to_string()),
                ..deposit_params(
                    "immutable-source-files/agents/r_noprefix.md",
                    "Test title",
                    "Beta is written in Rust.",
                )
            },
        )
        .await
        .unwrap();
        assert_eq!(v["hub"], serde_json::Value::Null);
        assert_eq!(v["hub_reason"], "no_prefix");

        // 4. unresolved: the matched key's hub cannot bind (redirect cycle).
        {
            let conn = ctx.conn.lock().unwrap();
            add_entity(&conn, "hub-a");
            add_entity(&conn, "cyc-1");
            add_entity(&conn, "cyc-2");
            add_redirect(&conn, "cyc-1", "cyc-2");
            add_redirect(&conn, "cyc-2", "cyc-1");
            std::fs::create_dir_all(&cyc_root).unwrap();
            std::fs::write(cyc_root.join(".ct-areas.yaml"), "areas: {}\n").unwrap();
            patch_config(
                &vault,
                "wisdom",
                serde_json::json!({
                    "areas_paths": {
                        "hub-a": checkout.join(".ct-areas.yaml").to_string_lossy(),
                        "cyc-1": cyc_root.join(".ct-areas.yaml").to_string_lossy()
                    },
                    "org_entity_id": "org-e2e"
                }),
            );
        }
        let cyc = cyc_root.join("f.rs");
        let v = dispatch_wisdom_deposit_awaiting_kick(
            &ctx,
            WisdomDepositParams {
                repo_path: Some(cyc.to_string_lossy().into_owned()),
                ..deposit_params(
                    "immutable-source-files/agents/r_unresolved.md",
                    "Test title",
                    "Beta is written in Rust.",
                )
            },
        )
        .await
        .unwrap();
        assert_eq!(v["hub"], serde_json::Value::Null);
        assert_eq!(v["hub_reason"], "unresolved", "cycle → unresolved");
    });
}

// ---------------------------------------------------------------------------
// (vi) status re-reads hub from the FILE, omits hub_reason
// ---------------------------------------------------------------------------

#[test]
fn status_rereads_hub_from_the_file_and_omits_hub_reason() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    let vault = dir.path().join("vault");
    let checkout = make_checkout(dir.path(), "hub-a");

    let brain_str = vault.to_string_lossy().into_owned();
    with_brain(&brain_str, || async move {
        let ctx = carrier_ctx(&vault);
        write_generation_config(&vault);
        patch_config(&vault, "wisdom", wisdom_block(&checkout, "hub-a"));
        {
            let conn = ctx.conn.lock().unwrap();
            add_entity(&conn, "hub-a");
        }
        let rel = "immutable-source-files/agents/status_hub.md";
        dispatch_wisdom_deposit_awaiting_kick(
            &ctx,
            WisdomDepositParams {
                repo_path: Some(checkout.join("f.rs").to_string_lossy().into_owned()),
                ..deposit_params(rel, "Test title", "Beta is written in Rust.")
            },
        )
        .await
        .unwrap();

        let s = dispatch_wisdom_deposit_status(
            &ctx,
            tauri_app_lib::wisdom_deposit::WisdomDepositStatusParams {
                path: rel.to_string(),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            s["hub"], "hub-a",
            "status must RE-READ hub from the deposit file"
        );
        assert!(
            s.get("hub_reason").is_none(),
            "status must NOT echo hub_reason (reply-only, spec rev-9 m1): {s}"
        );

        // And a no-hub deposit: status reports hub null, still no hub_reason.
        let rel2 = "immutable-source-files/agents/status_nohub.md";
        dispatch_wisdom_deposit_awaiting_kick(
            &ctx,
            deposit_params(rel2, "Test title", "Beta is written in Rust."),
        )
        .await
        .unwrap();
        let s2 = dispatch_wisdom_deposit_status(
            &ctx,
            tauri_app_lib::wisdom_deposit::WisdomDepositStatusParams {
                path: rel2.to_string(),
            },
        )
        .await
        .unwrap();
        assert_eq!(s2["hub"], serde_json::Value::Null);
        assert!(s2.get("hub_reason").is_none());
    });
}

// ---------------------------------------------------------------------------
// (vii) an absent hub emits NO production report line
// ---------------------------------------------------------------------------

/// (vii) §6.8.5's negative claim: an ABSENT hub is the normal case — the
/// reply carries no hub-related reason beyond `hub: null` and NEITHER §11.2
/// bucket ticks for it (only the three failure reasons are reportable).
#[test]
fn absent_hub_emits_no_production_report_line() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    let vault = dir.path().join("vault");

    let brain_str = vault.to_string_lossy().into_owned();
    with_brain(&brain_str, || async move {
        let ctx = carrier_ctx(&vault);
        write_generation_config(&vault);
        let unresolved_before = deposit_hub_unresolved_count();
        let unconfigured_before = tauri_app_lib::librarian::wisdom_hub_unconfigured_count();

        let rel = "immutable-source-files/agents/absent.md";
        let v = dispatch_wisdom_deposit_awaiting_kick(
            &ctx,
            deposit_params(rel, "Test title", "Beta is written in Rust."),
        )
        .await
        .unwrap();

        assert_eq!(v["hub"], serde_json::Value::Null);
        // The reply carries ONLY the contract keys — no free-form hub
        // diagnostic line beyond hub + hub_reason.
        let extra: Vec<&String> = v
            .as_object()
            .unwrap()
            .keys()
            .filter(|k| k.contains("hub") && **k != "hub" && **k != "hub_reason")
            .collect();
        assert!(
            extra.is_empty(),
            "an absent hub must emit NO production report line: {extra:?}"
        );
        assert_eq!(
            deposit_hub_unresolved_count(),
            unresolved_before,
            "absent must NOT tick the §11.2 unresolved bucket"
        );
        assert_eq!(
            tauri_app_lib::librarian::wisdom_hub_unconfigured_count(),
            unconfigured_before,
            "absent must NOT tick the §11.2 wisdom_unconfigured bucket"
        );
    });
}

// ---------------------------------------------------------------------------
// (viii) hand-written hub: live NON-configured entity (door-side answer)
// ---------------------------------------------------------------------------

/// (viii) A HAND-WRITTEN `hub:` naming a LIVE entity that is NOT a
/// configured hub: the deposit door itself still succeeds (a stray hub file
/// in the lane never blocks a deposit), and the door-visible reply —
/// `wisdom_deposit_status` re-reading `hub` from the file — carries
/// `hub: null` (never echoing hub_reason) while the §11.2 unresolved bucket
/// ticks via the shared read (the door-side twin of Task 6's read-side
/// test).
#[test]
fn hand_written_hub_to_live_nonconfigured_entity_unresolved() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    let vault = dir.path().join("vault");
    let checkout = make_checkout(dir.path(), "hub-a");

    let brain_str = vault.to_string_lossy().into_owned();
    with_brain(&brain_str, || async move {
        let ctx = carrier_ctx(&vault);
        write_generation_config(&vault);
        patch_config(&vault, "wisdom", wisdom_block(&checkout, "hub-a"));
        {
            let conn = ctx.conn.lock().unwrap();
            add_entity(&conn, "hub-a");
            // Live entity, NOT a configured hub.
            add_entity(&conn, "ent-stray");
        }

        // Hand-write the deposit file directly into the lane (§6.8.4: the
        // reader does not care which writer produced the file).
        let stray_rel = "immutable-source-files/agents/stray.md";
        let stray_path = vault.join(stray_rel);
        std::fs::write(
            &stray_path,
            "---\nhub: ent-stray\ntitle: Stray note\n---\n\nStrayRepo notes.\n",
        )
        .unwrap();
        // Seed the kick ledger row deposit_status_row requires.
        {
            let conn = ctx.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO deposit_kick_state (path, state, error, updated_ms)
                 VALUES (?1, 'no_ingest_host', NULL, 100)",
                [stray_rel],
            )
            .unwrap();
        }

        // The door itself still succeeds (stray hub file never blocks).
        let rel = "immutable-source-files/agents/after_stray.md";
        dispatch_wisdom_deposit_awaiting_kick(
            &ctx,
            WisdomDepositParams {
                repo_path: Some(checkout.join("f.rs").to_string_lossy().into_owned()),
                ..deposit_params(rel, "Test title", "Beta is written in Rust.")
            },
        )
        .await
        .expect("a hand-written stray hub file must never block a deposit");

        // Door-visible reply: status re-reads the stray hub → unresolved.
        let unresolved_before = deposit_hub_unresolved_count();
        let s = dispatch_wisdom_deposit_status(
            &ctx,
            tauri_app_lib::wisdom_deposit::WisdomDepositStatusParams {
                path: stray_rel.to_string(),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            s["hub"],
            serde_json::Value::Null,
            "a live NON-configured entity resolves to hub: null"
        );
        assert!(s.get("hub_reason").is_none(), "status never echoes hub_reason");
        assert!(
            deposit_hub_unresolved_count() > unresolved_before,
            "the unresolved hub must tick the §11.2 bucket"
        );
    });
}

// ---------------------------------------------------------------------------
// Resolver-shape guard: the door consumes resolve_deposit_hub directly
// ---------------------------------------------------------------------------

/// The resolver contract the door relies on stays three-way (Task 3): this
/// in-crate guard keeps the door's translation table honest.
#[test]
fn resolver_contract_guard_for_the_door() {
    let conn = open_in_memory().unwrap();
    assert_eq!(
        resolve_deposit_hub(&conn, None, "/anywhere/repo"),
        tauri_app_lib::config::hub_resolver::DepositHub::Unconfigured
    );
}
