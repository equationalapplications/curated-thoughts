pub mod chunker;
pub mod db;
pub mod embedder;
pub mod graph;
mod hasher;
pub mod indexer;
pub mod librarian;
mod pipeline;
pub mod recall_bench_fixture;
pub mod retrieval;
pub mod scifact_fixture;
pub mod search;
mod setup;
pub mod vault;
mod watcher;

use chunker::should_ingest_extension;
use db::AppDb;
use pipeline::start_pipeline;
#[cfg(feature = "test-utils")]
pub use pipeline::{PipelineJob, PipelineWorker};
use rusqlite::types::Value as SqlVal;
use rusqlite::OptionalExtension;
use serde_json::Value as JsonVal;
use setup::{
    check_ollama as ollama_check, list_local_models as ollama_models, pull_model as ollama_pull,
    recommended_model as ollama_recommended, start_ollama_server as ollama_start, OllamaStatus,
};
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};
use std::sync::{mpsc::SyncSender, Arc, Mutex};
use std::sync::atomic::AtomicUsize;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};
use vault::VaultConfig;
use watcher::{spawn_vault_watcher, VaultEvent, WatcherHandle};

struct DbState(Mutex<AppDb>);
struct VaultConfigState(Mutex<VaultConfig>);
struct PipelineHolder(Mutex<Option<(SyncSender<PipelineJob>, std::thread::JoinHandle<()>, Arc<AtomicUsize>)>>);
struct WatcherStarted(Mutex<Option<(PathBuf, WatcherHandle)>>);

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
    let normalized_path = normalize_workspace_path(&path);
    // hash_bytes returns hex::encode(sha256) — 64 lowercase hex chars — safe to slice to 16.
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
    for subdir in &["documents", "wiki"] {
        std::fs::create_dir_all(root.join(subdir)).map_err(|e| e.to_string())?;
    }
    std::fs::create_dir_all(root.join(".brain").join("converted")).map_err(|e| e.to_string())?;
    Ok(())
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
    let stub = AppDb::open(&stub_path).map_err(|e| e.to_string())?;
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

    let brain_dir = dirs::home_dir().unwrap_or_default().join(".brain");
    let src = brain_dir.join("brain.db");
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
    match (
        Path::new(current).canonicalize(),
        new_root.canonicalize(),
    ) {
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
    pipeline: &PipelineHolder,
    db_state: &DbState,
    vault_state: &VaultConfigState,
    watcher_started: &WatcherStarted,
) -> Result<(), String> {
    let target_canonical = canonical_vault_from_config(vault_state)?;

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

    let pipeline_tx = pipeline
        .0
        .lock()
        .unwrap()
        .as_ref()
        .ok_or_else(|| "pipeline not running".to_string())?
        .0
        .clone();

    let raw_docs = target_canonical.join("documents");
    let documents_root = std::fs::canonicalize(&raw_docs).unwrap_or(raw_docs.clone());

    {
        let guard = db_state.0.lock().unwrap();
        let conn = &guard.0;

        let db_paths: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT path FROM documents WHERE tier = 'user_doc'")
                .map_err(|e| e.to_string())?;
            let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
            let mut v = Vec::new();
            while let Some(row) = rows.next().map_err(|e| e.to_string())? {
                v.push(row.get::<_, String>(0).map_err(|e| e.to_string())?);
            }
            v
        };
        for path in db_paths {
            if !std::path::Path::new(&path).exists() {
                eprintln!("[reconcile] purging deleted file from index: {}", path);
                let _ = pipeline_tx.try_send(PipelineJob::Delete(path));
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
                    let _ = pipeline_tx.try_send(PipelineJob::ingest(normalized));
                }
            }
        }
    }

    let app = app.clone();
    let vault_for_watcher = target_canonical.clone();
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
        let job = match &event {
            VaultEvent::Added(_) | VaultEvent::Modified(_) => {
                Some(PipelineJob::ingest(normalized.clone()))
            }
            VaultEvent::Deleted(_) => Some(PipelineJob::Delete(normalized.clone())),
        };
        if let Some(j) = job {
            let _ = pipeline_tx.try_send(j);
        }
    })
    .map_err(|e| e.to_string())?;

    let mut watcher_guard = watcher_started.0.lock().unwrap();
    let still_canonical = match canonical_vault_from_config(vault_state) {
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

/// Best-effort restore of DB handle, pipeline, and file watcher after a failed `switch_vault`.
/// Returns whether `db_state` was successfully reopened on `db_path` (so temp stub files are safe to delete).
fn recover_after_failed_switch_vault(
    app: &AppHandle,
    db_path: &Path,
    db_state: &DbState,
    pipeline: &PipelineHolder,
    vault_state: &VaultConfigState,
    watcher_started: &WatcherStarted,
) -> bool {
    let reopened = (|| -> Result<(), String> {
        let mut guard = db_state.0.lock().map_err(|_| "db mutex poisoned".to_string())?;
        *guard = AppDb::open(db_path).map_err(|e| e.to_string())?;
        Ok(())
    })();
    if let Err(e) = &reopened {
        eprintln!("[switch_vault] recovery: failed to reopen {db_path:?}: {e}");
        return false;
    }
    if let Ok(mut g) = pipeline.0.lock() {
        if g.is_none() {
            let vault_root = vault_state.0.lock().ok()
                .and_then(|vc| vc.get_vault_path().ok().flatten())
                .map(PathBuf::from);
            *g = Some(start_pipeline(db_path.to_path_buf(), vault_root));
        }
    }
    if let Err(e) =
        start_file_watcher_inner(app, pipeline, db_state, vault_state, watcher_started)
    {
        eprintln!("[switch_vault] recovery: failed to restart file watcher: {e}");
    }
    true
}

#[tauri::command]
fn switch_vault(
    new_path: String,
    restore_backup: bool,
    app: AppHandle,
    db_state: State<DbState>,
    vault_state: State<VaultConfigState>,
    pipeline: State<PipelineHolder>,
    watcher_started: State<WatcherStarted>,
) -> Result<(), String> {
    let brain_dir = dirs::home_dir().unwrap_or_default().join(".brain");
    let db_path = brain_dir.join("brain.db");

    let new_root = validated_new_vault_root(&new_path)?;

    if let Ok(Some(ref current)) = vault_state.0.lock().unwrap().get_vault_path() {
        if switching_to_same_vault_as_configured(current, new_root.as_path()) {
            return Ok(());
        }
    }

    for subdir in &["documents", "wiki"] {
        std::fs::create_dir_all(new_root.join(subdir)).map_err(|e| e.to_string())?;
    }
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
        if let Some((tx, join, _pending)) = g.take() {
            drop(tx);
            let _ = join.join();
        }
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
            *guard = AppDb::open(&db_path).map_err(|e| e.to_string())?;
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
            &db_state,
            &pipeline,
            &vault_state,
            &watcher_started,
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
        if let Err(e) = start_file_watcher_inner(
            &app,
            &pipeline,
            &db_state,
            &vault_state,
            &watcher_started,
        ) {
            eprintln!(
                "[switch_vault] failed to restart file watcher after successful switch: {e}"
            );
        }
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

    #[cfg(not(any(
        target_os = "macos",
        target_os = "linux",
        target_os = "windows"
    )))]
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
) -> Result<(), String> {
    start_file_watcher_inner(&app, &pipeline, &db_state, &vault_state, &watcher_started)
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
    let guard = db_state.0.lock().unwrap();
    let conn = &guard.0;
    let paths = crate::db::list_indexed_user_doc_paths(conn).map_err(|e| e.to_string())?;
    let tx = {
        let pipeline_guard = pipeline.0.lock().unwrap();
        pipeline_guard
            .as_ref()
            .ok_or_else(|| "pipeline not running".to_string())?
            .0
            .clone()
    };
    let mut queued = 0usize;
    for path in paths {
        if !std::path::Path::new(&path).exists() {
            eprintln!("[queue_full_reindex] skip missing file: {path}");
            continue;
        }
        let job = if force_rechunk {
            PipelineJob::rechunk(path)
        } else {
            PipelineJob::ingest(path)
        };
        tx.send(job)
            .map_err(|e| format!("pipeline channel closed: {e}"))?;
        queued += 1;
    }
    Ok(queued)
}

