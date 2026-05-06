mod chunker;
mod db;
mod embedder;
mod hasher;
mod pipeline;
mod setup;
mod vault;
mod watcher;

use std::sync::{mpsc::SyncSender, Mutex};
use tauri::{AppHandle, Emitter, State};
use db::AppDb;
use pipeline::{start_pipeline, PipelineJob};
use setup::{check_ollama as ollama_check, list_local_models as ollama_models,
            pull_model as ollama_pull, recommended_model as ollama_recommended,
            start_ollama_server as ollama_start, OllamaStatus};
use vault::VaultConfig;
use watcher::{start_watcher, VaultEvent};

#[allow(dead_code)]
struct DbState(Mutex<AppDb>);
struct VaultConfigState(Mutex<VaultConfig>);
struct PipelineTx(Mutex<SyncSender<PipelineJob>>);

// ── Vault commands ────────────────────────────────────────────────────────────

#[tauri::command]
fn get_vault_path(state: State<VaultConfigState>) -> Result<Option<String>, String> {
    state.0.lock().unwrap().get_vault_path().map_err(|e| e.to_string())
}

#[tauri::command]
fn set_vault_path(path: String, state: State<VaultConfigState>) -> Result<(), String> {
    state.0.lock().unwrap().set_vault_path(&path).map_err(|e| e.to_string())
}

// ── Watcher + pipeline ────────────────────────────────────────────────────────

#[tauri::command]
fn start_file_watcher(
    vault_path: String,
    app: AppHandle,
    pipeline: State<PipelineTx>,
) -> Result<(), String> {
    let tx = pipeline.0.lock().unwrap().clone();
    start_watcher(vault_path.into(), move |event| {
        let _ = app.emit("vault-event", &event);
        let job = match &event {
            VaultEvent::Added(p) | VaultEvent::Modified(p) => {
                Some(PipelineJob::Ingest(p.clone()))
            }
            VaultEvent::Deleted(p) => Some(PipelineJob::Delete(p.clone())),
        };
        if let Some(j) = job {
            let _ = tx.try_send(j);
        }
    })
    .map_err(|e| e.to_string())
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

// ── Ollama commands ───────────────────────────────────────────────────────────

#[tauri::command]
fn check_ollama() -> OllamaStatus { ollama_check() }

#[tauri::command]
fn list_local_models() -> Result<Vec<String>, String> {
    ollama_models().map_err(|e| e.to_string())
}

#[tauri::command]
fn get_recommended_model() -> String { ollama_recommended().to_string() }

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

// ── App entry ─────────────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let brain_dir = dirs::home_dir().unwrap_or_default().join(".brain");
    std::fs::create_dir_all(&brain_dir).ok();

    let db_path = brain_dir.join("brain.db");
    let db = AppDb::open(&db_path).expect("failed to open database");
    let config = VaultConfig::new(brain_dir.join("config.json"));
    let pipeline_tx = start_pipeline(db_path);

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_os::init())
        .manage(DbState(Mutex::new(db)))
        .manage(VaultConfigState(Mutex::new(config)))
        .manage(PipelineTx(Mutex::new(pipeline_tx)))
        .invoke_handler(tauri::generate_handler![
            get_vault_path,
            set_vault_path,
            start_file_watcher,
            get_indexing_status,
            check_ollama,
            list_local_models,
            pull_model,
            start_ollama_server,
            get_recommended_model,
        ])
        .run(tauri::generate_context!())
        .expect("error running Tauri application");
}
