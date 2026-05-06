mod db;
mod vault;
mod watcher;
mod setup;

use std::sync::Mutex;
use tauri::{AppHandle, Emitter, State};
use db::AppDb;
use vault::VaultConfig;
use setup::{check_ollama as ollama_check, list_local_models as ollama_models,
            pull_model as ollama_pull, recommended_model as ollama_recommended,
            start_ollama_server as ollama_start, OllamaStatus};
use watcher::start_watcher;

#[allow(dead_code)] // used for DB access in future subprojects
struct DbState(Mutex<AppDb>);
struct VaultConfigState(Mutex<VaultConfig>);

#[tauri::command]
fn get_vault_path(state: State<VaultConfigState>) -> Result<Option<String>, String> {
    state.0.lock().unwrap().get_vault_path().map_err(|e| e.to_string())
}

#[tauri::command]
fn set_vault_path(path: String, state: State<VaultConfigState>) -> Result<(), String> {
    state.0.lock().unwrap().set_vault_path(&path).map_err(|e| e.to_string())
}

#[tauri::command]
fn start_file_watcher(vault_path: String, app: AppHandle) -> Result<(), String> {
    start_watcher(vault_path.into(), move |event| {
        let _ = app.emit("vault-event", &event);
    })
    .map_err(|e| e.to_string())
}

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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let brain_dir = dirs::home_dir().unwrap_or_default().join(".brain");
    std::fs::create_dir_all(&brain_dir).ok();

    let db = AppDb::open(&brain_dir.join("brain.db")).expect("failed to open database");
    let config = VaultConfig::new(brain_dir.join("config.json"));

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_os::init())
        .manage(DbState(Mutex::new(db)))
        .manage(VaultConfigState(Mutex::new(config)))
        .invoke_handler(tauri::generate_handler![
            get_vault_path,
            set_vault_path,
            start_file_watcher,
            check_ollama,
            list_local_models,
            pull_model,
            start_ollama_server,
            get_recommended_model,
        ])
        .run(tauri::generate_context!())
        .expect("error running Tauri application");
}
