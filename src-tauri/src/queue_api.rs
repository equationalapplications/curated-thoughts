//! Tauri commands for the human confirmation queue, Path B capture, and the
//! Active Verification Prompting drain (user-confirmation-queue plan,
//! Task 9 / spec §3.2-§3.4, §9). Confirm/deny/undeny/capture are APP-ONLY —
//! no MCP/CLI wrapper is registered beside them (spec §3.3, D2).

use crate::db::queue::QueueRow;
use crate::DbState;
use serde::Serialize;
use tauri::State;

/// One `queue_denials` row for the queue screen's denied section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DenialRow {
    pub document_path: String,
    pub chunk_hash: String,
    pub fact_text_hash: String,
    pub denied_by: String,
    pub denied_at: String,
}

#[tauri::command]
pub fn queue_list_cmd(
    db_state: State<'_, DbState>,
) -> Result<Vec<QueueRow>, String> {
    let guard = db_state.0.lock().map_err(|e| e.to_string())?;
    Ok(crate::db::queue::queue_rows(&guard.0))
}

/// Confirm a queue fact (spec §3.3). App-only human stamp.
#[tauri::command]
pub fn queue_confirm_cmd(fact_id: String, db_state: State<'_, DbState>) -> Result<(), String> {
    let guard = db_state.0.lock().map_err(|e| e.to_string())?;
    crate::db::queue::confirm_fact(&guard.0, &fact_id).map_err(|e| e.to_string())
}

/// Deny a queue fact (spec §3.3: archive + suppress). The UI must surface a
/// denial error rather than swallow it (Task 8 review note).
#[tauri::command]
pub fn queue_deny_cmd(fact_id: String, db_state: State<'_, DbState>) -> Result<(), String> {
    let guard = db_state.0.lock().map_err(|e| e.to_string())?;
    crate::db::queue::deny_fact(&guard.0, &fact_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn queue_undeny_cmd(
    key: crate::db::queue::DenialKey,
    db_state: State<'_, DbState>,
) -> Result<(), String> {
    let guard = db_state.0.lock().map_err(|e| e.to_string())?;
    crate::db::queue::undeny_fact(&guard.0, &key).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn queue_denied_cmd(db_state: State<'_, DbState>) -> Result<Vec<DenialRow>, String> {
    let guard = db_state.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = guard
        .0
        .prepare(
            "SELECT document_path, chunk_hash, fact_text_hash, denied_by, denied_at
               FROM queue_denials
              ORDER BY denied_at DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(DenialRow {
                document_path: r.get(0)?,
                chunk_hash: r.get(1)?,
                fact_text_hash: r.get(2)?,
                denied_by: r.get(3)?,
                denied_at: r.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// Capture a fresh statement typed into the queue view (Rule 15 / spec §9):
/// Path B — stated/ file, attest, kick. Runs the whole kick on a blocking
/// thread (instant confirm: human tier when it returns).
#[tauri::command]
pub async fn queue_capture_cmd(
    text: String,
    db_state: State<'_, DbState>,
    embed_profile: State<'_, crate::EmbedProfileState>,
) -> Result<String, String> {
    let ctx = capture_context(&db_state, &embed_profile)?;
    tokio::task::spawn_blocking(move || {
        crate::db::wisdom::capture_statement(&ctx, &text).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("capture task join error: {e}"))?
}

/// The queue-level confirm (Rule 15 / spec §9): a live fact_id confirms;
/// a confirm-with-no-fact (or `factId: null` with fresh text) routes through
/// capture_statement.
#[tauri::command]
pub async fn queue_confirm_or_capture_cmd(
    fact_id: Option<String>,
    fresh_text: Option<String>,
    db_state: State<'_, DbState>,
    embed_profile: State<'_, crate::EmbedProfileState>,
) -> Result<String, String> {
    let ctx = capture_context(&db_state, &embed_profile)?;
    tokio::task::spawn_blocking(move || {
        crate::db::wisdom::confirm_or_capture(&ctx, fact_id.as_deref(), fresh_text.as_deref())
            .map(|outcome| match outcome {
                crate::db::wisdom::QueueStatementOutcome::Confirmed => {
                    "confirmed".to_string()
                }
                crate::db::wisdom::QueueStatementOutcome::Captured(rel) => rel,
            })
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("capture task join error: {e}"))?
}

/// M1 contract (anti-payload-spoof): the prompting modal re-reads the fact BY
/// ID from the brain and renders THAT, never the drop-file payload text. The
/// queue view's row carries the evidence quote join, so the modal shows the
/// same provenance the queue shows.
#[tauri::command]
pub fn queue_fact_get_cmd(
    fact_id: String,
    db_state: State<'_, DbState>,
) -> Result<Option<QueueRow>, String> {
    let guard = db_state.0.lock().map_err(|e| e.to_string())?;
    Ok(crate::db::queue::queue_rows(&guard.0)
        .into_iter()
        .find(|r| r.fact_id == fact_id))
}

/// Drain the Active Verification Prompting drop file (spec §3.4): oldest
/// unacked first, capped at [`crate::prompt_drop::PROMPT_DRAIN_CAP`] per
/// drain, plus the remaining count for the modal's digest.
#[tauri::command]
pub fn unverified_prompts_drain_cmd(
    db_state: State<'_, DbState>,
) -> Result<crate::prompt_drop::DrainReport, String> {
    let vault_root = crate::db::stated_ledger::resolve_vault_root().map_err(|e| e.to_string())?;
    // The drain touches a vault file; keep the DB lock scope to the root
    // resolution (the guard above proves the app db is reachable).
    let _ = db_state;
    crate::prompt_drop::drain_unverified_matches(&vault_root, crate::prompt_drop::PROMPT_DRAIN_CAP)
        .map_err(|e| e.to_string())
}

/// Build the capture context from app state. The db path comes from the open
/// app db; the vault root from the configured brain config — the same
/// resolution every *_at backend function accepts explicitly.
fn capture_context(
    db_state: &State<'_, DbState>,
    embed_profile: &State<'_, crate::EmbedProfileState>,
) -> Result<crate::db::wisdom::CaptureContext, String> {
    let db_path = {
        let guard = db_state.0.lock().map_err(|e| e.to_string())?;
        guard
            .0
            .path()
            .map(std::path::PathBuf::from)
            .ok_or_else(|| "brain db has no path".to_string())?
    };
    let profile = embed_profile.0.lock().map_err(|e| e.to_string())?.clone();
    Ok(crate::db::wisdom::CaptureContext {
        db_path,
        vault_root: crate::db::stated_ledger::resolve_vault_root().map_err(|e| e.to_string())?,
        profile,
    })
}
