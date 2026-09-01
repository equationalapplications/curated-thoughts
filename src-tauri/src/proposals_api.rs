//! Tauri commands for OKF proposal review (native API + legacy shims).

use crate::db::commit::{resolve_proposal, CommitResult, ResolveOptions};
use crate::db::proposals::{
    get_proposal_detail, list_proposals, ItemDecision, ProposalDetail, ProposalFilter,
    ProposalSummary,
};
use crate::db::review_shim::{
    approve_proposal_shim, list_pending_review_pages, proposed_content_for_rowid,
    reject_proposal_shim, ShimReviewPage,
};
use crate::{run_embedding_sweep, DbState};
use tauri::State;

#[tauri::command]
pub fn list_proposals_cmd(
    filter: Option<ProposalFilter>,
    db_state: State<DbState>,
) -> Result<Vec<ProposalSummary>, String> {
    let guard = db_state.0.lock().map_err(|e| e.to_string())?;
    list_proposals(&guard.0, &filter.unwrap_or_default()).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_proposal_detail_cmd(
    proposal_id: String,
    db_state: State<DbState>,
) -> Result<Option<ProposalDetail>, String> {
    let guard = db_state.0.lock().map_err(|e| e.to_string())?;
    get_proposal_detail(&guard.0, &proposal_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn resolve_proposal_cmd(
    proposal_id: String,
    decisions: Vec<ItemDecision>,
    reject_reason: Option<String>,
    auto_approve: Option<bool>,
    db_state: State<DbState>,
) -> Result<CommitResult, String> {
    let mut guard = db_state.0.lock().map_err(|e| e.to_string())?;
    let result = resolve_proposal(
        &mut guard.0,
        &proposal_id,
        &decisions,
        reject_reason.as_deref(),
        ResolveOptions {
            auto_approve: auto_approve.unwrap_or(false),
            embed_profile: None,
        },
    )
    .map_err(|e| e.to_string())?;
    // Drop the write lock before the sweep; the sweep takes its own lock.
    drop(guard);
    if let Err(e) = run_embedding_sweep(&db_state) {
        eprintln!("post-commit embedding sweep skipped: {e}");
    }
    Ok(result)
}

// ── Legacy Review desk shims (wiki_pages-shaped API) ─────────────────────────

#[tauri::command]
pub fn get_review_queue(db_state: State<DbState>) -> Result<Vec<ShimReviewPage>, String> {
    let guard = db_state.0.lock().map_err(|e| e.to_string())?;
    list_pending_review_pages(&guard.0).map_err(|e| e.to_string())
}

/// All-accept `resolve_proposal`. The `content` parameter is ignored — edits cannot be
/// mapped back to discrete proposal items.
#[tauri::command]
pub fn approve_wiki_page(
    id: i64,
    _content: String,
    db_state: State<DbState>,
) -> Result<(), String> {
    let mut guard = db_state.0.lock().map_err(|e| e.to_string())?;
    approve_proposal_shim(&mut guard.0, id).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn reject_wiki_page(id: i64, db_state: State<DbState>) -> Result<(), String> {
    let mut guard = db_state.0.lock().map_err(|e| e.to_string())?;
    reject_proposal_shim(&mut guard.0, id, None).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn get_proposed_content(page_id: i64, db_state: State<DbState>) -> Result<String, String> {
    let guard = db_state.0.lock().map_err(|e| e.to_string())?;
    proposed_content_for_rowid(&guard.0, page_id).map_err(|e| e.to_string())
}
