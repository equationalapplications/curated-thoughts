//! Shared proposals review core — Human Verification Gate (hvg) Task 3.
//!
//! `review_approve` / `review_reject` are thin compositions over
//! [`crate::db::commit::resolve_proposal`] with `auto_approve: false`
//! semantics: accepted entries stamp `source_type = 'user_confirmed'`, and
//! summary-update conflicts SURFACE in [`ReviewOutcome::conflicts`] instead of
//! being silently skipped. This core serves the NEW review surfaces (CLI
//! `proposals review` loop and the curated_* MCP tools); `ct approve`
//! (`cmds.rs approve_one_on`) keeps its auto-approve semantics and does NOT
//! route through this module.
//!
//! Embedding posture: an approved `fact_add` that lands NULL-embedded is
//! invisible to semantic retrieval until something re-embeds it, and the
//! runtime `embed_sweep` that would do so runs ONLY inside the Tauri app
//! (`lib::run_embedding_sweep` takes a `DbState`). A headless `--mcp` process
//! has no sweep at all, so "the sweep will fill it" is not an available
//! fallback there. Both review surfaces therefore supply embeddings:
//! [`review_approve_with_profile`] embeds inline (the CLI, which holds no
//! lock), and [`load_review_embed_inputs`] + [`embed_review_inputs`] let the
//! MCP tool compute them OUTSIDE its write lock and hand them to
//! [`ReviewOptions::entry_embeddings`].
//! Both are best-effort: a failed embed still commits, leaving NULL.

use crate::db::commit::{
    load_items, precompute_entry_embeddings, resolve_proposal, EntryEmbeddings, ResolveAudit,
    ResolveOptions,
};
use crate::db::proposals::{get_proposal_detail, ItemDecision, ItemDecisionKind};
use crate::embedder::EmbedProfile;
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection};
use serde::Serialize;

/// Reason recorded when a reviewer rejects without giving one. Canonical here
/// so the CLI loop and the MCP decide tool cannot drift to different wording
/// for the same decision.
pub const DEFAULT_REJECT_REASON: &str = "Rejected during review";

/// What a review decision did, at the granularity the review surfaces need.
#[derive(Debug, Clone, Serialize)]
pub struct ReviewOutcome {
    pub proposal_id: String,
    pub status: String,
    /// Number of proposal items that landed as accepted in the resolver.
    pub committed: usize,
    /// Item ids that hit the summary-update conflict path (entity mutated
    /// after the proposal was raised). Never silently skipped: the reviewer
    /// must see them (spec §2).
    pub conflicts: Vec<String>,
    pub reviewed_by: String,
}

/// One row of the review queue.
#[derive(Debug, Clone, Serialize)]
pub struct PendingReviewItem {
    pub proposal_id: String,
    pub proposed_name: Option<String>,
    pub kind: String,
    pub item_count: usize,
    pub evidence_chunks: usize,
    pub source_docs: Vec<String>,
    pub created_at: i64,
}

/// The operator's OS account — `USER`, or `USERNAME` on Windows, which has no
/// standard `USER`. A set-but-blank value is treated as unset: persisting an
/// empty string would erase reviewer attribution just as silently as `NULL`.
///
/// Shared by every review surface so "who reviewed this" means the same thing
/// in the CLI and over MCP.
pub fn os_account() -> Option<String> {
    ["USER", "USERNAME"].iter().find_map(|key| {
        std::env::var(key)
            .ok()
            .filter(|value| !value.trim().is_empty())
    })
}

/// How a review surface wants its decision resolved.
///
/// Every field is optional and `Default` is the plain shape; the surfaces
/// differ only in how they supply embeddings and whether the decision is an
/// audited tool call.
#[derive(Debug, Clone, Default)]
pub struct ReviewOptions {
    /// Embed inline, inside the resolver. For callers holding no lock (the CLI).
    pub embed_profile: Option<EmbedProfile>,
    /// Embeddings computed by the caller OUTSIDE its write lock, via
    /// [`precompute_review_embeddings`]. Takes precedence over `embed_profile`
    /// in the resolver.
    pub entry_embeddings: Option<EntryEmbeddings>,
    /// Audit row to write in the resolution transaction (curated tool calls).
    pub audit: Option<ResolveAudit>,
}

