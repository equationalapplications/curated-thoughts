pub mod chunker;
pub mod cloud_bridge;
pub mod commands;
pub mod config;
pub mod db;
pub mod doctor;
pub mod embed_sweep;
pub mod embedder;
mod entities_api;
pub mod graph;
mod hasher;
pub mod indexer;
pub mod inference;
pub mod librarian;
#[cfg(feature = "mcp-server")]
pub mod mcp_server;
pub mod okf;
mod okf_api;
pub mod onboard;
pub mod ontology_config;
pub mod outbox;
mod pipeline;
pub mod privacy;
mod proposals_api;
pub mod recall_bench_fixture;
pub mod retrieval;
pub mod scifact_fixture;
pub mod search;
mod setup;
mod timeline_api;
pub mod tool_dispatch;
pub mod trusted_links;
pub mod vault;
pub mod walk_vault;
pub mod watcher;
pub mod wiki_graph;

use crate::embedder::embed_batch;
use crate::inference::{
    download_model_weights, download_sidecar_engine, generate_text, get_provider_config,
    initialize_provider, update_provider, GenerationProvider, InferenceState,
};
use crate::vault::safe_path::AGENTS_DEPOSIT_DIR;
use chunker::should_ingest_extension;
use cloud_bridge::pairing::PairingTokenStore;
use db::queue::enqueue_vault_event;
use db::AppDb;
use outbox::{
    postgres::{spawn_postgres_worker, OutboxWorkerHandle},
    OutboxConfig,
};
#[cfg(feature = "test-utils")]
pub use pipeline::watchdog::heartbeat::Heartbeat;
#[cfg(not(feature = "test-utils"))]
use pipeline::PipelineJob;
use pipeline::{start_pipeline, PipelineStatusEvent};
#[cfg(feature = "test-utils")]
pub use pipeline::{PipelineJob, PipelineWorker};
use rusqlite::types::Value as SqlVal;
use rusqlite::OptionalExtension;
use serde_json::{json, Value as JsonVal};
use setup::{
    check_ollama as ollama_check, list_local_models as ollama_models, pull_model as ollama_pull,
    recommended_model as ollama_recommended, start_ollama_server as ollama_start, OllamaStatus,
};
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{mpsc::Sender, Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};
use vault::VaultConfig;
use watcher::{spawn_vault_watcher, VaultEvent, VaultLock, WatcherHandle};

pub struct DbState(Mutex<AppDb>);
struct VaultConfigState(Mutex<VaultConfig>);
struct EmbedProfileState(Mutex<crate::embedder::EmbedProfile>);

/// Inner components of `PipelineHolder`: the job sender, worker thread, the
/// consumer-side job counter, the optional channel for `PipelineStatusEvent`
/// listeners, and the worker's `Heartbeat` (spec §2.1) shared with the
/// watchdog supervisor. Extracted as a type alias so `PipelineHolder`'s
/// declaration stays below clippy::type_complexity's threshold.
type PipelineHolderInner = Option<pipeline::PipelineHandle>;

struct PipelineHolder(Mutex<PipelineHolderInner>);
struct WatcherStarted(Mutex<Option<(PathBuf, WatcherHandle)>>);
struct HealScheduler(Mutex<Option<(Sender<()>, std::thread::JoinHandle<()>)>>);
struct WikiStatusState(Mutex<WikiStatusFlags>);
/// Active watchdog supervisor: stop flag + JoinHandle. `switch_vault` and
/// repeated `start_file_watcher` calls set the flag and join the handle
/// before spawning a new supervisor so old supervisors cannot keep writing
/// `flags.ingest.health` (CodeRabbit review PRRT_kwDOSVmXas6d28dj).
struct WatchdogSupervisor(Mutex<Option<WatchdogSupervisorHandle>>);
struct WatchdogSupervisorHandle {
    stop: Arc<AtomicBool>,
    join: std::thread::JoinHandle<()>,
}
struct OutboxWorkerState(Mutex<Option<OutboxWorkerHandle>>);
struct CloudBridgeState(Mutex<Option<cloud_bridge::CloudBridgeHandle>>);
struct CloudBridgeLifecycle(tokio::sync::Mutex<()>);

/// Latest `config-malformed` payload surfaced by the setup hook. The setup
/// thread can emit before the frontend listener registers (Tauri does not
/// buffer events), so we also stash the payload here and let `AppShell`
/// drain it on mount via `peek_pending_config_malformed` — CodeRabbit #19,
/// PR #120.
#[derive(serde::Serialize, Clone)]
struct ConfigMalformedPayload {
    config_path: String,
    diagnostics: Vec<String>,
    remediation: String,
}

struct PendingConfigMalformed(Mutex<Option<ConfigMalformedPayload>>);

// No longer `Copy`: `ingest` carries a stage and subject, so the derive
// narrows to `Clone` (spec §7).
#[derive(Clone, Default)]
struct IngestStatus {
    health: crate::pipeline::watchdog::PipelineHealth,
    stage: Option<String>,
    subject: Option<String>,
}

#[derive(Clone, Default)]
struct WikiStatusFlags {
    ingest: IngestStatus,
    librarian: bool,
    healing: bool,
    pruning: bool,
    forgetting: bool,
}

fn emit_wiki_status(app: &AppHandle, current: &WikiStatusFlags) {
    let _ = app.emit(
        "wiki-status-change",
        json!({
            "ingest": current.ingest.health.as_str(),
            "ingestStage": current.ingest.stage,
            "ingestSubject": current.ingest.subject,
            "librarian": current.librarian,
            "healing": current.healing,
            "pruning": current.pruning,
            "forgetting": current.forgetting,
        }),
    );
}

fn update_wiki_status(
    app: &AppHandle,
    state: &State<'_, WikiStatusState>,
    updater: impl FnOnce(&mut WikiStatusFlags),
) {
    let mut guard = state.0.lock().unwrap();
    updater(&mut guard);
    emit_wiki_status(app, &guard);
}

fn normalize_database_url(url: String) -> Option<String> {
    let db_url = url.trim();
    if db_url.is_empty() {
        None
    } else {
        Some(db_url.to_string())
    }
}

fn configured_database_url() -> Option<String> {
    std::env::var("DATABASE_URL")
        .ok()
        .and_then(normalize_database_url)
}

/// Builds the read-only `ToolDispatchContext` the cloud bridge queries against — its own
/// connection via `retrieval::open_brain_readonly`, independent of `DbState`, so a slow
/// embed/query never contends with the GUI's live read/write connection (mirrors how the
/// `--mcp` binary opens its own connection in `mcp_server::async_run`).
fn build_cloud_bridge_ctx() -> anyhow::Result<tool_dispatch::ToolDispatchContext> {
    let paths = retrieval::resolve_brain_paths();
    let profile = retrieval::load_embed_profile(&paths.config_path)?;
    let conn = retrieval::open_brain_readonly(&paths.db_path)?;
    let vault_dir = VaultConfig::new(paths.config_path.clone())
        .get_vault_path()
        .ok()
        .flatten()
        .map(PathBuf::from)
        .and_then(|p| p.canonicalize().ok());
    Ok(tool_dispatch::ToolDispatchContext {
        conn: Arc::new(Mutex::new(conn)),
        profile,
        vault_dir,
        client: "clanker-bridge".into(),
    })
}

async fn start_cloud_bridge_if_configured_unlocked(state: &CloudBridgeState) {
    let brain_dir = PathBuf::from(get_brain_dir_inner());
    let privacy = match privacy::resolve_privacy_state(
        &brain_dir,
        &cloud_bridge::pairing::KeyringPairingTokenStore,
    ) {
        Ok(state) => state,
        Err(e) => {
            eprintln!("[cloud_bridge] failed to resolve privacy state: {e}");
            return;
        }
    };
    if !privacy::allows_cloud_bridge(privacy.mode) {
        let existing = { state.0.lock().unwrap().take() };
        if let Some(handle) = existing {
            handle.stop().await;
        }
        return;
    }
    let Some(config) = cloud_bridge::CloudBridgeConfig::resolve() else {
        return;
    };
    let Ok(Some(token)) = cloud_bridge::pairing::KeyringPairingTokenStore.get() else {
        return;
    };
    let ctx = match build_cloud_bridge_ctx() {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("[cloud_bridge] failed to build dispatch context: {e}");
            return;
        }
    };
    let existing = { state.0.lock().unwrap().take() };
    if let Some(handle) = existing {
        handle.stop().await;
    }
    let handle = cloud_bridge::spawn(config, token, ctx);
    *state.0.lock().unwrap() = Some(handle);
}

async fn start_cloud_bridge_if_configured(
    lifecycle: &CloudBridgeLifecycle,
    state: &CloudBridgeState,
) {
    let _guard = lifecycle.0.lock().await;
    start_cloud_bridge_if_configured_unlocked(state).await;
}

fn cloud_bridge_is_pairing_configured() -> bool {
    let has_token = cloud_bridge::pairing::KeyringPairingTokenStore
        .get()
        .ok()
        .flatten()
        .is_some();
    has_token && cloud_bridge::CloudBridgeConfig::resolve().is_some()
}

fn validate_outbox_database_url(database_url: Option<String>) -> Result<String, String> {
    match database_url {
        Some(url) => {
            let url = url.trim();
            if url.is_empty() {
                Err("database_url cannot be empty".to_string())
            } else {
                Ok(url.to_string())
            }
        }
        None => configured_database_url().ok_or_else(|| {
            "DATABASE_URL is not configured; runtime outbox start is not allowed.".to_string()
        }),
    }
}

async fn replace_outbox_worker(state: &OutboxWorkerState, new_handle: OutboxWorkerHandle) -> bool {
    let old_handle = { state.0.lock().unwrap().take() };
    let replaced = old_handle.is_some();
    if let Some(handle) = old_handle {
        handle.stop().await;
    }
    *state.0.lock().unwrap() = Some(new_handle);
    replaced
}

fn update_wiki_status_from_app(app: &AppHandle, updater: impl FnOnce(&mut WikiStatusFlags)) {
    let state = app.state::<WikiStatusState>();
    update_wiki_status(app, &state, updater);
}

async fn spawn_outbox_worker_if_configured(
    app: &AppHandle,
    outbox_state: State<'_, OutboxWorkerState>,
    sqlite_path: PathBuf,
    fallback_config: Option<OutboxConfig>,
) {
    let existing_handle = {
        let mut guard = outbox_state.0.lock().unwrap();
        guard.take()
    };

    if let Some(handle) = existing_handle {
        handle.stop().await;
    }

    let config = if let Some(mut config) = fallback_config {
        config.sqlite_path = sqlite_path;
        config
    } else if let Some(db_url) = configured_database_url() {
        OutboxConfig {
            sqlite_path,
            db_url,
            ..OutboxConfig::default()
        }
    } else {
        return;
    };

    let handle = spawn_postgres_worker(config, Some(app.clone()));
    *outbox_state.0.lock().unwrap() = Some(handle);
    let _ = app.emit("outbox-worker-started", ());
}

/// Vault-relative display path; rejects traversal so `..` cannot be silently dropped.
fn to_forward_slash_relative(path: &Path) -> Result<String, String> {
    let mut out = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(seg) => out.push(seg.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir => return Err("path contains traversal segment".to_string()),
            Component::Prefix(_) => {
                return Err("path contains invalid prefix component".to_string())
            }
            Component::RootDir => return Err("path contains unexpected root component".to_string()),
        }
    }
    Ok(out.join("/"))
}

/// Convert an absolute path under the vault into a vault-relative forward-slash path.
/// Uses canonical prefixes so DB/search paths still work when spellings differ (symlinks,
/// `/var` vs `/private/var` on macOS, etc.). Paths that canonicalize outside the vault are rejected.
fn normalize_path_argument_to_vault_relative(
    path: &str,
    vault_root: &Path,
) -> Result<String, String> {
    let candidate = Path::new(path);
    if !candidate.is_absolute() {
        return Ok(path.to_string());
    }

    let canon_root = vault_root
        .canonicalize()
        .map_err(|e| format!("failed to canonicalize vault root: {}", e))?;

    if let Ok(canon_candidate) = candidate.canonicalize() {
        if !canon_candidate.starts_with(&canon_root) {
            return Err("absolute path outside vault".to_string());
        }
        let rel = canon_candidate
            .strip_prefix(&canon_root)
            .map_err(|_| "path strip failed".to_string())?;
        return to_forward_slash_relative(rel);
    }

    // Path may not exist yet, or the vault root spelling may differ from the path prefix until
    // an ancestor canonicalizes. Walk up until some prefix resolves under `canon_root`.
    let mut cur = candidate.to_path_buf();
    let mut tail = PathBuf::new();
    loop {
        match cur.canonicalize() {
            Ok(canon_prefix) => {
                if !canon_prefix.starts_with(&canon_root) {
                    return Err("absolute path outside vault".to_string());
                }
                let inside = canon_prefix
                    .strip_prefix(&canon_root)
                    .map_err(|_| "path strip failed".to_string())?;
                let combined = if inside.as_os_str().is_empty() {
                    tail
                } else if tail.as_os_str().is_empty() {
                    inside.to_path_buf()
                } else {
                    inside.join(&tail)
                };
                return to_forward_slash_relative(&combined);
            }
            Err(_) => {
                let name = cur
                    .file_name()
                    .ok_or_else(|| "absolute path outside vault".to_string())?
                    .to_owned();
                tail = Path::new(&name).join(&tail);
                if !cur.pop() {
                    return Err("absolute path outside vault".to_string());
                }
            }
        }
    }
}

fn heal_invalid_sources(db_state: &DbState, vault_state: &VaultConfigState) -> Result<(), String> {
    let vault = vault_state
        .0
        .lock()
        .unwrap()
        .get_vault_path()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "no vault configured".to_string())?;
    // The vault path is read here so the function fails fast when no vault
    // is configured (matches pre-fix behavior). The actual existence checks
    // are now driven by `source_ref_is_still_grounded`, which checks the
    // `documents.path` and `chunks.id` tables — the vault_root is not
    // consulted on disk any more.
    let _vault_root = std::path::PathBuf::from(&vault);

    let guard = db_state.0.lock().unwrap();
    let conn = &guard.0;

    let entries: Vec<(i64, String, String, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT e.rowid, e.source_ref, e.entity_id, e.id
                 FROM llm_wiki_entries e
                 WHERE e.deleted_at IS NULL
                   AND e.source_ref IS NOT NULL
                   AND e.source_type = 'librarian_inferred'",
            )
            .map_err(|e| e.to_string())?;
        let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
        let mut v = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            v.push((
                row.get::<_, i64>(0).map_err(|e| e.to_string())?,
                row.get::<_, String>(1).map_err(|e| e.to_string())?,
                row.get::<_, String>(2).map_err(|e| e.to_string())?,
                row.get::<_, String>(3).map_err(|e| e.to_string())?,
            ));
        }
        v
    };

    let mut healed_by_entity: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for (rowid, source_ref, entity_id, entry_id) in entries {
        // Bug A + B fix: use the shared consumer helper
        // (`source_ref_is_still_grounded`) instead of `safe_vault_path`.
        // The legacy producer wrote a vault-relative path; the post-c30f141
        // producer writes a JSON blob. The helper handles both and treats
        // empty / unparseable values as still-grounded (defensive — see
        // its docs).
        if !crate::db::commit::source_ref_is_still_grounded(conn, &source_ref) {
            let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
            tx.execute(
                "UPDATE llm_wiki_entries SET deleted_at = ?1 WHERE rowid = ?2",
                rusqlite::params![crate::db::commit::ms_now(), rowid],
            )
            .map_err(|e| e.to_string())?;
            crate::db::edge_purge::purge_edges_for_entry(&tx, &entry_id)
                .map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
            *healed_by_entity.entry(entity_id).or_insert(0) += 1;
        }
    }

    // Write healed events for entities that had entries repaired
    if !healed_by_entity.is_empty() {
        let (_, now_ms) = crate::db::commit::now_timestamps();
        for (entity_id, n) in healed_by_entity {
            conn.execute(
                "INSERT INTO llm_wiki_events (id, entity_id, event_type, summary, related_entry_id, created_at)
                 VALUES (?1, ?2, 'healed', ?3, NULL, ?4)",
                rusqlite::params![
                    crate::db::commit::generate_llm_id("evt_"),
                    entity_id,
                    format!("Healed {n} invalid source reference(s)"),
                    now_ms,
                ],
            )
            .map_err(|e| e.to_string())?;
        }
    }

    Ok(())
}

fn spawn_heal_scheduler(app: AppHandle) -> (Sender<()>, std::thread::JoinHandle<()>) {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let handle = std::thread::spawn(move || {
        let db_state = app.state::<DbState>();
        let vault_state = app.state::<VaultConfigState>();

        loop {
            if rx.recv().is_err() {
                break;
            }

            let mut deadline = Instant::now() + Duration::from_secs(3);
            loop {
                let now = Instant::now();
                let timeout = deadline.saturating_duration_since(now);
                match rx.recv_timeout(timeout) {
                    Ok(()) => {
                        deadline = Instant::now() + Duration::from_secs(3);
                        continue;
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                    Err(_) => return,
                }
            }

            update_wiki_status_from_app(&app, |flags| {
                flags.healing = true;
            });
            let _ = heal_invalid_sources(&db_state, &vault_state);
            update_wiki_status_from_app(&app, |flags| {
                flags.healing = false;
            });
        }
    });

    (tx, handle)
}

fn normalize_wiki_relative_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let already_wiki = matches!(
        Path::new(&normalized).components().next(),
        Some(Component::Normal(seg)) if seg == OsStr::new("wiki")
    );
    if already_wiki {
        normalized
    } else {
        format!("wiki/{}", normalized)
    }
}

// ── Workspace identity ────────────────────────────────────────────────────────

fn normalize_workspace_path(path: &str) -> String {
    let mut normalized = path.replace('\\', "/");
    if normalized != "/" {
        normalized = normalized.trim_end_matches('/').to_string();
        if normalized.ends_with(':') {
            normalized.push('/');
        }
        if normalized.is_empty() {
            normalized = "/".to_string();
        }
    }
    normalized
}

#[tauri::command]
fn get_workspace_id(path: String) -> String {
    // Canonicalize so a symlinked vault hashes consistently with the Rust pipeline
    // (which canonicalizes before starting). Fall back to the raw path when the
    // path doesn't exist yet (e.g. unit tests with fictional paths).
    let pb = std::path::PathBuf::from(&path);
    let canonical_str = pb
        .canonicalize()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or(path);
    // hash_bytes returns hex::encode(sha256) — 64 lowercase hex chars — safe to slice to 16.
    let normalized_path = normalize_workspace_path(&canonical_str);
    let hash = crate::hasher::hash_bytes(normalized_path.as_bytes());
    format!("tier_working::{}", &hash[..16])
}

// ── Vault commands ────────────────────────────────────────────────────────────

#[tauri::command]
fn get_vault_path(state: State<VaultConfigState>) -> Result<Option<String>, String> {
    state
        .0
        .lock()
        .unwrap()
        .get_vault_path()
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn set_vault_path(path: String, state: State<VaultConfigState>) -> Result<(), String> {
    // trusted: vault root from Tauri file picker dialog (user selects directory)
    state
        .0
        .lock()
        .unwrap()
        .set_vault_path(&path)
        .map_err(|e| e.to_string())?;
    let root = std::path::Path::new(&path);
    // v2 layout: migrate a v1 vault (documents/ → immutable-source-files/)
    // BEFORE creating subdirs, otherwise both folders would exist and the
    // startup migration would block with BothFoldersExist.
    crate::vault::config::migrate_vault(root).map_err(|e| e.to_string())?;
    for subdir in &[crate::vault::IMMUTABLE_DIR, crate::vault::WIKI_DIR] {
        std::fs::create_dir_all(root.join(subdir)).map_err(|e| e.to_string())?;
    }
    std::fs::create_dir_all(root.join(AGENTS_DEPOSIT_DIR)).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(root.join(".brain").join("converted")).map_err(|e| e.to_string())?;
    Ok(())
}

fn get_brain_dir_inner() -> String {
    let paths = retrieval::resolve_brain_paths();
    paths
        .db_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| paths.brain_dir.to_string_lossy().into_owned())
}

#[tauri::command]
fn get_brain_dir() -> String {
    get_brain_dir_inner()
}

/// Indicates how the resolved brain dir was chosen. The AgentIntegrationPanel
/// surfaces this so the user can verify they're pointing at the instance
/// they expect (env override vs. the home-dir default).
#[derive(serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum BrainDirSource {
    /// `CURATED_BRAIN_DIR` was set in the environment.
    Env,
    /// No env override; fell back to `$HOME/.brain`.
    Default,
}

/// Bundles the resolved brain dir with its source so the frontend can
/// display both in the AgentIntegrationPanel status line.
#[derive(serde::Serialize)]
struct BrainDirInfo {
    brain_dir: String,
    source: BrainDirSource,
}

#[tauri::command]
fn get_brain_dir_info() -> BrainDirInfo {
    let brain_dir = get_brain_dir_inner();
    let source = if std::env::var_os("CURATED_BRAIN_DIR").is_some() {
        BrainDirSource::Env
    } else {
        BrainDirSource::Default
    };
    BrainDirInfo { brain_dir, source }
}

#[tauri::command]
fn get_binary_path() -> Result<String, String> {
    std::env::current_exe()
        .map_err(|e| e.to_string())
        .map(|path| path.to_string_lossy().into_owned())
}

// ── OKF Write Commands ────────────────────────────────────────────────────────
// Thin Tauri adapters (spec v2): all write logic lives in `okf::write`.

fn vault_root_from_state(
    vault_root_state: &VaultConfigState,
) -> Result<std::path::PathBuf, String> {
    vault_root_state
        .0
        .lock()
        .unwrap()
        .get_vault_path()
        .map_err(|e| e.to_string())?
        .map(std::path::PathBuf::from)
        .ok_or_else(|| "Vault not configured".to_string())
}

