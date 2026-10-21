//! Tauri commands for OKF entity CRUD (Brain mode, Phase 4).

use crate::db::connections::{get_entity_connections, EntityConnections};
use crate::db::entities::{
    archive_entity, create_entity, get_entity, list_entities, update_entity_summary,
    CreateEntityInput, EntityDetail, EntityListFilter, EntitySort, EntitySummary,
};
use crate::db::wisdom::{archive_wisdom, precompute_entry_embedding, update_wisdom_with_blob};
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
    let mut guard = db_state.0.lock().map_err(|e| e.to_string())?;
    create_entity(&mut guard.0, &input).map_err(|e| e.to_string())
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
    let mut guard = db_state.0.lock().map_err(|e| e.to_string())?;
    archive_entity(&mut guard.0, &entity_id).map_err(|e| e.to_string())
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
pub async fn add_entity_fact_cmd(
    entity_id: String,
    body: String,
    db_state: State<'_, DbState>,
    embed_profile: State<'_, crate::EmbedProfileState>,
) -> Result<String, String> {
    // Task 9 (user-confirmation-queue, spec §3.7.6 + §9): the app's DIRECT
    // `user_stated` INSERT is RETIRED. A statement typed in the app is
    // captured as a `stated/` file, attested by the app, and ingested through
    // the Librarian, so it gains evidence and becomes deliverable mid-task.
    // Consequence, accepted by the spec: the entity binding is dropped —
    // where the fact lands is decided by the librarian's proposal targeting,
    // not by the page it was typed on.
    let _ = entity_id;
    // Compute the capture inputs OUTSIDE the blocking call; `embed_batch` and
    // the kick both block and must never hold the app-level DB lock.
    let (db_path, profile) = {
        let guard = db_state.0.lock().map_err(|e| e.to_string())?;
        let profile = embed_profile.0.lock().map_err(|e| e.to_string())?.clone();
        (
            guard
                .0
                .path()
                .map(std::path::PathBuf::from)
                .ok_or_else(|| "brain db has no path".to_string())?,
            profile,
        )
    };
    let ctx = crate::db::wisdom::CaptureContext {
        db_path,
        vault_root: crate::db::stated_ledger::resolve_vault_root().map_err(|e| e.to_string())?,
        profile,
    };
    // The capture runs the whole kick (ingest + librarian) synchronously on a
    // blocking thread: instant confirm — human tier when it returns.
    let result = tokio::task::spawn_blocking(move || {
        crate::db::wisdom::capture_statement(&ctx, &body).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("capture task join error: {e}"))?;
    result
}

#[tauri::command]
pub fn update_entity_fact_cmd(
    entity_id: String,
    fact_id: String,
    body: String,
    db_state: State<DbState>,
    embed_profile: State<crate::EmbedProfileState>,
) -> Result<(), String> {
    // Same contract as `add_entity_fact_cmd`: embed OUTSIDE the DbState mutex,
    // then write the blob with the body. Without this the edit would NULL the
    // vector and rely on a sweep that nothing on this path triggers, leaving
    // the just-edited fact invisible to semantic retrieval until a restart or
    // an unrelated write happened to fire one.
    let embedding_blob = {
        let profile = embed_profile.0.lock().map_err(|e| e.to_string())?;
        precompute_entry_embedding(Some(&profile), &body)
    };
    let mut guard = db_state.0.lock().map_err(|e| e.to_string())?;
    update_wisdom_with_blob(&mut guard.0, &entity_id, &fact_id, &body, embedding_blob)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn archive_entity_fact_cmd(
    entity_id: String,
    fact_id: String,
    db_state: State<DbState>,
) -> Result<(), String> {
    let mut guard = db_state.0.lock().map_err(|e| e.to_string())?;
    archive_wisdom(&mut guard.0, &entity_id, &fact_id).map_err(|e| e.to_string())
}