/// A human/agent-under-rules review decision: resolve the proposal through
/// the shared resolver with `auto_approve: false` and stamp the reviewer.
///
/// Every pending item is accepted (an all-accept decision set is built from
/// the live proposal rows); the resolver still rejects items whose own commit
/// path refuses them (e.g. summary-update conflicts), and those surface in
/// `conflicts`.
pub fn review_approve(
    conn: &mut Connection,
    proposal_id: &str,
    reviewed_by: &str,
) -> Result<ReviewOutcome> {
    review_approve_with(conn, proposal_id, reviewed_by, ReviewOptions::default())
}

/// [`review_approve`] with write-time entry embedding.
///
/// `embed_profile` is forwarded to the resolver exactly as `ct approve` does:
/// best-effort, so a failed embed still commits the proposal and leaves the
/// NULL blob for a later re-embed. The CLI review loop passes `Some(..)` so
/// approved facts are semantically searchable immediately.
pub fn review_approve_with_profile(
    conn: &mut Connection,
    proposal_id: &str,
    reviewed_by: &str,
    embed_profile: Option<EmbedProfile>,
) -> Result<ReviewOutcome> {
    review_approve_with(
        conn,
        proposal_id,
        reviewed_by,
        ReviewOptions {
            embed_profile,
            ..Default::default()
        },
    )
}

/// [`review_approve`] with the full option set.
pub fn review_approve_with(
    conn: &mut Connection,
    proposal_id: &str,
    reviewed_by: &str,
    opts: ReviewOptions,
) -> Result<ReviewOutcome> {
    let decisions = load_accept_all_decisions(conn, proposal_id)?;
    let result = resolve_proposal(
        conn,
        proposal_id,
        &decisions,
        None,
        ResolveOptions {
            auto_approve: false,
            reviewed_by: Some(reviewed_by.to_string()),
            embed_profile: opts.embed_profile,
            entry_embeddings: opts.entry_embeddings,
            audit: opts.audit,
            // Honour the on-disk `wiki.deposit_default_tier`, like every other
            // resolve path (desk, CLI approve, legacy shim, librarian). Without
            // it the resolver falls back to the shipped DEFAULT_DEPOSIT_TIER and
            // an operator who set `fact` would silently get deposit entries in
            // `wisdom` — but only when they approved through a review surface.
            deposit_default_tier: Some(crate::config::BrainConfig::deposit_default_tier_on_disk()),
        },
    )?;
    Ok(ReviewOutcome {
        proposal_id: proposal_id.to_string(),
        status: result.proposal_status,
        committed: result.committed.len(),
        conflicts: result.conflicts,
        reviewed_by: reviewed_by.to_string(),
    })
}

/// Reject every pending item in the proposal and record `reason` in
/// `curated_proposals.reject_reason`. Writes no `llm_wiki_entries`.
///
/// `reject_reason` is forwarded to the resolver's final UPDATE, which the
/// in-tx pending guard (Task 2) makes atomic against concurrent decisions.
pub fn review_reject(
    conn: &mut Connection,
    proposal_id: &str,
    reviewed_by: &str,
    reason: &str,
) -> Result<ReviewOutcome> {
    review_reject_with(
        conn,
        proposal_id,
        reviewed_by,
        reason,
        ReviewOptions::default(),
    )
}

/// [`review_reject`] with the full option set.
///
/// No `deposit_default_tier` and no embeddings: an all-reject resolution
/// writes no entries, so neither would ever be read.
pub fn review_reject_with(
    conn: &mut Connection,
    proposal_id: &str,
    reviewed_by: &str,
    reason: &str,
    opts: ReviewOptions,
) -> Result<ReviewOutcome> {
    let decisions = load_accept_all_decisions(conn, proposal_id)?;
    let rejected: Vec<ItemDecision> = decisions
        .into_iter()
        .map(|mut d| {
            d.decision = ItemDecisionKind::Reject;
            d
        })
        .collect();
    let result = resolve_proposal(
        conn,
        proposal_id,
        &rejected,
        Some(reason),
        ResolveOptions {
            auto_approve: false,
            reviewed_by: Some(reviewed_by.to_string()),
            audit: opts.audit,
            ..Default::default()
        },
    )?;
    Ok(ReviewOutcome {
        proposal_id: proposal_id.to_string(),
        status: result.proposal_status,
        committed: result.committed.len(),
        conflicts: result.conflicts,
        reviewed_by: reviewed_by.to_string(),
    })
}