#[tauri::command]
fn vault_write_note(
    _conn: State<DbState>,
    vault_root_state: State<VaultConfigState>,
    path: String,
    frontmatter: okf::OkfFrontmatter,
    body: String,
) -> Result<okf::WriteNoteResult, String> {
    let vault_root = vault_root_from_state(&vault_root_state)?;
    // The request frontmatter's `updated_at` is the If-Match token; on success
    // the core stamps a fresh token so the next edit must observe the new one.
    okf::write::write_note(
        &vault_root,
        &path,
        &frontmatter,
        &body,
        frontmatter.updated_at.as_deref(),
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn vault_upsert_index_entry(
    vault_root_state: State<VaultConfigState>,
    index_path: String,
    entry_name: String,
    entry_path: String,
    entry_type: String,
    metadata: Option<serde_json::Value>,
) -> Result<okf::UpsertResult, String> {
    let vault_root = vault_root_from_state(&vault_root_state)?;
    okf::write::upsert_index_entry(
        &vault_root,
        &index_path,
        &entry_name,
        &entry_path,
        &entry_type,
        metadata.as_ref(),
    )
    .map_err(|e| e.to_string())
}

/// Swaps the live DB handle for a temporary empty DB so `brain.db` can be replaced on disk.
/// Returns the temp stub path; callers must call [`cleanup_temp_stub_db`] after the stub
/// connection is dropped (otherwise `-wal` / `-shm` sidecars and the file may remain, especially on Windows).
fn release_global_db_lock(db_state: &DbState) -> Result<PathBuf, String> {
    let stub_path = std::env::temp_dir().join(format!(
        "curated-thoughts-db-stub-{}.db",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos()
    ));
    remove_sqlite_sidecars(&stub_path);
    let _ = std::fs::remove_file(&stub_path);
    // The stub lives in temp_dir() with no real config — pass the default
    // config path so open_with_config reads a (likely absent) config.json
    // and falls through to a vault_root = None migration, which is safe
    // for an empty stub DB.
    let stub = AppDb::open_with_config(&stub_path, VaultConfig::default_config_path())
        .map_err(|e| e.to_string())?;
    let mut guard = db_state.0.lock().unwrap();
    let prev = std::mem::replace(&mut *guard, stub);
    drop(guard);
    drop(prev);
    Ok(stub_path)
}

fn cleanup_temp_stub_db(stub_path: &Path) {
    remove_sqlite_sidecars(stub_path);
    let _ = std::fs::remove_file(stub_path);
}

#[tauri::command]
fn backup_vault_db(
    vault_state: State<VaultConfigState>,
    db_state: State<DbState>,
) -> Result<String, String> {
    let vault = vault_state
        .0
        .lock()
        .unwrap()
        .get_vault_path()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "no vault configured".to_string())?;

    let vault_root = validated_new_vault_root(&vault)?;
    let vault_meta = std::fs::metadata(&vault_root).map_err(|e| {
        format!(
            "configured vault is not accessible ({}): {}",
            vault_root.display(),
            e
        )
    })?;
    if !vault_meta.is_dir() {
        return Err(format!(
            "configured vault path is not a directory: {}",
            vault_root.display()
        ));
    }

    // Same fix as `switch_vault` (CodeRabbit review on PR #96): the
    // backup source path was hardcoded to `~/.brain/brain.db`. With
    // `CURATED_BRAIN_DIR` set, the backup silently captured the wrong
    // database. Route through the canonical resolver.
    let src = retrieval::resolve_brain_paths().db_path;
    if !src.exists() {
        return Err("no database to back up".to_string());
    }

    let dest_dir = vault_root.join(".brain");
    std::fs::create_dir_all(&dest_dir).map_err(|e| e.to_string())?;
    let dest = dest_dir.join("brain.db.bak");
    let _ = std::fs::remove_file(&dest);

    let guard = db_state.0.lock().unwrap();
    guard
        .0
        .backup(rusqlite::DatabaseName::Main, &dest, None)
        .map_err(|e| e.to_string())?;

    Ok(dest.to_string_lossy().into_owned())
}

/// Remove SQLite `-wal` / `-shm` siblings so a restored or reused `brain.db` is not paired with stale journals.
fn remove_sqlite_sidecars(db_path: &Path) {
    let base = db_path.to_string_lossy();
    let _ = std::fs::remove_file(format!("{base}-wal"));
    let _ = std::fs::remove_file(format!("{base}-shm"));
}

fn validated_new_vault_root(path: &str) -> Result<PathBuf, String> {
    if path.is_empty() {
        return Err("vault path is empty".to_string());
    }
    let p = Path::new(path);
    if !p.is_absolute() {
        return Err("vault path must be an absolute directory path".to_string());
    }
    if p.components().any(|c| c == Component::ParentDir) {
        return Err("vault path must not contain '..'".to_string());
    }
    Ok(p.to_path_buf())
}

/// Returns true when `new_root` is the same directory as the configured vault (symlinks resolved).
fn switching_to_same_vault_as_configured(current: &str, new_root: &Path) -> bool {
    match (Path::new(current).canonicalize(), new_root.canonicalize()) {
        (Ok(cur), Ok(next)) => cur == next,
        _ => false,
    }
}

fn canonical_vault_from_config(vault_state: &VaultConfigState) -> Result<PathBuf, String> {
    let configured_vault = vault_state
        .0
        .lock()
        .unwrap()
        .get_vault_path()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "no vault configured".to_string())?;
    let vault_root = PathBuf::from(&configured_vault);
    vault_root
        .canonicalize()
        .map_err(|e| format!("failed to canonicalize configured vault: {}", e))
}

fn start_file_watcher_inner(
    app: &AppHandle,
    pipeline: State<'_, PipelineHolder>,
    _db_state: State<'_, DbState>,
    vault_state: State<'_, VaultConfigState>,
    watcher_started: State<'_, WatcherStarted>,
    heal_scheduler: State<'_, HealScheduler>,
    status_state: State<'_, WikiStatusState>,
) -> Result<(), String> {
    let target_canonical = canonical_vault_from_config(&vault_state)?;

    let old_handle_to_stop = {
        let mut watcher_guard = watcher_started.0.lock().unwrap();
        if let Some((prev_path, handle)) = watcher_guard.take() {
            if prev_path == target_canonical {
                *watcher_guard = Some((prev_path, handle));
                return Ok(());
            }
            Some(handle)
        } else {
            None
        }
    };

    if let Some(h) = old_handle_to_stop {
        h.stop();
    }

    let old_heal_scheduler = {
        let mut scheduler_guard = heal_scheduler.0.lock().unwrap();
        scheduler_guard.take()
    };
    if let Some((sender, handle)) = old_heal_scheduler {
        drop(sender);
        let _ = handle.join();
    }

    let status_rx = {
        let mut guard = pipeline.0.lock().unwrap();
        let tuple = guard
            .as_mut()
            .ok_or_else(|| "pipeline not running".to_string())?;
        tuple.status_rx.take()
    };

    if let Some(status_rx) = status_rx {
        let app_handle = app.clone();
        std::thread::spawn(move || {
            for event in status_rx {
                let PipelineStatusEvent::PendingCount(count) = event;
                update_wiki_status_from_app(&app_handle, |flags| {
                    // The supervisor's on_health callback owns the
                    // Stalled/Degraded latches; this listener only refreshes
                    // the worker-derived stage/subject from the pending count
                    // and avoids overwriting health while latched
                    // (CodeRabbit review PRRT_kwDOSVmXas6d28dc).
                    if !matches!(
                        flags.ingest.health,
                        pipeline::watchdog::PipelineHealth::Stalled
                            | pipeline::watchdog::PipelineHealth::Degraded
                    ) {
                        flags.ingest.health = if count > 0 {
                            pipeline::watchdog::PipelineHealth::Working
                        } else {
                            pipeline::watchdog::PipelineHealth::Idle
                        };
                    }
                });
            }
        });
    }

    {
        let (tx, heartbeat) = {
            let guard = pipeline.0.lock().unwrap();
            match guard.as_ref() {
                Some(t) => (t.tx.clone(), t.heartbeat.clone()),
                None => return Err("pipeline not running".to_string()),
            }
        };
        let brain_paths = crate::retrieval::resolve_brain_paths();
        let report =
            crate::config::BrainConfig::load_lenient(&brain_paths).map_err(|e| e.to_string())?;
        let profile = report.config.embed_profile.clone().unwrap_or_default();
        let gen_timeout_secs = 600; // matches librarian/synthesis.rs default

        let app_handle = app.clone();
        let stop = Arc::new(AtomicBool::new(false));
        // on_replace_worker: actually rebuild the channel and spawn a fresh
        // worker, then publish the new sender so every producer — and the
        // supervisor's own sweep — stops writing into the queue the
        // abandoned worker still owns
        // (CodeRabbit review PRRT_kwDOSVmXas6d28dw / PRRT_kwDOSVmXas6d3ZYn).
        let replace_app = app.clone();
        let join = pipeline::watchdog::spawn_supervisor(pipeline::watchdog::SupervisorConfig {
            db_path: brain_paths.db_path.clone(),
            heartbeat: heartbeat.clone(),
            tx,
            profile,
            gen_timeout_secs,
            stop: stop.clone(),
            on_health: Box::new(move |update| {
                update_wiki_status_from_app(&app_handle, |flags| {
                    flags.ingest.health = update.health;
                    flags.ingest.stage = update.stage.map(|s| s.as_str().to_string());
                    flags.ingest.subject = update.subject;
                });
            }),
            on_replace_worker: Box::new(move || {
                let holder = replace_app.try_state::<PipelineHolder>()?;
                let mut guard = match holder.0.lock() {
                    Ok(g) => g,
                    // A panicking producer poisoned the holder; recovering the
                    // sender from it would be worse than parking in degraded.
                    Err(e) => {
                        eprintln!("[watchdog] pipeline holder poisoned: {e}");
                        return None;
                    }
                };
                let handle = guard.as_mut()?;
                let new_tx = handle.respawn_worker();
                eprintln!("[watchdog] replacement pipeline worker spawned");
                Some(new_tx)
            }),
        });
        // Stash the supervisor's stop flag + join handle so the next
        // `switch_vault` (or another watcher start) can signal and join
        // before spawning a new supervisor
        // (CodeRabbit review PRRT_kwDOSVmXas6d28dj).
        if let Some(sup_state) = app.try_state::<WatchdogSupervisor>() {
            let prev = sup_state
                .0
                .lock()
                .unwrap()
                .replace(WatchdogSupervisorHandle {
                    stop: stop.clone(),
                    join,
                });
            if let Some(prev) = prev {
                prev.stop.store(true, std::sync::atomic::Ordering::SeqCst);
                let _ = prev.join.join();
            }
        }
    }

    let raw_docs = target_canonical.join(crate::vault::safe_path::IMMUTABLE_DIR);
    let documents_root = std::fs::canonicalize(&raw_docs).unwrap_or(raw_docs.clone());

    {
        // Open a fresh WAL-mode connection for the reconcile pass instead of
        // holding `db_state.0.lock()` across the (potentially slow) sha256
        // hashing. This keeps the UI's long-lived connection free to read
        // while the reconcile writes. See spec §11 mutex trap.
        //
        // If the connection open fails (e.g. fresh install with no brain.db
        // yet), log and skip the reconcile pass — but DO continue to spawn the
        // watcher below, which will catch new events as they arrive.
        //
        // Use the shared `retrieval::resolve_brain_paths()` so desktop and
        // `ct watch` agree on the brain path (see CodeRabbit review on
        // PR #96 — previously this constructed the path inline via
        // `dirs::home_dir().join(".brain")` which silently diverged from
        // the canonical resolver when `CURATED_BRAIN_DIR` was set).
        let brain_db_path = retrieval::resolve_brain_paths().db_path;
        // Open a fresh WAL-mode connection for the reconcile pass (spec §11
        // mutex trap — keeps the UI's long-lived `DbState` connection free
        // to read while we hash and upsert).
        //
        // If the open fails (e.g. fresh install with no `brain.db` yet), log
        // and skip the reconcile — but DO continue so the watcher spawn below
        // still runs and catches new events as they arrive.
        let mut conn_opt: Option<rusqlite::Connection> =
            match rusqlite::Connection::open(&brain_db_path) {
                Ok(c) => {
                    // Busy-timeout pragma (CodeRabbit review on PR #96):
                    // the reconcile writer participates in WAL mode,
                    // but `ct watch`'s per-event connection also opens
                    // RW against the same DB. Without a timeout,
                    // contention with a checkpoint or another writer
                    // fails instantly with SQLITE_BUSY. 5s matches
                    // `tauri_app_lib::db::AppDb`'s default.
                    if let Err(e) = c.busy_timeout(std::time::Duration::from_secs(5)) {
                        eprintln!("[reconcile] failed to set busy_timeout: {e}");
                        // Non-fatal: continue without the pragma.
                    }
                    Some(c)
                }
                Err(e) => {
                    eprintln!(
                    "[reconcile] skipping reconcile pass — failed to open {brain_db_path:?}: {e}"
                );
                    None
                }
            };

        // Purge documents rows whose backing file no longer exists on disk.
        // Each per-row query failure is logged and skipped — we do NOT
        // abort the reconcile pass on a single bad row, otherwise a
        // single corrupt DB row would silently disable vault reconcile
        // (CodeRabbit review on PR #96).
        if let Some(conn) = conn_opt.as_mut() {
            let db_paths: Vec<String> = match conn
                .prepare("SELECT path FROM documents WHERE tier = 'user_doc'")
                .and_then(|mut stmt| {
                    let mut rows = stmt.query([])?;
                    let mut v = Vec::new();
                    while let Some(row) = rows.next()? {
                        v.push(row.get::<_, String>(0)?);
                    }
                    Ok::<_, rusqlite::Error>(v)
                }) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!(
                        "[reconcile] skipping path purge — SELECT path FROM documents failed: {e}"
                    );
                    Vec::new()
                }
            };
            for path in db_paths {
                if !std::path::Path::new(&path).exists() {
                    eprintln!("[reconcile] purging deleted file from index: {}", path);
                    if let Err(e) = enqueue_vault_event(
                        conn,
                        notify::EventKind::Remove(notify::event::RemoveKind::Any),
                        std::path::Path::new(&path),
                    ) {
                        eprintln!("[reconcile] enqueue_vault_event (Remove) failed: {e}");
                    }
                }
            }

            if raw_docs.exists() {
                for entry in walkdir::WalkDir::new(&raw_docs)
                    .min_depth(1)
                    .into_iter()
                    .filter_map(|e| e.ok())
                    .filter(|e| e.file_type().is_file())
                {
                    let ext = entry
                        .path()
                        .extension()
                        .and_then(|s| s.to_str())
                        .unwrap_or("");
                    if should_ingest_extension(ext) {
                        let normalized = std::fs::canonicalize(entry.path())
                            .unwrap_or_else(|_| entry.path().to_path_buf())
                            .to_string_lossy()
                            .into_owned();
                        update_wiki_status(app, &status_state, |flags| {
                            flags.ingest.health = pipeline::watchdog::PipelineHealth::Working;
                        });
                        if let Err(e) = enqueue_vault_event(
                            conn,
                            notify::EventKind::Create(notify::event::CreateKind::Any),
                            std::path::Path::new(&normalized),
                        ) {
                            eprintln!("[reconcile] enqueue_vault_event (Create) failed: {e}");
                        }
                    }
                }
            }
        }
        // `conn_opt` drops here, releasing the WAL writer slot.
    }

    // Acquire the vault lock. This function is called from TWO outer
    // entrypoints:
    //   1. `start_file_watcher` (Tauri command) — no upstream lock.
    //   2. `switch_vault` — acquires the lock at its own line ~1077
    //      BEFORE the teardown block (CodeRabbit review on PR #96
    //      pass 3, comment #11). That outer acquire is the FAIL-FAST
    //      gate that protects the watcher from being torn down while a
    //      headless `ct watch` holds the lock.
    //
    // The double-acquire here is intentional and harmless: `flock` is
    // per-file-descriptor on Linux (and per-handle on Windows), so the
    // same process acquiring the same lock file from two `OpenOptions`
    // handles produces two independent locks. Both release on drop. The
    // `WatcherHandle` returned below stores THIS lock (the inner one),
    // so its drop releases the inner lock; the outer lock in `switch_vault`
    // drops when that function returns.
    //
    // ...[truncated]

    let (heal_tx, heal_thread) = spawn_heal_scheduler(app.clone());
    let mut scheduler_guard = heal_scheduler.0.lock().unwrap();
    *scheduler_guard = Some((heal_tx.clone(), heal_thread));

    // Resolve the brain DB path for the per-event ephemeral WAL-mode
    // connection (line 910 below). The vault lock was already acquired
    // at the top of this function — see the double-acquire rationale in
    // the block comment above. (CodeRabbit review on PR #96 pass 3.)
    let brain_paths = retrieval::resolve_brain_paths();
    let brain_db_path = brain_paths.db_path.clone();

    // Acquire the vault lock for the WatcherHandle. The outer lock in
    // `switch_vault` is the FAIL-FAST gate that prevents tearing down
    // the old watcher when a headless `ct watch` holds the lock (see
    // CodeRabbit review on PR #96 pass 3, comment #11). This inner
    // lock is owned by the returned `WatcherHandle` and released when
    // `WatcherHandle::stop` runs. Same-process double-flock is harmless
    // (flock is per-fd); see the detailed rationale in the block comment
    // above the `start_file_watcher_inner` function.
    let vault_lock = VaultLock::acquire(&brain_paths.brain_dir).map_err(|e| {
        format!(
            "failed to acquire vault lock for {:?}: {e}",
            brain_paths.brain_dir
        )
    })?;

    let app = app.clone();
    let vault_for_watcher = target_canonical.clone();
    // Brain DB path for the per-event ephemeral WAL-mode connection.
    // Spec §11 mutex trap: do NOT touch `db_state.0` here — holding the lock
    // during the (potentially slow) sha256 hash freezes the UI.
    let handle = spawn_vault_watcher(vault_for_watcher, move |event| {
        let _ = app.emit("vault-event", &event);
        let path_str = match &event {
            VaultEvent::Added(p) | VaultEvent::Modified(p) | VaultEvent::Deleted(p) => p,
        };
        let canonical =
            std::fs::canonicalize(path_str).unwrap_or_else(|_| std::path::PathBuf::from(path_str));
        if !canonical.starts_with(&documents_root) {
            return;
        }
        let normalized = canonical.to_string_lossy().into_owned();
        // Open a fresh WAL-mode connection per event. WAL mode allows
        // concurrent reader + writer, so the UI's long-lived connection in
        // `DbState` keeps reading while we hash and upsert here.
        let conn_result = rusqlite::Connection::open(&brain_db_path);
        let mut conn = match conn_result {
            Ok(c) => c,
            Err(e) => {
                eprintln!(
                    "[watch] failed to open brain.db at {brain_db_path:?}: {e}; event dropped: {normalized}"
                );
                return;
            }
        };
        // Busy-timeout pragma (CodeRabbit review on PR #96):
        // without this, transient contention with a WAL checkpoint
        // or another writer instantly fails with SQLITE_BUSY. 5s
        // matches `tauri_app_lib::db::AppDb`'s default and the
        // `ct watch` side (`tools/src/write.rs::open_rw`).
        if let Err(e) = conn.busy_timeout(std::time::Duration::from_secs(5)) {
            eprintln!("[watch] failed to set busy_timeout: {e}");
            // Non-fatal: continue.
        }
        let event_kind = match &event {
            VaultEvent::Added(_) => {
                update_wiki_status_from_app(&app, |flags| {
                    flags.ingest.health =
                        pipeline::watchdog::PipelineHealth::Working;
                });
                Ok(notify::EventKind::Create(notify::event::CreateKind::Any))
            }
            VaultEvent::Modified(_) => {
                update_wiki_status_from_app(&app, |flags| {
                    flags.ingest.health =
                        pipeline::watchdog::PipelineHealth::Working;
                });
                Ok(notify::EventKind::Modify(notify::event::ModifyKind::Any))
            }
            VaultEvent::Deleted(_) => {
                let _ = heal_tx.send(());
                Ok(notify::EventKind::Remove(notify::event::RemoveKind::Any))
            }
        };
        let event_kind = match event_kind {
            Ok(k) => k,
            Err(()) => return,
        };
        if let Err(e) =
            enqueue_vault_event(&mut conn, event_kind, std::path::Path::new(&normalized))
        {
            eprintln!("[watch] enqueue_vault_event failed for {normalized}: {e}");
        }
        // conn drops here, releasing the WAL writer slot.
    })
    .map_err(|e| e.to_string())?
    .with_lock(vault_lock);

    let mut watcher_guard = watcher_started.0.lock().unwrap();
    let still_canonical = match canonical_vault_from_config(&vault_state) {
        Ok(p) => p,
        Err(e) => {
            drop(watcher_guard);
            handle.stop();
            return Err(e);
        }
    };
    if still_canonical != target_canonical {
        drop(watcher_guard);
        handle.stop();
        return Ok(());
    }

    if let Some((_p, old)) = watcher_guard.take() {
        old.stop();
    }
    *watcher_guard = Some((still_canonical, handle));
    Ok(())
}

