//! `#[tauri::command]` handlers for the chunk overlay surface.

use crate::db::queries::find_chunk_overlay;
use crate::DbState;
use serde::Serialize;
use std::sync::Mutex;
use tauri::State;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChunkOverlay {
    pub start_line: u32,
    pub end_line: u32,
}

#[tauri::command]
pub fn resolve_chunk_overlay_cmd(
    db: State<'_, DbState>,
    path: String,
    hash: String,
) -> Result<Option<ChunkOverlay>, String> {
    let guard = db.0.lock().map_err(|e| format!("db lock poisoned: {e}"))?;
    find_chunk_overlay(&guard.0, &path, &hash)
        .map(|opt| opt.map(|(s, e)| ChunkOverlay { start_line: s, end_line: e }))
        .map_err(|e| e.to_string())
}