/// Items + all-accept decisions for a review, loaded under whatever read lock
/// the caller holds and carried ACROSS the lock drop.
///
/// Exists so a caller that resolves under a write lock (the MCP decide tool,
/// whose `with_rw` closure holds the RW connection mutex) can pay the blocking
/// embedder round-trip while holding NO lock at all: load with
/// [`load_review_embed_inputs`], drop the guard, embed with
/// [`embed_review_inputs`], then hand the map to
/// [`ReviewOptions::entry_embeddings`]. Same three-phase shape the desk's
/// `resolve_proposal_cmd` uses to stay off the app-level mutex — split in two
/// so the read guard is never held across the network call.
pub struct ReviewEmbedInputs {
    items: Vec<crate::db::commit::LoadedItem>,
    decisions: Vec<ItemDecision>,
}

/// Phase 1: load the proposal's items and all-accept decisions. Cheap (two
/// SELECTs) and safe to run under a read lock. Errors if the proposal is not
/// pending, so a doomed decision fails before any write lock is taken.
pub fn load_review_embed_inputs(conn: &Connection, proposal_id: &str) -> Result<ReviewEmbedInputs> {
    let decisions = load_accept_all_decisions(conn, proposal_id)?;
    let items = load_items(conn, proposal_id)?;
    Ok(ReviewEmbedInputs { items, decisions })
}

/// Phase 2: the blocking embedder round-trip. Takes no connection — call it
/// with every database lock released.
///
/// Best-effort in the same way the resolver's internal precompute is: items
/// the embedder could not handle are simply absent from the map and land NULL.
pub fn embed_review_inputs(
    inputs: &ReviewEmbedInputs,
    profile: Option<&EmbedProfile>,
) -> EntryEmbeddings {
    precompute_entry_embeddings(&inputs.items, &inputs.decisions, profile)
}

/// The review queue: proposals in the requested status, oldest first.
///
/// Counts are derived from the live `curated_proposal_items` rows
/// (`item_count`, `evidence_chunks` — the latter summed across each item's
/// stored evidence array via JSON1), and `source_docs` from the joined
/// documents table, trigger role first (same ordering as the review shim).
pub fn pending_review_queue(
    conn: &Connection,
    status: &str,
    limit: usize,
) -> Result<Vec<PendingReviewItem>> {
    let mut stmt = conn.prepare(
        "SELECT p.id, p.proposed_name, p.kind, p.created_at,
                (SELECT COUNT(*) FROM curated_proposal_items i WHERE i.proposal_id = p.id),
                COALESCE((SELECT SUM(json_array_length(i.evidence))
                          FROM curated_proposal_items i WHERE i.proposal_id = p.id), 0)
         FROM curated_proposals p
         WHERE p.status = ?1
         ORDER BY p.created_at ASC, p.id ASC
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![status, limit as i64], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, i64>(4)?,
            r.get::<_, i64>(5)?,
        ))
    })?;

    let mut out = Vec::new();
    for row in rows {
        let (proposal_id, proposed_name, kind, created_at, item_count, evidence_chunks) = row?;
        let docs = source_doc_paths(conn, &proposal_id)?;
        out.push(PendingReviewItem {
            proposal_id,
            proposed_name,
            kind,
            item_count: item_count.max(0) as usize,
            evidence_chunks: evidence_chunks.max(0) as usize,
            source_docs: docs,
            created_at,
        });
    }
    Ok(out)
}

/// All-accept decisions over the proposal's live items. `get_proposal_detail`
/// is the read model the review surfaces use, so the decision set is built
/// from the same view (and inherits its JSON/deleted-source validation).
fn load_accept_all_decisions(conn: &Connection, proposal_id: &str) -> Result<Vec<ItemDecision>> {
    let detail = get_proposal_detail(conn, proposal_id)?
        .with_context(|| format!("proposal {proposal_id} not found"))?;
    if detail.status != "pending" {
        bail!("proposal {proposal_id} is not pending: {}", detail.status);
    }
    if detail.items.is_empty() {
        bail!("proposal {proposal_id} has no items");
    }
    Ok(detail
        .items
        .iter()
        .map(|item| ItemDecision {
            item_id: item.id.clone(),
            decision: ItemDecisionKind::Accept,
            edited_payload: None,
        })
        .collect())
}