/// Best-effort restore of DB handle, pipeline, file watcher, and outbox worker after a failed `switch_vault`.
/// Returns whether `db_state` was successfully reopened on `db_path` (so temp stub files are safe to delete).
#[allow(clippy::too_many_arguments)]
fn recover_after_failed_switch_vault(
    app: &AppHandle,
    db_path: &Path,
    db_state: State<'_, DbState>,
    pipeline: State<'_, PipelineHolder>,
    vault_state: State<'_, VaultConfigState>,
    watcher_started: State<'_, WatcherStarted>,
    heal_scheduler: State<'_, HealScheduler>,
    status_state: State<'_, WikiStatusState>,
    _outbox_state: State<'_, OutboxWorkerState>,
) -> bool {
    let config_path = retrieval::resolve_brain_paths().config_path;
    let reopened = (|| -> Result<(), String> {
        let mut guard = db_state
            .0
            .lock()
            .map_err(|_| "db mutex poisoned".to_string())?;
        *guard = AppDb::open_with_config(db_path, &config_path).map_err(|e| e.to_string())?;
        Ok(())
    })();
    if let Err(e) = &reopened {
        eprintln!("[switch_vault] recovery: failed to reopen {db_path:?}: {e}");
        return false;
    }
    if let Ok(mut g) = pipeline.0.lock() {
        if g.is_none() {
            let vault_root = vault_state
                .0
                .lock()
                .ok()
                .and_then(|vc| vc.get_vault_path().ok().flatten())
                .map(|s| {
                    let p = PathBuf::from(s);
                    p.canonicalize().unwrap_or(p)
                });
            *g = Some(start_pipeline(db_path.to_path_buf(), vault_root));
        }
    }
    if let Err(e) = start_file_watcher_inner(
        app,
        pipeline,
        db_state.clone(),
        vault_state.clone(),
        watcher_started.clone(),
        heal_scheduler.clone(),
        status_state.clone(),
    ) {
        eprintln!("[switch_vault] recovery: failed to restart file watcher: {e}");
    }
    // NOTE: worker spawn moved to switch_vault recovery branch to avoid
    // duplicate workers after failed switch.
    true
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn switch_vault(
    new_path: String,
    restore_backup: bool,
    app: AppHandle,
    db_state: State<'_, DbState>,
    vault_state: State<'_, VaultConfigState>,
    pipeline: State<'_, PipelineHolder>,
    watcher_started: State<'_, WatcherStarted>,
    heal_scheduler: State<'_, HealScheduler>,
    status_state: State<'_, WikiStatusState>,
    outbox_state: State<'_, OutboxWorkerState>,
) -> Result<(), String> {
    // CodeRabbit review on PR #96: the inner `start_file_watcher_inner`
    // was already migrated to `retrieval::resolve_brain_paths()` for the
    // watcher brain dir / brain db path, but the outer `switch_vault`
    // scope was still using the hardcoded `~/.brain/brain.db`. With
    // `CURATED_BRAIN_DIR` set, the outbox worker + sidecar recovery +
    // db-path-cleanup were silently operating on the WRONG brain.db.
    // Route through the canonical resolver so the two paths agree.
    let db_path = retrieval::resolve_brain_paths().db_path;
    let config_path = retrieval::resolve_brain_paths().config_path;

    let new_root = validated_new_vault_root(&new_path)?;

    if let Ok(Some(ref current)) = vault_state.0.lock().unwrap().get_vault_path() {
        if switching_to_same_vault_as_configured(current, new_root.as_path()) {
            return Ok(());
        }
    }

    // Acquire the vault lock BEFORE tearing anything down. If a headless
    // `ct watch` is currently holding the lock, we want to fail FAST
    // with the old watcher + pipeline + outbox still running — the previous
    // ordering (teardown, then lock acquire at line 884 inside
    // `start_file_watcher_inner`) silently disabled desktop file watching
    // on lock-conflict and required an app restart to recover.
    // CodeRabbit review on PR #96 (review pass 3, comment #11).
    let brain_dir_for_lock = retrieval::resolve_brain_paths().brain_dir;
    let _vault_lock = VaultLock::acquire(&brain_dir_for_lock).map_err(|e| {
        format!(
            "failed to acquire vault lock for {brain_dir_for_lock:?}: {e} \
             — a headless `ct watch` may be holding this vault; \
             stop it and retry"
        )
    })?;

    // v2 layout: migrate a v1 vault (documents/ → immutable-source-files/)
    // BEFORE creating subdirs, otherwise both folders would exist and the
    // startup migration would block with BothFoldersExist.
    crate::vault::config::migrate_vault(new_root.as_path()).map_err(|e| e.to_string())?;
    for subdir in &[crate::vault::IMMUTABLE_DIR, crate::vault::WIKI_DIR] {
        std::fs::create_dir_all(new_root.join(subdir)).map_err(|e| e.to_string())?;
    }
    std::fs::create_dir_all(new_root.join(AGENTS_DEPOSIT_DIR)).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(new_root.join(".brain").join("converted"))
        .map_err(|e| e.to_string())?;

    {
        let mut g = watcher_started.0.lock().unwrap();
        if let Some((_p, h)) = g.take() {
            h.stop();
        }
    }

    {
        let mut g = pipeline.0.lock().unwrap();
        if let Some(handle) = g.take() {
            let pipeline::PipelineHandle {
                tx,
                join,
                heartbeat,
                ..
            } = handle;
            drop(tx);
            // Supersede first, so a worker that later wakes exits without
            // racing the replacement (spec §4.1), then give it a bounded
            // window rather than blocking switch_vault forever (spec §6).
            heartbeat.bump_epoch();
            let finished = pipeline::watchdog::join_with_timeout(join, Duration::from_secs(10));
            if !finished {
                eprintln!("[switch_vault] pipeline worker did not exit within 10s; abandoning it");
            }
        }
    }

    // Stop outbox worker before WAL cleanup and DB file operations; its dedicated
    // SQLite connection would otherwise keep polling a stale/replaced file.
    // Do not notify the frontend here, because vault switching internally restarts
    // the worker and emitting a stop event would race with the ongoing database swap.
    let (maybe_outbox_config, maybe_handle) = {
        let mut g = outbox_state.0.lock().unwrap();
        let maybe_handle = g.take();
        let maybe_config = maybe_handle.as_ref().map(|handle| handle.config.clone());
        (maybe_config, maybe_handle)
    };
    if let Some(handle) = maybe_handle {
        handle.stop().await;
    }

    let stub_path = release_global_db_lock(&db_state)?;
    let mut pending_config_align_to: Option<String> = None;

    let switch_result = (|| -> Result<(), String> {
        let backup_path = new_root.join(".brain").join("brain.db.bak");
        let has_backup = backup_path.exists();

        remove_sqlite_sidecars(&db_path);

        if restore_backup && has_backup {
            std::fs::copy(&backup_path, &db_path).map_err(|e| e.to_string())?;
            pending_config_align_to = Some(new_path.clone());
        } else {
            let mut conn = rusqlite::Connection::open(&db_path).map_err(|e| e.to_string())?;
            db::clear_vault_tables(&mut conn).map_err(|e| e.to_string())?;
            pending_config_align_to = Some(new_path.clone());
        }

        {
            let mut guard = db_state.0.lock().unwrap();
            *guard = AppDb::open_with_config(&db_path, &config_path).map_err(|e| e.to_string())?;
        }

        {
            let mut g = pipeline.0.lock().unwrap();
            let canon_root = new_root.canonicalize().unwrap_or_else(|_| new_root.clone());
            *g = Some(start_pipeline(db_path.clone(), Some(canon_root)));
        }

        vault_state
            .0
            .lock()
            .unwrap()
            .set_vault_path(&new_path)
            .map_err(|e| e.to_string())?;

        pending_config_align_to = None;

        let _ = app.emit("vault-switched", &new_path);

        Ok(())
    })();

    let mut recovery_reopened_db = false;
    if switch_result.is_err() {
        if let Some(ref p) = pending_config_align_to {
            if let Err(e) = vault_state.0.lock().unwrap().set_vault_path(p) {
                eprintln!(
                    "[switch_vault] failed to align vault config with on-disk DB after error: {e}"
                );
            }
        }
        eprintln!(
            "[switch_vault] switch failed ({:?}); attempting recovery for configured vault",
            switch_result.as_ref().err()
        );
        recovery_reopened_db = recover_after_failed_switch_vault(
            &app,
            &db_path,
            db_state.clone(),
            pipeline.clone(),
            vault_state.clone(),
            watcher_started.clone(),
            heal_scheduler.clone(),
            status_state.clone(),
            outbox_state.clone(),
        );
    }

    // Only delete stub files after `db_state` no longer holds an open connection to `stub_path`.
    if switch_result.is_ok() || recovery_reopened_db {
        cleanup_temp_stub_db(&stub_path);
    } else {
        eprintln!(
            "[switch_vault] skipping temp stub cleanup; real DB reopen failed and db_state may still reference {:?}",
            stub_path
        );
    }

    if switch_result.is_ok() {
        spawn_outbox_worker_if_configured(
            &app,
            outbox_state.clone(),
            db_path.clone(),
            maybe_outbox_config.clone(),
        )
        .await;
        if let Err(e) = start_file_watcher_inner(
            &app,
            pipeline.clone(),
            db_state.clone(),
            vault_state.clone(),
            watcher_started.clone(),
            heal_scheduler.clone(),
            status_state.clone(),
        ) {
            eprintln!("[switch_vault] failed to restart file watcher after successful switch: {e}");
        }
    } else if recovery_reopened_db {
        spawn_outbox_worker_if_configured(
            &app,
            outbox_state.clone(),
            db_path.clone(),
            maybe_outbox_config,
        )
        .await;
    } else if maybe_outbox_config.is_some() {
        // Both switch and recovery failed; the worker was stopped and will not be
        // restarted. Notify the frontend so it disables outbox writes.
        let _ = app.emit("outbox-worker-stopped", ());
    }

    switch_result
}

#[tauri::command]
fn check_vault_backup(path: String) -> Result<bool, String> {
    let root = validated_new_vault_root(&path)?;
    Ok(root.join(".brain").join("brain.db.bak").exists())
}

#[tauri::command]
fn reveal_vault(vault_state: State<VaultConfigState>) -> Result<(), String> {
    let vault = vault_state
        .0
        .lock()
        .unwrap()
        .get_vault_path()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "no vault configured".to_string())?;

    #[cfg(target_os = "macos")]
    std::process::Command::new("open")
        .arg(&vault)
        .spawn()
        .map_err(|e| e.to_string())?;

    #[cfg(target_os = "linux")]
    std::process::Command::new("xdg-open")
        .arg(&vault)
        .spawn()
        .map_err(|e| e.to_string())?;

    #[cfg(target_os = "windows")]
    std::process::Command::new("explorer")
        .arg(&vault)
        .spawn()
        .map_err(|e| e.to_string())?;

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = vault;
        return Err("reveal_vault is not supported on this platform".to_string());
    }

    Ok(())
}

// ── Watcher + pipeline ────────────────────────────────────────────────────────

#[tauri::command]
fn start_file_watcher(
    app: AppHandle,
    pipeline: State<PipelineHolder>,
    db_state: State<DbState>,
    vault_state: State<VaultConfigState>,
    watcher_started: State<WatcherStarted>,
    heal_scheduler: State<HealScheduler>,
    status_state: State<WikiStatusState>,
) -> Result<(), String> {
    start_file_watcher_inner(
        &app,
        pipeline.clone(),
        db_state.clone(),
        vault_state.clone(),
        watcher_started.clone(),
        heal_scheduler.clone(),
        status_state.clone(),
    )
}

// ── Indexing status ───────────────────────────────────────────────────────────

#[derive(serde::Serialize)]
pub struct IndexingStatus {
    pub indexed: i64,
    pub pending: i64,
}

#[tauri::command]
fn get_indexing_status(db_state: State<DbState>) -> Result<IndexingStatus, String> {
    let guard = db_state.0.lock().unwrap();
    let conn = &guard.0;
    let indexed = db::count_indexed_documents(conn).map_err(|e| e.to_string())?;
    let pending = db::count_pending_documents(conn).map_err(|e| e.to_string())?;
    Ok(IndexingStatus { indexed, pending })
}

/// Enqueue ingestion for every indexed user document. After chunk strategy (`ast_*`) or
/// embedding model changes, pass `force_rechunk: true` so work runs even when bytes are
/// unchanged (`Ingest` alone would no-op on matching hash).
#[tauri::command]
fn queue_full_reindex(
    force_rechunk: bool,
    pipeline: State<PipelineHolder>,
    db_state: State<DbState>,
) -> Result<usize, String> {
    // Read the work list under the lock, then release it. Holding `db_state`
    // across the whole enqueue loop serializes every other IPC handler behind
    // this command for the duration of a full-vault reindex, freezing the UI
    // (spec §11 mutex trap).
    let paths = {
        let guard = db_state.0.lock().unwrap();
        crate::db::list_indexed_user_doc_paths(&guard.0).map_err(|e| e.to_string())?
    };
    let tx = {
        let pipeline_guard = pipeline.0.lock().unwrap();
        pipeline_guard
            .as_ref()
            .ok_or_else(|| "pipeline not running".to_string())?
            .tx
            .clone()
    };
    let mut queued = 0usize;
    let mut deferred: Vec<String> = Vec::new();
    let total = paths.len();
    for path in paths {
        if !std::path::Path::new(&path).exists() {
            eprintln!("[queue_full_reindex] skip missing file: {path}");
            continue;
        }
        let job = if force_rechunk {
            PipelineJob::rechunk(&path)
        } else {
            PipelineJob::ingest(&path)
        };
        // try_send, not send: a blocking send here freezes the Tauri IPC
        // thread when the channel fills (spec §6). These rows were selected
        // from `list_indexed_user_doc_paths` (status='indexed'), so the §5
        // pending sweep cannot rescue them — stage them below so the sweep
        // can (CodeRabbit review PRRT_kwDOSVmXas6d28dn).
        match tx.try_send(job) {
            Ok(()) => queued += 1,
            Err(std::sync::mpsc::TrySendError::Full(_)) => deferred.push(path),
            Err(std::sync::mpsc::TrySendError::Disconnected(e)) => {
                return Err(format!("pipeline channel closed: {e:?}"));
            }
        }
    }

    if !deferred.is_empty() {
        // Stage every deferred row in one short critical section. The status
        // must match the job that was dropped: only a forced rechunk becomes
        // 'pending_reindex' (which the sweep re-enqueues with force=true); a
        // plain ingest becomes 'pending' so the sweep does not silently
        // upgrade it into a rechunk.
        let staged_status = if force_rechunk {
            crate::pipeline::watchdog::sweep::STATUS_PENDING_REINDEX
        } else {
            "pending"
        };
        let guard = db_state.0.lock().unwrap();
        for path in &deferred {
            if let Err(e) = guard.0.execute(
                "UPDATE documents SET status = ?2
                   WHERE path = ?1 AND status = 'indexed'",
                rusqlite::params![path, staged_status],
            ) {
                eprintln!("[queue_full_reindex] failed to stage {path} as {staged_status}: {e}");
            }
        }
        eprintln!(
            "[queue_full_reindex] queued {queued} of {total}; {} deferred (status={staged_status} for sweep pickup)",
            deferred.len()
        );
    }
    Ok(queued)
}

// ── Maintenance commands ──────────────────────────────────────────────────────

fn heal_lost_librarian_inferred(
    conn: &rusqlite::Connection,
    _vault_root: &Path,
) -> Result<usize, String> {
    // Source-ref consumer is now the shared helper
    // (`source_ref_is_still_grounded`) which handles both legacy path and
    // JSON shapes, so the on-disk `vault_root` no longer participates in
    // the existence check. Kept in the signature so existing call sites
    // continue to compile unchanged.
    let entries: Vec<(i64, String, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT rowid, source_ref, id FROM llm_wiki_entries
                     WHERE deleted_at IS NULL
                       AND source_ref IS NOT NULL
                       AND source_type = 'librarian_inferred'",
            )
            .map_err(|e| e.to_string())?;
        let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
        let mut v = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            v.push((
                row.get::<_, i64>(0).map_err(|e| e.to_string())?,
                row.get::<_, String>(1).map_err(|e| e.to_string())?,
                row.get::<_, String>(2).map_err(|e| e.to_string())?,
            ));
        }
        v
    };

    let mut updated = 0;
    for (rowid, source_ref, entry_id) in entries {
        if !crate::db::commit::source_ref_is_still_grounded(conn, &source_ref) {
            // Soft-delete and edge purge must be atomic (spec §2): a crash
            // between them would strand the edges. These sites had no
            // transaction before this change.
            let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
            updated += tx
                .execute(
                    "UPDATE llm_wiki_entries SET deleted_at = ?1 WHERE rowid = ?2",
                    rusqlite::params![crate::db::commit::ms_now(), rowid],
                )
                .map_err(|e| e.to_string())?;
            crate::db::edge_purge::purge_edges_for_entry(&tx, &entry_id)
                .map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
        }
    }
    Ok(updated)
}

/// Hard-delete `librarian_inferred` entries that have been soft-deleted for
/// more than 7 days.
///
/// `now_ms` is unix **milliseconds**, matching `llm_wiki_entries.deleted_at`,
/// which every writer stores in milliseconds (`ms_now()`, `db::commit`). This
/// function previously took seconds and compared them against millisecond
/// stamps, so it never deleted anything; see design spec §2.1.
fn prune_old_librarian_inferred(conn: &rusqlite::Connection, now_ms: i64) -> Result<usize, String> {
    const SEVEN_DAYS_MS: i64 = 7 * 86_400 * 1000;
    let cutoff = now_ms - SEVEN_DAYS_MS;

    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;

    // Collect the doomed ids first: the DELETE is predicate-driven, but the edge
    // purge and the outbox push both need concrete ids (and the outbox push
    // also needs entity_id). All three must commit together (spec §2).
    //
    // Spec override (2026-09-01): the seconds→ms fix (commit `2333479`) made
    // this hard-delete actually fire for the first time. Without an
    // `OutboxOperation::Delete` push, replicas created from any prune that
    // runs after that fix will silently retain pruned rows. The spec had this
    // as a non-goal; the human partner upgraded it to in-scope once the ms/s
    // fix was live. See `.superpowers/sdd/.../remediation/task-R7-brief.md`.
    let doomed: Vec<(String, String)> = {
        let mut stmt = tx
            .prepare(
                "SELECT id, entity_id FROM llm_wiki_entries
                     WHERE source_type = 'librarian_inferred'
                       AND deleted_at IS NOT NULL
                       AND deleted_at < ?1",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([cutoff], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| e.to_string())?;
        rows.collect::<rusqlite::Result<Vec<(String, String)>>>()
            .map_err(|e| e.to_string())?
    };

    let doomed_ids: Vec<String> = doomed.iter().map(|(id, _)| id.clone()).collect();
    // HARD delete, so the hard-delete purge. `purge_edges_for_entries` keeps an
    // edge whose partner is still alive — correct at the soft-delete sites,
    // where the dead row can be recovered — but here the id disappears from
    // every endpoint table, so such an edge would reference nothing and no
    // later cascade could ever find it to clean up (spec §2, "Soft delete vs
    // hard delete").
    crate::db::edge_purge::purge_edges_for_hard_deleted(&tx, &doomed_ids)
        .map_err(|e| e.to_string())?;

    // Push one OutboxOperation::Delete row per doomed entry so replicas
    // converge on the prune. Same shape as `commit_fact_archive` /
    // `archive_fact`. The entity_id is sourced from the doomed row itself
    // (not assumed uniform) because pruning may span multiple entities if a
    // future migration ever allows that; today all librarian_inferred rows
    // share an entity in practice, but the outbox is keyed on entity and
    // pushing the wrong one would mis-attribute the delete.
    for (id, entity_id) in &doomed {
        crate::db::commit::push_entries_outbox(
            &tx,
            entity_id,
            id,
            crate::db::outbox_format::OutboxOperation::Delete,
            serde_json::json!({ "id": id }),
            now_ms,
        )
        .map_err(|e| e.to_string())?;
    }

    let deleted = tx
        .execute(
            "DELETE FROM llm_wiki_entries
                 WHERE source_type = 'librarian_inferred'
                   AND deleted_at IS NOT NULL
                   AND deleted_at < ?1",
            [cutoff],
        )
        .map_err(|e| e.to_string())?;

    tx.commit().map_err(|e| e.to_string())?;
    Ok(deleted)
}

/// Run a bounded null-embedding sweep against the app's live DB connection.
///
/// The sweep is the recovery path for entries whose write-time embed did not
/// run or failed (Part B's `precompute_entry_embeddings` returns an empty map
/// on provider error). It is triggered from two places (design spec §3):
/// once at startup and from `run_wiki_heal`. Its `embedding_blob IS NULL`
/// predicate makes a no-op run nearly free, so over-triggering is cheap and
/// under-triggering loses embeddings.
///
/// Errors from the embedding provider are absorbed by the sweep itself and
/// reported in `SweepReport::failed`; this returns `Err` only if the database
/// is unreachable.
pub(crate) fn run_embedding_sweep(db_state: &DbState) -> Result<embed_sweep::SweepReport, String> {
    const MAX_BATCHES_PER_RUN: usize = 8; // <= 512 entries per trigger

    let paths = retrieval::resolve_brain_paths();
    let profile = retrieval::load_embed_profile(&paths.config_path).map_err(|e| e.to_string())?;

    // Critical: do NOT hold `db_state.0` across `embed_batch` (the blocking
    // network round-trip) — every Tauri DB command queues behind this mutex,
    // and a long embed would starve proposal approval, the heal path, and
    // user-initiated reads. The invariant is: lock only for the SELECT and
    // UPDATE halves of a batch, drop it before the network call.
    let mut report = embed_sweep::SweepReport::default();

    for _ in 0..MAX_BATCHES_PER_RUN {
        let pending: Vec<(String, String, String)> = {
            let guard = db_state.0.lock().unwrap();
            embed_sweep::pending_null_batch(&guard.0, embed_sweep::SWEEP_BATCH_SIZE)
                .map_err(|e| e.to_string())?
        };
        if pending.is_empty() {
            break;
        }

        let texts: Vec<String> = pending
            .iter()
            .map(|(_, title, body)| embed_sweep::embed_text_for_entry(title, body))
            .collect();

        // Lock is NOT held here — other Tauri DB commands may interleave.
        let vectors = match embed_batch(&profile, texts) {
            Ok(v) => v,
            Err(e) => {
                eprintln!(
                    "embed_sweep: batch of {} entries failed to embed: {e}",
                    pending.len()
                );
                report.failed += pending.len();
                // Stop rather than hammering a provider that is already failing.
                break;
            }
        };

        let apply_result = {
            let guard = db_state.0.lock().unwrap();
            embed_sweep::apply_embeddings(&guard.0, &pending, &vectors)
        };
        let filled = match apply_result {
            Ok(n) => n,
            // R6 zip-truncation guard: apply_embeddings refuses to pair a
            // mismatched batch. Count it as failed and stop the run — the
            // next batch could have different ids and we don't want to mix
            // them in either.
            Err(e) if e.to_string().contains("length mismatch") => {
                eprintln!(
                    "embed_sweep: provider returned a mismatched number of vectors; \
                     skipping batch to avoid mis-pairing"
                );
                report.failed += pending.len();
                break;
            }
            Err(e) => return Err(e.to_string()),
        };
        report.filled += filled;
    }

    report.remaining_null = {
        let guard = db_state.0.lock().unwrap();
        embed_sweep::count_null_entries(&guard.0).map_err(|e| e.to_string())?
    };

    Ok(report)
}

#[tauri::command]
async fn run_wiki_heal(
    app: AppHandle,
    db_state: State<'_, DbState>,
    vault_state: State<'_, VaultConfigState>,
    status_state: State<'_, WikiStatusState>,
) -> Result<(), String> {
    update_wiki_status(&app, &status_state, |flags| {
        flags.healing = true;
    });

    let result = (|| -> Result<(), String> {
        let vault = vault_state
            .0
            .lock()
            .unwrap()
            .get_vault_path()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "no vault configured".to_string())?;
        let vault_root = std::path::PathBuf::from(&vault);

        {
            let guard = db_state.0.lock().unwrap();
            heal_lost_librarian_inferred(&guard.0, &vault_root)?;
        } // guard dropped here — run_embedding_sweep takes its own lock

        if let Err(e) = run_embedding_sweep(&db_state) {
            eprintln!("wiki heal: embedding sweep skipped: {e}");
        }
        Ok(())
    })();

    update_wiki_status(&app, &status_state, |flags| {
        flags.healing = false;
    });

    result
}

