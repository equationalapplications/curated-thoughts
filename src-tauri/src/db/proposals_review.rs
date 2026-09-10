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
//! Embedding posture: [`review_approve_with_profile`] takes the same
//! best-effort `embed_profile` that `ct approve` passes, so an approved
//! `fact_add` is searchable immediately instead of waiting on a sweep. The
//! bare [`review_approve`] keeps the `None` shorthand (entries land
//! NULL-embedded and the runtime `embed_sweep` fills them, per
//! ResolveOptions' documented contract) for callers with no profile in hand.

use crate::db::commit::{resolve_proposal, ResolveOptions};
use crate::db::proposals::{get_proposal_detail, ItemDecision, ItemDecisionKind};
use crate::embedder::EmbedProfile;
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection};
use serde::Serialize;

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
    review_approve_with_profile(conn, proposal_id, reviewed_by, None)
}

/// [`review_approve`] with write-time entry embedding.
///
/// `embed_profile` is forwarded to the resolver exactly as `ct approve` does:
/// best-effort, so a failed embed still commits the proposal and leaves the
/// NULL blob for the runtime sweep. The CLI review loop passes `Some(..)` so
/// approved facts are semantically searchable without waiting on a sweep. The
/// MCP decide tool deliberately does NOT: its `with_rw` closure already holds
/// the RW connection mutex, and a blocking embed round-trip under that lock is
/// the exact starvation pattern the dispatcher's three-phase commits avoid.
pub fn review_approve_with_profile(
    conn: &mut Connection,
    proposal_id: &str,
    reviewed_by: &str,
    embed_profile: Option<EmbedProfile>,
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
            embed_profile,
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
    }

    #[test]
    fn review_approve_surfaces_summary_conflicts() {
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
    }

    #[test]
    fn review_functions_reject_missing_proposal() {
        let mut conn = open_in_memory().unwrap();
        assert!(review_approve(&mut conn, "nope", "tessera").is_err());
        assert!(review_reject(&mut conn, "nope", "tessera", "r").is_err());
    }
}