fn source_doc_paths(conn: &Connection, proposal_id: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT d.path
         FROM curated_proposal_sources s
         JOIN documents d ON d.id = s.doc_id
         WHERE s.proposal_id = ?1
         ORDER BY CASE s.role WHEN 'trigger' THEN 0 ELSE 1 END, d.path",
    )?;
    let paths = stmt
        .query_map([proposal_id], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunker::{Chunk, ChunkStrategyTag};
    use crate::db::connection::open_in_memory;
    use crate::db::proposals::{
        insert_proposal, NewProposal, NewProposalItem, NewProposalSource, ProposalKind,
        ProposalSourceRole, StoredEvidenceChunk,
    };
    use crate::db::queries::{insert_chunk, upsert_document};
    use rusqlite::params;

    /// Run `body` with brain-path resolution redirected into a temp dir.
    ///
    /// Approving reads `wiki.deposit_default_tier` from the resolved config
    /// (like every other resolve path), so an unredirected test would resolve
    /// the LIVE `~/.brain` (issue #178). Same guard the review-shim tests use.
    fn with_brain<T>(body: impl FnOnce() -> T) -> T {
        let tmp = tempfile::TempDir::new().unwrap();
        let brain = tmp.path().to_string_lossy().into_owned();
        temp_env::with_vars(
            [
                ("CURATED_BRAIN_DIR", Some(brain.as_str())),
                // An inherited value would override the redirected dir.
                ("CURATED_BRAIN_CONFIG", None::<&str>),
                ("CURATED_BRAIN_DB", None::<&str>),
            ],
            body,
        )
    }

    fn seed_document(conn: &Connection, path: &str) -> i64 {
        upsert_document(conn, path, "hash").unwrap()
    }

    fn seed_chunk(conn: &Connection, doc_id: i64) -> i64 {
        let chunk = Chunk {
            text: "evidence".into(),
            start_line: 1,
            end_line: 2,
            symbol_name: None,
            defined_symbol: None,
            strategy: ChunkStrategyTag::Prose,
        };
        insert_chunk(conn, doc_id, &chunk, 0, "tier_fact", "").unwrap()
    }

    fn seed_entity(conn: &Connection, id: &str, name: &str, summary: &str, updated_at: i64) {
        conn.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
             VALUES (?1, ?2, 'concept', ?3, ?4, ?4)",
            params![id, name, summary, updated_at],
        )
        .unwrap();
    }

    fn fact_item(id: &str, chunk_id: i64, body: &str) -> NewProposalItem {
        NewProposalItem {
            id: id.into(),
            item_type: "fact_add".into(),
            target_id: None,
            payload: serde_json::json!({ "body": body, "tags": [], "confidence": "inferred" }),
            evidence: vec![StoredEvidenceChunk {
                chunk_id: Some(chunk_id),
                content_hash: String::new(),
                quote: "evidence".into(),
                start_line: Some(1),
                end_line: Some(2),
                source_kind: None,
            }],
        }
    }

    fn summary_item(id: &str, summary: &str) -> NewProposalItem {
        NewProposalItem {
            id: id.into(),
            item_type: "summary_update".into(),
            target_id: None,
            payload: serde_json::json!({ "summary": summary }),
            evidence: vec![],
        }
    }

    /// Same shape as commit.rs `insert_test_proposal` (trigger source,
    /// model "test"), with a caller-chosen proposed_name.
    fn insert_test_proposal_named(
        conn: &Connection,
        id: &str,
        kind: ProposalKind,
        entity_id: Option<&str>,
        items: Vec<NewProposalItem>,
        doc_id: i64,
        proposed_name: &str,
    ) {
        insert_proposal(
            conn,
            &NewProposal {
                id: id.into(),
                kind,
                entity_id: entity_id.map(str::to_string),
                proposed_name: Some(proposed_name.into()),
                proposed_type: Some("project".into()),
                reasoning: Some("Because.".into()),
                model: "test".into(),
            },
            &items,
            &[NewProposalSource {
                doc_id,
                role: ProposalSourceRole::Trigger,
            }],
        )
        .unwrap();
    }

    /// Seed a pending new_entity proposal with one anchored fact_add item.
    /// The doc path and proposed_name are derived from the id so that
    /// `supersede_stale_pending` (same proposed_name + same trigger doc)
    /// never supersedes a sibling seed in the same DB.
    fn seed_pending(conn: &Connection, id: &str) {
        let doc_id = seed_document(conn, &format!("/vault/documents/{id}.pdf"));
        let chunk_id = seed_chunk(conn, doc_id);
        insert_test_proposal_named(
            conn,
            id,
            ProposalKind::NewEntity,
            None,
            vec![fact_item(
                &format!("item-{id}"),
                chunk_id,
                "A verified fact.",
            )],
            doc_id,
            &format!("Project {id}"),
        );
    }

    #[test]
    fn review_approve_stamps_user_confirmed_and_reviewer() {
        with_brain(|| {
            let mut conn = open_in_memory().unwrap();
            let pid = "prop-approve-1";
            seed_pending(&conn, pid);
            let out = review_approve(&mut conn, pid, "tessera").unwrap();
            assert_eq!(out.status, "approved");
            assert_eq!(out.reviewed_by, "tessera");
            assert!(out.committed >= 1);
            let rb: Option<String> = conn
                .query_row(
                    "SELECT reviewed_by FROM curated_proposals WHERE id = ?1",
                    [pid],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(rb.as_deref(), Some("tessera"));
            let st: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM llm_wiki_entries WHERE source_type = 'user_confirmed'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(st >= 1);
        });
    }

    #[test]
    fn review_approve_surfaces_summary_conflicts() {
        with_brain(|| {
            // Mirror the commit.rs `summary_update_conflict_surfaces_for_manual_path`
            // fixture: entity mutated after the proposal was raised → the
            // summary_update item conflicts. auto_approve:false must NOT skip it
            // silently; the outcome must carry the item id.
            let mut conn = open_in_memory().unwrap();
            let doc_id = seed_document(&conn, "/vault/documents/a.pdf");
            let _chunk_id = seed_chunk(&conn, doc_id);

            insert_test_proposal_named(
                &conn,
                "prop-conflict",
                ProposalKind::UpdateEntity,
                Some("ent-1"),
                vec![summary_item("item-sum", "New summary")],
                doc_id,
                "Existing",
            );
            // Entity touched AFTER the proposal was created → conflict path.
            // insert_proposal stamps created_at with wall-clock seconds, so the
            // "entity mutated since proposal" threshold needs an updated_at in
            // the future relative to now.
            let now: i64 = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            seed_entity(&conn, "ent-1", "Existing", "Newer summary", now + 60);

            let out = review_approve(&mut conn, "prop-conflict", "tessera").unwrap();
            assert!(out.conflicts.contains(&"item-sum".to_string()));

            // The conflicting item did not write: entity summary unchanged.
            let summary: String = conn
                .query_row(
                    "SELECT summary FROM curated_entities WHERE id = 'ent-1'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(summary, "Newer summary");
        });
    }

    #[test]
    fn review_reject_rejects_all_with_reason() {
        let mut conn = open_in_memory().unwrap();
        let pid = "prop-reject-1";
        seed_pending(&conn, pid);
        let out = review_reject(&mut conn, pid, "tessera", "stale corpus").unwrap();
        assert_eq!(out.status, "rejected");
        let rr: String = conn
            .query_row(
                "SELECT reject_reason FROM curated_proposals WHERE id = ?1",
                [pid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rr, "stale corpus");
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_entries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "reject writes no entries");
    }

    #[test]
    fn review_reject_new_entity_writes_no_event_but_columns_persist() {
        let mut conn = open_in_memory().unwrap();
        let pid = "prop-reject-2";
        seed_pending(&conn, pid);
        let out = review_reject(&mut conn, pid, "tessera", "not ready").unwrap();
        assert_eq!(out.status, "rejected");

        // No resolution event for a new_entity rejection: create_entity_if_needed
        // never ran (nothing accepted) so entity_id stays NULL, and
        // write_resolution_event is gated on entity_id presence.
        let entity_id: Option<String> = conn
            .query_row(
                "SELECT entity_id FROM curated_proposals WHERE id = ?1",
                [pid],
                |r| r.get(0),
            )
            .unwrap();
        assert!(entity_id.is_none());
        let events: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(events, 0, "rejected new_entity writes no events");

        // reviewed_by + reject_reason persist.
        let (rb, rr): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT reviewed_by, reject_reason FROM curated_proposals WHERE id = ?1",
                [pid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(rb.as_deref(), Some("tessera"));
        assert_eq!(rr.as_deref(), Some("not ready"));

        // No entries, and nothing in curated_entities was created.
        let entries: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_entries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(entries, 0);
        let entities: i64 = conn
            .query_row("SELECT COUNT(*) FROM curated_entities", [], |r| r.get(0))
            .unwrap();
        assert_eq!(entities, 0, "rejected new_entity creates no entity");
    }

    #[test]
    fn pending_queue_lists_only_requested_status_with_counts() {
        with_brain(|| {
            let mut conn = open_in_memory().unwrap();
            seed_pending(&conn, "prop-q-1");
            seed_pending(&conn, "prop-q-2");
            // An approved one, which must NOT appear under "pending".
            seed_pending(&conn, "prop-q-3");
            let out = review_approve(&mut conn, "prop-q-3", "tessera").unwrap();
            assert_eq!(out.status, "approved");

            let pending = pending_review_queue(&conn, "pending", 50).unwrap();
            assert_eq!(pending.len(), 2, "only pending proposals listed");
            assert!(pending.iter().all(|p| p.proposal_id != "prop-q-3"));
            for p in &pending {
                assert!(p.item_count > 0, "item_count must be populated");
                assert!(p.evidence_chunks > 0, "evidence_chunks must be populated");
                assert_eq!(p.kind, "new_entity");
                assert!(p.proposed_name.is_some());
                assert!(
                    !p.source_docs.is_empty(),
                    "source_docs must be hydrated: {:?}",
                    p.source_docs
                );
            }

            let approved = pending_review_queue(&conn, "approved", 50).unwrap();
            assert_eq!(approved.len(), 1);
            assert_eq!(approved[0].proposal_id, "prop-q-3");
        });
    }

    #[test]
    fn review_reject_does_not_stamp_user_confirmed_entries() {
        // Spec §2: user_confirmed is an APPROVE-path stamp. A rejection writes
        // no entries at all, so source_type stays untouched everywhere.
        let mut conn = open_in_memory().unwrap();
        let pid = "prop-reject-3";
        seed_pending(&conn, pid);
        review_reject(&mut conn, pid, "tessera", "dup").unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_entries WHERE source_type = 'user_confirmed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn review_approve_second_decision_fails_already_resolved() {
        with_brain(|| {
            let mut conn = open_in_memory().unwrap();
            let pid = "prop-double-1";
            seed_pending(&conn, pid);
            let first = review_approve(&mut conn, pid, "tessera").unwrap();
            assert_eq!(first.status, "approved");
            let second = review_approve(&mut conn, pid, "tessera");
            let msg = format!("{}", second.unwrap_err());
            assert!(
                msg.contains("pending") || msg.contains("already resolved"),
                "second decision must fail with the pending/already-resolved guard, got: {msg}"
            );
        });
    }

    #[test]
    fn review_functions_reject_missing_proposal() {
        let mut conn = open_in_memory().unwrap();
        assert!(review_approve(&mut conn, "nope", "tessera").is_err());
        assert!(review_reject(&mut conn, "nope", "tessera", "r").is_err());
    }
    /// Regression: the review surfaces must honour `wiki.deposit_default_tier`
    /// like every other resolve path (desk, `ct approve`, legacy shim,
    /// librarian). Building ResolveOptions with `..Default::default()` left it
    /// `None`, so the resolver fell back to the shipped DEFAULT_DEPOSIT_TIER
    /// and an operator who configured `fact` silently got `wisdom` — but only
    /// for proposals approved through a review surface.
    #[test]
    fn review_approve_honours_the_configured_deposit_tier() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("config.json"),
            r#"{"wiki":{"deposit_default_tier":"fact"}}"#,
        )
        .unwrap();
        let brain = tmp.path().to_string_lossy().into_owned();
        temp_env::with_vars(
            [
                ("CURATED_BRAIN_DIR", Some(brain.as_str())),
                ("CURATED_BRAIN_CONFIG", None::<&str>),
                ("CURATED_BRAIN_DB", None::<&str>),
            ],
            || {
                assert_eq!(
                    crate::config::BrainConfig::deposit_default_tier_on_disk(),
                    "fact",
                    "fixture precondition: the config on disk configures `fact`"
                );
                let mut conn = open_in_memory().unwrap();
                // Deposit-origin path shape (spec §3.2) — the classifier keys
                // on `immutable-source-files`.
                let doc_id = seed_document(
                    &conn,
                    "/Users/x/Vault/immutable-source-files/agents/deposit.md",
                );
                let chunk_id = seed_chunk(&conn, doc_id);
                insert_test_proposal_named(
                    &conn,
                    "prop-deposit",
                    ProposalKind::NewEntity,
                    None,
                    vec![fact_item("item-deposit", chunk_id, "A deposited note.")],
                    doc_id,
                    "Deposit Entity",
                );
                review_approve(&mut conn, "prop-deposit", "reviewer").unwrap();

                let tier: Option<String> = conn
                    .query_row("SELECT tier FROM llm_wiki_entries LIMIT 1", [], |r| {
                        r.get(0)
                    })
                    .unwrap();
                assert_eq!(
                    tier.as_deref(),
                    Some("fact"),
                    "the review path must stamp the CONFIGURED tier, not DEFAULT_DEPOSIT_TIER"
                );
            },
        );
    }
}