#[tauri::command]
async fn run_wiki_prune(
    app: AppHandle,
    db_state: State<'_, DbState>,
    status_state: State<'_, WikiStatusState>,
) -> Result<(), String> {
    update_wiki_status(&app, &status_state, |flags| {
        flags.pruning = true;
    });

    let result = (|| -> Result<(), String> {
        let guard = db_state.0.lock().unwrap();
        let conn = &guard.0;
        prune_old_librarian_inferred(conn, crate::db::commit::ms_now())?;
        Ok(())
    })();

    update_wiki_status(&app, &status_state, |flags| {
        flags.pruning = false;
    });

    result
}

#[tauri::command]
async fn run_wiki_forget(
    app: AppHandle,
    db_state: State<'_, DbState>,
    vault_state: State<'_, VaultConfigState>,
    status_state: State<'_, WikiStatusState>,
    source_path: String,
) -> Result<(), String> {
    update_wiki_status(&app, &status_state, |flags| {
        flags.forgetting = true;
    });

    let result = (|| -> Result<(), String> {
        let root = vault_state
            .0
            .lock()
            .unwrap()
            .get_vault_path()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "no vault path configured".to_string())?;
        let vault_root = std::path::PathBuf::from(&root);
        let normalized_rel = normalize_path_argument_to_vault_relative(&source_path, &vault_root)?;

        let safe = crate::vault::safe_vault_path(
            &vault_root,
            &normalized_rel,
            crate::vault::WRITABLE_SUBDIRS,
            crate::vault::PathMode::MayCreate,
        )
        .map_err(|e| e.to_string())?;
        let safe_string = safe.to_string_lossy().into_owned();

        let mut guard = db_state.0.lock().unwrap();
        let conn = &mut guard.0;
        let removed = crate::db::wiki_forget::forget_entries_by_source_refs(
            conn,
            &[normalized_rel.clone(), safe_string.clone()],
        )
        .map_err(|e| e.to_string())?;
        eprintln!("run_wiki_forget: removed {removed} entries and their edges");
        Ok(())
    })();

    update_wiki_status(&app, &status_state, |flags| {
        flags.forgetting = false;
    });

    result
}

#[tauri::command]
async fn run_wiki_reembed(
    app: AppHandle,
    db_state: State<'_, DbState>,
    pipeline: State<'_, PipelineHolder>,
    status_state: State<'_, WikiStatusState>,
) -> Result<usize, String> {
    let tx = {
        let pipeline_guard = pipeline.0.lock().unwrap();
        match pipeline_guard.as_ref() {
            Some(p) => p.tx.clone(),
            None => {
                update_wiki_status(&app, &status_state, |flags| {
                    flags.ingest.health = pipeline::watchdog::PipelineHealth::Idle;
                });
                return Err("pipeline not running".to_string());
            }
        }
    };

    let result = (|| -> Result<usize, String> {
        // Read the work list under the lock, then release it. Holding
        // `db_state` across the whole enqueue loop serializes every other IPC
        // handler behind this command for the duration of a full-vault
        // re-embed, which freezes the UI (spec §11 mutex trap).
        let paths = {
            let guard = db_state.0.lock().unwrap();
            crate::db::list_indexed_user_doc_paths(&guard.0).map_err(|e| e.to_string())?
        };

        let mut queued = 0usize;
        let mut deferred: Vec<String> = Vec::new();
        let total = paths.len();
        for path in paths {
            if !std::path::Path::new(&path).exists() {
                continue;
            }
            // try_send, not send: a blocking send here freezes the Tauri IPC
            // thread when the channel fills (spec §6). The original rows
            // were 'indexed' so the §5 pending sweep can't rescue them.
            // Stage them as 'pending_reindex' so the sweep picks them up
            // as `PipelineJob::rechunk_for_reembed`
            // (CodeRabbit review PRRT_kwDOSVmXas6d28dn).
            match tx.try_send(PipelineJob::rechunk_for_reembed(path.clone())) {
                Ok(()) => queued += 1,
                Err(std::sync::mpsc::TrySendError::Full(_)) => deferred.push(path),
                Err(std::sync::mpsc::TrySendError::Disconnected(e)) => {
                    return Err(format!("pipeline channel closed: {e:?}"));
                }
            }
        }

        // Stage every deferred row in one short critical section rather than
        // re-taking the lock per path.
        if !deferred.is_empty() {
            let guard = db_state.0.lock().unwrap();
            for path in &deferred {
                if let Err(e) = guard.0.execute(
                    "UPDATE documents SET status = 'pending_reindex'
                       WHERE path = ?1 AND status = 'indexed'",
                    rusqlite::params![path],
                ) {
                    eprintln!("[run_wiki_reembed] failed to stage {path} as pending_reindex: {e}");
                }
            }
            eprintln!(
                "[run_wiki_reembed] queued {queued} of {total}; {} deferred (status=pending_reindex for sweep pickup)",
                deferred.len()
            );
        }
        Ok(queued)
    })();

    if result.is_err() {
        update_wiki_status(&app, &status_state, |flags| {
            flags.ingest.health = pipeline::watchdog::PipelineHealth::Idle;
        });
    }

    result
}

#[tauri::command]
async fn run_wiki_reindex(
    app: AppHandle,
    db_state: State<'_, DbState>,
    pipeline: State<'_, PipelineHolder>,
    status_state: State<'_, WikiStatusState>,
) -> Result<usize, String> {
    run_wiki_reembed(app, db_state, pipeline, status_state).await
}

// ── Wiki SQL bridge ───────────────────────────────────────────────────────────
// Implements SQLiteAdapter interface for @equationalapplications/react-llm-wiki

fn json_to_sql(v: &JsonVal) -> SqlVal {
    match v {
        JsonVal::Null => SqlVal::Null,
        JsonVal::Bool(b) => SqlVal::Integer(if *b { 1 } else { 0 }),
        JsonVal::Number(n) => {
            if let Some(i) = n.as_i64() {
                SqlVal::Integer(i)
            } else {
                SqlVal::Real(n.as_f64().unwrap_or(0.0))
            }
        }
        JsonVal::String(s) => SqlVal::Text(s.clone()),
        JsonVal::Array(a) => SqlVal::Blob(
            a.iter()
                .filter_map(|v| v.as_u64().map(|n| n as u8))
                .collect(),
        ),
        JsonVal::Object(_) => SqlVal::Null,
    }
}

fn row_to_json(
    row: &rusqlite::Row,
    col_names: &[String],
) -> rusqlite::Result<serde_json::Map<String, JsonVal>> {
    let mut map = serde_json::Map::new();
    for (i, name) in col_names.iter().enumerate() {
        let val = match row.get_ref(i)? {
            rusqlite::types::ValueRef::Null => JsonVal::Null,
            rusqlite::types::ValueRef::Integer(n) => JsonVal::Number(n.into()),
            rusqlite::types::ValueRef::Real(f) => serde_json::Number::from_f64(f)
                .map(JsonVal::Number)
                .unwrap_or(JsonVal::Null),
            rusqlite::types::ValueRef::Text(s) => {
                JsonVal::String(String::from_utf8_lossy(s).into())
            }
            rusqlite::types::ValueRef::Blob(b) => {
                JsonVal::Array(b.iter().map(|&n| JsonVal::Number(n.into())).collect())
            }
        };
        map.insert(name.clone(), val);
    }
    Ok(map)
}

fn query_rows(
    sql: &str,
    params: &[JsonVal],
    conn: &rusqlite::Connection,
) -> Result<Vec<serde_json::Map<String, JsonVal>>, String> {
    let sql_params: Vec<SqlVal> = params.iter().map(json_to_sql).collect();
    let refs: Vec<&dyn rusqlite::ToSql> = sql_params
        .iter()
        .map(|v| v as &dyn rusqlite::ToSql)
        .collect();
    let mut out = Vec::new();
    {
        let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
        let col_names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
        let mut rows = stmt.query(refs.as_slice()).map_err(|e| e.to_string())?;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            out.push(row_to_json(row, &col_names).map_err(|e| e.to_string())?);
        }
    }
    Ok(out)
}

#[tauri::command]
fn wiki_exec(sql: String, db_state: State<DbState>) -> Result<(), String> {
    db_state
        .0
        .lock()
        .unwrap()
        .0
        .execute_batch(&sql)
        .map_err(|e| e.to_string())
}

#[derive(serde::Serialize)]
struct WikiRunResult {
    changes: i64,
    last_insert_row_id: i64,
}

#[tauri::command]
fn wiki_run(
    sql: String,
    params: Vec<JsonVal>,
    db_state: State<DbState>,
) -> Result<WikiRunResult, String> {
    let guard = db_state.0.lock().unwrap();
    let conn = &guard.0;
    let sql_params: Vec<SqlVal> = params.iter().map(json_to_sql).collect();
    let refs: Vec<&dyn rusqlite::ToSql> = sql_params
        .iter()
        .map(|v| v as &dyn rusqlite::ToSql)
        .collect();
    let changes = conn
        .execute(&sql, refs.as_slice())
        .map_err(|e| e.to_string())?;
    Ok(WikiRunResult {
        changes: changes as i64,
        last_insert_row_id: conn.last_insert_rowid(),
    })
}

#[tauri::command]
fn wiki_get_all(
    sql: String,
    params: Vec<JsonVal>,
    db_state: State<DbState>,
) -> Result<Vec<serde_json::Map<String, JsonVal>>, String> {
    query_rows(&sql, &params, &db_state.0.lock().unwrap().0)
}

#[tauri::command]
fn wiki_get_first(
    sql: String,
    params: Vec<JsonVal>,
    db_state: State<DbState>,
) -> Result<Option<serde_json::Map<String, JsonVal>>, String> {
    Ok(query_rows(&sql, &params, &db_state.0.lock().unwrap().0)?
        .into_iter()
        .next())
}

// ── Embed text (for wiki llmProvider.embed) ───────────────────────────────────

