//! Phase-5 API: task management (list/create/update status).

use crate::db::tasks::{archive_task, create_task, list_tasks, set_task_status, TaskRow};
use crate::DbState;
use tauri::State;

#[tauri::command]
pub fn list_tasks_cmd(
    status: Option<String>,
    include_archived: Option<bool>,
    db_state: State<DbState>,
) -> Result<Vec<TaskRow>, String> {
    let guard = db_state.0.lock().map_err(|e| e.to_string())?;
    list_tasks(
        &guard.0,
        status.as_deref(),
        include_archived.unwrap_or(false),
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn create_task_cmd(
    entity_id: String,
    description: String,
    db_state: State<DbState>,
) -> Result<TaskRow, String> {
    let mut guard = db_state.0.lock().map_err(|e| e.to_string())?;
    create_task(&mut guard.0, &entity_id, &description).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_task_status_cmd(
    task_id: String,
    status: String,
    db_state: State<DbState>,
) -> Result<(), String> {
    let mut guard = db_state.0.lock().map_err(|e| e.to_string())?;
    set_task_status(&mut guard.0, &task_id, &status).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn archive_task_cmd(
    task_id: String,
    db_state: State<DbState>,
) -> Result<(), String> {
    let mut guard = db_state.0.lock().map_err(|e| e.to_string())?;
    archive_task(&mut guard.0, &task_id).map_err(|e| e.to_string())
}