// ── Maintenance commands ──────────────────────────────────────────────────────

#[tauri::command]
async fn run_wiki_heal(
    app: AppHandle,
    db_state: State<'_, DbState>,
    vault_state: State<'_, VaultConfigState>,
) -> Result<(), String> {
    app.emit(
        "wiki-status-change",
        serde_json::json!({"heal": true, "ingesting": false, "librarian": false}),
    )
    .ok();

    let result = (|| -> Result<(), String> {
        let vault = vault_state
            .0
            .lock()
            .unwrap()
            .get_vault_path()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "no vault configured".to_string())?;
        let vault_root = std::path::PathBuf::from(&vault);

        let guard = db_state.0.lock().unwrap();
        let conn = &guard.0;

        // Fetch non-deleted entries that have a source reference.
        let entries: Vec<(i64, String)> = {
            let mut stmt = conn
                .prepare(
                    "SELECT rowid, source_ref FROM llm_wiki_entries
                     WHERE deleted_at IS NULL AND source_ref IS NOT NULL",
                )
                .map_err(|e| e.to_string())?;
            let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
            let mut v = Vec::new();
            while let Some(row) = rows.next().map_err(|e| e.to_string())? {
                v.push((
                    row.get::<_, i64>(0).map_err(|e| e.to_string())?,
                    row.get::<_, String>(1).map_err(|e| e.to_string())?,
                ));
            }
            v
        };

        for (rowid, source_ref) in entries {
            // Only accept vault-relative refs. Absolute paths, traversal segments,
            // symlink escapes, or missing files are treated as missing to prevent
            // heal from probing outside the vault.
            let safe = crate::vault::safe_vault_path(
                &vault_root,
                &source_ref,
                &["."],
                crate::vault::PathMode::MustExist,
            );
            if safe.is_err() {
                conn.execute(
                    "UPDATE llm_wiki_entries SET deleted_at = unixepoch() WHERE rowid = ?1",
                    [rowid],
                )
                .map_err(|e| e.to_string())?;
            }
        }

        Ok(())
    })();

    app.emit(
        "wiki-status-change",
        serde_json::json!({"heal": false, "ingesting": false, "librarian": false}),
    )
    .ok();

    result
}