#[tauri::command]
fn embed_text(text: String) -> Result<Vec<f32>, String> {
    crate::embedder::get_or_init_local_embedder()
        .and_then(|guard| guard.as_ref().unwrap().embed(vec![text]))
        .map(|mut v| v.pop().unwrap_or_default())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn init_fastembed(app: AppHandle) -> Result<(), String> {
    let _ = app.emit("embed-init-progress", ());
    match crate::embedder::get_or_init_local_embedder() {
        Ok(_) => {
            let _ = app.emit("embed-init-done", ());
            Ok(())
        }
        Err(e) => {
            let _ = app.emit(
                "embed-init-error",
                serde_json::json!({ "message": e.to_string() }),
            );
            Err(e.to_string())
        }
    }
}

// ── Ollama commands ───────────────────────────────────────────────────────────

#[tauri::command]
fn check_ollama() -> OllamaStatus {
    ollama_check()
}

#[tauri::command]
fn list_local_models() -> Result<Vec<String>, String> {
    ollama_models().map_err(|e| e.to_string())
}

#[tauri::command]
fn get_recommended_model() -> String {
    ollama_recommended().to_string()
}

#[tauri::command]
fn start_ollama_server() -> Result<(), String> {
    ollama_start().map_err(|e| e.to_string())
}

#[tauri::command]
fn pull_model(model_id: String, app: AppHandle) -> Result<(), String> {
    ollama_pull(&model_id, move |completed, total| {
        let _ = app.emit(
            "ollama-pull-progress",
            serde_json::json!({ "completed": completed, "total": total }),
        );
    })
    .map_err(|e| e.to_string())
}

// ── Search commands ───────────────────────────────────────────────────────────

#[tauri::command]
fn search_vault(
    query: String,
    limit: usize,
    db_state: State<DbState>,
    cfg: State<VaultConfigState>,
) -> Result<Vec<search::SearchResult>, String> {
    let profile = cfg
        .0
        .lock()
        .unwrap()
        .get_embed_profile()
        .map_err(|e| e.to_string())?;
    let guard = db_state.0.lock().unwrap();
    retrieval::semantic_search_chunks(&guard.0, &profile, &query, limit).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_related_chunks(
    doc_path: String,
    limit: usize,
    db_state: State<DbState>,
    vault_state: State<VaultConfigState>,
) -> Result<Vec<search::SearchResult>, String> {
    let root = vault_state
        .0
        .lock()
        .unwrap()
        .get_vault_path()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "no vault path set".to_string())?;
    let vault_root = std::path::PathBuf::from(&root);

    let normalized_rel = normalize_path_argument_to_vault_relative(&doc_path, &vault_root)?;
    // `related_chunks_try_paths` only reads SQLite; the document row may still exist after the
    // file was removed from disk. MayCreate validates containment via the parent dir without
    // requiring the target file to exist.
    let safe = crate::vault::safe_vault_path(
        &vault_root,
        &normalized_rel,
        crate::vault::READABLE_SUBDIRS,
        crate::vault::PathMode::MayCreate,
    )
    .map_err(|e| e.to_string())?;

    let mut candidates: Vec<String> = Vec::new();
    let mut push = |s: String| {
        if !candidates.iter().any(|e| e == &s) {
            candidates.push(s);
        }
    };
    if safe.exists() {
        if let Ok(canon) = std::fs::canonicalize(&safe) {
            push(canon.to_string_lossy().into_owned());
        }
    }
    push(safe.to_string_lossy().into_owned());
    push(normalized_rel.clone());
    push(
        vault_root
            .join(Path::new(&normalized_rel))
            .to_string_lossy()
            .into_owned(),
    );

    let guard = db_state.0.lock().unwrap();
    crate::search::related_chunks_try_paths(&guard.0, &candidates, limit).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_impact_radius(
    root_chunk_id: i64,
    entity_id: String,
    direction: String,
    max_hops: u32,
    db_state: State<DbState>,
) -> Result<Vec<graph::NeighborRow>, String> {
    let max_hops = max_hops.min(5);
    let guard = db_state.0.lock().unwrap();
    let conn = &guard.0;

    match direction.as_str() {
        "callees" => graph::get_callees(conn, root_chunk_id, &entity_id, max_hops),
        "callers" => graph::get_callers(conn, root_chunk_id, &entity_id, max_hops),
        "both" => graph::get_both(conn, root_chunk_id, &entity_id, max_hops),
        other => Err(anyhow::anyhow!("unknown direction: {}", other)),
    }
    .map_err(|e| e.to_string())
}

/// Returns graph-adjacent chunks for `doc_path` with `structural: true` and `rel_type` set,
/// so the frontend can display them as "Connected" results alongside semantic hits.
const MAX_STRUCTURAL_NEIGHBORS: usize = 100;

#[tauri::command]
fn get_structural_neighbors(
    doc_path: String,
    max_hops: u32,
    db_state: State<DbState>,
    vault_state: State<VaultConfigState>,
) -> Result<Vec<search::SearchResult>, String> {
    let root = vault_state
        .0
        .lock()
        .unwrap()
        .get_vault_path()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "no vault path set".to_string())?;
    let vault_root = {
        let p = std::path::PathBuf::from(&root);
        p.canonicalize().unwrap_or(p)
    };

    let normalized_rel = normalize_path_argument_to_vault_relative(&doc_path, &vault_root)?;
    let safe = crate::vault::safe_vault_path(
        &vault_root,
        &normalized_rel,
        crate::vault::READABLE_SUBDIRS,
        crate::vault::PathMode::MustExist,
    )
    .map_err(|e| e.to_string())?;
    let abs_path = safe.to_string_lossy().into_owned();
    let canonical_root = vault_root.to_string_lossy().into_owned();
    let entity_id = pipeline::entity_id_for_path(&abs_path, Some(&canonical_root));

    let max_hops = max_hops.min(5);
    let guard = db_state.0.lock().unwrap();
    let conn = &guard.0;

    let source_chunk_ids: Vec<i64> = {
        let mut stmt = conn
            .prepare(
                "SELECT c.id FROM chunks c
                 JOIN documents d ON d.id = c.doc_id
                 WHERE d.path = ?1 AND d.status = 'indexed'",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([&abs_path], |row| row.get::<_, i64>(0))
            .map_err(|e| e.to_string())?;
        rows.filter_map(|r| r.ok()).collect()
    };

    let mut seen_ids = std::collections::HashSet::new();
    for id in &source_chunk_ids {
        seen_ids.insert(*id);
    }

    let mut neighbor_pairs: Vec<(i64, String)> = Vec::new();
    for chunk_id in source_chunk_ids {
        if neighbor_pairs.len() >= MAX_STRUCTURAL_NEIGHBORS {
            break;
        }
        if let Ok(neighbors) = crate::graph::get_both(conn, chunk_id, &entity_id, max_hops) {
            for n in neighbors {
                if neighbor_pairs.len() >= MAX_STRUCTURAL_NEIGHBORS {
                    break;
                }
                if seen_ids.insert(n.chunk_id) {
                    neighbor_pairs.push((n.chunk_id, n.rel_type));
                }
            }
        }
    }

    let mut results = Vec::new();
    for (chunk_id, rel_type) in neighbor_pairs {
        let row = conn.query_row(
            "SELECT d.path, c.chunk_text, c.position, c.start_line, c.end_line,
             COALESCE(c.symbol_name, '') AS symbol_name, c.strategy
             FROM chunks c JOIN documents d ON d.id = c.doc_id
             WHERE c.id = ?1 AND d.status = 'indexed'",
            [chunk_id],
            |row| {
                let sym: String = row.get(5)?;
                Ok(search::SearchResult {
                    doc_path: row.get(0)?,
                    chunk_text: row.get(1)?,
                    chunk_position: row.get(2)?,
                    score: 0.0,
                    start_line: row.get(3)?,
                    end_line: row.get(4)?,
                    symbol_name: if sym.is_empty() { None } else { Some(sym) },
                    strategy: row.get(6)?,
                    structural: Some(true),
                    rel_type: Some(rel_type.clone()),
                    entity_id: None,
                })
            },
        );
        if let Ok(r) = row {
            results.push(r);
        }
    }

    Ok(results)
}

#[tauri::command]
fn get_chunk_ids_for_wiki_entry(
    entry_id: i64,
    entity_id: String,
    db_state: State<DbState>,
    vault_state: State<VaultConfigState>,
) -> Result<Vec<i64>, String> {
    let root = vault_state
        .0
        .lock()
        .unwrap()
        .get_vault_path()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "no vault path set".to_string())?;
    let vault_root = std::path::PathBuf::from(&root);

    let source_ref: Option<String> = {
        let guard = db_state.0.lock().unwrap();
        let conn = &guard.0;
        conn.query_row(
            "SELECT source_ref FROM llm_wiki_entries WHERE rowid = ?1 OR id = ?1",
            [entry_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .flatten()
    };

    let source_ref = match source_ref {
        Some(r) => r,
        None => return Ok(Vec::new()),
    };

    let normalized_rel = match normalize_path_argument_to_vault_relative(&source_ref, &vault_root) {
        Ok(r) => r,
        Err(_) => return Ok(Vec::new()),
    };

    let safe = match crate::vault::safe_vault_path(
        &vault_root,
        &normalized_rel,
        crate::vault::READABLE_SUBDIRS,
        crate::vault::PathMode::MustExist,
    ) {
        Ok(p) => p,
        Err(_) => return Ok(Vec::new()),
    };
    let abs_path = safe.to_string_lossy().into_owned();

    let guard = db_state.0.lock().unwrap();
    let conn = &guard.0;
    let mut stmt = conn
        .prepare(
            "SELECT c.id FROM chunks c
             JOIN documents d ON d.id = c.doc_id
             WHERE d.path = ?1 AND c.entity_id = ?2 AND d.status = 'indexed'",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([&abs_path, &entity_id], |row| row.get::<_, i64>(0))
        .map_err(|e| e.to_string())?;

    let mut ids = Vec::new();
    for id in rows.flatten() {
        ids.push(id);
    }

    Ok(ids)
}

// ── Vault file listing ────────────────────────────────────────────────────────

#[derive(serde::Serialize, Clone)]
pub struct VaultFile {
    pub path: String,
    pub name: String,
    pub tier: String, // "user_doc" — wiki/ is archive-only post-V7, not listed for ingest
}

#[tauri::command]
fn list_vault_files(state: State<VaultConfigState>) -> Result<Vec<VaultFile>, String> {
    let root = match state
        .0
        .lock()
        .unwrap()
        .get_vault_path()
        .map_err(|e| e.to_string())?
    {
        Some(p) => std::path::PathBuf::from(p),
        None => return Ok(vec![]),
    };

    let root = root
        .canonicalize()
        .map_err(|e| format!("vault path not accessible: {}", e))?;

    let mut files = Vec::new();

    // Scan ONLY the immutable source tier. wiki/ is archive-only post-V7
    // (and now post-v2) and must never appear in list_vault_files — the
    // folder tree renders wiki/ from get_vault_layout instead.
    for subdir in &[crate::vault::IMMUTABLE_DIR] {
        let dir = root.join(subdir);
        if !dir.exists() {
            continue;
        }
        let walker = walkdir::WalkDir::new(&dir)
            .min_depth(1)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .filter(|e| {
                let ext = e.path().extension().and_then(|s| s.to_str()).unwrap_or("");
                should_ingest_extension(ext)
            });

        for entry in walker {
            // Return vault-relative paths for safe_vault_path compatibility.
            // Skip any entry that can't be made relative (shouldn't happen in normal operation,
            // but could occur if the vault contains symlinks pointing outside).
            let Some(relative) = entry.path().strip_prefix(&root).ok() else {
                continue;
            };
            let Ok(path) = to_forward_slash_relative(relative) else {
                continue;
            };
            let name = entry.file_name().to_string_lossy().to_string();
            files.push(VaultFile {
                path,
                name,
                tier: "user_doc".to_string(),
            });
        }
    }

    Ok(files)
}

#[tauri::command]
fn read_document(path: String, state: State<VaultConfigState>) -> Result<String, String> {
    let root = match state
        .0
        .lock()
        .unwrap()
        .get_vault_path()
        .map_err(|e| e.to_string())?
    {
        Some(p) => std::path::PathBuf::from(p),
        None => return Err("no vault path set".to_string()),
    };

    let normalized_path = normalize_path_argument_to_vault_relative(&path, &root)?;

    let safe = crate::vault::safe_vault_path(
        &root,
        &normalized_path,
        crate::vault::READABLE_SUBDIRS,
        crate::vault::PathMode::MustExist,
    )
    .map_err(|e| e.to_string())?;

    std::fs::read_to_string(&safe).map_err(|e| e.to_string())
}

// ── Review queue (see proposals_api.rs) ───────────────────────────────────────

// ── Folder rules ──────────────────────────────────────────────────────────────

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct FolderRule {
    pub id: i64,
    pub folder_path: String,
    pub librarian_mode: String,
    pub auto_approve: bool,
}

#[tauri::command]
fn get_folder_rules(db_state: State<DbState>) -> Result<Vec<FolderRule>, String> {
    let guard = db_state.0.lock().unwrap();
    let conn = &guard.0;
    let mut stmt = conn
        .prepare("SELECT id, folder_path, librarian_mode, auto_approve FROM folder_rules ORDER BY folder_path")
        .map_err(|e| e.to_string())?;
    let mut rules = Vec::new();
    let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        rules.push(FolderRule {
            id: row.get(0).map_err(|e| e.to_string())?,
            folder_path: row.get(1).map_err(|e| e.to_string())?,
            librarian_mode: row.get(2).map_err(|e| e.to_string())?,
            auto_approve: row.get::<_, i64>(3).map_err(|e| e.to_string())? != 0,
        });
    }
    Ok(rules)
}

#[derive(serde::Serialize)]
pub struct VaultFolder {
    pub name: String,
    pub label: String,
}

#[tauri::command]
fn get_vault_layout() -> Vec<VaultFolder> {
    vec![
        VaultFolder {
            name: crate::vault::safe_path::IMMUTABLE_DIR.to_string(),
            label: "Source Files".to_string(),
        },
        VaultFolder {
            name: crate::vault::safe_path::WIKI_DIR.to_string(),
            label: "Wiki Pages".to_string(),
        },
    ]
}

#[tauri::command]
fn set_folder_rule(
    folder_path: String,
    librarian_mode: String,
    auto_approve: bool,
    db_state: State<DbState>,
) -> Result<(), String> {
    let auto_i: i64 = if auto_approve { 1 } else { 0 };
    db_state
        .0
        .lock()
        .unwrap()
        .0
        .execute(
            "INSERT INTO folder_rules (folder_path, librarian_mode, auto_approve)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(folder_path) DO UPDATE SET librarian_mode = ?2, auto_approve = ?3",
            rusqlite::params![folder_path, librarian_mode, auto_i],
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn delete_folder_rule(id: i64, db_state: State<DbState>) -> Result<(), String> {
    db_state
        .0
        .lock()
        .unwrap()
        .0
        .execute("DELETE FROM folder_rules WHERE id = ?1", [id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn save_wiki_page(
    path: String,
    content: String,
    vault_state: State<VaultConfigState>,
) -> Result<(), String> {
    let vault = vault_state
        .0
        .lock()
        .unwrap()
        .get_vault_path()
        .map_err(|e| e.to_string())?
        .ok_or("no vault set".to_string())?;
    let vault_root = std::path::PathBuf::from(&vault);
    // Ensure the allowed subdir exists before resolving the user path.
    std::fs::create_dir_all(vault_root.join("wiki")).map_err(|e| e.to_string())?;

    // Reject absolute paths before normalization
    if std::path::Path::new(&path).is_absolute() {
        return Err("absolute paths not allowed".to_string());
    }

    // Normalize separators and ensure a wiki/ prefix for backward compatibility.
    let normalized_path = normalize_wiki_relative_path(&path);

    let safe = crate::vault::safe_vault_path(
        &vault_root,
        &normalized_path,
        crate::vault::WRITABLE_SUBDIRS,
        crate::vault::PathMode::MayCreate,
    )
    .map_err(|e| e.to_string())?;

    crate::vault::safe_path::safe_write_bytes(&safe, content.as_bytes())
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ── File management ───────────────────────────────────────────────────────────

#[tauri::command]
fn delete_vault_file(path: String, state: State<VaultConfigState>) -> Result<(), String> {
    let root = state
        .0
        .lock()
        .unwrap()
        .get_vault_path()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "no vault set".to_string())?;
    let vault_root = std::path::PathBuf::from(&root);

    let normalized_path = normalize_path_argument_to_vault_relative(&path, &vault_root)?;

    let safe = crate::vault::safe_vault_path(
        &vault_root,
        &normalized_path,
        crate::vault::READABLE_SUBDIRS,
        crate::vault::PathMode::MustExist,
    )
    .map_err(|e| e.to_string())?;

    std::fs::remove_file(&safe).map_err(|e| e.to_string())
}

/// `n == 0` returns `original`; larger `n` inserts ` (n)` before the extension (Windows/macOS style).
fn drop_destination_filename(original: &str, n: u32) -> String {
    if n == 0 {
        return original.to_string();
    }
    let p = std::path::Path::new(original);
    let stem = p.file_stem().and_then(|s| s.to_str());
    let ext = p.extension().and_then(|e| e.to_str());
    match (stem, ext) {
        (Some(stem), Some(ext)) if !stem.is_empty() => {
            format!("{} ({}).{}", stem, n, ext)
        }
        _ => format!("{} ({})", original, n),
    }
}

fn unique_drop_destination(
    vault_root: &std::path::Path,
    original_file_name: &str,
) -> Result<std::path::PathBuf, String> {
    use crate::vault::safe_path::IMMUTABLE_DIR;
    const MAX_TRIES: u32 = 10_000;
    for n in 0..MAX_TRIES {
        let candidate_name = drop_destination_filename(original_file_name, n);
        let rel = format!("{IMMUTABLE_DIR}/{candidate_name}");
        match crate::vault::safe_vault_path(
            vault_root,
            &rel,
            &[IMMUTABLE_DIR],
            crate::vault::PathMode::MustExist,
        ) {
            Ok(_) => continue,
            Err(crate::vault::SafePathError::NotFound(_)) => {
                return crate::vault::safe_vault_path(
                    vault_root,
                    &rel,
                    &[IMMUTABLE_DIR],
                    crate::vault::PathMode::MayCreate,
                )
                .map_err(|e| e.to_string());
            }
            // Directory, symlink escape, or other non-file collision: treat as "name taken"
            // and try the next ` (n)` suffix instead of failing the whole drop batch.
            Err(
                crate::vault::SafePathError::NotARegularFile
                | crate::vault::SafePathError::Outside
                | crate::vault::SafePathError::InvalidName
                | crate::vault::SafePathError::Absolute
                | crate::vault::SafePathError::Traversal,
            ) => continue,
            Err(crate::vault::SafePathError::Io(e)) => return Err(e.to_string()),
        }
    }
    Err(format!(
        "could not find a free filename under {IMMUTABLE_DIR}/ for {original_file_name}"
    ))
}

fn copy_os_drop_paths_to_vault(
    app: &AppHandle,
    paths: &[std::path::PathBuf],
) -> Result<Vec<String>, String> {
    use crate::vault::safe_path::IMMUTABLE_DIR;
    let vault_path = app
        .state::<VaultConfigState>()
        .0
        .lock()
        .unwrap()
        .get_vault_path()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "no vault configured".to_string())?;
    let vault_root = std::path::PathBuf::from(&vault_path);
    std::fs::create_dir_all(vault_root.join(IMMUTABLE_DIR)).map_err(|e| e.to_string())?;

    let mut copied_paths = Vec::new();

    for src in paths {
        if !src.is_file() {
            continue;
        }

        let file_name = match src.file_name().and_then(|n| n.to_str()) {
            Some(name) => name,
            None => {
                eprintln!("[drop-copy] skipping file with invalid filename: {:?}", src);
                continue;
            }
        };

        let dest = unique_drop_destination(&vault_root, file_name)?;

        crate::vault::safe_path::safe_copy_file(src, &dest).map_err(|e| e.to_string())?;

        // Ingest and vault-event are emitted by the filesystem watcher; no
        // manual enqueue/emit here to avoid duplicated pipeline jobs and UI
        // events for every dropped file.
        copied_paths.push(dest.to_string_lossy().into_owned());
    }

    Ok(copied_paths)
}

// ── Test utilities ────────────────────────────────────────────────────────────

pub use pipeline::{
    entity_id_for_path, entity_id_for_virtual_path, ingest_document, ingest_document_virtual,
    ingest_document_with_vault_root,
};

#[tauri::command]
fn ingest_document_cmd(
    app: tauri::AppHandle,
    db: tauri::State<'_, crate::DbState>,
    embed_profile: tauri::State<'_, crate::EmbedProfileState>,
    path: String,
) -> Result<(), String> {
    run_ingest_with_app(&app, &db.0, &embed_profile.0, path)
}

/// Phase 9: gate query so the frontend knows whether to mount the
/// `SplashScreen` and wait for `migration-complete`. Returns `true` when
/// at least one chunk row is missing `content_hash` (i.e. the one-time
/// chunk-hash migration still has work to do). The migration itself is
/// dispatched by the `setup` hook at startup; this command is the
/// read-side companion the frontend polls on mount. Defaults to `true`
/// on error so the splash mounts and the user at least sees a stuck UI
/// rather than a silent dead-load if the gate query itself fails.
#[tauri::command]
fn needs_chunk_hash_migration(db: tauri::State<'_, crate::DbState>) -> Result<bool, String> {
    let guard = db.0.lock().map_err(|e| format!("db lock poisoned: {e}"))?;
    crate::db::migration::chunks_have_content_hash(&guard.0)
        .map(|ok| !ok)
        .map_err(|e| e.to_string())
}

/// Synchronous ingest + progress event emitter. Extracted from
/// `ingest_document_cmd` so the event-emission sequence can be exercised in a
/// test against a real `tauri::App<MockRuntime>` (AppHandle as a Tauri command
/// argument is not supported by `MockRuntime`'s `CommandArg` extractor).
/// Non-destructive read of the most recent `config-malformed` payload
/// stashed by the setup hook. The setup thread can emit the event before
/// the frontend listener registers (Tauri does not buffer events emitted
/// before the corresponding `listen()` resolves), so `AppShell` peeks on
/// mount to recover any payload that was missed — CodeRabbit #19, PR #120.
/// Returns `None` if the setup hook never produced a malformed-config
/// report. Pairs with `ack_pending_config_malformed`, which the caller
/// MUST invoke after successfully rendering the payload — CodeRabbit #21,
/// PR #120. The earlier destructive `take_` variant dropped the payload
/// whenever cleanup ran before the IPC `.then` resolved.
#[tauri::command]
fn peek_pending_config_malformed(
    state: tauri::State<'_, PendingConfigMalformed>,
) -> Option<ConfigMalformedPayload> {
    state.0.lock().unwrap().clone()
}

/// Clears the stashed `config-malformed` payload after the caller has
/// successfully rendered it — pairs with `peek_pending_config_malformed`.
/// No-op if no payload is pending. Safe to call multiple times.
#[tauri::command]
fn ack_pending_config_malformed(state: tauri::State<'_, PendingConfigMalformed>) {
    let _ = state.0.lock().unwrap().take();
}
fn run_ingest_with_app<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    db: &Mutex<AppDb>,
    embed_profile: &Mutex<crate::embedder::EmbedProfile>,
    path: String,
) -> Result<(), String> {
    let conn = db.lock().map_err(|e| e.to_string())?;
    let profile = embed_profile.lock().map_err(|e| e.to_string())?.clone();
    let _ = app.emit(
        "ingest-progress",
        serde_json::json!({"phase": "chunking", "path": path}),
    );
    let result = crate::pipeline::ingest_document(&conn.0, &profile, &path, false);
    let _ = app.emit(
        "ingest-progress",
        serde_json::json!({"phase": "embedding", "path": path}),
    );
    match &result {
        Ok(()) => {
            // Look up the most-recent pending proposal citing this path (if any)
            // and surface its id in the ready event. `None` is serialized as
            // JSON `null` — Task 3's TypeScript contract is `proposalId: string | null`,
            // and Task 7's `useEffect` skips routing when the value is `null`
            // (it would otherwise route to a nonexistent proposal id).
            let proposal_id = crate::db::proposals::latest_pending_for_path(&conn.0, &path)
                .ok()
                .flatten();
            let _ = app.emit(
                "ingest-proposal-ready",
                serde_json::json!({"path": path, "proposalId": proposal_id}),
            );
        }
        Err(e) => {
            let _ = app.emit(
                "ingest-error",
                serde_json::json!({"message": e.to_string()}),
            );
        }
    }
    Ok(())
}

#[cfg(feature = "test-utils")]
pub fn make_test_app(tmp_path: &std::path::Path) -> tauri::App<tauri::test::MockRuntime> {
    let db_path = tmp_path.join("brain.db");
    let config_path = tmp_path.join("config.json");
    let db = db::AppDb::open_with_config(&db_path, &config_path).expect("open test db");
    let config = vault::VaultConfig::new(tmp_path.join("config.json"));
    tauri::test::mock_builder()
        .manage(DbState(std::sync::Mutex::new(db)))
        .manage(VaultConfigState(std::sync::Mutex::new(config)))
        // Registered so commands that take `State<EmbedProfileState>` (e.g.,
        // `add_entity_fact_cmd`, `update_entity_fact_cmd`) can extract it.
        //
        // Deliberately NOT `EmbedProfile::default()` — that is
        // `Local { nomic-embed-code }`, an Ollama profile, so every commit
        // path that now embeds at write time would make an HTTP call to
        // localhost:11434 from the test suite: a hang or a connection error
        // depending on whether the machine happens to run Ollama, and
        // different results on a developer box than on CI. `Cloud` returns
        // "cloud embed not implemented" without touching the network, which
        // collapses to the NULL-blob path every non-embedding test expects.
        //
        // Tests that DO want vectors set `CURATED_EMBED_STUB=constant8`;
        // `embed_batch` checks that env var before it looks at the profile, so
        // the stub still works with this profile in place.
        .manage(EmbedProfileState(std::sync::Mutex::new(
            crate::embedder::EmbedProfile::Cloud {
                provider: crate::embedder::CloudProvider::OpenAi,
                model: "offline-test-profile".to_string(),
                api_key: String::new(),
            },
        )))
        // No background pipeline thread: integration tests invoke DB-only commands;
        // pipeline work is covered by `PipelineWorker` tests in `tests/pipeline.rs`.
        .manage(PipelineHolder(std::sync::Mutex::new(None)))
        .invoke_handler(tauri::generate_handler![
            get_vault_path,
            set_vault_path,
            get_workspace_id,
            proposals_api::get_review_queue,
            proposals_api::approve_wiki_page,
            proposals_api::reject_wiki_page,
            proposals_api::get_proposed_content,
            entities_api::list_entities_cmd,
            entities_api::get_entity_cmd,
            entities_api::create_entity_cmd,
            entities_api::update_entity_summary_cmd,
            entities_api::archive_entity_cmd,
            entities_api::get_entity_connections_cmd,
            entities_api::add_entity_fact_cmd,
            entities_api::update_entity_fact_cmd,
            entities_api::archive_entity_fact_cmd,
            timeline_api::list_events_cmd,
            timeline_api::list_tasks_cmd,
            timeline_api::create_task_cmd,
            timeline_api::set_task_status_cmd,
            timeline_api::archive_task_cmd,
            okf_api::okf_export_bundle_cmd,
            okf_api::okf_import_preview_cmd,
            okf_api::okf_import_apply_cmd,
            proposals_api::list_proposals_cmd,
            proposals_api::get_proposal_detail_cmd,
            proposals_api::resolve_proposal_cmd,
            get_folder_rules,
            set_folder_rule,
            delete_folder_rule,
            get_vault_layout,
            get_indexing_status,
            list_vault_files,
            read_document,
            search_vault,
            get_related_chunks,
            save_wiki_page,
            queue_full_reindex,
            get_impact_radius,
            get_binary_path,
            get_brain_dir,
            get_brain_dir_info,
            commands::chunks::resolve_chunk_overlay,
            commands::chunks::fetch_chunk_content,
            needs_chunk_hash_migration,
            vault_write_note,
            vault_upsert_index_entry,
            get_ontology_selection,
            set_ontology_selection,
            list_pending_links,
            approve_link,
            revoke_link,
        ])
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap()
}

// ── App entry ─────────────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Same fix as `switch_vault` (CodeRabbit review on PR #96): the
    // entrypoint was hardcoded to `~/.brain/`. With `CURATED_BRAIN_DIR`
    // set, the app would bootstrap a directory the user never asked
    // for. Route through the canonical resolver.
    let paths = retrieval::resolve_brain_paths();
    let brain_dir = paths.brain_dir.clone();
    std::fs::create_dir_all(&brain_dir).ok();

    let db_path = paths.db_path.clone();
    let config_path = paths.config_path.clone();

    // Task 17: surface malformed config immediately so headless launches
    // (and CI logs) see the hint before the Tauri setup hook runs.
    // The in-app banner is emitted from the setup hook below; this early
    // check is a stderr fallback that never blocks startup.  Reuses the
    // unified lenient loader so the diagnostic wording stays in sync with
    // the desktop banner — no second copy of the JSON parse gate.
    if paths.config_path.exists() {
        // load_lenient now returns Result; a fatal Err (malformed JSON,
        // non-object root, non-string vault_path) is the desktop §7 M2
        // "show actionable banner, continue with defaults" path.
        if let Err(e) = crate::config::BrainConfig::load_lenient(&paths) {
            eprintln!(
                "warning: config.json failed to load at {}: {e}. Run `curated-thoughts --doctor`, then `curated-thoughts --onboard --force` to repair.",
                paths.config_path.display()
            );
        }
    }

    let config = VaultConfig::new(config_path.clone());
    let vault_path_for_migration = config.get_vault_path().ok().flatten();
    let vault_migration_needed = !config.has_migrated_to_v2().unwrap_or(false);

    if config.get_vault_path().ok().flatten().is_none() {
        let default_vault = VaultConfig::default_vault_path();
        let mut all_dirs_created = true;
        for subdir in &[crate::vault::safe_path::IMMUTABLE_DIR, "wiki"] {
            if let Err(e) = std::fs::create_dir_all(default_vault.join(subdir)) {
                eprintln!("warning: failed to create default vault subdirectory {subdir}: {e}");
                all_dirs_created = false;
            }
        }
        if let Err(e) = std::fs::create_dir_all(default_vault.join(AGENTS_DEPOSIT_DIR)) {
            eprintln!("warning: failed to create default vault immutable-source-files/agents: {e}");
            all_dirs_created = false;
        }
        if let Err(e) = std::fs::create_dir_all(default_vault.join(".brain").join("converted")) {
            eprintln!("warning: failed to create default vault .brain/converted: {e}");
            all_dirs_created = false;
        }
        if all_dirs_created {
            if let Some(vault_str) = default_vault.to_str() {
                if let Err(e) = config.set_vault_path(vault_str) {
                    eprintln!("warning: failed to persist default vault path: {e}");
                }
            } else {
                eprintln!("warning: default vault path contains invalid UTF-8");
            }
        } else {
            // Fallback: use temp directory to prevent app from getting stuck in setup
            eprintln!("error: failed to create default vault directory structure; falling back to temporary directory");
            let fallback_vault = std::env::temp_dir().join("Curated-Thoughts-recovery");
            let mut fallback_dirs_created = true;
            for subdir in &[crate::vault::safe_path::IMMUTABLE_DIR, "wiki"] {
                if let Err(e) = std::fs::create_dir_all(fallback_vault.join(subdir)) {
                    eprintln!("error: failed to create fallback vault subdir {subdir}: {e}");
                    fallback_dirs_created = false;
                }
            }
            if let Err(e) = std::fs::create_dir_all(fallback_vault.join(AGENTS_DEPOSIT_DIR)) {
                eprintln!("error: failed to create fallback vault subdir immutable-source-files/agents: {e}");
                fallback_dirs_created = false;
            }
            if let Err(e) = std::fs::create_dir_all(fallback_vault.join(".brain").join("converted"))
            {
                eprintln!("error: failed to create fallback vault subdir .brain/converted: {e}");
                fallback_dirs_created = false;
            }
            if fallback_dirs_created {
                eprintln!(
                    "warning: using temporary recovery vault at: {}",
                    fallback_vault.display()
                );
                if let Some(vault_str) = fallback_vault.to_str() {
                    if let Err(e) = config.set_vault_path(vault_str) {
                        eprintln!("warning: failed to persist fallback vault path: {e}");
                    }
                }
            } else {
                eprintln!("error: also failed to create fallback vault directory structure");
            }
        }
    }

    let db = AppDb::open_with_config(&db_path, &config_path).expect("failed to open database");
    // Phase 9: one-time content_hash migration gate. The V9 schema adds
    // the column; this returns true on the first start after the schema
    // ships. The actual data migration is dispatched in the setup
    // closure (below) once the AppHandle is available; this block only
    // captures the flag for that closure to pick up. On error from the
    // check, default to "needs migration" so the gate self-heals.
    let needs_migration = !crate::db::migration::chunks_have_content_hash(&db.0).unwrap_or(false);
    let initial_vault_root = config.get_vault_path().ok().flatten().map(|p| {
        let pb = PathBuf::from(&p);
        pb.canonicalize().unwrap_or(pb)
    });
    // The migration derives `chunks.entity_id` from this root. It must
    // match the canonical spelling used by the pipeline and by the
    // graph readers, otherwise the hashed `tier_working::` prefix
    // diverges for non-`documents/` paths and neighbor lookups return
    // nothing. Reuse the canonicalized value before `initial_vault_root`
    // is moved into `start_pipeline`.
    let migration_vault_root = initial_vault_root
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned());
    let pipeline = start_pipeline(db_path.clone(), initial_vault_root);

    let embed_profile = config
        .get_embed_profile()
        .unwrap_or_else(|_| crate::embedder::EmbedProfile::default());

    tauri::Builder::default()
        .manage(OutboxWorkerState(Mutex::new(None)))
        .manage(CloudBridgeState(Mutex::new(None)))
        .manage(CloudBridgeLifecycle(tokio::sync::Mutex::new(())))
        .setup({
            let db_path = db_path.clone();
            move |app| {
                // Phase 9: run the chunk-hash migration if the gate
                // flagged it. The frontend SplashScreen (Task 9)
                // listens to `migration-progress` /
                // `migration-complete` / `migration-error` events and
                // gates the rest of the UI until the migration
                // finishes; spawn_blocking keeps the runtime
                // responsive while the transaction runs. The DbState
                // wraps Mutex<AppDb> and Connection isn't Clone, so
                // we re-take the lock inside the spawn_blocking
                // closure (Option B from the brief) instead of
                // trying to move the lock across threads.

                // Vault migration: migrate documents/ -> immutable-source-files/
                if vault_migration_needed {
                    if let Some(vault_path) = vault_path_for_migration {
                        let vault_root = PathBuf::from(&vault_path);
                        let config_for_migration = VaultConfig::new(retrieval::resolve_brain_paths().config_path);
                        match crate::vault::config::migrate_vault(&vault_root) {
                            Ok(()) => {
                                if let Err(e) = config_for_migration.set_migrated_to_v2() {
                                    eprintln!("warning: failed to persist migration flag: {e}");
                                }
                                let _ = app.emit(
                                    "vault-migration-success",
                                    serde_json::json!({
                                        "message": "Documents folder renamed to 'immutable-source-files' to match app conventions. Your files are unchanged.",
                                    }),
                                );
                            }
                            Err(crate::vault::config::MigrationError::BothFoldersExist { old, new }) => {
                                // Blocking error: emit blocking dialog event
                                let _ = app.emit(
                                    "vault-migration-blocked",
                                    serde_json::json!({
                                        "message": format!(
                                            "Migration blocked: both '{}' and '{}' exist. Move your files from '{}' into '{}' manually, then restart the app. (Files left in '{}' are not visible to the app.)",
                                            old.display(), new.display(), old.display(), new.display(), old.display()
                                        ),
                                    }),
                                );
                            }
                            Err(e) => {
                                eprintln!("warning: vault migration failed: {e}");
                                let _ = app.emit(
                                    "vault-migration-error",
                                    serde_json::json!({
                                        "message": format!("Migration failed: {e}"),
                                    }),
                                );
                            }
                        }
                    }
                }

                if needs_migration {
                    let app_handle = app.app_handle().clone();
                    tauri::async_runtime::spawn_blocking(move || {
                        let db_state = app_handle.state::<DbState>();
                        let mut guard = match db_state.0.lock() {
                            Ok(g) => g,
                            Err(e) => {
                                let _ = app_handle.emit(
                                    "migration-error",
                                    serde_json::json!({
                                        "message": format!("db lock poisoned: {e}"),
                                    }),
                                );
                                return;
                            }
                        };
                        let emit = |p: crate::db::migration::MigrationProgress| {
                            let _ = app_handle.emit(
                                "migration-progress",
                                serde_json::json!({
                                    "current": p.current,
                                    "total": p.total,
                                    "phase": p.phase,
                                }),
                            );
                        };
                        match crate::db::migration::run_chunk_hash_migration(
                            &mut guard.0,
                            migration_vault_root.as_deref(),
                            emit,
                        ) {
                            Ok(()) => {
                                let _ = app_handle.emit("migration-complete", ());
                            }
                            Err(e) => {
                                let _ = app_handle.emit(
                                    "migration-error",
                                    serde_json::json!({
                                        "message": e.to_string(),
                                    }),
                                );
                            }
                        }
                    });
                }

                if let Some(db_url) = configured_database_url() {
                    let config = OutboxConfig {
                        sqlite_path: db_path.clone(),
                        db_url,
                        ..OutboxConfig::default()
                    };
                    let handle = spawn_postgres_worker(config, Some(app.app_handle().clone()));
                    let state = app.state::<OutboxWorkerState>();
                    *state.0.lock().unwrap() = Some(handle);
                }

                let app_handle = app.app_handle().clone();
                tauri::async_runtime::spawn(async move {
                    let state = app_handle.state::<CloudBridgeState>();
                    let lifecycle = app_handle.state::<CloudBridgeLifecycle>();
                    start_cloud_bridge_if_configured(&lifecycle, &state).await;
                });

                let app_handle = app.app_handle().clone();
                std::thread::spawn(move || {
                    let _ = app_handle.emit("embed-init-progress", ());
                    match crate::embedder::get_or_init_local_embedder() {
                        Ok(_) => {
                            let _ = app_handle.emit("embed-init-done", ());
                        }
                        Err(e) => {
                            let _ = app_handle.emit(
                                "embed-init-error",
                                serde_json::json!({ "message": e.to_string() }),
                            );
                        }
                    }

                    let brain_dir_str = get_brain_dir_inner();
                    let brain_path = std::path::Path::new(&brain_dir_str);
                    // Use the unified resolver so we honor `CURATED_BRAIN_DB`
                    // / `CURATED_BRAIN_CONFIG` and stay in sync with every
                    // other config-loading call site.
                    let resolved_paths = crate::retrieval::resolve_brain_paths();
                    let report = crate::config::BrainConfig::load_lenient(
                        &crate::retrieval::BrainPaths {
                            brain_dir: brain_path.to_path_buf(),
                            config_path: resolved_paths.config_path.clone(),
                            db_path: brain_path.join("brain.db"),
                        },
                    );
                    // Task 17: desktop startup surfaces malformed config with a
                    // recoverable banner instead of crash-looping.  The same
                    // load_lenient call drives the in-app banner and a
                    // stderr hint for headless launches; on Err we fall back
                    // to a default config so the session can still run
                    // --doctor (the §7 M2 degrade-visibly-not-crash-loop rule).
                    let (config, malformed_diagnostics): (
                        crate::config::BrainConfig,
                        Vec<String>,
                    ) = match report {
                        Ok(report) => {
                            // Leniency diagnostics are noisy on their own; only
                            // surface those that explicitly mention "malformed"
                            // (matches the wording of `ConfigError::MalformedJson`).
                            let mut diags: Vec<String> = report
                                .diagnostics
                                .iter()
                                .filter(|d| d.contains("malformed"))
                                .cloned()
                                .collect();
                            // Required-block absences are loud per spec §4 — the
                            // banner must fire when `load_lenient` defaulted a
                            // required block (missing key, OR unparseable nested
                            // block whose serde error doesn't contain the literal
                            // "malformed"). Without this trigger a user with e.g.
                            // `{"generation": {"provider": "future_kind"}}` gets a
                            // defaulted Unconfigured LLM and no signal to fix it.
                            if report.generation_missing {
                                diags.push(
                                    "generation block missing or unparseable".to_string(),
                                );
                            }
                            if report.embedding_missing {
                                diags.push(
                                    "embedding block missing or unparseable".to_string(),
                                );
                            }
                            if report.privacy_missing {
                                diags.push("privacy block missing or unparseable".to_string());
                            }
                            (report.config, diags)
                        }
                        Err(e) => (
                            crate::config::BrainConfig::default(),
                            vec![format!("config.json failed to load: {e}")],
                        ),
                    };
                    if !malformed_diagnostics.is_empty() {
                        eprintln!(
                            "warning: config.json is malformed at {}. Run `curated-thoughts --doctor`, then `curated-thoughts --onboard --force` to repair.",
                            resolved_paths.config_path.display()
                        );
                        // Stash the payload so `AppShell` can recover it on
                        // mount via `peek_pending_config_malformed`. Tauri
                        // does not buffer events emitted before the frontend
                        // listener registers, and this background thread can
                        // finish before the React app boots — CodeRabbit #19,
                        // PR #120.
                        let payload = ConfigMalformedPayload {
                            config_path: resolved_paths.config_path.to_string_lossy().to_string(),
                            diagnostics: malformed_diagnostics,
                            remediation: "Run `curated-thoughts --doctor`, then `curated-thoughts --onboard --force` to repair.".to_string(),
                        };
                        *app_handle.state::<PendingConfigMalformed>().0.lock().unwrap() =
                            Some(payload.clone());
                        let _ = app_handle.emit("config-malformed", payload);
                    }

                    match initialize_provider(brain_path, &config.generation, &app_handle) {
                        Ok(provider) => {
                            let state = app_handle.state::<InferenceState>();
                            let mut guard = state.0.lock().unwrap();
                            *guard = provider;
                            let _ = app_handle.emit("provider-ready", ());
                        }
                        Err(e) => {
                            let _ = app_handle.emit(
                                "provider-error",
                                serde_json::json!({ "message": e.to_string() }),
                            );
                        }
                    }
                });

                // Post-schema-guard null-embedding sweep (spec §3). Errors
                // here are normal offline conditions (provider unreachable,
                // missing config); the sweep itself absorbs provider failures
                // into `SweepReport::failed` and only returns `Err` when the
                // DB is unreachable. Runs on a background thread so up to 8
                // remote-embedding round-trips never freeze the wry/window
                // event loop on first launch. The mutex is locked only for
                // each DB read/write half (per the `run_embedding_sweep`
                // contract) — never across the network call.
                let app_handle = app.app_handle().clone();
                std::thread::spawn(move || {
                    let db_state = app_handle.state::<DbState>();
                    match run_embedding_sweep(&db_state) {
                        Ok(report) if report.filled > 0 || report.failed > 0 => {
                            eprintln!(
                                "startup embedding sweep: filled {}, failed {}, {} still null",
                                report.filled, report.failed, report.remaining_null
                            );
                        }
                        Ok(_) => {}
                        Err(e) => eprintln!("startup embedding sweep skipped: {e}"),
                    }
                });

                Ok(())
            }
        })
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_notification::init())
        // Handle file-drop in Rust so source paths come from OS drop events,
        // not from attacker-controlled webview command arguments.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = event {
                let paths = paths.clone();
                let app = window.app_handle().clone();
                // FS copies can be large; keep the wry/window event loop responsive.
                std::thread::spawn(move || {
                    if let Err(e) = copy_os_drop_paths_to_vault(&app, &paths) {
                        eprintln!("[drop-copy] failed: {e}");
                    }
                });
            }
        })
        .manage(DbState(Mutex::new(db)))
        .manage(VaultConfigState(Mutex::new(config)))
        .manage(EmbedProfileState(Mutex::new(embed_profile)))
        .manage(PipelineHolder(Mutex::new(Some(pipeline))))
        .manage(WikiStatusState(Mutex::new(WikiStatusFlags::default())))
        .manage(InferenceState(Mutex::new(GenerationProvider::Unconfigured)))
        .manage(WatcherStarted(Mutex::new(None)))
        .manage(HealScheduler(Mutex::new(None)))
        .manage(WatchdogSupervisor(Mutex::new(None)))
        .manage(PendingConfigMalformed(Mutex::new(None)))
        .invoke_handler(tauri::generate_handler![
            get_vault_path,
            set_vault_path,
            get_workspace_id,
            backup_vault_db,
            switch_vault,
            check_vault_backup,
            reveal_vault,
            start_file_watcher,
            get_indexing_status,
            queue_full_reindex,
            wiki_exec,
            wiki_run,
            wiki_get_all,
            wiki_get_first,
            embed_text,
            generate_text,
            update_provider,
            get_provider_config,
            download_sidecar_engine,
            download_model_weights,
            init_fastembed,
            check_ollama,
            list_local_models,
            pull_model,
            start_ollama_server,
            get_recommended_model,
            search_vault,
            get_related_chunks,
            list_vault_files,
            read_document,
            proposals_api::get_review_queue,
            proposals_api::approve_wiki_page,
            proposals_api::reject_wiki_page,
            proposals_api::get_proposed_content,
            entities_api::list_entities_cmd,
            entities_api::get_entity_cmd,
            entities_api::create_entity_cmd,
            entities_api::update_entity_summary_cmd,
            entities_api::archive_entity_cmd,
            entities_api::get_entity_connections_cmd,
            entities_api::add_entity_fact_cmd,
            entities_api::update_entity_fact_cmd,
            entities_api::archive_entity_fact_cmd,
            timeline_api::list_events_cmd,
            timeline_api::list_tasks_cmd,
            timeline_api::create_task_cmd,
            timeline_api::set_task_status_cmd,
            timeline_api::archive_task_cmd,
            okf_api::okf_export_bundle_cmd,
            okf_api::okf_import_preview_cmd,
            okf_api::okf_import_apply_cmd,
            proposals_api::list_proposals_cmd,
            proposals_api::get_proposal_detail_cmd,
            proposals_api::resolve_proposal_cmd,
            get_folder_rules,
            set_folder_rule,
            delete_folder_rule,
            get_vault_layout,
            save_wiki_page,
            delete_vault_file,
            run_wiki_heal,
            run_wiki_prune,
            run_wiki_forget,
            run_wiki_reembed,
            run_wiki_reindex,
            get_chunk_ids_for_wiki_entry,
            get_impact_radius,
            get_structural_neighbors,
            start_outbox_worker,
            stop_outbox_worker,
            outbox_is_configured,
            set_cloud_bridge_pairing_token,
            clear_cloud_bridge_pairing_token,
            get_cloud_bridge_status,
            retry_cloud_bridge_now,
            get_privacy_mode,
            set_privacy_mode,
            acknowledge_migration_disclosure,
            acknowledge_ephemeral_disclosure,
            get_ontology_selection,
            set_ontology_selection,
            list_pending_links,
            approve_link,
            revoke_link,
            get_binary_path,
            get_brain_dir,
            get_brain_dir_info,
            commands::chunks::resolve_chunk_overlay,
            commands::chunks::fetch_chunk_content,
            ingest_document_cmd,
            needs_chunk_hash_migration,
            peek_pending_config_malformed,
            ack_pending_config_malformed,
            vault_write_note,
            vault_upsert_index_entry,
        ])
        .run(tauri::generate_context!())
        .expect("error running Tauri application");
}

