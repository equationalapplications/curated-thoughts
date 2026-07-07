//! Tauri commands for OKF entity CRUD (Brain mode, Phase 4).

use crate::db::connections::{get_entity_connections, EntityConnections};
use crate::db::entities::{
    archive_entity, create_entity, get_entity, list_entities, update_entity_summary,
    CreateEntityInput, EntityDetail, EntityFact, EntityListFilter, EntitySort, EntitySummary,
};
use crate::db::facts::{add_fact, archive_fact, update_fact};
use crate::DbState;
use tauri::State;

#[tauri::command]
pub fn list_entities_cmd(
    sort: Option<EntitySort>,
    filter: Option<EntityListFilter>,
    db_state: State<DbState>,
) -> Result<Vec<EntitySummary>, String> {
    let guard = db_state.0.lock().map_err(|e| e.to_string())?;
    list_entities(
        &guard.0,
        sort.unwrap_or_default(),
        &filter.unwrap_or_default(),
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_entity_cmd(
    entity_id: String,
    db_state: State<DbState>,
) -> Result<Option<EntityDetail>, String> {
    let guard = db_state.0.lock().map_err(|e| e.to_string())?;
    get_entity(&guard.0, &entity_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn create_entity_cmd(
    input: CreateEntityInput,
    db_state: State<DbState>,
) -> Result<EntityDetail, String> {
    let guard = db_state.0.lock().map_err(|e| e.to_string())?;
    create_entity(&guard.0, &input).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_entity_summary_cmd(
    entity_id: String,
    summary: String,
    db_state: State<DbState>,
) -> Result<(), String> {
    let guard = db_state.0.lock().map_err(|e| e.to_string())?;
    update_entity_summary(&guard.0, &entity_id, &summary).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn archive_entity_cmd(entity_id: String, db_state: State<DbState>) -> Result<(), String> {
    let guard = db_state.0.lock().map_err(|e| e.to_string())?;
    archive_entity(&guard.0, &entity_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_entity_connections_cmd(
    entity_id: String,
    db_state: State<DbState>,
) -> Result<EntityConnections, String> {
    let guard = db_state.0.lock().map_err(|e| e.to_string())?;
    get_entity_connections(&guard.0, &entity_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn add_entity_fact_cmd(
    entity_id: String,
    body: String,
    db_state: State<DbState>,
) -> Result<EntityFact, String> {
    let mut guard = db_state.0.lock().map_err(|e| e.to_string())?;
    add_fact(&mut guard.0, &entity_id, &body).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_entity_fact_cmd(
    entity_id: String,
    fact_id: String,
    body: String,
    db_state: State<DbState>,
) -> Result<(), String> {
    let mut guard = db_state.0.lock().map_err(|e| e.to_string())?;
    update_fact(&mut guard.0, &entity_id, &fact_id, &body).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn archive_entity_fact_cmd(
    entity_id: String,
    fact_id: String,
    db_state: State<DbState>,
) -> Result<(), String> {
    let mut guard = db_state.0.lock().map_err(|e| e.to_string())?;
    archive_fact(&mut guard.0, &entity_id, &fact_id).map_err(|e| e.to_string())
}
