//! Legacy Review desk shims over `curated_proposals` (integer rowid ↔ proposal id).

use crate::db::commit::{resolve_proposal, CommitResult, ResolveOptions};
use crate::db::proposals::{get_proposal_detail, ItemDecision, ItemDecisionKind, ProposalDetail};
use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ShimReviewPage {
    pub id: i64,
    pub path: String,
    pub source_doc_ids: String,
    pub generated_by: String,
    pub reasoning_summary: Option<String>,
}

pub fn list_pending_review_pages(conn: &Connection) -> Result<Vec<ShimReviewPage>> {
    let mut stmt = conn.prepare(
        "SELECT rowid, id, kind, entity_id, proposed_name, reasoning, model
         FROM curated_proposals
         WHERE status = 'pending'
         ORDER BY created_at ASC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, Option<String>>(4)?,
            r.get::<_, Option<String>>(5)?,
            r.get::<_, String>(6)?,
        ))
    })?;

    let mut out = Vec::new();
    for row in rows {
        let (rowid, proposal_id, kind, entity_id, proposed_name, reasoning, model) = row?;
        let target_name =
            resolve_shim_target_name(conn, &kind, entity_id.as_deref(), proposed_name.as_deref())?;
        let source_paths = source_paths_json(conn, &proposal_id)?;
        out.push(ShimReviewPage {
            id: rowid,
            path: target_name,
            source_doc_ids: source_paths,
            generated_by: model,
            reasoning_summary: reasoning,
        });
    }
    Ok(out)
}