#[tauri::command]
async fn start_outbox_worker(
    app_handle: tauri::AppHandle,
    database_url: Option<String>,
    poll_interval_ms: Option<u64>,
    batch_size: Option<usize>,
    on_error: Option<String>,
    state: tauri::State<'_, OutboxWorkerState>,
) -> Result<(), String> {
    let db_url = validate_outbox_database_url(database_url)?;

    let sqlite_path = {
        let db_state = app_handle.state::<DbState>();
        let guard = db_state
            .0
            .lock()
            .map_err(|e| format!("db state lock poisoned: {e}"))?;
        guard
            .0
            .path()
            .ok_or_else(|| "database path unavailable".to_string())
            .map(PathBuf::from)?
    };

    let config = OutboxConfig {
        sqlite_path,
        db_url,
        poll_interval_ms: poll_interval_ms.unwrap_or(5000).clamp(100, 60_000),
        batch_size: batch_size.unwrap_or(100).clamp(1, 10_000),
        on_error: match on_error.as_deref() {
            Some("skip") => outbox::ErrorPolicy::Skip,
            Some("halt") | None => outbox::ErrorPolicy::Halt,
            Some(other) => {
                return Err(format!("unsupported on_error value: {}", other));
            }
        },
        ..OutboxConfig::default()
    };

    // Take the existing handle out of state before any await to avoid
    // holding MutexGuard across await (which violates Send bounds).
    let existing = {
        let mut guard = state.0.lock().unwrap();
        match &*guard {
            Some(handle) if !handle.is_finished() && handle.config == config => return Ok(()),
            Some(_) => guard.take(),
            None => None,
        }
    };
    if let Some(handle) = existing {
        handle.stop().await;
    }

    let _ = replace_outbox_worker(
        &state,
        spawn_postgres_worker(config.clone(), Some(app_handle.clone())),
    )
    .await;

    // Notify frontend that outbox is now active so it can recreate the wiki
    // with enableOutbox: true for runtime worker starts.
    let _ = app_handle.emit("outbox-worker-started", ());

    Ok(())
}

#[tauri::command]
async fn stop_outbox_worker(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, OutboxWorkerState>,
) -> Result<(), String> {
    let handle = {
        let mut guard = state.0.lock().unwrap();
        guard.take()
    };
    if let Some(handle) = handle {
        handle.stop().await;
        let _ = app_handle.emit("outbox-worker-stopped", ());
    }
    Ok(())
}

#[tauri::command]
fn outbox_is_configured(state: tauri::State<'_, OutboxWorkerState>) -> bool {
    state
        .0
        .lock()
        .map(|guard| match &*guard {
            Some(handle) => !handle.is_finished(),
            None => false,
        })
        .unwrap_or(false)
}

#[derive(serde::Serialize)]
struct PrivacyStatePayload {
    mode: &'static str,
    chosen: bool,
    needs_migration_disclosure: bool,
    ephemeral_disclosure_acknowledged: bool,
}

fn privacy_mode_label(mode: privacy::PrivacyMode) -> &'static str {
    match mode {
        privacy::PrivacyMode::Strict => "strict",
        privacy::PrivacyMode::Ephemeral => "ephemeral",
        privacy::PrivacyMode::Connected => "connected",
    }
}

fn privacy_state_payload(state: privacy::PrivacyState) -> PrivacyStatePayload {
    PrivacyStatePayload {
        mode: privacy_mode_label(state.mode),
        chosen: state.chosen,
        needs_migration_disclosure: state.needs_migration_disclosure,
        ephemeral_disclosure_acknowledged: state.ephemeral_disclosure_acknowledged,
    }
}

fn parse_privacy_mode(mode: &str) -> Result<privacy::PrivacyMode, String> {
    match mode {
        "strict" => Ok(privacy::PrivacyMode::Strict),
        "ephemeral" => Ok(privacy::PrivacyMode::Ephemeral),
        "connected" => Ok(privacy::PrivacyMode::Connected),
        _ => Err(format!("unknown privacy mode: {mode}")),
    }
}

#[tauri::command]
fn get_privacy_mode() -> Result<PrivacyStatePayload, String> {
    let brain_dir = PathBuf::from(get_brain_dir_inner());
    let state = privacy::resolve_privacy_state(
        &brain_dir,
        &cloud_bridge::pairing::KeyringPairingTokenStore,
    )
    .map_err(|e| e.to_string())?;
    Ok(privacy_state_payload(state))
}

#[derive(serde::Serialize)]
struct SetPrivacyModeResult {
    disconnected_bridge: bool,
    state: PrivacyStatePayload,
}

#[tauri::command]
async fn set_privacy_mode(
    mode: String,
    state: tauri::State<'_, CloudBridgeState>,
    lifecycle: tauri::State<'_, CloudBridgeLifecycle>,
    app: tauri::AppHandle,
) -> Result<SetPrivacyModeResult, String> {
    let mode = parse_privacy_mode(&mode)?;
    let brain_dir = PathBuf::from(get_brain_dir_inner());
    let _guard = lifecycle.0.lock().await;
    let (privacy_state, disconnected_bridge) = privacy::set_privacy_mode_config(
        &brain_dir,
        mode,
        &cloud_bridge::pairing::KeyringPairingTokenStore,
    )
    .map_err(|e| e.to_string())?;
    if disconnected_bridge {
        let handle = { state.0.lock().unwrap().take() };
        if let Some(handle) = handle {
            handle.stop().await;
        }
    } else if privacy::allows_cloud_bridge(privacy_state.mode) {
        start_cloud_bridge_if_configured_unlocked(&state).await;
    }
    let payload = privacy_state_payload(privacy_state);
    let _ = app.emit("privacy-mode-changed", &payload);
    Ok(SetPrivacyModeResult {
        disconnected_bridge,
        state: payload,
    })
}

#[tauri::command]
fn acknowledge_migration_disclosure() -> Result<(), String> {
    let brain_dir = PathBuf::from(get_brain_dir_inner());
    privacy::acknowledge_migration_disclosure(&brain_dir).map_err(|e| e.to_string())
}

#[tauri::command]
fn acknowledge_ephemeral_disclosure() -> Result<(), String> {
    let brain_dir = PathBuf::from(get_brain_dir_inner());
    privacy::acknowledge_ephemeral_disclosure(&brain_dir).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_ontology_selection() -> Result<String, String> {
    use crate::ontology_config::OntologySelection;
    let paths = retrieval::resolve_brain_paths();
    let report = config::BrainConfig::load_lenient(&paths).map_err(|e| e.to_string())?;
    // An unparseable `ontology` block (e.g. `{"schema":"unknown"}`) must
    // surface as an error, not fall back to the desktop default — that
    // fallback is reserved for "never chosen" (the field genuinely absent).
    // Masking a parse failure here would silently start the General
    // ontology when config.json actually names something else.
    if report.ontology_unparseable {
        return Err(format!(
            "ontology selection in config.json is invalid: {}",
            report
                .diagnostics
                .iter()
                .find(|d| d.starts_with("ontology block unparseable"))
                .cloned()
                .unwrap_or_else(|| "unparseable ontology block".to_string())
        ));
    }
    let selection = report
        .config
        .ontology
        .schema
        // Never chosen → the Desktop default. The CLI writes its own default
        // during --onboard, so an absent value here means a Desktop-first vault.
        .unwrap_or(OntologySelection::DESKTOP_DEFAULT);
    serde_json::to_value(selection)
        .map_err(|e| e.to_string())?
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "ontology selection did not serialize to a string".to_string())
}

#[tauri::command]
fn set_ontology_selection(selection: String) -> Result<(), String> {
    use crate::ontology_config::OntologySelection;
    let parsed: OntologySelection =
        serde_json::from_value(serde_json::Value::String(selection.clone()))
            .map_err(|_| format!("unknown ontology selection: {selection}"))?;

    let paths = retrieval::resolve_brain_paths();
    let mut cfg = config::BrainConfig::load(&paths).map_err(|e| e.to_string())?;
    cfg.ontology.schema = Some(parsed);
    cfg.write(&paths).map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(serde::Serialize)]
struct PendingLinkPayload {
    link: String,
    target: String,
}

#[tauri::command]
fn list_pending_links() -> Result<Vec<PendingLinkPayload>, String> {
    let paths = retrieval::resolve_brain_paths();
    // Use the lenient loader so a malformed-but-mine unrelated block
    // (e.g. `generation` missing after an upstream provider rename) does
    // not block the remediation surface. The walker only needs
    // `vault_path` + `trusted_links`, both of which `load_lenient`
    // populates with sensible fallbacks (`*_missing = true` if absent)
    // and returns in the same shape.
    let report = config::BrainConfig::load_lenient(&paths).map_err(|e| e.to_string())?;
    let cfg = report.config;
    let vault_root = cfg
        .vault_path
        .clone()
        .ok_or_else(|| "no vault configured".to_string())?;

    let outcome = walk_vault::walk_vault(
        std::path::Path::new(&vault_root),
        &cfg.trusted_links,
        dirs::home_dir().as_deref(),
    );
    Ok(outcome
        .pending
        .into_iter()
        .map(|p| PendingLinkPayload {
            link: p.link,
            target: p.target,
        })
        .collect())
}

