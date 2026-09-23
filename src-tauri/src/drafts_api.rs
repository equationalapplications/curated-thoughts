//! Tauri command for draft promotion (spec CT-REQ-DRAFT-01).

use crate::db::commit::ms_now;
use crate::db::drafts::promote_draft;
use crate::DbState;
use tauri::State;

/// Promote a live draft to `stable` with a `human:local` trust entry and an
/// outbox row. Errors are `not_found` / `not_draft` or a DB error string.
#[tauri::command]
pub fn promote_draft_cmd(
    entry_id: String,
    entity_id: String,
    db_state: State<DbState>,
) -> Result<(), String> {
    let guard = db_state.0.lock().map_err(|e| e.to_string())?;
    promote_draft(&guard.0, &entry_id, &entity_id, ms_now()).map_err(|e| e.to_string())
}