fn resolve_shim_target_name(
    conn: &Connection,
    kind: &str,
    entity_id: Option<&str>,
    proposed_name: Option<&str>,
) -> Result<String> {
    if kind == "new_entity" {
        return Ok(proposed_name.unwrap_or("New entity").to_string());
    }
    if let Some(eid) = entity_id {
        let name: Option<String> = conn
            .query_row(
                "SELECT name FROM curated_entities WHERE id = ?1 AND deleted_at IS NULL",
                [eid],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(n) = name {
            return Ok(n);
        }
        return Ok(eid.to_string());
    }
    Ok(proposed_name.unwrap_or("Unknown entity").to_string())
}

fn source_paths_json(conn: &Connection, proposal_id: &str) -> Result<String> {
    let mut stmt = conn.prepare(
        "SELECT d.path
         FROM curated_proposal_sources s
         JOIN documents d ON d.id = s.doc_id
         WHERE s.proposal_id = ?1
         ORDER BY CASE s.role WHEN 'trigger' THEN 0 ELSE 1 END, d.path",
    )?;
    let paths: Vec<String> = stmt
        .query_map([proposal_id], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(serde_json::to_string(&paths)?)
}

pub fn proposal_id_for_rowid(conn: &Connection, rowid: i64) -> Result<Option<String>> {
    conn.query_row(
        "SELECT id FROM curated_proposals WHERE rowid = ?1",
        [rowid],
        |r| r.get(0),
    )
    .optional()
    .map_err(Into::into)
}

pub fn format_proposal_preview(detail: &ProposalDetail) -> String {
    let mut md = format!("# {}\n\n", detail.target_name);
    if let Some(reasoning) = detail.reasoning.as_deref().filter(|s| !s.is_empty()) {
        md.push_str(reasoning);
        md.push('\n');
    }
    if detail.items.is_empty() {
        md.push_str("\n*No proposed items.*\n");
        return md;
    }

    md.push_str("\n## Proposed changes\n\n");
    for item in &detail.items {
        match item.item_type.as_str() {
            "summary_update" => {
                let summary = item
                    .payload
                    .get("summary")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                md.push_str("### Summary update\n\n");
                md.push_str(summary);
                md.push_str("\n\n");
            }
            "fact_add" => {
                let body = item
                    .payload
                    .get("body")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                md.push_str(&format!("- **Add fact:** {body}\n"));
            }
            "fact_update" => {
                let body = item
                    .payload
                    .get("body")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let tid = item.target_id.as_deref().unwrap_or("?");
                md.push_str(&format!("- **Update fact** `{tid}`: {body}\n"));
            }
            "fact_archive" => {
                let tid = item.target_id.as_deref().unwrap_or("?");
                md.push_str(&format!("- **Archive fact** `{tid}`\n"));
            }
            "edge_add" => {
                let edge_type = item
                    .payload
                    .get("edge_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("related");
                md.push_str(&format!("- **Add edge:** {edge_type}\n"));
            }
            "task_add" => {
                let desc = item
                    .payload
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                md.push_str(&format!("- **Add task:** {desc}\n"));
            }
            other => {
                md.push_str(&format!("- **{other}**\n"));
            }
        }
        if !item.evidence.is_empty() {
            md.push_str("  - Evidence:\n");
            for ev in &item.evidence {
                let quote: String = ev.quote.chars().take(120).collect();
                md.push_str(&format!("    - \"{quote}\"\n"));
            }
        }
    }
    md
}

pub fn approve_proposal_shim(
    conn: &mut Connection,
    rowid: i64,
    embed_profile: Option<&crate::embedder::EmbedProfile>,
) -> Result<CommitResult> {
    let proposal_id =
        proposal_id_for_rowid(conn, rowid)?.context("proposal not found for review id")?;
    let detail = get_proposal_detail(conn, &proposal_id)?.context("proposal detail missing")?;
    let decisions: Vec<ItemDecision> = detail
        .items
        .iter()
        .map(|item| ItemDecision {
            item_id: item.id.clone(),
            decision: ItemDecisionKind::Accept,
            edited_payload: None,
        })
        .collect();
    resolve_proposal(
        conn,
        &proposal_id,
        &decisions,
        None,
        ResolveOptions {
            auto_approve: false,
            embed_profile: embed_profile.cloned(),
            ..Default::default()
        },
    )
}

pub fn reject_proposal_shim(
    conn: &mut Connection,
    rowid: i64,
    reject_reason: Option<&str>,
    embed_profile: Option<&crate::embedder::EmbedProfile>,
) -> Result<CommitResult> {
    let proposal_id =
        proposal_id_for_rowid(conn, rowid)?.context("proposal not found for review id")?;
    let detail = get_proposal_detail(conn, &proposal_id)?.context("proposal detail missing")?;
    let decisions: Vec<ItemDecision> = detail
        .items
        .iter()
        .map(|item| ItemDecision {
            item_id: item.id.clone(),
            decision: ItemDecisionKind::Reject,
            edited_payload: None,
        })
        .collect();
    resolve_proposal(
        conn,
        &proposal_id,
        &decisions,
        reject_reason,
        ResolveOptions {
            auto_approve: false,
            embed_profile: embed_profile.cloned(),
            ..Default::default()
        },
    )
}

pub fn proposed_content_for_rowid(conn: &Connection, rowid: i64) -> Result<String> {
    let proposal_id =
        proposal_id_for_rowid(conn, rowid)?.context("proposal not found for review id")?;
    let detail = get_proposal_detail(conn, &proposal_id)?.context("proposal detail missing")?;
    Ok(format_proposal_preview(&detail))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunker::{Chunk, ChunkStrategyTag};
    use crate::db::connection::open_in_memory;
    use crate::db::proposals::{
        insert_proposal, NewProposal, NewProposalItem, NewProposalSource, ProposalFilter,
        ProposalKind, ProposalSourceRole, StoredEvidenceChunk,
    };
    use crate::db::queries::{insert_chunk, upsert_document};

    fn seed_proposal(conn: &Connection, id: &str, name: &str) -> i64 {
        let doc_id = upsert_document(conn, "/vault/documents/src.pdf", "hash").unwrap();
        let chunk = Chunk {
            text: "evidence".into(),
            start_line: 1,
            end_line: 2,
            symbol_name: None,
            defined_symbol: None,
            strategy: ChunkStrategyTag::Prose,
        };
        let chunk_id = insert_chunk(conn, doc_id, &chunk, 0, "tier_fact", "").unwrap();
        insert_proposal(
            conn,
            &NewProposal {
                id: id.into(),
                kind: ProposalKind::NewEntity,
                entity_id: None,
                proposed_name: Some(name.into()),
                proposed_type: Some("concept".into()),
                reasoning: Some("Because the doc discusses it.".into()),
                model: "test-model".into(),
            },
            &[NewProposalItem {
                id: "item-1".into(),
                item_type: "fact_add".into(),
                target_id: None,
                payload: serde_json::json!({
                    "body": "A proposed fact.",
                    "tags": [],
                    "confidence": "inferred"
                }),
                evidence: vec![StoredEvidenceChunk {
                    chunk_id: Some(chunk_id),
                    content_hash: String::new(),
                    quote: "evidence".into(),
                    start_line: Some(1),
                    end_line: Some(2),
                    source_kind: None,
                }],
            }],
            &[NewProposalSource {
                doc_id,
                role: ProposalSourceRole::Trigger,
            }],
        )
        .unwrap();
        conn.query_row(
            "SELECT rowid FROM curated_proposals WHERE id = ?1",
            [id],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn shim_queue_maps_proposal_to_review_page() {
        let conn = open_in_memory().unwrap();
        let rowid = seed_proposal(&conn, "prop-shim", "Project X");
        let pages = list_pending_review_pages(&conn).unwrap();
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].id, rowid);
        assert_eq!(pages[0].path, "Project X");
        assert_eq!(pages[0].generated_by, "test-model");
        assert_eq!(
            pages[0].reasoning_summary.as_deref(),
            Some("Because the doc discusses it.")
        );
        let sources: Vec<String> = serde_json::from_str(&pages[0].source_doc_ids).unwrap();
        assert!(sources[0].contains("src.pdf"));
    }

    #[test]
    fn format_preview_includes_facts_and_reasoning() {
        let conn = open_in_memory().unwrap();
        seed_proposal(&conn, "prop-fmt", "Alpha");
        let detail = get_proposal_detail(&conn, "prop-fmt").unwrap().unwrap();
        let md = format_proposal_preview(&detail);
        assert!(md.contains("# Alpha"));
        assert!(md.contains("Because the doc discusses it."));
        assert!(md.contains("Add fact"));
        assert!(md.contains("A proposed fact."));
    }

    #[test]
    fn approve_shim_commits_and_clears_queue() {
        let mut conn = open_in_memory().unwrap();
        let rowid = seed_proposal(&conn, "prop-approve", "Beta");
        approve_proposal_shim(&mut conn, rowid, None).unwrap();
        assert!(list_pending_review_pages(&conn).unwrap().is_empty());
        let status: String = conn
            .query_row(
                "SELECT status FROM curated_proposals WHERE id = 'prop-approve'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "approved");
    }

    #[test]
    fn reject_shim_marks_rejected() {
        let mut conn = open_in_memory().unwrap();
        let rowid = seed_proposal(&conn, "prop-reject", "Gamma");
        reject_proposal_shim(&mut conn, rowid, Some("Not relevant"), None).unwrap();
        let status: String = conn
            .query_row(
                "SELECT status FROM curated_proposals WHERE id = 'prop-reject'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "rejected");
    }

    #[test]
    fn list_proposals_matches_store() {
        let conn = open_in_memory().unwrap();
        seed_proposal(&conn, "prop-list", "Listed");
        let summaries =
            crate::db::proposals::list_proposals(&conn, &ProposalFilter::default()).unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].target_name, "Listed");
    }
}
