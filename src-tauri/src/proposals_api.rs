//! Tauri commands for OKF proposal review (native API + legacy shims).

use crate::db::commit::{
    load_items, load_proposal, precompute_entry_embeddings, resolve_proposal, CommitResult,
    ResolveOptions,
};
use crate::db::proposals::{
    get_proposal_detail, list_proposals, ItemDecision, ItemDecisionKind, ProposalDetail,
    ProposalFilter, ProposalSummary,
};
use crate::db::review_shim::{
    list_pending_review_pages, proposal_id_for_rowid, proposed_content_for_rowid, ShimReviewPage,
};
use crate::{run_embedding_sweep, DbState};
use anyhow::Context;
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
    embed_profile: State<crate::EmbedProfileState>,
) -> Result<CommitResult, String> {
    // Three-phase commit so the blocking embed round-trip never runs under the
    // app-level DbState mutex (which gates every other Tauri command).
    //
    // 1. Inside the lock: validate proposal + load items (cheap, ~1 ms).
    // 2. Outside the lock: precompute entry embeddings via the embedder
    //    (slow, up to EXTERNAL_EMBED_TIMEOUT_SECS per call).
    // 3. Re-acquire the lock: run the actual commit transaction.
    //
    // Phases 1 and 3 each load the items, but item loads are a single SELECT
    // and the extra DB work is dominated by the avoided mutex-hold window.
    let items = {
        let guard = db_state.0.lock().map_err(|e| e.to_string())?;
        let proposal = load_proposal(&guard.0, &proposal_id).map_err(|e| e.to_string())?;
        if proposal.status != "pending" {
            return Err(format!("proposal is not pending: {}", proposal.status));
        }
        let items = load_items(&guard.0, &proposal_id).map_err(|e| e.to_string())?;
        if items.is_empty() {
            return Err("proposal has no items".into());
        }
        items
    };

    let entry_embeddings = {
        let profile = embed_profile.0.lock().map_err(|e| e.to_string())?;
        precompute_entry_embeddings(&items, &decisions, Some(&profile))
    };

    let mut guard = db_state.0.lock().map_err(|e| e.to_string())?;
    let result = resolve_proposal(
        &mut guard.0,
        &proposal_id,
        &decisions,
        reject_reason.as_deref(),
        ResolveOptions {
            auto_approve: auto_approve.unwrap_or(false),
            embed_profile: None,
            entry_embeddings: Some(entry_embeddings),
        },
    )
    .map_err(|e| e.to_string())?;
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
///
/// Inlines the three-phase precompute-outside-the-lock pattern instead of
/// delegating to `approve_proposal_shim`, so the blocking embed round-trip
/// never holds the `DbState` mutex while every other Tauri command queues.
#[tauri::command]
pub fn approve_wiki_page(
    id: i64,
    _content: String,
    db_state: State<DbState>,
    embed_profile: State<crate::EmbedProfileState>,
) -> Result<(), String> {
    let (proposal_id, items) = {
        let guard = db_state.0.lock().map_err(|e| e.to_string())?;
        let proposal_id = proposal_id_for_rowid(&guard.0, id)
            .map_err(|e| e.to_string())?
            .context("proposal not found for review id")
            .map_err(|e| e.to_string())?;
        let items = load_items(&guard.0, &proposal_id).map_err(|e| e.to_string())?;
        if items.is_empty() {
            return Err("proposal has no items".into());
        }
        (proposal_id, items)
    };

    let decisions: Vec<ItemDecision> = items
        .iter()
        .map(|item| ItemDecision {
            item_id: item.id.clone(),
            decision: ItemDecisionKind::Accept,
            edited_payload: None,
        })
        .collect();

    let entry_embeddings = {
        let profile = embed_profile.0.lock().map_err(|e| e.to_string())?;
        precompute_entry_embeddings(&items, &decisions, Some(&profile))
    };

    let mut guard = db_state.0.lock().map_err(|e| e.to_string())?;
    resolve_proposal(
        &mut guard.0,
        &proposal_id,
        &decisions,
        None,
        ResolveOptions {
            auto_approve: false,
            embed_profile: None,
            entry_embeddings: Some(entry_embeddings),
        },
    )
    .map_err(|e| e.to_string())?;
    drop(guard);

    // `precompute_entry_embeddings` leaves an entry NULL-embedded when the
    // provider is transiently down, and `resolve_proposal` commits it that way.
    // Without this sweep the approved entry stays invisible to semantic
    // retrieval until a restart or an unrelated write happens to trigger one.
    if let Err(e) = run_embedding_sweep(&db_state) {
        eprintln!("post-commit embedding sweep skipped: {e}");
    }
    Ok(())
}

#[tauri::command]
pub fn reject_wiki_page(
    id: i64,
    db_state: State<DbState>,
    _embed_profile: State<crate::EmbedProfileState>,
) -> Result<(), String> {
    let (proposal_id, items) = {
        let guard = db_state.0.lock().map_err(|e| e.to_string())?;
        let proposal_id = proposal_id_for_rowid(&guard.0, id)
            .map_err(|e| e.to_string())?
            .context("proposal not found for review id")
            .map_err(|e| e.to_string())?;
        let items = load_items(&guard.0, &proposal_id).map_err(|e| e.to_string())?;
        if items.is_empty() {
            return Err("proposal has no items".into());
        }
        (proposal_id, items)
    };

    // All-reject: no embeddings to compute. The three-phase precompute is
    // still applied for symmetry with `approve_wiki_page`, but the map it
    // returns is empty (no `fact_add`/`fact_update` items are accepted) so
    // the embedder round-trip is never issued.
    let decisions: Vec<ItemDecision> = items
        .iter()
        .map(|item| ItemDecision {
            item_id: item.id.clone(),
            decision: ItemDecisionKind::Reject,
            edited_payload: None,
        })
        .collect();

    let mut guard = db_state.0.lock().map_err(|e| e.to_string())?;
    resolve_proposal(
        &mut guard.0,
        &proposal_id,
        &decisions,
        None,
        ResolveOptions {
            auto_approve: false,
            embed_profile: None,
            entry_embeddings: Some(std::collections::HashMap::new()),
        },
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn get_proposed_content(page_id: i64, db_state: State<DbState>) -> Result<String, String> {
    let guard = db_state.0.lock().map_err(|e| e.to_string())?;
    proposed_content_for_rowid(&guard.0, page_id).map_err(|e| e.to_string())
}