#[tauri::command]
fn approve_link(link: String) -> Result<(), String> {
    use crate::trusted_links::{approve_into, LinkVerdict};

    let paths = retrieval::resolve_brain_paths();
    let mut cfg = config::BrainConfig::load(&paths).map_err(|e| e.to_string())?;
    let vault_root = std::path::PathBuf::from(
        cfg.vault_path
            .clone()
            .ok_or_else(|| "no vault configured".to_string())?,
    );
    // Canonicalize the vault root so `classify_link` compares both sides on
    // matching prefixes. Without this, a vault reached through a symlink
    // would let a link to its physical parent pass as Pending instead of
    // being refused as a vault ancestor.
    let vault_root = std::fs::canonicalize(&vault_root).unwrap_or(vault_root);

    let verdict = approve_into(
        &mut cfg.trusted_links,
        &link,
        &vault_root,
        dirs::home_dir().as_deref(),
    )?;
    match verdict {
        // In-vault (Trusted) and newly-approved (Pending) both produce a
        // possibly-changed ledger, so persist either way. The helper
        // already skipped the push for Trusted (spec D3a — vault
        // containment is the trust boundary), so this write may be a
        // no-op for the in-vault case.
        LinkVerdict::Denied(reason) => Err(format!("refused: {}", reason.message())),
        LinkVerdict::Trusted | LinkVerdict::Pending => cfg.write(&paths).map_err(|e| e.to_string()),
    }
}

#[tauri::command]
fn revoke_link(link: String) -> Result<(), String> {
    let paths = retrieval::resolve_brain_paths();
    let mut cfg = config::BrainConfig::load(&paths).map_err(|e| e.to_string())?;
    cfg.trusted_links.retain(|e| e.link != link);
    cfg.write(&paths).map_err(|e| e.to_string())
}

#[tauri::command]
async fn set_cloud_bridge_pairing_token(
    token: String,
    state: tauri::State<'_, CloudBridgeState>,
    lifecycle: tauri::State<'_, CloudBridgeLifecycle>,
) -> Result<(), String> {
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err("pairing token cannot be empty".to_string());
    }
    let brain_dir = PathBuf::from(get_brain_dir_inner());
    let privacy_state = privacy::resolve_privacy_state(
        &brain_dir,
        &cloud_bridge::pairing::KeyringPairingTokenStore,
    )
    .map_err(|e| e.to_string())?;
    if !privacy::allows_cloud_bridge(privacy_state.mode) {
        return Err("Cloud Bridge is only available in Connected agent privacy mode".to_string());
    }
    let _guard = lifecycle.0.lock().await;
    cloud_bridge::pairing::KeyringPairingTokenStore
        .set(&token)
        .map_err(|e| e.to_string())?;
    start_cloud_bridge_if_configured_unlocked(&state).await;
    Ok(())
}

#[tauri::command]
async fn clear_cloud_bridge_pairing_token(
    state: tauri::State<'_, CloudBridgeState>,
    lifecycle: tauri::State<'_, CloudBridgeLifecycle>,
) -> Result<(), String> {
    let _guard = lifecycle.0.lock().await;
    cloud_bridge::pairing::KeyringPairingTokenStore
        .delete()
        .map_err(|e| e.to_string())?;
    let handle = { state.0.lock().unwrap().take() };
    if let Some(handle) = handle {
        handle.stop().await;
    }
    Ok(())
}

#[derive(serde::Serialize)]
struct CloudBridgeStatusPayload {
    configured: bool,
    connection_status: &'static str,
}

#[tauri::command]
fn retry_cloud_bridge_now(state: tauri::State<'_, CloudBridgeState>) -> Result<(), String> {
    let guard = state.0.lock().unwrap();
    let Some(handle) = guard.as_ref() else {
        return Err("cloud bridge is not running".to_string());
    };
    handle.retry_now();
    Ok(())
}

#[tauri::command]
fn get_cloud_bridge_status(state: tauri::State<'_, CloudBridgeState>) -> CloudBridgeStatusPayload {
    let guard = state.0.lock().unwrap();
    match guard.as_ref() {
        Some(handle) => CloudBridgeStatusPayload {
            configured: true,
            connection_status: match handle.status() {
                cloud_bridge::ConnectionStatus::Disconnected => "disconnected",
                cloud_bridge::ConnectionStatus::Connecting => "connecting",
                cloud_bridge::ConnectionStatus::Authenticating => "authenticating",
                cloud_bridge::ConnectionStatus::Connected => "connected",
                cloud_bridge::ConnectionStatus::Reconnecting => "reconnecting",
                cloud_bridge::ConnectionStatus::AuthRejected => "auth_rejected",
            },
        },
        None => CloudBridgeStatusPayload {
            configured: cloud_bridge_is_pairing_configured(),
            connection_status: "disconnected",
        },
    }
}

#[cfg(test)]
mod normalize_path_tests {
    use super::normalize_path_argument_to_vault_relative;
    use std::fs;
    use std::path::Path;

    #[test]
    fn absolute_path_inside_vault_nonexistent_target_normalizes_without_canonicalize() {
        let tmp = tempfile::TempDir::new().unwrap();
        let vault = tmp.path();
        fs::create_dir_all(vault.join("documents")).unwrap();
        let canon = vault.canonicalize().unwrap();
        let missing = canon.join("documents").join("not_created_yet.md");
        let rel =
            normalize_path_argument_to_vault_relative(&missing.to_string_lossy(), vault).unwrap();
        assert_eq!(rel, "documents/not_created_yet.md");
    }

    /// When the configured vault path and an absolute argument use different spellings for the
    /// same directory (symlink), normalization must still produce a vault-relative path.
    #[test]
    #[cfg(unix)]
    fn absolute_path_normalizes_when_vault_is_symlink_alias() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::TempDir::new().unwrap();
        let real_vault = tmp.path().join("real_vault");
        fs::create_dir_all(real_vault.join("documents")).unwrap();
        let link = tmp.path().join("link_vault");
        symlink(&real_vault, &link).unwrap();

        let file = real_vault.join("documents").join("note.md");
        fs::write(&file, b"x").unwrap();

        let abs_via_real = file.canonicalize().unwrap();
        let rel = normalize_path_argument_to_vault_relative(
            &abs_via_real.to_string_lossy(),
            Path::new(&link),
        )
        .unwrap();
        assert_eq!(rel, "documents/note.md");
    }
}

#[cfg(test)]
mod heal_invalid_sources_tests {
    use super::{heal_invalid_sources, DbState, VaultConfigState};
    use crate::db::AppDb;
    use crate::vault::VaultConfig;
    use rusqlite::params;
    use std::sync::Mutex;
    use tempfile::TempDir;

    #[test]
    fn missing_vault_sources_are_marked_deleted() {
        let tmp = TempDir::new().unwrap();
        let vault_root = tmp.path().join("vault");
        std::fs::create_dir_all(vault_root.join("documents")).unwrap();

        let config = VaultConfig::new(tmp.path().join("config.json"));
        config.set_vault_path(vault_root.to_str().unwrap()).unwrap();

        let db_path = tmp.path().join("brain.db");
        let db = AppDb::open_with_config(&db_path, tmp.path().join("config.json")).unwrap();
        let db_state = DbState(Mutex::new(db));
        let vault_state = VaultConfigState(Mutex::new(config));

        {
            let guard = db_state.0.lock().unwrap();
            let conn = &guard.0;
            conn.execute(
                "INSERT INTO llm_wiki_entries (
                    id, entity_id, title, body, tags, confidence, source_type, source_ref,
                    created_at, updated_at, deleted_at
                 ) VALUES (?1, ?2, ?3, ?4, '[]', 'inferred', 'librarian_inferred', ?5, 1, 1, NULL)",
                params![
                    "entry-missing",
                    "tier_fact",
                    "Missing",
                    "body",
                    "documents/missing.md"
                ],
            )
            .unwrap();

            let before: Option<i64> = conn
                .query_row(
                    "SELECT deleted_at FROM llm_wiki_entries WHERE rowid = 1",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(before.is_none());
        }

        heal_invalid_sources(&db_state, &vault_state).unwrap();

        let after: Option<i64> = db_state
            .0
            .lock()
            .unwrap()
            .0
            .query_row(
                "SELECT deleted_at FROM llm_wiki_entries WHERE rowid = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(after.is_some());
    }
}

#[cfg(test)]
mod drop_destination_tests {
    use super::drop_destination_filename;
    use super::unique_drop_destination;
    use std::fs;

    #[test]
    fn drop_destination_filename_zero_is_unchanged() {
        assert_eq!(drop_destination_filename("note.md", 0), "note.md");
    }

    #[test]
    fn drop_destination_filename_inserts_counter_before_extension() {
        assert_eq!(drop_destination_filename("note.md", 1), "note (1).md");
        assert_eq!(drop_destination_filename("note.md", 2), "note (2).md");
    }

    #[test]
    fn unique_drop_uses_original_when_unused() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(crate::vault::safe_path::IMMUTABLE_DIR)).unwrap();
        let p = unique_drop_destination(root, "fresh.txt").unwrap();
        assert_eq!(p.file_name().and_then(|n| n.to_str()), Some("fresh.txt"));
    }

    #[test]
    fn unique_drop_avoids_overwriting_existing_basename() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(crate::vault::safe_path::IMMUTABLE_DIR)).unwrap();
        fs::write(
            root.join(crate::vault::safe_path::IMMUTABLE_DIR)
                .join("dup.md"),
            b"x",
        )
        .unwrap();
        let p = unique_drop_destination(root, "dup.md").unwrap();
        assert_eq!(p.file_name().and_then(|n| n.to_str()), Some("dup (1).md"));
    }

    #[test]
    fn unique_drop_skips_directory_with_same_basename() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(crate::vault::safe_path::IMMUTABLE_DIR)).unwrap();
        fs::create_dir_all(
            root.join(crate::vault::safe_path::IMMUTABLE_DIR)
                .join("dup.md"),
        )
        .unwrap();
        let p = unique_drop_destination(root, "dup.md").unwrap();
        assert_eq!(p.file_name().and_then(|n| n.to_str()), Some("dup (1).md"));
    }
}

#[cfg(test)]
mod outbox_runtime_tests {
    use super::{replace_outbox_worker, validate_outbox_database_url, OutboxWorkerState};
    use crate::outbox::postgres::dummy_outbox_handle;
    use std::sync::Mutex;

    #[tokio::test]
    async fn validate_outbox_database_url_rejects_empty_string() {
        let err = validate_outbox_database_url(Some("  ".to_string())).unwrap_err();
        assert_eq!(err, "database_url cannot be empty");
    }

    #[tokio::test]
    async fn replace_outbox_worker_stops_existing_handle_before_inserting_new_one() {
        let state = OutboxWorkerState(Mutex::new(Some(dummy_outbox_handle())));
        let replaced = replace_outbox_worker(&state, dummy_outbox_handle()).await;

        assert!(replaced);
        assert!(state.0.lock().unwrap().is_some());
    }
}

#[cfg(test)]
mod workspace_id_tests {
    use super::get_workspace_id;

    #[test]
    fn has_tier_working_prefix() {
        let id = get_workspace_id("/Users/foo/Vault".to_string());
        assert!(id.starts_with("tier_working::"), "got: {id}");
    }

    #[test]
    fn hash_segment_is_16_lowercase_hex_chars() {
        let id = get_workspace_id("/Users/foo/Vault".to_string());
        let hash = id.strip_prefix("tier_working::").unwrap();
        assert_eq!(
            hash.len(),
            16,
            "hash segment should be 16 chars, got: {hash}"
        );
        assert!(
            hash.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
            "hash should be lowercase hex, got: {hash}"
        );
    }

    #[test]
    fn is_deterministic() {
        assert_eq!(
            get_workspace_id("/Users/foo/Vault".to_string()),
            get_workspace_id("/Users/foo/Vault".to_string())
        );
    }

    #[test]
    fn normalizes_trailing_slashes_and_windows_paths() {
        assert_eq!(
            get_workspace_id("/Users/foo/Vault".to_string()),
            get_workspace_id("/Users/foo/Vault/".to_string())
        );
        assert_eq!(
            get_workspace_id("C:\\Users\\foo\\Vault".to_string()),
            get_workspace_id("C:/Users/foo/Vault".to_string())
        );
    }

    #[test]
    fn different_vaults_produce_different_ids() {
        assert_ne!(
            get_workspace_id("/Users/foo/VaultA".to_string()),
            get_workspace_id("/Users/foo/VaultB".to_string())
        );
    }
}

#[cfg(test)]
mod maintenance_command_tests {
    use super::{
        heal_invalid_sources, heal_lost_librarian_inferred, prune_old_librarian_inferred,
        run_embedding_sweep, DbState, VaultConfigState,
    };
    use crate::db::connection::open_in_memory;
    use rusqlite::{params, Connection};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use tempfile::TempDir;

