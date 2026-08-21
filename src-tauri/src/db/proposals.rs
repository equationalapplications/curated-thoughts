//! CRUD for `curated_proposals` / items / sources — staging layer for OKF review.

use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalKind {
    NewEntity,
    UpdateEntity,
}

impl ProposalKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::NewEntity => "new_entity",
            Self::UpdateEntity => "update_entity",
        }
    }

    pub(crate) fn from_db(s: &str) -> Result<Self> {
        match s {
            "new_entity" => Ok(Self::NewEntity),
            "update_entity" => Ok(Self::UpdateEntity),
            other => bail!("unknown proposal kind: {other}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalSourceRole {
    Trigger,
    Evidence,
}

impl ProposalSourceRole {
    fn as_str(self) -> &'static str {
        match self {
            Self::Trigger => "trigger",
            Self::Evidence => "evidence",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredEvidenceChunk {
    /// Legacy rowid; nullable after migration. New writes always carry
    /// `content_hash` and may leave `chunk_id` as `None`.
    pub chunk_id: Option<i64>,
    /// Stable SHA-256 first-16-bytes hex. Required: empty string for
    /// pre-migration fixtures, real hash for post-migration inserts.
    pub content_hash: String,
    pub quote: String,
    pub start_line: Option<i32>,
    pub end_line: Option<i32>,
    pub source_kind: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewProposal {
    pub id: String,
    pub kind: ProposalKind,
    pub entity_id: Option<String>,
    pub proposed_name: Option<String>,
    pub proposed_type: Option<String>,
    pub reasoning: Option<String>,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewProposalItem {
    pub id: String,
    pub item_type: String,
    pub target_id: Option<String>,
    pub payload: serde_json::Value,
    pub evidence: Vec<StoredEvidenceChunk>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewProposalSource {
    pub doc_id: i64,
    pub role: ProposalSourceRole,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProposalFilter {
    /// When set, only proposals with this status (e.g. `pending`).
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposalItemCounts {
    pub total: i64,
    pub facts: i64,
    pub edges: i64,
    pub tasks: i64,
    pub summary_updates: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposalSummary {
    pub id: String,
    pub kind: ProposalKind,
    pub target_name: String,
    pub entity_id: Option<String>,
    pub source_doc_paths: Vec<String>,
    pub item_counts: ProposalItemCounts,
    pub created_at: i64,
    pub age_secs: i64,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HydratedEvidenceChunk {
    pub chunk_id: Option<i64>,
    pub quote: String,
    pub start_line: Option<i32>,
    pub end_line: Option<i32>,
    pub doc_path: Option<String>,
    pub source_deleted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposalItem {
    pub id: String,
    pub item_type: String,
    pub target_id: Option<String>,
    pub payload: serde_json::Value,
    pub evidence: Vec<HydratedEvidenceChunk>,
    pub status: String,
    pub edited_payload: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposalDetail {
    pub id: String,
    pub kind: ProposalKind,
    pub entity_id: Option<String>,
    pub proposed_name: Option<String>,
    pub proposed_type: Option<String>,
    pub target_name: String,
    pub reasoning: Option<String>,
    pub model: String,
    pub status: String,
    pub created_at: i64,
    pub source_doc_paths: Vec<String>,
    pub items: Vec<ProposalItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemDecisionKind {
    Accept,
    Reject,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemDecision {
    pub item_id: String,
    pub decision: ItemDecisionKind,
    pub edited_payload: Option<serde_json::Value>,
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn trigger_doc_id(sources: &[NewProposalSource]) -> Option<i64> {
    sources
        .iter()
        .find(|s| s.role == ProposalSourceRole::Trigger)
        .map(|s| s.doc_id)
        .or_else(|| sources.first().map(|s| s.doc_id))
}

fn supersede_stale_pending(
    conn: &Connection,
    new_id: &str,
    proposal: &NewProposal,
    trigger_doc: i64,
    now: i64,
) -> Result<()> {
    match proposal.kind {
        ProposalKind::UpdateEntity => {
            let entity_id = proposal
                .entity_id
                .as_deref()
                .context("update_entity proposal requires entity_id")?;
            conn.execute(
                "UPDATE curated_proposals
                 SET status = 'superseded', resolved_at = ?1
                 WHERE status = 'pending'
                   AND id != ?2
                   AND entity_id = ?3
                   AND EXISTS (
                     SELECT 1 FROM curated_proposal_sources s
                     WHERE s.proposal_id = curated_proposals.id
                       AND s.doc_id = ?4
                       AND s.role = 'trigger'
                   )",
                params![now, new_id, entity_id, trigger_doc],
            )?;
        }
        ProposalKind::NewEntity => {
            let proposed_name = proposal
                .proposed_name
                .as_deref()
                .context("new_entity proposal requires proposed_name")?;
            conn.execute(
                "UPDATE curated_proposals
                 SET status = 'superseded', resolved_at = ?1
                 WHERE status = 'pending'
                   AND id != ?2
                   AND kind = 'new_entity'
                   AND proposed_name = ?3
                   AND EXISTS (
                     SELECT 1 FROM curated_proposal_sources s
                     WHERE s.proposal_id = curated_proposals.id
                       AND s.doc_id = ?4
                       AND s.role = 'trigger'
                   )",
                params![now, new_id, proposed_name, trigger_doc],
            )?;
        }
    }
    Ok(())
}

/// Insert proposal + items + sources atomically; supersede older pending for same target + trigger doc.
pub fn insert_proposal(
    conn: &Connection,
    proposal: &NewProposal,
    items: &[NewProposalItem],
    sources: &[NewProposalSource],
) -> Result<()> {
    if sources.is_empty() {
        bail!("proposal requires at least one source document");
    }

    let now = now_secs();
    conn.execute_batch("BEGIN IMMEDIATE;")?;
    let result = (|| -> Result<()> {
        conn.execute(
            "INSERT INTO curated_proposals (
                id, kind, entity_id, proposed_name, proposed_type, reasoning, model, status, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending', ?8)",
            params![
                proposal.id,
                proposal.kind.as_str(),
                proposal.entity_id,
                proposal.proposed_name,
                proposal.proposed_type,
                proposal.reasoning,
                proposal.model,
                now,
            ],
        )?;

        for item in items {
            let evidence_json = serde_json::to_string(&item.evidence)?;
            let payload_json = serde_json::to_string(&item.payload)?;
            conn.execute(
                "INSERT INTO curated_proposal_items (
                    id, proposal_id, item_type, target_id, payload, evidence, status
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending')",
                params![
                    item.id,
                    proposal.id,
                    item.item_type,
                    item.target_id,
                    payload_json,
                    evidence_json,
                ],
            )?;
        }

        for source in sources {
            conn.execute(
                "INSERT INTO curated_proposal_sources (proposal_id, doc_id, role)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(proposal_id, doc_id) DO UPDATE SET
                   role = CASE
                     WHEN excluded.role = 'trigger' THEN 'trigger'
                     ELSE curated_proposal_sources.role
                   END",
                params![proposal.id, source.doc_id, source.role.as_str()],
            )?;
        }

        if let Some(trigger_doc) = trigger_doc_id(sources) {
            supersede_stale_pending(conn, &proposal.id, proposal, trigger_doc, now)?;
        }

        Ok(())
    })();

    if result.is_err() {
        let _ = conn.execute_batch("ROLLBACK;");
        return result;
    }
    conn.execute_batch("COMMIT;")?;
    Ok(())
}

fn resolve_target_name(
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

fn source_paths_for_proposal(conn: &Connection, proposal_id: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT d.path
         FROM curated_proposal_sources s
         JOIN documents d ON d.id = s.doc_id
         WHERE s.proposal_id = ?1
         ORDER BY CASE s.role WHEN 'trigger' THEN 0 ELSE 1 END, d.path",
    )?;
    let rows = stmt
        .query_map([proposal_id], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(rows)
}

fn item_counts_for_proposal(conn: &Connection, proposal_id: &str) -> Result<ProposalItemCounts> {
    let mut stmt = conn.prepare(
        "SELECT item_type, COUNT(*) FROM curated_proposal_items
         WHERE proposal_id = ?1 GROUP BY item_type",
    )?;
    let mut counts = ProposalItemCounts {
        total: 0,
        facts: 0,
        edges: 0,
        tasks: 0,
        summary_updates: 0,
    };
    let rows = stmt.query_map([proposal_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    for row in rows {
        let (item_type, n) = row?;
        counts.total += n;
        match item_type.as_str() {
            "fact_add" | "fact_update" | "fact_archive" => counts.facts += n,
            "edge_add" => counts.edges += n,
            "task_add" => counts.tasks += n,
            "summary_update" => counts.summary_updates += n,
            _ => {}
        }
    }
    Ok(counts)
}

/// Queue cards for Review mode (oldest first).
pub fn list_proposals(conn: &Connection, filter: &ProposalFilter) -> Result<Vec<ProposalSummary>> {
    let now = now_secs();
    let status = filter.status.as_deref().unwrap_or("pending");

    let mut stmt = conn.prepare(
        "SELECT id, kind, entity_id, proposed_name, model, created_at
         FROM curated_proposals
         WHERE status = ?1
         ORDER BY created_at ASC",
    )?;
    let rows: Vec<(String, String, Option<String>, Option<String>, String, i64)> = stmt
        .query_map([status], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut out = Vec::with_capacity(rows.len());
    for (id, kind_str, entity_id, proposed_name, model, created_at) in rows {
        let kind = ProposalKind::from_db(&kind_str)?;
        let target_name = resolve_target_name(
            conn,
            &kind_str,
            entity_id.as_deref(),
            proposed_name.as_deref(),
        )?;
        out.push(ProposalSummary {
            id: id.clone(),
            kind,
            target_name,
            entity_id,
            source_doc_paths: source_paths_for_proposal(conn, &id)?,
            item_counts: item_counts_for_proposal(conn, &id)?,
            created_at,
            age_secs: now.saturating_sub(created_at),
            model,
        });
    }
    Ok(out)
}

fn hydrate_evidence(
    conn: &Connection,
    stored: &[StoredEvidenceChunk],
) -> Result<Vec<HydratedEvidenceChunk>> {
    let mut out = Vec::with_capacity(stored.len());
    for chunk in stored {
        // Resolve the doc path the evidence came from. Prefer the stable
        // `content_hash` lookup (covers post-migration evidence); fall
        // back to the legacy `chunk_id` for pre-migration fixtures that
        // carry no hash. The hash must win when both are present —
        // `chunk_id` is a SQLite rowid and is re-issued every time the
        // chunker rechunks a document, so a rowid-based lookup can
        // point at an unrelated chunk after a re-chunk and silently
        // orphan a proposal's anchor.
        let resolved: Option<(String,)> = if !chunk.content_hash.is_empty() {
            conn.query_row(
                "SELECT d.path
                 FROM chunks c
                 JOIN documents d ON d.id = c.doc_id
                 WHERE c.content_hash = ?1
                 LIMIT 1",
                [&chunk.content_hash],
                |r| Ok((r.get(0)?,)),
            )
            .optional()?
        } else if let Some(cid) = chunk.chunk_id {
            conn.query_row(
                "SELECT d.path
                 FROM chunks c
                 JOIN documents d ON d.id = c.doc_id
                 WHERE c.id = ?1",
                [cid],
                |r| Ok((r.get(0)?,)),
            )
            .optional()?
        } else {
            None
        };
        let (doc_path, source_deleted) = match resolved {
            Some((path,)) => (Some(path), false),
            None => (None, true),
        };
        out.push(HydratedEvidenceChunk {
            chunk_id: chunk.chunk_id,
            quote: chunk.quote.clone(),
            start_line: chunk.start_line,
            end_line: chunk.end_line,
            doc_path,
            source_deleted,
        });
    }
    Ok(out)
}

/// Full proposal for the editorial desk — items with hydrated evidence and deleted-source flags.
pub fn get_proposal_detail(conn: &Connection, proposal_id: &str) -> Result<Option<ProposalDetail>> {
    let row = conn
        .query_row(
            "SELECT kind, entity_id, proposed_name, proposed_type, reasoning, model, status, created_at
             FROM curated_proposals WHERE id = ?1",
            [proposal_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, i64>(7)?,
                ))
            },
        )
        .optional()?;

    let Some((
        kind_str,
        entity_id,
        proposed_name,
        proposed_type,
        reasoning,
        model,
        status,
        created_at,
    )) = row
    else {
        return Ok(None);
    };

    let kind = ProposalKind::from_db(&kind_str)?;
    let target_name = resolve_target_name(
        conn,
        &kind_str,
        entity_id.as_deref(),
        proposed_name.as_deref(),
    )?;

    let mut item_stmt = conn.prepare(
        "SELECT id, item_type, target_id, payload, evidence, status, edited_payload
         FROM curated_proposal_items
         WHERE proposal_id = ?1
         ORDER BY rowid ASC",
    )?;
    let item_rows = item_stmt.query_map([proposal_id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, Option<String>>(6)?,
        ))
    })?;

    let mut items = Vec::new();
    for row in item_rows {
        let (id, item_type, target_id, payload_raw, evidence_raw, item_status, edited_raw) = row?;
        let payload: serde_json::Value = serde_json::from_str(&payload_raw)
            .with_context(|| format!("invalid payload JSON on item {id}"))?;
        let stored_evidence: Vec<StoredEvidenceChunk> = serde_json::from_str(&evidence_raw)
            .with_context(|| format!("invalid evidence JSON on item {id}"))?;
        let edited_payload = edited_raw
            .map(|s| serde_json::from_str(&s))
            .transpose()
            .with_context(|| format!("invalid edited_payload JSON on item {id}"))?;
        items.push(ProposalItem {
            id,
            item_type,
            target_id,
            payload,
            evidence: hydrate_evidence(conn, &stored_evidence)?,
            status: item_status,
            edited_payload,
        });
    }

    Ok(Some(ProposalDetail {
        id: proposal_id.to_string(),
        kind,
        entity_id,
        proposed_name,
        proposed_type,
        target_name,
        reasoning,
        model,
        status,
        created_at,
        source_doc_paths: source_paths_for_proposal(conn, proposal_id)?,
        items,
    }))
}

/// Proposals citing a document (Library reverse lookup).
pub fn list_proposals_for_document(conn: &Connection, doc_id: i64) -> Result<Vec<ProposalSummary>> {
    let mut stmt = conn.prepare(
        "SELECT p.id
         FROM curated_proposals p
         JOIN curated_proposal_sources s ON s.proposal_id = p.id
         WHERE s.doc_id = ?1 AND p.status = 'pending'
         ORDER BY p.created_at ASC",
    )?;
    let ids: Vec<String> = stmt
        .query_map([doc_id], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let pending = list_proposals(conn, &ProposalFilter {
        status: Some("pending".into()),
    })?;
    let by_id: HashMap<_, _> = pending.into_iter().map(|p| (p.id.clone(), p)).collect();
    Ok(ids.into_iter().filter_map(|id| by_id.get(&id).cloned()).collect())
}

/// Most-recent pending proposal that cites the given document path as a source.
/// Returns `Ok(None)` if no such proposal exists. Used by the ingest command to
/// emit `ingest-proposal-ready` with the new proposal id (StepWatchItThink may
/// also pick it up via the queued source path).
pub fn latest_pending_for_path(conn: &Connection, path: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare(
        "SELECT p.id
         FROM curated_proposals p
         JOIN curated_proposal_sources s ON s.proposal_id = p.id
         JOIN documents d ON d.id = s.doc_id
         WHERE d.path = ?1 AND p.status = 'pending'
         ORDER BY p.created_at DESC
         LIMIT 1",
    )?;
    let mut rows = stmt.query([path])?;
    if let Some(row) = rows.next()? {
        Ok(Some(row.get(0)?))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::open_in_memory;
    use crate::db::queries::{insert_chunk, upsert_document};
    use crate::chunker::{Chunk, ChunkStrategyTag};

    fn seed_document(conn: &Connection, path: &str) -> i64 {
        upsert_document(conn, path, "hash").unwrap()
    }

    fn seed_chunk(conn: &Connection, doc_id: i64, text: &str) -> i64 {
        let chunk = Chunk {
            text: text.into(),
            start_line: 2,
            end_line: 4,
            symbol_name: None,
            defined_symbol: None,
            strategy: ChunkStrategyTag::Prose,
        };
        insert_chunk(conn, doc_id, &chunk, 0, "tier_fact", "").unwrap()
    }

    fn sample_new_proposal(id: &str, proposed_name: &str) -> NewProposal {
        NewProposal {
            id: id.into(),
            kind: ProposalKind::NewEntity,
            entity_id: None,
            proposed_name: Some(proposed_name.into()),
            proposed_type: Some("concept".into()),
            reasoning: Some("Because the doc mentions it.".into()),
            model: "test-model".into(),
        }
    }

    fn sample_fact_item(id: &str, chunk_id: i64, quote: &str) -> NewProposalItem {
        NewProposalItem {
            id: id.into(),
            item_type: "fact_add".into(),
            target_id: None,
            payload: serde_json::json!({
                "body": "A stable fact.",
                "tags": [],
                "confidence": "inferred"
            }),
            evidence: vec![StoredEvidenceChunk {
                chunk_id: Some(chunk_id),
                content_hash: String::new(),
                quote: quote.into(),
                start_line: Some(2),
                end_line: Some(4),
                source_kind: None,
            }],
        }
    }

    #[test]
    fn insert_and_list_proposal_summary() {
        let conn = open_in_memory().unwrap();
        let doc_id = seed_document(&conn, "/vault/documents/notes.pdf");
        let chunk_id = seed_chunk(&conn, doc_id, "quoted text");

        let proposal = sample_new_proposal("prop-1", "Project X");
        insert_proposal(
            &conn,
            &proposal,
            &[sample_fact_item("item-1", chunk_id, "quoted text")],
            &[NewProposalSource {
                doc_id,
                role: ProposalSourceRole::Trigger,
            }],
        )
        .unwrap();

        let queue = list_proposals(&conn, &ProposalFilter::default()).unwrap();
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].target_name, "Project X");
        assert_eq!(queue[0].item_counts.total, 1);
        assert_eq!(queue[0].item_counts.facts, 1);
        assert!(queue[0].source_doc_paths[0].contains("notes.pdf"));
    }

    #[test]
    fn supersede_marks_older_pending_for_same_target_and_trigger() {
        let conn = open_in_memory().unwrap();
        let doc_id = seed_document(&conn, "/vault/documents/a.pdf");
        let chunk_id = seed_chunk(&conn, doc_id, "x");

        insert_proposal(
            &conn,
            &sample_new_proposal("prop-old", "Alpha"),
            &[sample_fact_item("item-old", chunk_id, "x")],
            &[NewProposalSource {
                doc_id,
                role: ProposalSourceRole::Trigger,
            }],
        )
        .unwrap();

        insert_proposal(
            &conn,
            &sample_new_proposal("prop-new", "Alpha"),
            &[sample_fact_item("item-new", chunk_id, "x")],
            &[NewProposalSource {
                doc_id,
                role: ProposalSourceRole::Trigger,
            }],
        )
        .unwrap();

        let pending = list_proposals(&conn, &ProposalFilter::default()).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, "prop-new");

        let old_status: String = conn
            .query_row(
                "SELECT status FROM curated_proposals WHERE id = 'prop-old'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(old_status, "superseded");
    }

    #[test]
    fn get_proposal_detail_hydrates_evidence() {
        let conn = open_in_memory().unwrap();
        let doc_id = seed_document(&conn, "/vault/documents/ev.pdf");
        let chunk_id = seed_chunk(&conn, doc_id, "evidence quote");

        insert_proposal(
            &conn,
            &sample_new_proposal("prop-ev", "Entity"),
            &[sample_fact_item("item-ev", chunk_id, "evidence quote")],
            &[NewProposalSource {
                doc_id,
                role: ProposalSourceRole::Trigger,
            }],
        )
        .unwrap();

        let detail = get_proposal_detail(&conn, "prop-ev").unwrap().unwrap();
        assert_eq!(detail.items.len(), 1);
        let ev = &detail.items[0].evidence[0];
        assert!(!ev.source_deleted);
        assert_eq!(ev.quote, "evidence quote");
        assert!(ev.doc_path.as_ref().unwrap().contains("ev.pdf"));
        assert_eq!(detail.reasoning.as_deref(), Some("Because the doc mentions it."));
    }

    #[test]
    fn get_proposal_detail_marks_deleted_chunk_source() {
        let conn = open_in_memory().unwrap();
        let doc_id = seed_document(&conn, "/vault/documents/gone.pdf");
        let chunk_id = seed_chunk(&conn, doc_id, "will vanish");

        insert_proposal(
            &conn,
            &sample_new_proposal("prop-del", "Gone"),
            &[sample_fact_item("item-del", chunk_id, "will vanish")],
            &[NewProposalSource {
                doc_id,
                role: ProposalSourceRole::Trigger,
            }],
        )
        .unwrap();

        conn.execute("DELETE FROM documents WHERE id = ?1", [doc_id])
            .unwrap();

        let detail = get_proposal_detail(&conn, "prop-del").unwrap().unwrap();
        let ev = &detail.items[0].evidence[0];
        assert!(ev.source_deleted);
        assert!(ev.doc_path.is_none());
        assert_eq!(ev.quote, "will vanish");
    }

    #[test]
    fn get_proposal_detail_resolves_hash_only_evidence_by_content_hash() {
        // Phase 9: post-migration evidence carries `chunk_id: None` and
        // a real `content_hash`. The hydrator must fall back to the
        // content_hash lookup so the proposal doesn't render as
        // "source deleted" while the underlying chunk still exists.
        let conn = open_in_memory().unwrap();
        let doc_id = seed_document(&conn, "/vault/documents/hashed.pdf");
        // Use a non-empty content_hash and a known text so the lookup
        // resolves to the seeded chunk.
        let chunk_id = seed_chunk(&conn, doc_id, "evidence quote");
        let hash = crate::db::chunk_hash::compute_chunk_hash(
            "evidence quote",
            "/vault/documents/hashed.pdf",
            0,
        );
        // Move the chunk_id out of reach so the legacy-rowid branch
        // can't succeed: simulate a chunk that has been replaced by
        // deleting the rowid and re-inserting under a fresh one. We
        // keep the same content_hash so the hash-based lookup resolves.
        let replacement_id: i64 = {
            let mut stmt = conn
                .prepare(
                    "INSERT INTO chunks
                         (doc_id, chunk_text, position, start_line, end_line,
                          symbol_name, strategy, defined_symbol, entity_id,
                          content_hash)
                     VALUES (?1, ?2, 1, 2, 4, NULL, 'prose', NULL, 'tier_fact', ?3)
                     RETURNING id",
                )
                .unwrap();
            stmt.query_row(
                rusqlite::params![doc_id, "evidence quote", hash],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_ne!(
            chunk_id, replacement_id,
            "precondition: replacement must be a different rowid"
        );

        let proposal = sample_new_proposal("prop-hash", "Hash");
        let mut item = sample_fact_item("item-hash", chunk_id, "evidence quote");
        item.evidence[0].chunk_id = None;
        item.evidence[0].content_hash = hash.clone();
        insert_proposal(
            &conn,
            &proposal,
            &[item],
            &[NewProposalSource {
                doc_id,
                role: ProposalSourceRole::Trigger,
            }],
        )
        .unwrap();

        let detail = get_proposal_detail(&conn, "prop-hash").unwrap().unwrap();
        let ev = &detail.items[0].evidence[0];
        assert!(
            !ev.source_deleted,
            "hash-only evidence must not be marked deleted when the chunk exists"
        );
        assert!(ev.doc_path.as_ref().unwrap().contains("hashed.pdf"));
    }

    #[test]
    fn list_proposals_for_document_filters_by_doc_id() {
        let conn = open_in_memory().unwrap();
        let doc_a = seed_document(&conn, "/vault/documents/a.pdf");
        let doc_b = seed_document(&conn, "/vault/documents/b.pdf");
        let chunk_a = seed_chunk(&conn, doc_a, "a");
        let chunk_b = seed_chunk(&conn, doc_b, "b");

        insert_proposal(
            &conn,
            &sample_new_proposal("prop-a", "A"),
            &[sample_fact_item("item-a", chunk_a, "a")],
            &[NewProposalSource {
                doc_id: doc_a,
                role: ProposalSourceRole::Trigger,
            }],
        )
        .unwrap();
        insert_proposal(
            &conn,
            &sample_new_proposal("prop-b", "B"),
            &[sample_fact_item("item-b", chunk_b, "b")],
            &[NewProposalSource {
                doc_id: doc_b,
                role: ProposalSourceRole::Trigger,
            }],
        )
        .unwrap();

        let for_a = list_proposals_for_document(&conn, doc_a).unwrap();
        assert_eq!(for_a.len(), 1);
        assert_eq!(for_a[0].id, "prop-a");
    }
}