#[tauri::command]
async fn run_wiki_prune(
    app: AppHandle,
    db_state: State<'_, DbState>,
) -> Result<(), String> {
    app.emit(
        "wiki-status-change",
        serde_json::json!({"heal": false, "ingesting": false, "librarian": true}),
    )
    .ok();

    let result = (|| -> Result<(), String> {
        let guard = db_state.0.lock().unwrap();
        let conn = &guard.0;
        // Hard-delete librarian_inferred entries soft-deleted more than 7 days ago.
        conn.execute(
            "DELETE FROM llm_wiki_entries
             WHERE source_type = 'librarian_inferred'
               AND deleted_at IS NOT NULL
               AND deleted_at < (unixepoch() - 7 * 86400)",
            [],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })();

    app.emit(
        "wiki-status-change",
        serde_json::json!({"heal": false, "ingesting": false, "librarian": false}),
    )
    .ok();

    result
}

#[tauri::command]
async fn run_wiki_reembed(
    app: AppHandle,
    db_state: State<'_, DbState>,
    pipeline: State<'_, PipelineHolder>,
) -> Result<usize, String> {
    app.emit(
        "wiki-status-change",
        serde_json::json!({"heal": false, "ingesting": true, "librarian": false}),
    )
    .ok();

    let (tx, pending) = {
        let pipeline_guard = pipeline.0.lock().unwrap();
        match pipeline_guard.as_ref() {
            Some(p) => (p.0.clone(), p.2.clone()),
            None => {
                app.emit(
                    "wiki-status-change",
                    serde_json::json!({"heal": false, "ingesting": false, "librarian": false}),
                )
                .ok();
                return Err("pipeline not running".to_string());
            }
        }
    };

    let result = (|| -> Result<usize, String> {
        let guard = db_state.0.lock().unwrap();
        let conn = &guard.0;
        let paths = crate::db::list_indexed_user_doc_paths(conn).map_err(|e| e.to_string())?;
        drop(guard);
        let mut queued = 0usize;
        for path in paths {
            if !std::path::Path::new(&path).exists() {
                continue;
            }
            pending.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            tx.send(PipelineJob::rechunk_for_reembed(path))
                .map_err(|e| {
                    pending.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                    format!("pipeline channel closed: {e}")
                })?;
            queued += 1;
        }
        Ok(queued)
    })();

    match &result {
        Ok(queued) => {
            if *queued > 0 {
                let app_handle = app.clone();
                let pending = pending.clone();
                std::thread::spawn(move || {
                    while pending.load(std::sync::atomic::Ordering::SeqCst) > 0 {
                        std::thread::sleep(Duration::from_millis(250));
                    }
                    app_handle
                        .emit(
                            "wiki-status-change",
                            serde_json::json!({"heal": false, "ingesting": false, "librarian": false}),
                        )
                        .ok();
                });
            } else {
                app.emit(
                    "wiki-status-change",
                    serde_json::json!({"heal": false, "ingesting": false, "librarian": false}),
                )
                .ok();
            }
        }
        Err(_) => {
            app.emit(
                "wiki-status-change",
                serde_json::json!({"heal": false, "ingesting": false, "librarian": false}),
            )
            .ok();
        }
    }

    result
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
fn embed_text(text: String, cfg: State<VaultConfigState>) -> Result<Vec<f32>, String> {
    let profile = cfg
        .0
        .lock()
        .unwrap()
        .get_embed_profile()
        .map_err(|e| e.to_string())?;
    crate::embedder::embed_one(&profile, text).map_err(|e| e.to_string())
}

// ── Ollama generate (for wiki llmProvider.generateText) ───────────────────────

#[tauri::command]
async fn ollama_generate(system_prompt: String, user_prompt: String) -> Result<String, String> {
    let model = ollama_recommended();
    let client = reqwest::Client::new();
    let resp = client
        .post("http://localhost:11434/api/generate")
        .json(&serde_json::json!({
            "model": model,
            "system": system_prompt,
            "prompt": user_prompt,
            "stream": false
        }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let body: JsonVal = resp.json().await.map_err(|e| e.to_string())?;
    body["response"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "missing 'response' in Ollama reply".to_string())
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
        &["documents", "wiki"],
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
        "both"    => graph::get_both(conn, root_chunk_id, &entity_id, max_hops),
        other     => Err(anyhow::anyhow!("unknown direction: {}", other)),
    }
    .map_err(|e| e.to_string())
}

/// Returns graph-adjacent chunks for `doc_path` with `structural: true` and `rel_type` set,
/// so the frontend can display them as "Connected" results alongside semantic hits.
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
    let vault_root = std::path::PathBuf::from(&root);

    let normalized_rel = normalize_path_argument_to_vault_relative(&doc_path, &vault_root)?;
    let safe = crate::vault::safe_vault_path(
        &vault_root,
        &normalized_rel,
        &["."],
        crate::vault::PathMode::MustExist,
    )
    .map_err(|e| e.to_string())?;
    let abs_path = safe.to_string_lossy().into_owned();
    let entity_id = pipeline::entity_id_for_path(&abs_path, Some(&root));

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
        if let Ok(neighbors) = crate::graph::get_both(conn, chunk_id, &entity_id, max_hops) {
            for n in neighbors {
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
        &["."],
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
    for row in rows {
        if let Ok(id) = row {
            ids.push(id);
        }
    }

    Ok(ids)
}

// ── Vault file listing ────────────────────────────────────────────────────────

#[derive(serde::Serialize, Clone)]
pub struct VaultFile {
    pub path: String,
    pub name: String,
    pub tier: String, // "user_doc" | "wiki"
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

    for (subdir, tier) in &[("documents", "user_doc"), ("wiki", "wiki")] {
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
                tier: tier.to_string(),
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
        &["documents", "wiki"],
        crate::vault::PathMode::MustExist,
    )
    .map_err(|e| e.to_string())?;

    std::fs::read_to_string(&safe).map_err(|e| e.to_string())
}

// ── Review queue ──────────────────────────────────────────────────────────────

#[derive(serde::Serialize, Clone)]
pub struct ReviewPage {
    pub id: i64,
    pub path: String,
    pub source_doc_ids: String,
    pub generated_by: String,
}

#[tauri::command]
fn get_review_queue(db_state: State<DbState>) -> Result<Vec<ReviewPage>, String> {
    let guard = db_state.0.lock().unwrap();
    let conn = &guard.0;
    let mut stmt = conn
        .prepare(
            "SELECT id, path, source_doc_ids, generated_by
             FROM wiki_pages WHERE status = 'pending_review'
             ORDER BY id DESC",
        )
        .map_err(|e| e.to_string())?;
    let mut pages = Vec::new();
    let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        pages.push(ReviewPage {
            id: row.get(0).map_err(|e| e.to_string())?,
            path: row.get(1).map_err(|e| e.to_string())?,
            source_doc_ids: row.get(2).map_err(|e| e.to_string())?,
            generated_by: row.get(3).map_err(|e| e.to_string())?,
        });
    }
    Ok(pages)
}

#[tauri::command]
fn approve_wiki_page(
    id: i64,
    content: String,
    db_state: State<DbState>,
    vault_state: State<VaultConfigState>,
) -> Result<(), String> {
    let guard = db_state.0.lock().unwrap();
    let conn = &guard.0;
    let page_path: String = conn
        .query_row("SELECT path FROM wiki_pages WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .map_err(|e| e.to_string())?;

    let vault_path = vault_state
        .0
        .lock()
        .unwrap()
        .get_vault_path()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "no vault configured".to_string())?;
    let vault_root = std::path::PathBuf::from(&vault_path);
    std::fs::create_dir_all(vault_root.join("wiki")).map_err(|e| e.to_string())?;

    // Reject absolute paths before normalization
    if std::path::Path::new(&page_path).is_absolute() {
        return Err("absolute paths not allowed".to_string());
    }

    // Normalize separators and ensure a wiki/ prefix for backward compatibility.
    let normalized_path = normalize_wiki_relative_path(&page_path);

    let safe = crate::vault::safe_vault_path(
        &vault_root,
        &normalized_path,
        &["wiki"],
        crate::vault::PathMode::MayCreate,
    )
    .map_err(|e| e.to_string())?;

    crate::vault::safe_path::safe_write_bytes(&safe, content.as_bytes())
        .map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE wiki_pages SET status = 'approved', last_synced = unixepoch() WHERE id = ?1",
        [id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn reject_wiki_page(id: i64, db_state: State<DbState>) -> Result<(), String> {
    db_state
        .0
        .lock()
        .unwrap()
        .0
        .execute(
            "UPDATE wiki_pages SET status = 'rejected' WHERE id = ?1",
            [id],
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

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
fn get_proposed_content(
    page_id: i64,
    db_state: State<DbState>,
    vault_state: State<VaultConfigState>,
) -> Result<String, String> {
    let page_path: String = {
        let guard = db_state.0.lock().unwrap();
        guard
            .0
            .query_row(
                "SELECT path FROM wiki_pages WHERE id = ?1",
                [page_id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?
    };
    let vault = vault_state
        .0
        .lock()
        .unwrap()
        .get_vault_path()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "no vault set".to_string())?;
    let vault_root = std::path::PathBuf::from(&vault);
    std::fs::create_dir_all(vault_root.join(".brain").join("proposed"))
        .map_err(|e| e.to_string())?;

    if std::path::Path::new(&page_path).is_absolute() {
        return Err("absolute paths not allowed".to_string());
    }

    let page_rel = page_path.replace('\\', "/");
    let safe = crate::vault::safe_vault_path(
        &vault_root,
        &format!(".brain/proposed/{}", page_rel),
        &[".brain/proposed"],
        crate::vault::PathMode::MustExist,
    );

    let placeholder = || format!("# {}\n\n*Proposed wiki page — content not available.*", page_rel);
    match safe {
        Ok(p) => match std::fs::read_to_string(&p) {
            Ok(content) => Ok(content),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(placeholder()),
            Err(e) => Err(e.to_string()),
        },
        Err(crate::vault::SafePathError::NotFound(_)) => Ok(placeholder()),
        Err(e) => Err(e.to_string()),
    }
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
        &["wiki"],
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
        &["documents"],
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
    const MAX_TRIES: u32 = 10_000;
    for n in 0..MAX_TRIES {
        let candidate_name = drop_destination_filename(original_file_name, n);
        let rel = format!("documents/{candidate_name}");
        match crate::vault::safe_vault_path(
            vault_root,
            &rel,
            &["documents"],
            crate::vault::PathMode::MustExist,
        ) {
            Ok(_) => continue,
            Err(crate::vault::SafePathError::NotFound(_)) => {
                return crate::vault::safe_vault_path(
                    vault_root,
                    &rel,
                    &["documents"],
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
        "could not find a free filename under documents/ for {original_file_name}"
    ))
}

fn copy_os_drop_paths_to_vault(
    app: &AppHandle,
    paths: &[std::path::PathBuf],
) -> Result<Vec<String>, String> {
    let vault_path = app
        .state::<VaultConfigState>()
        .0
        .lock()
        .unwrap()
        .get_vault_path()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "no vault configured".to_string())?;
    let vault_root = std::path::PathBuf::from(&vault_path);
    std::fs::create_dir_all(vault_root.join("documents")).map_err(|e| e.to_string())?;

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

pub use pipeline::{entity_id_for_path, ingest_document, ingest_document_with_vault_root};

#[cfg(feature = "test-utils")]
pub fn make_test_app(tmp_path: &std::path::Path) -> tauri::App<tauri::test::MockRuntime> {
    let db_path = tmp_path.join("brain.db");
    let db = db::AppDb::open(&db_path).expect("open test db");
    let config = vault::VaultConfig::new(tmp_path.join("config.json"));
    tauri::test::mock_builder()
        .manage(DbState(std::sync::Mutex::new(db)))
        .manage(VaultConfigState(std::sync::Mutex::new(config)))
        // No background pipeline thread: integration tests invoke DB-only commands;
        // pipeline work is covered by `PipelineWorker` tests in `tests/pipeline.rs`.
        .manage(PipelineHolder(std::sync::Mutex::new(None)))
        .invoke_handler(tauri::generate_handler![
            get_vault_path,
            set_vault_path,
            get_workspace_id,
            get_review_queue,
            approve_wiki_page,
            reject_wiki_page,
            get_proposed_content,
            get_folder_rules,
            set_folder_rule,
            delete_folder_rule,
            get_indexing_status,
            list_vault_files,
            read_document,
            search_vault,
            get_related_chunks,
            save_wiki_page,
            queue_full_reindex,
        ])
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap()
}

// ── App entry ─────────────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let brain_dir = dirs::home_dir().unwrap_or_default().join(".brain");
    std::fs::create_dir_all(&brain_dir).ok();

    let db_path = brain_dir.join("brain.db");

    let config = VaultConfig::new(VaultConfig::default_config_path());
    if config.get_vault_path().ok().flatten().is_none() {
        let default_vault = VaultConfig::default_vault_path();
        let mut all_dirs_created = true;
        for subdir in &["documents", "wiki"] {
            if let Err(e) = std::fs::create_dir_all(default_vault.join(subdir)) {
                eprintln!("warning: failed to create default vault subdirectory {subdir}: {e}");
                all_dirs_created = false;
            }
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
            for subdir in &["documents", "wiki"] {
                if let Err(e) = std::fs::create_dir_all(fallback_vault.join(subdir)) {
                    eprintln!("error: failed to create fallback vault subdir {subdir}: {e}");
                    fallback_dirs_created = false;
                }
            }
            if let Err(e) = std::fs::create_dir_all(fallback_vault.join(".brain").join("converted")) {
                eprintln!("error: failed to create fallback vault subdir .brain/converted: {e}");
                fallback_dirs_created = false;
            }
            if fallback_dirs_created {
                eprintln!("warning: using temporary recovery vault at: {}", fallback_vault.display());
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

    let db = AppDb::open(&db_path).expect("failed to open database");
    let initial_vault_root = config.get_vault_path().ok().flatten().map(|p| {
        let pb = PathBuf::from(&p);
        pb.canonicalize().unwrap_or(pb)
    });
    let pipeline = start_pipeline(db_path.clone(), initial_vault_root);

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_os::init())
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
        .manage(PipelineHolder(Mutex::new(Some(pipeline))))
        .manage(WatcherStarted(Mutex::new(None)))
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
            ollama_generate,
            check_ollama,
            list_local_models,
            pull_model,
            start_ollama_server,
            get_recommended_model,
            search_vault,
            get_related_chunks,
            list_vault_files,
            read_document,
            get_review_queue,
            approve_wiki_page,
            reject_wiki_page,
            get_folder_rules,
            set_folder_rule,
            delete_folder_rule,
            get_proposed_content,
            save_wiki_page,
            delete_vault_file,
            run_wiki_heal,
            run_wiki_prune,
            run_wiki_reembed,
            get_chunk_ids_for_wiki_entry,
            get_impact_radius,
            get_structural_neighbors,
        ])
        .run(tauri::generate_context!())
        .expect("error running Tauri application");
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
        fs::create_dir_all(root.join("documents")).unwrap();
        let p = unique_drop_destination(root, "fresh.txt").unwrap();
        assert_eq!(p.file_name().and_then(|n| n.to_str()), Some("fresh.txt"));
    }

    #[test]
    fn unique_drop_avoids_overwriting_existing_basename() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("documents")).unwrap();
        fs::write(root.join("documents").join("dup.md"), b"x").unwrap();
        let p = unique_drop_destination(root, "dup.md").unwrap();
        assert_eq!(p.file_name().and_then(|n| n.to_str()), Some("dup (1).md"));
    }

    #[test]
    fn unique_drop_skips_directory_with_same_basename() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("documents")).unwrap();
        fs::create_dir_all(root.join("documents").join("dup.md")).unwrap();
        let p = unique_drop_destination(root, "dup.md").unwrap();
        assert_eq!(p.file_name().and_then(|n| n.to_str()), Some("dup (1).md"));
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
        assert_eq!(hash.len(), 16, "hash segment should be 16 chars, got: {hash}");
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