    fn insert_wiki_entry(
        conn: &Connection,
        id: &str,
        source_type: &str,
        source_ref: &str,
        deleted_at: Option<i64>,
    ) {
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type, source_ref,
                created_at, updated_at, deleted_at
             ) VALUES (?1, 'tier_fact', ?2, 'body', '[]', 'inferred', ?3, ?4, 1, 1, ?5)",
            params![
                id,
                format!("Title {id}"),
                source_type,
                source_ref,
                deleted_at
            ],
        )
        .unwrap();
    }

    /// Like `insert_wiki_entry`, but names the unit explicitly: `deleted_at` is
    /// MILLISECONDS, matching every production writer (`ms_now()`).
    fn insert_wiki_entry_ms(
        conn: &Connection,
        id: &str,
        source_type: &str,
        source_ref: &str,
        deleted_at_ms: Option<i64>,
    ) {
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                source_hash, source_ref, created_at, updated_at, last_accessed_at,
                access_count, deleted_at, embedding_blob, embedding
             ) VALUES (?1, 'ent-1', 'T', 'B', '[]', 'inferred', ?2,
                       NULL, ?3, 100, 100, NULL, 0, ?4, NULL, NULL)",
            params![id, source_type, source_ref, deleted_at_ms],
        )
        .unwrap();
    }

    #[test]
    fn prune_deletes_old_rows_when_deleted_at_is_milliseconds() {
        let conn = open_in_memory().unwrap();
        let now_ms: i64 = 1_756_000_000_000; // a plausible 2026 millisecond stamp
        let eight_days_ms = 8 * 86_400 * 1000;
        let one_day_ms = 86_400 * 1000;

        // Production writers store `deleted_at` in MILLISECONDS (ms_now()).
        insert_wiki_entry_ms(
            &conn,
            "old-inferred",
            "librarian_inferred",
            "documents/old.md",
            Some(now_ms - eight_days_ms),
        );
        insert_wiki_entry_ms(
            &conn,
            "fresh-inferred",
            "librarian_inferred",
            "documents/fresh.md",
            Some(now_ms - one_day_ms),
        );
        insert_wiki_entry_ms(
            &conn,
            "old-immutable",
            "immutable_document",
            "documents/immutable.md",
            Some(now_ms - eight_days_ms),
        );

        let deleted = prune_old_librarian_inferred(&conn, now_ms).unwrap();

        assert_eq!(
            deleted, 1,
            "only the old librarian_inferred row should be deleted"
        );
        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_entries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(remaining, 2);
        let old_gone: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_entries WHERE id = 'old-inferred'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(old_gone, 0);
    }

    #[test]
    fn prune_only_removes_old_librarian_inferred_rows() {
        let conn = open_in_memory().unwrap();

        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        let old = now - Duration::from_secs(7 * 86400 + 1);
        let fresh = now - Duration::from_secs(7 * 86400 - 1);

        insert_wiki_entry_ms(
            &conn,
            "old-inferred",
            "librarian_inferred",
            "documents/old.md",
            Some(old.as_millis() as i64),
        );
        insert_wiki_entry_ms(
            &conn,
            "fresh-inferred",
            "librarian_inferred",
            "documents/fresh.md",
            Some(fresh.as_millis() as i64),
        );
        insert_wiki_entry_ms(
            &conn,
            "old-immutable",
            "immutable_document",
            "documents/immutable.md",
            Some(old.as_millis() as i64),
        );

        let deleted = prune_old_librarian_inferred(&conn, now.as_millis() as i64).unwrap();
        assert_eq!(
            deleted, 1,
            "only the old librarian_inferred row should be deleted"
        );

        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_entries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(remaining, 2, "immutable or fresh rows must remain");

        let types: Vec<String> = conn
            .prepare("SELECT source_type FROM llm_wiki_entries ORDER BY source_type")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(
            types,
            vec![
                "immutable_document".to_string(),
                "librarian_inferred".to_string()
            ]
        );
    }

    // ── Spec section 6: heal_unit_contracts (E1-E5) ──────────────────────────
    // The original `heal_soft_deletes_missing_librarian_inferred_entries_only`
    // test (Bug A pre-fix) is replaced here. Its three-row setup (existing,
    // missing, immutable) is preserved across E1 + E2 + E3, with the new
    // contract: every assertion must reflect the JSON `source_ref` shape and
    // the `librarian_inferred` filter, not the legacy path check.

    /// E1 — JSON source_ref with at least one live chunk → preserved (no
    /// soft-delete). Refactor of the existing test's "existing inferred
    /// row stays alive" assertion under the new JSON contract.
    #[test]
    fn e1_heal_lost_librarian_inferred_preserves_json_source_refs_with_alive_chunk() {
        let conn = open_in_memory().unwrap();

        // Seed a document + chunk so `source_ref_is_still_grounded` finds
        // at least one live chunk_id.
        conn.execute(
            "INSERT INTO documents (path, hash, tier, status)
             VALUES ('documents/notes.md', 'h', 'user_doc', 'indexed')",
            [],
        )
        .unwrap();
        let doc_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO chunks (doc_id, chunk_text, position, start_line, end_line,
                                 symbol_name, strategy, content_hash)
             VALUES (?1, 'chunk text', 0, 1, 3, NULL, 'prose', 'h_alive')",
            [doc_id],
        )
        .unwrap();
        let chunk_id = conn.last_insert_rowid();

        let live_json = format!(
            r#"{{"proposal_id":"prop_alive","evidence":[{{"chunk_id"{chunk_id},"content_hash":"h_alive","quote":"q","start_line":1,"end_line":3,"source_kind":"prose"}}]}}"#,
            chunk_id = chunk_id,
        );
        insert_wiki_entry(&conn, "json-live", "librarian_inferred", &live_json, None);

        let tmp = tempfile::TempDir::new().unwrap();
        let updated = heal_lost_librarian_inferred(&conn, tmp.path()).unwrap();
        assert_eq!(
            updated, 0,
            "JSON-shaped source_ref with a live chunk must be preserved"
        );

        let deleted_at: Option<i64> = conn
            .query_row(
                "SELECT deleted_at FROM llm_wiki_entries WHERE id = 'json-live'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            deleted_at.is_none(),
            "live JSON source_ref must NOT have deleted_at set"
        );
    }

    /// E2 — JSON source_ref whose chunk is gone from `chunks` → soft-delete.
    /// Refactor of the existing test's "missing inferred row gets deleted"
    /// assertion under the new JSON contract.
    #[test]
    fn e2_heal_lost_librarian_inferred_soft_deletes_json_source_refs_with_all_dead_chunks() {
        let conn = open_in_memory().unwrap();

        conn.execute(
            "INSERT INTO documents (path, hash, tier, status)
             VALUES ('documents/gone.md', 'h', 'user_doc', 'indexed')",
            [],
        )
        .unwrap();
        let doc_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO chunks (doc_id, chunk_text, position, start_line, end_line,
                                 symbol_name, strategy, content_hash)
             VALUES (?1, 'gone text', 0, 1, 3, NULL, 'prose', 'h_gone')",
            [doc_id],
        )
        .unwrap();
        let chunk_id = conn.last_insert_rowid();

        let dead_json = format!(
            r#"{{"proposal_id":"prop_dead","evidence":[{{"chunk_id":{chunk_id},"content_hash":"h_gone","quote":"q","start_line":1,"end_line":3,"source_kind":"prose"}}]}}"#
        );
        insert_wiki_entry(&conn, "json-dead", "librarian_inferred", &dead_json, None);

        // Kill the chunk — simulate vault-pruning.
        conn.execute("DELETE FROM chunks WHERE id = ?1", [chunk_id])
            .unwrap();

        let tmp = tempfile::TempDir::new().unwrap();
        let updated = heal_lost_librarian_inferred(&conn, tmp.path()).unwrap();
        assert_eq!(
            updated, 1,
            "JSON source_ref whose chunk is gone must be soft-deleted"
        );

        let deleted_at: Option<i64> = conn
            .query_row(
                "SELECT deleted_at FROM llm_wiki_entries WHERE id = 'json-dead'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            deleted_at.is_some(),
            "dead JSON source_ref must be marked deleted"
        );
    }

    /// E3 — `heal_invalid_sources` is scoped to `librarian_inferred`. The
    /// watcher fires this on every `VaultEvent::Deleted`; without the
    /// source_type filter it would wipe `user_stated` rows (whose source_ref
    /// is the `MANUAL_SOURCE_REF` JSON sentinel — which `safe_vault_path`
    /// would also reject) and `immutable_document` rows.
    #[test]
    fn e3_heal_invalid_sources_is_scoped_to_librarian_inferred() {
        let tmp = tempfile::TempDir::new().unwrap();
        let vault_root = tmp.path();
        std::fs::create_dir_all(vault_root.join("documents")).unwrap();

        // Build a DbState backed by a file DB and seed three rows: one
        // librarian_inferred (must be soft-deleted), one user_stated
        // (must NOT be touched), one immutable_document (must NOT be
        // touched). The heal function takes &DbState directly so we seed
        // against the AppDb's connection rather than an in-memory one.
        let db_path = tmp.path().join("test.db");
        let db = crate::db::AppDb::open_with_config(&db_path, tmp.path().join("config.json"))
            .expect("open test db");
        let manual = r#"{"proposal_id":null,"evidence":[]}"#;
        // Three rows: one librarian_inferred (must be touched), one
        // user_stated with the MANUAL sentinel (must NOT be touched), one
        // immutable_document with a missing path (must NOT be touched).
        db.0.execute(
            "INSERT INTO llm_wiki_entries
                (id, entity_id, title, body, tags, confidence, source_type, source_ref,
                 created_at, updated_at, deleted_at)
             VALUES (?1, 'tier_fact', 'Title lost-inferred', 'body', '[]',
                     'inferred', 'librarian_inferred', 'documents/gone.md', 1, 1, NULL)",
            rusqlite::params!["lost-inferred"],
        )
        .expect("insert librarian_inferred");
        db.0.execute(
            "INSERT INTO llm_wiki_entries
                (id, entity_id, title, body, tags, confidence, source_type, source_ref,
                 created_at, updated_at, deleted_at)
             VALUES (?1, 'tier_fact', 'Title user-stated', 'body', '[]',
                     'confirmed', 'user_stated', ?2, 1, 1, NULL)",
            rusqlite::params!["user-stated", manual],
        )
        .expect("insert user_stated");
        db.0.execute(
            "INSERT INTO llm_wiki_entries
                (id, entity_id, title, body, tags, confidence, source_type, source_ref,
                 created_at, updated_at, deleted_at)
             VALUES (?1, 'tier_fact', 'Title immutable', 'body', '[]',
                     'inferred', 'immutable_document', 'documents/gone.md', 1, 1, NULL)",
            rusqlite::params!["immutable"],
        )
        .expect("insert immutable_document");

        let cfg = crate::vault::VaultConfig::new(tmp.path().join("config.json"));
        cfg.set_vault_path(&vault_root.to_string_lossy())
            .expect("set vault");
        let db_state = DbState(std::sync::Mutex::new(db));
        let vault_state = VaultConfigState(std::sync::Mutex::new(cfg));

        heal_invalid_sources(&db_state, &vault_state).expect("heal_invalid_sources");

        let deleted_at_lost: Option<i64> = db_state
            .0
            .lock()
            .unwrap()
            .0
            .query_row(
                "SELECT deleted_at FROM llm_wiki_entries WHERE id = 'lost-inferred'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            deleted_at_lost.is_some(),
            "lost-inferred (librarian_inferred) must be soft-deleted by heal_invalid_sources"
        );
        for id in ["user-stated", "immutable"] {
            let deleted_at: Option<i64> = db_state
                .0
                .lock()
                .unwrap()
                .0
                .query_row(
                    "SELECT deleted_at FROM llm_wiki_entries WHERE id = ?1",
                    [id],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(
                deleted_at.is_none(),
                "{id} (non-librarian_inferred) must NOT be touched by heal_invalid_sources"
            );
        }
    }

    /// E4 — Both heal writers write `deleted_at` in milliseconds (post-fix).
    /// Direct regression test for Bug C: assert the value is at least
    /// `SEC_VS_MS_THRESHOLD` (any ms timestamp after 2001-09-09), so a
    /// regressed seconds-valued writer (≈1.7e9) would fail loudly.
    #[test]
    fn e4_heal_writers_set_deleted_at_in_milliseconds() {
        use crate::db::schema::SEC_VS_MS_THRESHOLD;

        // (a) heal_lost_librarian_inferred
        let conn = open_in_memory().unwrap();
        insert_wiki_entry(
            &conn,
            "target-lost",
            "librarian_inferred",
            "documents/missing.md",
            None,
        );
        let tmp = tempfile::TempDir::new().unwrap();
        heal_lost_librarian_inferred(&conn, tmp.path()).unwrap();
        let deleted_at_lost: i64 = conn
            .query_row(
                "SELECT deleted_at FROM llm_wiki_entries WHERE id = 'target-lost'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            deleted_at_lost >= SEC_VS_MS_THRESHOLD,
            "heal_lost_librarian_inferred wrote seconds (deleted_at={deleted_at_lost})"
        );

        // (b) heal_invalid_sources
        let db_path = tmp.path().join("e4.db");
        let db = crate::db::AppDb::open_with_config(&db_path, tmp.path().join("e4-config.json"))
            .expect("open test db");
        db.0.execute(
            "INSERT INTO llm_wiki_entries
                (id, entity_id, title, body, tags, confidence, source_type, source_ref,
                 created_at, updated_at, deleted_at)
             VALUES ('target-invalid', 'tier_fact', 'Title target-invalid', 'body',
                     '[]', 'inferred', 'librarian_inferred', 'documents/missing.md',
                     1, 1, NULL)",
            [],
        )
        .expect("insert target-invalid");
        let cfg = crate::vault::VaultConfig::new(tmp.path().join("e4-config.json"));
        cfg.set_vault_path(&tmp.path().to_string_lossy())
            .expect("set vault");
        let db_state = DbState(std::sync::Mutex::new(db));
        let vault_state = VaultConfigState(std::sync::Mutex::new(cfg));
        heal_invalid_sources(&db_state, &vault_state).expect("heal_invalid_sources");
        let deleted_at_invalid: Option<i64> = db_state
            .0
            .lock()
            .unwrap()
            .0
            .query_row(
                "SELECT deleted_at FROM llm_wiki_entries WHERE id = 'target-invalid'",
                [],
                |row| row.get(0),
            )
            .expect("query target-invalid");
        assert!(
            deleted_at_invalid.is_some_and(|v| v >= SEC_VS_MS_THRESHOLD),
            "heal_invalid_sources wrote seconds or NULL (deleted_at={deleted_at_invalid:?})"
        );
    }

    /// E5 — MIGRATION_V12 mix-and-idempotency: a row whose `deleted_at` is
    /// in seconds (the only known bad unit) gets multiplied; an already-ms
    /// row is left alone; a second run is a no-op.
    #[test]
    fn e5_v12_promotes_seconds_leaves_ms_alone_and_is_idempotent() {
        use crate::db::schema::SEC_VS_MS_THRESHOLD;

        let conn = open_in_memory().unwrap();
        insert_wiki_entry(
            &conn,
            "secs",
            "librarian_inferred",
            "documents/gone.md",
            Some(1_700_000_000),
        );
        insert_wiki_entry(
            &conn,
            "ms",
            "user_stated",
            r#"{"proposal_id":null,"evidence":[]}"#,
            Some(SEC_VS_MS_THRESHOLD + 60_000),
        );

        conn.execute_batch(&format!(
            "BEGIN;\n{}\nCOMMIT;",
            crate::db::schema::MIGRATION_V12
        ))
        .unwrap();
        let secs_after: i64 = conn
            .query_row(
                "SELECT deleted_at FROM llm_wiki_entries WHERE id = 'secs'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let ms_after: i64 = conn
            .query_row(
                "SELECT deleted_at FROM llm_wiki_entries WHERE id = 'ms'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(secs_after, 1_700_000_000 * 1000, "seconds row promoted");
        assert_eq!(ms_after, SEC_VS_MS_THRESHOLD + 60_000, "ms row untouched");

        // Idempotency: re-run and re-check.
        conn.execute_batch(&format!(
            "BEGIN;\n{}\nCOMMIT;",
            crate::db::schema::MIGRATION_V12
        ))
        .unwrap();
        let secs_after2: i64 = conn
            .query_row(
                "SELECT deleted_at FROM llm_wiki_entries WHERE id = 'secs'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let ms_after2: i64 = conn
            .query_row(
                "SELECT deleted_at FROM llm_wiki_entries WHERE id = 'ms'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            secs_after2,
            1_700_000_000 * 1000,
            "second run is no-op for secs"
        );
        assert_eq!(
            ms_after2,
            SEC_VS_MS_THRESHOLD + 60_000,
            "second run is no-op for ms"
        );
    }

    #[test]
    fn prune_purges_edges_of_hard_deleted_entries() {
        let conn = open_in_memory().unwrap();
        let now_ms: i64 = 1_756_000_000_000;
        let eight_days_ms = 8 * 86_400 * 1000;

        insert_wiki_entry_ms(
            &conn,
            "old-inferred",
            "librarian_inferred",
            "documents/old.md",
            Some(now_ms - eight_days_ms),
        );
        // R1 (remediation): the new heterogeneous contract only purges edges
        // whose partner is also dead in every endpoint table. Soft-delete
        // live-a (with a recent timestamp so it is NOT prune-eligible —
        // prune only matches `deleted_at < cutoff`) so the cascade treats
        // edge_doomed as purgeable. Without this, edge_doomed would survive
        // because live-a remains alive in `llm_wiki_entries`.
        insert_wiki_entry_ms(
            &conn,
            "live-a",
            "librarian_inferred",
            "documents/a.md",
            Some(now_ms),
        );
        insert_wiki_entry_ms(
            &conn,
            "live-b",
            "librarian_inferred",
            "documents/b.md",
            None,
        );

        conn.execute(
            "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
             VALUES ('edge_doomed', 'ent-1', 'old-inferred', 'live-a', 'related_to', 100)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
             VALUES ('edge_survivor', 'ent-1', 'live-a', 'live-b', 'related_to', 100)",
            [],
        )
        .unwrap();

        let deleted = prune_old_librarian_inferred(&conn, now_ms).unwrap();
        assert_eq!(deleted, 1, "only old-inferred is older than 7 days");

        let remaining: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT id FROM llm_wiki_edges ORDER BY id")
                .unwrap();
            let rows = stmt.query_map([], |r| r.get(0)).unwrap();
            rows.collect::<rusqlite::Result<Vec<String>>>().unwrap()
        };
        assert_eq!(
            remaining,
            vec!["edge_survivor".to_string()],
            "the hard-deleted entry's edge must go, the live one must stay"
        );
    }

    /// R7: each hard-deleted `librarian_inferred` row must produce exactly
    /// one `llm_wiki_outbox` row with `operation = 'Delete'`, keyed by the
    /// doomed row's entity_id (so replicas converge on the prune). Rows that
    /// are too fresh, or that are not `librarian_inferred`, must NOT emit
    /// outbox rows.
    #[test]
    fn prune_old_librarian_inferred_pushes_delete_outbox_per_pruned_id() {
        let conn = open_in_memory().unwrap();
        let now_ms: i64 = 1_756_000_000_000;
        let eight_days_ms = 8 * 86_400 * 1000;
        let one_day_ms = 86_400 * 1000;

        // Two doomed librarian_inferred rows that must each produce an outbox Delete.
        insert_wiki_entry_ms(
            &conn,
            "old-inferred-1",
            "librarian_inferred",
            "documents/old1.md",
            Some(now_ms - eight_days_ms),
        );
        insert_wiki_entry_ms(
            &conn,
            "old-inferred-2",
            "librarian_inferred",
            "documents/old2.md",
            Some(now_ms - eight_days_ms),
        );
        // Fresh librarian_inferred row: must NOT be pruned and must NOT produce an outbox row.
        insert_wiki_entry_ms(
            &conn,
            "fresh-inferred",
            "librarian_inferred",
            "documents/fresh.md",
            Some(now_ms - one_day_ms),
        );
        // Old non-librarian row: must NOT be pruned and must NOT produce an outbox row.
        insert_wiki_entry_ms(
            &conn,
            "old-immutable",
            "immutable_document",
            "documents/immutable.md",
            Some(now_ms - eight_days_ms),
        );

        let deleted = prune_old_librarian_inferred(&conn, now_ms).unwrap();
        assert_eq!(
            deleted, 2,
            "only the two old librarian_inferred rows should be deleted"
        );

        // Outbox: exactly two entries-table Delete rows, one per pruned id,
        // both carrying entity_id = 'ent-1' (the entity the doomed rows lived
        // in). No rows for the fresh row or the immutable row.
        let outbox_rows: Vec<(String, String, String, String)> = {
            let mut stmt = conn
                .prepare(
                    "SELECT record_id, entity_id, operation, payload
                     FROM llm_wiki_outbox
                     WHERE table_name = 'entries'
                     ORDER BY record_id",
                )
                .unwrap();
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                })
                .unwrap();
            rows.collect::<rusqlite::Result<Vec<_>>>().unwrap()
        };

        assert_eq!(
            outbox_rows.len(),
            2,
            "expected exactly two entries-table outbox rows, got: {outbox_rows:?}"
        );

        let expected: std::collections::BTreeMap<String, (String, String)> = [
            (
                "old-inferred-1".to_string(),
                (
                    "ent-1".to_string(),
                    r#"{"id":"old-inferred-1"}"#.to_string(),
                ),
            ),
            (
                "old-inferred-2".to_string(),
                (
                    "ent-1".to_string(),
                    r#"{"id":"old-inferred-2"}"#.to_string(),
                ),
            ),
        ]
        .into_iter()
        .collect();

        let actual: std::collections::BTreeMap<String, (String, String)> = outbox_rows
            .into_iter()
            .map(|(record_id, entity_id, operation, payload)| {
                assert_eq!(
                    operation, "DELETE",
                    "prune must push DELETE outbox rows, got {operation} for {record_id}"
                );
                (record_id, (entity_id, payload))
            })
            .collect();

        assert_eq!(
            actual, expected,
            "outbox rows must be Delete keyed by doomed id with matching entity_id"
        );

        // Belt-and-braces: the doomed rows are gone, the survivors remain.
        let remaining_ids: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT id FROM llm_wiki_entries ORDER BY id")
                .unwrap();
            let rows = stmt.query_map([], |r| r.get(0)).unwrap();
            rows.collect::<rusqlite::Result<Vec<_>>>().unwrap()
        };
        assert_eq!(
            remaining_ids,
            vec!["fresh-inferred".to_string(), "old-immutable".to_string()],
            "only the pruned rows should be gone; the fresh and immutable rows stay"
        );
    }

    #[test]
    fn heal_lost_librarian_inferred_purges_edges_of_soft_deleted_entries() {
        let conn = open_in_memory().unwrap();
        // An entry whose source_ref points at a document that does not exist
        // in the `documents` table is "lost" and gets soft-deleted by the
        // heal pass. Use a legacy path shape (not the JSON `{"doc_path":...}`
        // shape) because `source_ref_is_still_grounded` treats a JSON shape
        // without an `evidence` field as already-grounded; the legacy path
        // branch is what the heal's "demonstrably stale" policy actually
        // catches in this fixture.
        insert_wiki_entry_ms(
            &conn,
            "lost-entry",
            "librarian_inferred",
            "documents/vanished.md",
            None,
        );
        insert_wiki_entry_ms(
            &conn,
            "live-a",
            "librarian_inferred",
            "documents/a.md",
            None,
        );

        // R1 (remediation): the new heterogeneous contract only purges edges
        // whose partner is also dead in every endpoint table. Soft-delete
        // live-a so the cascade treats edge_doomed as purgeable, and so live-a
        // is excluded from the heal's own SELECT (which filters
        // `deleted_at IS NULL`) — without the exclusion, live-a would also
        // be considered ungrounded and the heal would report `healed = 2`.
        conn.execute(
            "UPDATE llm_wiki_entries SET deleted_at = 100 WHERE id = 'live-a'",
            [],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
             VALUES ('edge_doomed', 'ent-1', 'lost-entry', 'live-a', 'related_to', 100)",
            [],
        )
        .unwrap();

        let healed = heal_lost_librarian_inferred(&conn, std::path::Path::new("/vault")).unwrap();
        assert_eq!(healed, 1, "the ungrounded entry should be soft-deleted");

        let dangling: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_edges
                  WHERE source_id = 'lost-entry' OR target_id = 'lost-entry'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(dangling, 0, "soft-delete must purge edges too (spec §2)");
    }

    // ── Spec section 7: embedding sweep trigger ──────────────────────────────
    // The sweep is the sole recovery path for a write-time embed that failed.
    // `run_embedding_sweep` is wired at startup and on `run_wiki_heal`; this
    // test pins the contract — given two live NULL-blob entries, it returns
    // `SweepReport { filled: 2, remaining_null: 0 }` and writes 32-byte blobs
    // (8 dims from the `constant8` stub).

    /// Build a `DbState` backed by an in-memory-style file DB at `tmp`, then
    /// point the resolver env vars at that temp dir so `run_embedding_sweep`
    /// finds the same config. The stub (set by the caller via
    /// `temp_env::with_vars`) determines the embedding shape; the profile
    /// itself only needs to load cleanly.
    fn build_test_db_state_with_brain_paths() -> (DbState, TempDir) {
        let tmp = TempDir::new().unwrap();
        let config_path = tmp.path().join("config.json");
        let db_path = tmp.path().join("brain.db");
        // Touch config.json with a minimal vault so `get_embed_profile` has
        // something to read. The default `EmbedProfile` Local is fine — the
        // `CURATED_EMBED_STUB` env overrides `embed_batch` regardless.
        let cfg = crate::vault::VaultConfig::new(config_path.clone());
        let _ = cfg.set_vault_path(tmp.path().to_str().unwrap());

        let db = crate::db::AppDb::open_with_config(&db_path, &config_path).unwrap();
        let db_state = DbState(std::sync::Mutex::new(db));
        (db_state, tmp)
    }

    #[test]
    fn run_embedding_sweep_fills_null_blobs_against_the_live_connection() {
        let (db_state, tmp) = build_test_db_state_with_brain_paths();
        let brain_dir = tmp.path().to_str().unwrap().to_string();
        let config_path = tmp.path().join("config.json");
        let db_path = tmp.path().join("brain.db");

        temp_env::with_vars(
            [
                ("CURATED_EMBED_STUB", Some("constant8")),
                ("CURATED_BRAIN_DIR", Some(brain_dir.as_str())),
                ("CURATED_BRAIN_CONFIG", Some(config_path.to_str().unwrap())),
                ("CURATED_BRAIN_DB", Some(db_path.to_str().unwrap())),
            ],
            || {
                {
                    let guard = db_state.0.lock().unwrap();
                    insert_wiki_entry_ms(
                        &guard.0,
                        "fact_a",
                        "librarian_inferred",
                        "documents/a.md",
                        None,
                    );
                    insert_wiki_entry_ms(
                        &guard.0,
                        "fact_b",
                        "librarian_inferred",
                        "documents/b.md",
                        None,
                    );
                }

                let report = run_embedding_sweep(&db_state).unwrap();

                assert_eq!(report.filled, 2);
                assert_eq!(report.remaining_null, 0);

                let guard = db_state.0.lock().unwrap();
                let blob_len: i64 = guard
                    .0
                    .query_row(
                        "SELECT length(embedding_blob) FROM llm_wiki_entries WHERE id = 'fact_a'",
                        [],
                        |r| r.get(0),
                    )
                    .unwrap();
                assert_eq!(blob_len, 32, "constant8 gives 8 dims -> 32 bytes");
            },
        );
    }
}

#[cfg(test)]
mod ingest_document_command_tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tauri::Listener;
    use tempfile::TempDir;

    // `pipeline::ingest_document` reads `CURATED_EMBED_STUB` to bypass Ollama,
    // and so do tests elsewhere in this crate — `embed_sweep` in particular
    // sets `constant8_short` and unsets the var entirely. Env is process-wide,
    // so those must not run interleaved with this one.
    //
    // This used to be a private `EMBED_STUB_GUARD: Mutex<()>` taken by a
    // `lock_stub()` helper — but the helper bound the guard to a local, so it
    // was dropped on return and the mutex protected nothing. That is why this
    // test failed intermittently under the full suite. `temp_env::with_vars`
    // holds one global lock shared by every other caller in the crate, which
    // is the property the static mutex was reaching for; use it instead of
    // reintroducing a second, unshared lock.

    #[test]
    fn ingest_document_command_emits_progress_and_proposal_ready() {
        temp_env::with_vars([("CURATED_EMBED_STUB", Some("constant8"))], || {
            let tmp = TempDir::new().expect("tempdir");
            let db_path = tmp.path().join("brain.db");
            let db = db::AppDb::open_with_config(&db_path, tmp.path().join("config.json"))
                .expect("open test db");
            let config = vault::VaultConfig::new(tmp.path().join("config.json"));

            // Real markdown doc with enough words for chunk_autodetect to produce
            // at least one chunk — without a chunk, `ingest_document` returns Ok(())
            // but the embedding leg still runs the same code path so we still emit
            // both progress events.
            let doc_path = tmp.path().join("note.md");
            std::fs::write(
                &doc_path,
                "# Test Note\n\n".to_owned() + &"word ".repeat(40),
            )
            .expect("write doc");

            // Real `tauri::App<MockRuntime>` (the limitation in the brief is about
            // `AppHandle` as a *command argument*; emitting on an `AppHandle` you
            // already hold works fine and listeners observe the events).
            let app = tauri::test::mock_builder()
                .manage(DbState(Mutex::new(db)))
                .manage(VaultConfigState(Mutex::new(config)))
                .manage(EmbedProfileState(Mutex::new(
                    crate::embedder::EmbedProfile::default(),
                )))
                .manage(PipelineHolder(Mutex::new(None)))
                .build(tauri::test::mock_context(tauri::test::noop_assets()))
                .expect("build mock app");

            // Capture emitted events into a shared vector. We don't care about the
            // payload of `ingest-error` here — the happy-path event shapes are what
            // we lock in.
            let captured: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));

            let progress_cap = captured.clone();
            app.listen("ingest-progress", move |event| {
                progress_cap
                    .lock()
                    .unwrap()
                    .push(("ingest-progress".into(), event.payload().into()));
            });

            let ready_cap = captured.clone();
            app.listen("ingest-proposal-ready", move |event| {
                ready_cap
                    .lock()
                    .unwrap()
                    .push(("ingest-proposal-ready".into(), event.payload().into()));
            });

            // Extract the state via `app.state::<T>()` — the same path the Tauri
            // command argument extractor uses — and call the helper directly with
            // the inner locks.
            let db_state = app.state::<DbState>();
            let profile_state = app.state::<EmbedProfileState>();
            run_ingest_with_app(
                app.handle(),
                &db_state.0,
                &profile_state.0,
                doc_path.to_string_lossy().into_owned(),
            )
            .expect("run_ingest_with_app");

            // Allow Tauri's event loop to deliver the listen notifications before
            // we read the buffer.
            std::thread::sleep(std::time::Duration::from_millis(100));

            let events = captured.lock().unwrap();
            let names: Vec<&str> = events.iter().map(|(n, _)| n.as_str()).collect();
            assert!(
                names.contains(&"ingest-progress"),
                "expected at least one ingest-progress event, got: {events:?}",
            );
            assert!(
                events
                    .iter()
                    .any(|(n, p)| n == "ingest-progress" && p.contains("\"chunking\"")),
                "expected ingest-progress with phase=chunking, got: {events:?}",
            );
            assert!(
                events
                    .iter()
                    .any(|(n, p)| n == "ingest-progress" && p.contains("\"embedding\"")),
                "expected ingest-progress with phase=embedding, got: {events:?}",
            );
            assert!(
                names.contains(&"ingest-proposal-ready"),
                "expected at least one ingest-proposal-ready event, got: {events:?}",
            );
            // The test document doesn't produce a pending proposal (synthesis is
            // async), so `proposalId` must serialize as JSON `null` — not `""`,
            // which the wizard would otherwise route to as a nonexistent id.
            let ready_payload = events
                .iter()
                .find(|(n, _)| n == "ingest-proposal-ready")
                .map(|(_, p)| p.clone())
                .expect("ingest-proposal-ready payload");
            let payload: serde_json::Value =
                serde_json::from_str(&ready_payload).expect("parse ingest-proposal-ready payload");
            assert_eq!(
                payload.get("proposalId"),
                Some(&serde_json::Value::Null),
                "expected proposalId to be JSON null for the no-proposal case, got: {payload}",
            );
        });
    }
}
