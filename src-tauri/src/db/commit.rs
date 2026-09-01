//! Proposal resolution — commits accepted items to `llm_wiki_*` + outbox in one transaction.

use crate::db::outbox_format::{self, OutboxOperation, OutboxPushParams};
use crate::db::proposals::{ItemDecision, ItemDecisionKind, ProposalKind, StoredEvidenceChunk};
use crate::embedder::EmbedProfile;
use anyhow::{bail, Context, Result};
use rand::Rng;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default)]
pub struct ResolveOptions {
    /// When true, summary-update conflicts are skipped silently (auto-approve path).
    pub auto_approve: bool,
    /// When `Some`, entry embeddings are computed before the commit transaction
    /// opens and stored with the new facts. `None` disables write-time
    /// embedding — rows land with NULL and the sweep fills them later. Either
    /// way the fact is committed; this only affects how soon it is searchable.
    pub embed_profile: Option<EmbedProfile>,
    /// Pre-computed entry embeddings keyed by `LoadedItem::id`. When `Some`,
    /// the resolver skips its internal `precompute_entry_embeddings` call and
    /// uses this map directly. Used by callers that want to compute the
    /// embeddings OUTSIDE an app-level mutex (the provider round-trip is
    /// blocking and must not run while the lock is held). A missing key means
    /// "no embedding available" — the entry inserts with NULL and the sweep
    /// fills it later, matching the internal-compute contract.
    pub entry_embeddings: Option<std::collections::HashMap<String, Vec<f32>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommittedRef {
    pub item_id: String,
    pub table: String,
    pub record_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitResult {
    pub committed: Vec<CommittedRef>,
    pub conflicts: Vec<String>,
    pub dropped_edges: Vec<String>,
    pub proposal_status: String,
}

pub(crate) struct LoadedProposal {
    pub(crate) id: String,
    pub(crate) kind: ProposalKind,
    pub(crate) entity_id: Option<String>,
    pub(crate) proposed_name: Option<String>,
    pub(crate) proposed_type: Option<String>,
    pub(crate) created_at: i64,
    pub(crate) status: String,
}

pub(crate) struct LoadedItem {
    pub(crate) id: String,
    pub(crate) item_type: String,
    pub(crate) target_id: Option<String>,
    pub(crate) payload: serde_json::Value,
    pub(crate) evidence: Vec<StoredEvidenceChunk>,
    pub(crate) edited_payload: Option<serde_json::Value>,
}

struct CommitContext {
    proposal_id: String,
    proposal_created_at: i64,
    entity_id: String,
    entity_name: String,
    source_type: &'static str,
    now_secs: i64,
    now_ms: i64,
    committed: Vec<CommittedRef>,
    conflicts: Vec<String>,
    dropped_edges: Vec<String>,
    accepted_count: usize,
    rejected_count: usize,
    facts_added: usize,
    facts_updated: usize,
    facts_archived: usize,
    tasks_added: usize,
    facts_duplicated: usize,
    /// Entry embeddings computed before the transaction opened, keyed by
    /// `LoadedItem::id`. A missing key means "no embedding available" — the
    /// entry is inserted with NULL and the sweep retries it.
    entry_embeddings: std::collections::HashMap<String, Vec<f32>>,
}

pub(crate) fn generate_llm_id(prefix: &str) -> String {
    let mut bytes = [0u8; 12];
    rand::rng().fill_bytes(&mut bytes);
    format!("{prefix}{}", hex::encode(bytes))
}

pub(crate) fn now_timestamps() -> (i64, i64) {
    let dur = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    (dur.as_secs() as i64, dur.as_millis() as i64)
}

/// Millisecond-precision "now" used by the two `llm_wiki_entries.deleted_at`
/// heal writers (`lib.rs:400` and `lib.rs:1414`) so they match the convention
/// every other writer in the schema uses for this column. Returns `0` if the
/// system clock is somehow before the Unix epoch (would only happen on a
/// pathological test harness).
pub fn ms_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Returns true iff `source_ref` represents a chunk/fact that still exists in
/// the vault. `source_ref` can be either a vault-relative path (legacy
/// producer contract) or the JSON `{"proposal_id":..., "evidence":[...]}`
/// shape produced by `evidence_json_with_hashes` since commit c30f141.
///
/// Empty / null / parse-error → returns `true` (no-op). The heal policy is
/// "soft-delete if the reference is *demonstrably* stale", and a row that
/// can't be parsed isn't demonstrably stale — it's a legacy path or a future
/// producer we don't know about yet. Logging is the right response, not
/// deletion. Same defensive policy applies to DB lookup errors — a failing
/// lookup is *not* the same thing as a demonstrably-stale reference, and
/// silently soft-deleting on a broken DB would amplify outages. (This is the
/// contract that the six D-tests in commit.rs lock in.)
pub fn source_ref_is_still_grounded(conn: &Connection, source_ref: &str) -> bool {
    let trimmed = source_ref.trim();
    if trimmed.is_empty() {
        return true;
    }
    // Legacy contract: a plain vault-relative path. Existence-check against
    // `documents.path = ?1` with `status='indexed'`. The legacy producer never
    // started its value with `{`, so the leading-byte test is sufficient.
    //
    // Defensive error handling: if the lookup itself errors (DB I/O,
    // corrupted schema, lock contention), we treat the reference as
    // *still-grounded* — same policy as the parse-error branch below. A DB
    // failure is not the same thing as a demonstrably-stale reference; the
    // heal policy is "soft-delete iff the reference is *demonstrably*
    // stale", and silently soft-deleting on a broken DB would amplify
    // outages. `unwrap_or(None)` previously collapsed the two cases
    // (NoRows vs Err) and could over-delete during DB faults. The operator
    // sees a warning either way.
    if !trimmed.starts_with('{') {
        match conn.query_row(
            "SELECT 1 FROM documents WHERE path = ?1 AND status = 'indexed' LIMIT 1",
            [trimmed],
            |r| r.get::<_, i64>(0),
        ) {
            Ok(_) => return true,
            Err(rusqlite::Error::QueryReturnedNoRows) => return false,
            Err(err) => {
                warn_source_ref_db_error(trimmed, "documents.path", &err);
                return true;
            }
        }
    }
    // New contract: JSON shape. Defensive parse — see comment above.
    let value: serde_json::Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(err) => {
            warn_source_ref_parse_error(trimmed, &err);
            return true;
        }
    };
    let evidence = match value.get("evidence").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => return true,
    };
    // Collect chunk_ids; if the entry has no evidence (e.g. MANUAL_SOURCE_REF
    // for user_stated facts) it's not librarian-grounded and we leave it alone.
    let chunk_ids: Vec<i64> = evidence
        .iter()
        .filter_map(|entry| entry.get("chunk_id").and_then(|v| v.as_i64()))
        .collect();
    if chunk_ids.is_empty() {
        return true;
    }
    // Any surviving chunk keeps the fact partially grounded; the soft-delete
    // policy only fires when *every* underlying chunk is gone. Same
    // defensive-on-error rule as the legacy branch above.
    for chunk_id in &chunk_ids {
        match conn.query_row(
            "SELECT 1 FROM chunks WHERE id = ?1 LIMIT 1",
            [chunk_id],
            |r| r.get::<_, i64>(0),
        ) {
            Ok(_) => return true,
            Err(rusqlite::Error::QueryReturnedNoRows) => continue,
            Err(err) => {
                warn_source_ref_db_error(&format!("chunk_id={chunk_id}"), "chunks.id", &err);
                return true;
            }
        }
    }
    false
}

#[cfg(feature = "mcp-server")]
fn warn_source_ref_parse_error(source_ref: &str, err: &serde_json::Error) {
    tracing::warn!(
        target: "ct::heal",
        source_ref = %source_ref,
        error = %err,
        "source_ref is JSON-looking but unparseable; treating as still-grounded (defensive)"
    );
}

#[cfg(not(feature = "mcp-server"))]
fn warn_source_ref_parse_error(source_ref: &str, err: &serde_json::Error) {
    eprintln!(
        "[ct::heal WARN] source_ref is JSON-looking but unparseable; treating as still-grounded: source_ref={source_ref:?} error={err}"
    );
}

/// Logs a heal-path DB lookup failure so operators can see when the
/// grounding check ran into an error (vs. a definitive NoRows). Same
/// defensive "still-grounded" policy as the parse-error branch — we
/// refuse to soft-delete when the lookup itself didn't succeed.
#[cfg(feature = "mcp-server")]
fn warn_source_ref_db_error(source_ref: &str, lookup: &str, err: &rusqlite::Error) {
    tracing::warn!(
        target: "ct::heal",
        source_ref = %source_ref,
        lookup = %lookup,
        error = %err,
        "source_ref DB lookup failed; treating as still-grounded (defensive)"
    );
}

#[cfg(not(feature = "mcp-server"))]
fn warn_source_ref_db_error(source_ref: &str, lookup: &str, err: &rusqlite::Error) {
    eprintln!(
        "[ct::heal WARN] source_ref DB lookup failed; treating as still-grounded: source_ref={source_ref:?} lookup={lookup:?} error={err}"
    );
}

fn effective_payload(item: &LoadedItem, decision: &ItemDecision) -> serde_json::Value {
    decision
        .edited_payload
        .clone()
        .or(item.edited_payload.clone())
        .unwrap_or_else(|| item.payload.clone())
}

pub(crate) fn load_proposal(conn: &Connection, proposal_id: &str) -> Result<LoadedProposal> {
    let row = conn
        .query_row(
            "SELECT kind, entity_id, proposed_name, proposed_type, created_at, status
             FROM curated_proposals WHERE id = ?1",
            [proposal_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, String>(5)?,
                ))
            },
        )
        .optional()?
        .context("proposal not found")?;
    let (kind_str, entity_id, proposed_name, proposed_type, created_at, status) = row;
    Ok(LoadedProposal {
        id: proposal_id.to_string(),
        kind: ProposalKind::from_db(&kind_str)?,
        entity_id,
        proposed_name,
        proposed_type,
        created_at,
        status,
    })
}

pub(crate) fn load_items(conn: &Connection, proposal_id: &str) -> Result<Vec<LoadedItem>> {
    let mut stmt = conn.prepare(
        "SELECT id, item_type, target_id, payload, evidence, edited_payload
         FROM curated_proposal_items
         WHERE proposal_id = ?1
         ORDER BY rowid ASC",
    )?;
    let rows = stmt.query_map([proposal_id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, Option<String>>(5)?,
        ))
    })?;

    let mut items = Vec::new();
    for row in rows {
        let (id, item_type, target_id, payload_raw, evidence_raw, edited_raw) = row?;
        let payload: serde_json::Value = serde_json::from_str(&payload_raw)
            .with_context(|| format!("invalid payload JSON on item {id}"))?;
        let evidence: Vec<StoredEvidenceChunk> = serde_json::from_str(&evidence_raw)
            .with_context(|| format!("invalid evidence JSON on item {id}"))?;
        let edited_payload = edited_raw
            .map(|s| serde_json::from_str(&s))
            .transpose()
            .with_context(|| format!("invalid edited_payload JSON on item {id}"))?;
        items.push(LoadedItem {
            id,
            item_type,
            target_id,
            payload,
            evidence,
            edited_payload,
        });
    }
    Ok(items)
}

fn entity_display_name(conn: &Connection, entity_id: &str) -> Result<String> {
    let name: Option<String> = conn
        .query_row(
            "SELECT name FROM curated_entities WHERE id = ?1",
            [entity_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(name.unwrap_or_else(|| entity_id.to_string()))
}

fn trigger_source_label(conn: &Connection, proposal_id: &str) -> Result<String> {
    let path: Option<String> = conn
        .query_row(
            "SELECT d.path
             FROM curated_proposal_sources s
             JOIN documents d ON d.id = s.doc_id
             WHERE s.proposal_id = ?1 AND s.role = 'trigger'
             LIMIT 1",
            [proposal_id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(p) = path {
        return Ok(p.rsplit('/').next().unwrap_or(&p).to_string());
    }
    let fallback: Option<String> = conn
        .query_row(
            "SELECT d.path
             FROM curated_proposal_sources s
             JOIN documents d ON d.id = s.doc_id
             WHERE s.proposal_id = ?1
             LIMIT 1",
            [proposal_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(fallback
        .map(|p| p.rsplit('/').next().unwrap_or(&p).to_string())
        .unwrap_or_else(|| "unknown source".into()))
}

pub(crate) fn fact_title_from_body(body: &str) -> String {
    let line = body.lines().next().unwrap_or(body).trim();
    if line.is_empty() {
        return "Untitled fact".into();
    }
    if line.chars().count() > 120 {
        let truncated: String = line.chars().take(117).collect();
        format!("{truncated}...")
    } else {
        line.to_string()
    }
}

/// Build a `source_ref` payload where each evidence entry's `content_hash`
/// is resolved from the `chunks` table. The chunk row is the authoritative
/// source of truth — the proposal's in-memory value (which may be empty for
/// legacy fixtures) is preferred only when non-empty, otherwise we look up
/// the chunk row. Returns an empty string when the chunk row is missing so
/// stale proposals don't surface a bogus hash. Real SQLite errors from the
/// lookup propagate as `Err` rather than being silently swallowed — a
/// poisoned connection or schema drift should fail the commit, not write
/// an empty hash into `source_ref`.
fn evidence_json_with_hashes(
    conn: &Connection,
    proposal_id: &str,
    evidence: &[StoredEvidenceChunk],
) -> Result<String> {
    let mut entries = Vec::with_capacity(evidence.len());
    for e in evidence {
        // Always prefer the chunk row's content_hash (truth on disk)
        // over the proposal's in-memory value (which may be empty).
        let resolved_hash: String = if !e.content_hash.is_empty() {
            e.content_hash.clone()
        } else if let Some(cid) = e.chunk_id {
            // `.optional()` collapses the "row missing" case to `Ok(None)`
            // (legitimate — stale proposals reference chunks that no
            // longer exist) while letting rusqlite errors propagate so a
            // poisoned connection or schema drift fails the commit.
            let from_row: Option<String> = conn
                .query_row(
                    "SELECT content_hash FROM chunks WHERE id = ?1",
                    [cid],
                    |r| r.get(0),
                )
                .optional()?;
            from_row.unwrap_or_default()
        } else {
            String::new()
        };
        entries.push(serde_json::json!({
            "chunk_id": e.chunk_id,
            "content_hash": resolved_hash,
            "quote": e.quote,
            "start_line": e.start_line,
            "end_line": e.end_line,
            "source_kind": e.source_kind,
        }));
    }
    Ok(serde_json::json!({
        "proposal_id": proposal_id,
        "evidence": entries,
    })
    .to_string())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn wiki_fact_outbox_payload(
    id: &str,
    entity_id: &str,
    title: &str,
    body: &str,
    tags: &[String],
    confidence: &str,
    source_type: &str,
    source_hash: Option<&str>,
    source_ref: &str,
    okf_type: Option<&str>,
    okf_sources: Option<&str>,
    okf_verified: Option<&str>,
    okf_usage_window: Option<&str>,
    created_at: i64,
    updated_at: i64,
    deleted_at: Option<i64>,
    lifecycle_status: Option<&str>,
    stale_after: Option<i64>,
    generated_by: Option<&str>,
    last_verified_at: Option<i64>,
    last_verified_by: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "entity_id": entity_id,
        "title": title,
        "body": body,
        "tags": tags,
        "confidence": confidence,
        "source_type": source_type,
        "source_hash": source_hash,
        "source_ref": source_ref,
        "okf_type": okf_type,
        "okf_sources": okf_sources,
        "okf_verified": okf_verified,
        "okf_usage_window": okf_usage_window,
        "lifecycle_status": lifecycle_status,
        "stale_after": stale_after,
        "generated_by": generated_by,
        "last_verified_at": last_verified_at,
        "last_verified_by": last_verified_by,
        "created_at": created_at,
        "updated_at": updated_at,
        "last_accessed_at": null,
        "access_count": 0,
        "deleted_at": deleted_at,
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn wiki_task_outbox_payload(
    id: &str,
    entity_id: &str,
    description: &str,
    status: &str,
    priority: i64,
    created_at: i64,
    updated_at: i64,
    resolved_at: Option<i64>,
    deleted_at: Option<i64>,
    okf_type: Option<&str>,
    okf_sources: Option<&str>,
    okf_verified: Option<&str>,
    okf_usage_window: Option<&str>,
    lifecycle_status: Option<&str>,
    stale_after: Option<i64>,
    generated_by: Option<&str>,
    last_verified_at: Option<i64>,
    last_verified_by: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "entity_id": entity_id,
        "description": description,
        "status": status,
        "priority": priority,
        "created_at": created_at,
        "updated_at": updated_at,
        "resolved_at": resolved_at,
        "deleted_at": deleted_at,
        "okf_type": okf_type,
        "okf_sources": okf_sources,
        "okf_verified": okf_verified,
        "okf_usage_window": okf_usage_window,
        "lifecycle_status": lifecycle_status,
        "stale_after": stale_after,
        "generated_by": generated_by,
        "last_verified_at": last_verified_at,
        "last_verified_by": last_verified_by,
    })
}

pub(crate) fn push_entries_outbox(
    conn: &Connection,
    entity_id: &str,
    record_id: &str,
    operation: OutboxOperation,
    payload: serde_json::Value,
    created_at_ms: i64,
) -> Result<()> {
    outbox_format::push_outbox_row(
        conn,
        &OutboxPushParams {
            entity_id: entity_id.into(),
            table_name: "entries".into(),
            record_id: record_id.into(),
            operation,
            payload,
        },
        Some(created_at_ms),
    )?;
    Ok(())
}

pub(crate) fn push_tasks_outbox(
    conn: &Connection,
    entity_id: &str,
    record_id: &str,
    operation: OutboxOperation,
    payload: serde_json::Value,
    created_at_ms: i64,
) -> Result<()> {
    outbox_format::push_outbox_row(
        conn,
        &OutboxPushParams {
            entity_id: entity_id.into(),
            table_name: "tasks".into(),
            record_id: record_id.into(),
            operation,
            payload,
        },
        Some(created_at_ms),
    )?;
    Ok(())
}

fn parse_string_field(payload: &serde_json::Value, field: &str) -> Result<String> {
    payload
        .get(field)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .with_context(|| format!("missing or invalid `{field}` in payload"))
}

fn parse_tags(payload: &serde_json::Value) -> Vec<String> {
    payload
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn create_entity_if_needed(
    conn: &Connection,
    proposal: &LoadedProposal,
    accepted_any: bool,
    now_secs: i64,
) -> Result<Option<String>> {
    if !accepted_any || proposal.kind != ProposalKind::NewEntity {
        return Ok(proposal.entity_id.clone());
    }
    if proposal.entity_id.is_some() {
        return Ok(proposal.entity_id.clone());
    }
    let name = proposal
        .proposed_name
        .clone()
        .context("new_entity proposal missing proposed_name")?;
    let entity_type = proposal
        .proposed_type
        .clone()
        .unwrap_or_else(|| "concept".into());
    let entity_id = generate_llm_id("ent_");
    conn.execute(
        "INSERT INTO curated_entities (id, name, entity_type, summary, summary_embedding, created_at, updated_at, deleted_at)
         VALUES (?1, ?2, ?3, '', NULL, ?4, ?4, NULL)",
        params![entity_id, name, entity_type, now_secs],
    )?;
    conn.execute(
        "UPDATE curated_proposals SET entity_id = ?1 WHERE id = ?2",
        params![entity_id, proposal.id],
    )?;
    Ok(Some(entity_id))
}

fn resolve_edge_ref(
    conn: &Connection,
    value: &serde_json::Value,
    entity_id: &str,
) -> Result<Option<String>> {
    if value == "self" || value.as_str() == Some("self") {
        return Ok(Some(entity_id.to_string()));
    }
    if let Some(id) = value.get("existing_id").and_then(|v| v.as_str()) {
        return Ok(Some(id.to_string()));
    }
    if let Some(name) = value.get("new_name").and_then(|v| v.as_str()) {
        let resolved: Option<String> = conn
            .query_row(
                "SELECT id FROM curated_entities
                 WHERE name = ?1 AND deleted_at IS NULL
                 LIMIT 1",
                [name],
                |r| r.get(0),
            )
            .optional()?;
        return Ok(resolved);
    }
    bail!("unsupported edge endpoint reference: {value}");
}

fn commit_fact_add(
    conn: &Connection,
    ctx: &mut CommitContext,
    item: &LoadedItem,
    payload: &serde_json::Value,
) -> Result<FactAddOutcome> {
    let body = parse_string_field(payload, "body")?;
    let confidence = payload
        .get("confidence")
        .and_then(|v| v.as_str())
        .unwrap_or("inferred");
    let tags = parse_tags(payload);

    // Phase-1 dedupe: exact match on normalized body, scoped to the target
    // entity. No fuzzy/similarity matching.
    let normalized = normalize_fact_body(&body);
    // TODO(pr-followup): loading every non-deleted body for the entity into
    // memory is O(N) per fact_add and unbounded as the entity grows. Consider
    // a precomputed index or `SELECT body WHERE normalized_body = ?1` if
    // entities routinely accumulate hundreds of facts. Flagged by
    // aws-cloud-agent-pr-review on PR #84 as a theoretical perf concern;
    // not blocking this PR. Filed in procedures/curated-thoughts-improvement-backlog.md.
    let existing_bodies: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT body FROM llm_wiki_entries
             WHERE entity_id = ?1 AND deleted_at IS NULL",
        )?;
        let rows = stmt.query_map(params![ctx.entity_id], |r| r.get::<_, String>(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    if existing_bodies
        .iter()
        .any(|existing| normalize_fact_body(existing) == normalized)
    {
        return Ok(FactAddOutcome::Duplicate);
    }

    let fact_id = generate_llm_id("fact_");
    let title = fact_title_from_body(&body);
    let source_ref = evidence_json_with_hashes(conn, &ctx.proposal_id, &item.evidence)?;

    let embedding_blob: Option<Vec<u8>> = ctx
        .entry_embeddings
        .get(&item.id)
        .map(|v| crate::wiki_graph::f32_vec_to_blob(v));

    conn.execute(
        "INSERT INTO llm_wiki_entries (
            id, entity_id, title, body, tags, confidence, source_type,
            source_hash, source_ref, created_at, updated_at, last_accessed_at,
            access_count, deleted_at, embedding_blob, embedding
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, ?8, ?9, ?9, NULL, 0, NULL, ?10, NULL)",
        params![
            fact_id,
            ctx.entity_id,
            title,
            body,
            serde_json::to_string(&tags)?,
            confidence,
            ctx.source_type,
            source_ref,
            ctx.now_ms,
            embedding_blob,
        ],
    )?;

    push_entries_outbox(
        conn,
        &ctx.entity_id,
        &fact_id,
        OutboxOperation::Insert,
        wiki_fact_outbox_payload(
            &fact_id,
            &ctx.entity_id,
            &title,
            &body,
            &tags,
            confidence,
            ctx.source_type,
            None,
            &source_ref,
            None,
            None,
            None,
            None,
            ctx.now_ms,
            ctx.now_ms,
            None,
            // Proposal-created facts default to the stable lifecycle; the
            // OKF v0.2 fields populate on import / verified annotation.
            Some("stable"),
            None,
            None,
            None,
            None,
        ),
        ctx.now_ms,
    )?;

    ctx.committed.push(CommittedRef {
        item_id: item.id.clone(),
        table: "entries".into(),
        record_id: fact_id,
    });
    ctx.facts_added += 1;
    Ok(FactAddOutcome::Applied)
}

fn commit_fact_update(
    conn: &Connection,
    ctx: &mut CommitContext,
    item: &LoadedItem,
    payload: &serde_json::Value,
) -> Result<()> {
    let fact_id = item
        .target_id
        .as_deref()
        .context("fact_update requires target_id")?;
    let body = parse_string_field(payload, "body")?;
    let confidence = payload
        .get("confidence")
        .and_then(|v| v.as_str())
        .unwrap_or("inferred");
    let tags = parse_tags(payload);

    // (source_ref, created_at, body, source_hash, okf_type, okf_sources,
    //  okf_verified, okf_usage_window, lifecycle_status, stale_after,
    //  generated_by, last_verified_at, last_verified_by)
    type WikiFactRow = Option<(
        String,
        i64,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
        Option<i64>,
        Option<String>,
        Option<i64>,
        Option<String>,
    )>;

    let existing: WikiFactRow = conn
        .query_row(
            // COALESCE handles imported facts with a NULL source_ref so
            // the r.get::<_, String>(0) deserializer doesn't bail before
            // the update and outbox write can proceed.
            "SELECT COALESCE(source_ref, ''), created_at,
                    body,
                    source_hash, okf_type, okf_sources, okf_verified, okf_usage_window,
                    lifecycle_status, stale_after, generated_by,
                    last_verified_at, last_verified_by
             FROM llm_wiki_entries
             WHERE id = ?1 AND entity_id = ?2 AND deleted_at IS NULL",
            params![fact_id, ctx.entity_id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                    r.get(9)?,
                    r.get(10)?,
                    r.get(11)?,
                    r.get(12)?,
                ))
            },
        )
        .optional()?;
    let Some((
        existing_source_ref,
        created_at,
        previous_body,
        existing_source_hash,
        existing_okf_type,
        existing_okf_sources,
        existing_okf_verified,
        existing_okf_usage_window,
        existing_lifecycle_status,
        existing_stale_after,
        existing_generated_by,
        existing_last_verified_at,
        existing_last_verified_by,
    )) = existing
    else {
        bail!("fact_update target not found: {fact_id}");
    };
    let body_changed = previous_body != body;

    // Write the freshly computed embedding if the pre-pass produced one;
    // otherwise NULL so the sweep re-embeds — never leave a vector describing
    // text the entry no longer contains.
    let embedding_blob: Option<Vec<u8>> = ctx
        .entry_embeddings
        .get(&item.id)
        .map(|v| crate::wiki_graph::f32_vec_to_blob(v));

    let title = fact_title_from_body(&body);

    if body_changed {
        conn.execute(
            "UPDATE llm_wiki_entries
             SET title = ?1, body = ?2, tags = ?3, confidence = ?4, updated_at = ?5,
                 embedding_blob = ?6
             WHERE id = ?7 AND entity_id = ?8",
            params![
                title,
                body,
                serde_json::to_string(&tags)?,
                confidence,
                ctx.now_ms,
                embedding_blob,
                fact_id,
                ctx.entity_id,
            ],
        )?;
    } else {
        conn.execute(
            "UPDATE llm_wiki_entries
             SET title = ?1, body = ?2, tags = ?3, confidence = ?4, updated_at = ?5
             WHERE id = ?6 AND entity_id = ?7",
            params![
                title,
                body,
                serde_json::to_string(&tags)?,
                confidence,
                ctx.now_ms,
                fact_id,
                ctx.entity_id,
            ],
        )?;
    }

    push_entries_outbox(
        conn,
        &ctx.entity_id,
        fact_id,
        OutboxOperation::Update,
        wiki_fact_outbox_payload(
            fact_id,
            &ctx.entity_id,
            &title,
            &body,
            &tags,
            confidence,
            ctx.source_type,
            existing_source_hash.as_deref(),
            &existing_source_ref,
            existing_okf_type.as_deref(),
            existing_okf_sources.as_deref(),
            existing_okf_verified.as_deref(),
            existing_okf_usage_window.as_deref(),
            created_at,
            ctx.now_ms,
            None,
            Some(existing_lifecycle_status.as_str()),
            existing_stale_after,
            existing_generated_by.as_deref(),
            existing_last_verified_at,
            existing_last_verified_by.as_deref(),
        ),
        ctx.now_ms,
    )?;

    ctx.committed.push(CommittedRef {
        item_id: item.id.clone(),
        table: "entries".into(),
        record_id: fact_id.to_string(),
    });
    ctx.facts_updated += 1;
    Ok(())
}

fn commit_fact_archive(
    conn: &Connection,
    ctx: &mut CommitContext,
    item: &LoadedItem,
) -> Result<()> {
    let fact_id = item
        .target_id
        .as_deref()
        .context("fact_archive requires target_id")?;

    let changes = conn.execute(
        "UPDATE llm_wiki_entries
         SET deleted_at = ?1, updated_at = ?1
         WHERE id = ?2 AND entity_id = ?3 AND deleted_at IS NULL",
        params![ctx.now_ms, fact_id, ctx.entity_id],
    )?;
    if changes == 0 {
        bail!("fact_archive target not found: {fact_id}");
    }

    // Edges die with their endpoints, in the same transaction (design spec §2).
    // No outbox rows: edges are not replicated.
    crate::db::edge_purge::purge_edges_for_entry(conn, fact_id)?;

    push_entries_outbox(
        conn,
        &ctx.entity_id,
        fact_id,
        OutboxOperation::Delete,
        serde_json::json!({
            "id": fact_id,
            "entity_id": ctx.entity_id,
            "deleted_at": ctx.now_ms,
        }),
        ctx.now_ms,
    )?;

    ctx.committed.push(CommittedRef {
        item_id: item.id.clone(),
        table: "entries".into(),
        record_id: fact_id.to_string(),
    });
    ctx.facts_archived += 1;
    Ok(())
}

fn commit_summary_update(
    conn: &Connection,
    ctx: &mut CommitContext,
    item: &LoadedItem,
    payload: &serde_json::Value,
    auto_approve: bool,
) -> Result<SummaryUpdateOutcome> {
    let summary = parse_string_field(payload, "summary")?;
    let entity_updated_at: i64 = conn.query_row(
        "SELECT updated_at FROM curated_entities WHERE id = ?1 AND deleted_at IS NULL",
        [&ctx.entity_id],
        |r| r.get(0),
    )?;

    if entity_updated_at > ctx.proposal_created_at {
        if auto_approve {
            return Ok(SummaryUpdateOutcome::SkippedSilent);
        }
        ctx.conflicts.push(item.id.clone());
        return Ok(SummaryUpdateOutcome::Conflict);
    }

    conn.execute(
        "UPDATE curated_entities SET summary = ?1, updated_at = ?2 WHERE id = ?3",
        params![summary, ctx.now_secs, ctx.entity_id],
    )?;

    ctx.committed.push(CommittedRef {
        item_id: item.id.clone(),
        table: "entities".into(),
        record_id: ctx.entity_id.clone(),
    });
    Ok(SummaryUpdateOutcome::Applied)
}

enum SummaryUpdateOutcome {
    Applied,
    Conflict,
    SkippedSilent,
}

enum ItemCommitOutcome {
    Applied,
    Rejected,
}

enum FactAddOutcome {
    Applied,
    /// Normalized body exactly matches an existing fact on the same entity.
    Duplicate,
}

/// Normalize a fact body for exact-match dedupe: trim edges and collapse
/// internal whitespace runs to single spaces.
fn normalize_fact_body(body: &str) -> String {
    body.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn commit_task_add(
    conn: &Connection,
    ctx: &mut CommitContext,
    item: &LoadedItem,
    payload: &serde_json::Value,
) -> Result<()> {
    let description = parse_string_field(payload, "description")?;
    let priority = payload
        .get("priority")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let task_id = generate_llm_id("task_");

    conn.execute(
        "INSERT INTO llm_wiki_tasks (
            id, entity_id, description, status, priority,
            created_at, updated_at, resolved_at, deleted_at
         ) VALUES (?1, ?2, ?3, 'pending', ?4, ?5, ?5, NULL, NULL)",
        params![task_id, ctx.entity_id, description, priority, ctx.now_ms],
    )?;

    push_tasks_outbox(
        conn,
        &ctx.entity_id,
        &task_id,
        OutboxOperation::Insert,
        wiki_task_outbox_payload(
            &task_id,
            &ctx.entity_id,
            &description,
            "pending",
            priority,
            ctx.now_ms,
            ctx.now_ms,
            None,
            None,
            None,
            None,
            None,
            None,
            // Proposal-created tasks default to the stable lifecycle; the
            // OKF v0.2 fields populate on import / verified annotation.
            Some("stable"),
            None,
            None,
            None,
            None,
        ),
        ctx.now_ms,
    )?;

    ctx.committed.push(CommittedRef {
        item_id: item.id.clone(),
        table: "tasks".into(),
        record_id: task_id,
    });
    ctx.tasks_added += 1;
    Ok(())
}

fn commit_edge_add(
    conn: &Connection,
    ctx: &mut CommitContext,
    item: &LoadedItem,
    payload: &serde_json::Value,
) -> Result<()> {
    let edge_type = parse_string_field(payload, "edge_type")?;
    let source_ref = payload
        .get("source")
        .context("edge_add payload missing source")?;
    let target_ref = payload
        .get("target")
        .context("edge_add payload missing target")?;

    let source_id = match resolve_edge_ref(conn, source_ref, &ctx.entity_id)? {
        Some(id) => id,
        None => {
            ctx.dropped_edges.push(item.id.clone());
            return Ok(());
        }
    };
    let target_id = match resolve_edge_ref(conn, target_ref, &ctx.entity_id)? {
        Some(id) => id,
        None => {
            ctx.dropped_edges.push(item.id.clone());
            return Ok(());
        }
    };

    let edge_id = generate_llm_id("edge_");
    let inserted = conn.execute(
        "INSERT OR IGNORE INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            edge_id,
            ctx.entity_id,
            source_id,
            target_id,
            edge_type,
            ctx.now_secs,
        ],
    )?;

    if inserted > 0 {
        ctx.committed.push(CommittedRef {
            item_id: item.id.clone(),
            table: "edges".into(),
            record_id: edge_id,
        });
    }
    Ok(())
}

fn write_resolution_event(
    conn: &Connection,
    ctx: &CommitContext,
    proposal_status: &str,
    source_label: &str,
) -> Result<()> {
    let event_type = match proposal_status {
        "rejected" => "rejected",
        _ => "approved",
    };
    let mut parts = Vec::new();
    if ctx.facts_added > 0 {
        parts.push(format!("{} fact(s) added", ctx.facts_added));
    }
    if ctx.facts_updated > 0 {
        parts.push(format!("{} fact(s) updated", ctx.facts_updated));
    }
    if ctx.facts_archived > 0 {
        parts.push(format!("{} fact(s) archived", ctx.facts_archived));
    }
    if ctx.tasks_added > 0 {
        parts.push(format!("{} task(s) added", ctx.tasks_added));
    }
    if ctx.facts_duplicated > 0 {
        parts.push(format!(
            "{} duplicate fact(s) skipped",
            ctx.facts_duplicated
        ));
    }

    let summary = if proposal_status == "rejected" {
        if parts.is_empty() {
            format!(
                "Rejected proposal for *{}* from *{}*",
                ctx.entity_name, source_label
            )
        } else {
            format!(
                "Rejected proposal for *{}* from *{}*: {}",
                ctx.entity_name,
                source_label,
                parts.join(", ")
            )
        }
    } else if parts.is_empty() {
        format!(
            "Approved proposal for *{}* from *{}*",
            ctx.entity_name, source_label
        )
    } else {
        format!(
            "Approved: {} to *{}* from *{}*",
            parts.join(", "),
            ctx.entity_name,
            source_label
        )
    };

    let event_id = generate_llm_id("evt_");
    conn.execute(
        "INSERT INTO llm_wiki_events (id, entity_id, event_type, summary, related_entry_id, created_at)
         VALUES (?1, ?2, ?3, ?4, NULL, ?5)",
        params![event_id, ctx.entity_id, event_type, summary, ctx.now_ms],
    )?;
    Ok(())
}

fn finalize_proposal_status(accepted: usize, rejected: usize) -> &'static str {
    if accepted == 0 {
        "rejected"
    } else if rejected == 0 {
        "approved"
    } else {
        "partial"
    }
}

/// Embed the bodies of every accepted `fact_add` in one batch.
///
/// Returns an empty map when no profile is configured or the provider fails —
/// an embedding is a derived artifact and must never fail a commit. The
/// null-embedding sweep picks up whatever is missing.
///
/// `pub(crate)` so the Tauri commands in `proposals_api.rs` can hoist this
/// call outside the app-level `DbState` mutex. Inside `resolve_proposal`
/// itself, the precompute runs while holding the SQLite IMMEDIATE
/// transaction (acquired a few lines later), but the OUTER app-level mutex
/// is what blocks other Tauri commands — and that is what this helper
/// exists to escape.
pub(crate) fn precompute_entry_embeddings(
    items: &[LoadedItem],
    decisions: &[ItemDecision],
    profile: Option<&EmbedProfile>,
) -> std::collections::HashMap<String, Vec<f32>> {
    use std::collections::HashMap;

    let Some(profile) = profile else {
        return HashMap::new();
    };

    let accepted: std::collections::HashSet<&str> = decisions
        .iter()
        .filter(|d| d.decision == ItemDecisionKind::Accept)
        .map(|d| d.item_id.as_str())
        .collect();

    let mut ids: Vec<String> = Vec::new();
    let mut texts: Vec<String> = Vec::new();
    for item in items {
        if !matches!(item.item_type.as_str(), "fact_add" | "fact_update")
            || !accepted.contains(item.id.as_str())
        {
            continue;
        }
        let decision = decisions.iter().find(|d| d.item_id == item.id);
        let payload = match decision {
            Some(d) => effective_payload(item, d),
            None => continue,
        };
        let Some(body) = payload.get("body").and_then(|v| v.as_str()) else {
            continue;
        };
        let title = fact_title_from_body(body);
        ids.push(item.id.clone());
        texts.push(crate::embed_sweep::embed_text_for_entry(&title, body));
    }

    if ids.is_empty() {
        return HashMap::new();
    }

    // Cap the batch the same way the sweep does.
    let mut out = HashMap::new();
    for (id_chunk, text_chunk) in ids
        .chunks(crate::embed_sweep::SWEEP_BATCH_SIZE)
        .zip(texts.chunks(crate::embed_sweep::SWEEP_BATCH_SIZE))
    {
        match crate::embedder::embed_batch(profile, text_chunk.to_vec()) {
            Ok(vectors) => {
                // R6 zip-truncation guard: a provider that drops a row would
                // silently mis-pair the surviving vectors with the wrong ids
                // via the zip below. Skip this chunk — those rows land with
                // NULL embeddings and the sweep fills them later.
                if vectors.len() != id_chunk.len() {
                    // codeql[rust/cleartext-logging]: vectors.len() and id_chunk.len()
                    // are local usize counts; the API key resolved inside
                    // embed_batch never reaches this string. Collapsed to a single
                    // line so CodeQL doesn't flag a per-argument sub-sink.
                    eprintln!(
                        "precompute_entry_embeddings: provider returned a mismatched \
                         number of vectors; skipping chunk to avoid mis-pairing"
                    );
                    continue;
                }
                for (id, vector) in id_chunk.iter().zip(vectors) {
                    out.insert(id.clone(), vector);
                }
            }
            Err(e) => {
                eprintln!(
                    "commit: entry embedding failed for {} items, leaving NULL for the sweep: {e}",
                    id_chunk.len()
                );
            }
        }
    }
    out
}

/// Resolve a pending proposal inside `BEGIN IMMEDIATE` — all mutations and outbox rows roll back together on failure.
pub fn resolve_proposal(
    conn: &mut Connection,
    proposal_id: &str,
    decisions: &[ItemDecision],
    reject_reason: Option<&str>,
    options: ResolveOptions,
) -> Result<CommitResult> {
    let proposal = load_proposal(conn, proposal_id)?;
    if proposal.status != "pending" {
        bail!("proposal is not pending: {}", proposal.status);
    }

    let items = load_items(conn, proposal_id)?;
    if items.is_empty() {
        bail!("proposal has no items");
    }

    let decisions_by_id: std::collections::HashMap<&str, &ItemDecision> =
        decisions.iter().map(|d| (d.item_id.as_str(), d)).collect();

    let accepted_any = items.iter().any(|item| {
        decisions_by_id
            .get(item.id.as_str())
            .is_some_and(|d| d.decision == ItemDecisionKind::Accept)
    });

    let (now_secs, now_ms) = now_timestamps();
    let source_type = if options.auto_approve {
        "librarian_inferred"
    } else {
        "user_confirmed"
    };

    // Compute entry embeddings BEFORE opening the transaction: embed_batch is a
    // blocking network call and must never run while a write lock is held.
    //
    // This necessarily happens before the dedupe check (which needs the
    // transaction), so a duplicate fact_add burns a provider call. Accepted:
    // batches are small, duplicates are rare, and hoisting dedupe out of the
    // transaction would trade that for a TOCTOU race.
    //
    // When the caller supplies `entry_embeddings`, the work was already done
    // outside the app-level `DbState` mutex — used by `resolve_proposal_cmd`
    // so the blocking round-trip does not gate every other Tauri command.
    let entry_embeddings = options.entry_embeddings.clone().unwrap_or_else(|| {
        precompute_entry_embeddings(&items, decisions, options.embed_profile.as_ref())
    });

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

    let entity_id = create_entity_if_needed(&tx, &proposal, accepted_any, now_secs)?
        .or(proposal.entity_id.clone());

    let mut ctx = CommitContext {
        proposal_id: proposal_id.to_string(),
        proposal_created_at: proposal.created_at,
        entity_id: entity_id.clone().unwrap_or_default(),
        entity_name: proposal
            .proposed_name
            .clone()
            .unwrap_or_else(|| entity_id.clone().unwrap_or_else(|| "Unknown".into())),
        source_type,
        now_secs,
        now_ms,
        committed: Vec::new(),
        conflicts: Vec::new(),
        dropped_edges: Vec::new(),
        accepted_count: 0,
        rejected_count: 0,
        facts_added: 0,
        facts_updated: 0,
        facts_archived: 0,
        tasks_added: 0,
        facts_duplicated: 0,
        entry_embeddings,
    };

    if let Some(eid) = entity_id.as_deref() {
        ctx.entity_name = if proposal.kind == ProposalKind::NewEntity {
            proposal
                .proposed_name
                .clone()
                .unwrap_or_else(|| eid.to_string())
        } else {
            entity_display_name(&tx, eid)?
        };
        ctx.entity_id = eid.to_string();
    }

    for item in &items {
        let Some(decision) = decisions_by_id.get(item.id.as_str()) else {
            ctx.rejected_count += 1;
            tx.execute(
                "UPDATE curated_proposal_items SET status = 'rejected' WHERE id = ?1",
                [&item.id],
            )?;
            continue;
        };

        if decision.decision == ItemDecisionKind::Reject {
            ctx.rejected_count += 1;
            tx.execute(
                "UPDATE curated_proposal_items SET status = 'rejected' WHERE id = ?1",
                [&item.id],
            )?;
            continue;
        }

        if entity_id.is_none() {
            let _ = tx.rollback();
            bail!("proposal has no entity_id");
        }

        let payload = effective_payload(item, decision);
        let item_outcome: Result<ItemCommitOutcome> =
            match item.item_type.as_str() {
                "fact_add" => {
                    commit_fact_add(&tx, &mut ctx, item, &payload).map(|outcome| match outcome {
                        FactAddOutcome::Applied => ItemCommitOutcome::Applied,
                        FactAddOutcome::Duplicate => {
                            ctx.facts_duplicated += 1;
                            ItemCommitOutcome::Rejected
                        }
                    })
                }
                "fact_update" => commit_fact_update(&tx, &mut ctx, item, &payload)
                    .map(|_| ItemCommitOutcome::Applied),
                "fact_archive" => {
                    commit_fact_archive(&tx, &mut ctx, item).map(|_| ItemCommitOutcome::Applied)
                }
                "summary_update" => {
                    commit_summary_update(&tx, &mut ctx, item, &payload, options.auto_approve).map(
                        |outcome| match outcome {
                            SummaryUpdateOutcome::Applied => ItemCommitOutcome::Applied,
                            SummaryUpdateOutcome::Conflict
                            | SummaryUpdateOutcome::SkippedSilent => ItemCommitOutcome::Rejected,
                        },
                    )
                }
                "task_add" => commit_task_add(&tx, &mut ctx, item, &payload)
                    .map(|_| ItemCommitOutcome::Applied),
                "edge_add" => commit_edge_add(&tx, &mut ctx, item, &payload).map(|_| {
                    if ctx.dropped_edges.iter().any(|id| id == &item.id) {
                        ItemCommitOutcome::Rejected
                    } else {
                        ItemCommitOutcome::Applied
                    }
                }),
                other => bail!("unsupported item_type: {other}"),
            };

        match item_outcome {
            Ok(ItemCommitOutcome::Applied) => {
                ctx.accepted_count += 1;
                let edited_json = decision
                    .edited_payload
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()?;
                tx.execute(
                    "UPDATE curated_proposal_items
                     SET status = 'accepted', edited_payload = COALESCE(?2, edited_payload)
                     WHERE id = ?1",
                    params![item.id, edited_json],
                )?;
            }
            Ok(ItemCommitOutcome::Rejected) => {
                ctx.rejected_count += 1;
                tx.execute(
                    "UPDATE curated_proposal_items SET status = 'rejected' WHERE id = ?1",
                    [&item.id],
                )?;
            }
            Err(e) => {
                let _ = tx.rollback();
                return Err(e);
            }
        }
    }

    let proposal_status = finalize_proposal_status(ctx.accepted_count, ctx.rejected_count);
    let source_label = trigger_source_label(&tx, proposal_id)?;
    if entity_id.is_some() {
        write_resolution_event(&tx, &ctx, proposal_status, &source_label)?;
    }

    tx.execute(
        "UPDATE curated_proposals
         SET status = ?1, resolved_at = ?2, reject_reason = ?3
         WHERE id = ?4",
        params![proposal_status, now_secs, reject_reason, proposal_id,],
    )?;

    tx.commit()?;

    Ok(CommitResult {
        committed: ctx.committed,
        conflicts: ctx.conflicts,
        dropped_edges: ctx.dropped_edges,
        proposal_status: proposal_status.to_string(),
    })
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

    /// Inserts a live `llm_wiki_entries` row directly. `deleted_at` is NULL.
    fn seed_fact_row(conn: &Connection, id: &str, entity_id: &str, body: &str) {
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                source_hash, source_ref, created_at, updated_at, last_accessed_at,
                access_count, deleted_at, embedding_blob, embedding
             ) VALUES (?1, ?2, ?3, ?4, '[]', 'inferred', 'librarian_inferred',
                       NULL, NULL, 100, 100, NULL, 0, NULL, NULL, NULL)",
            params![id, entity_id, body, body],
        )
        .unwrap();
    }

    fn seed_edge_row(conn: &Connection, id: &str, entity_id: &str, source: &str, target: &str) {
        conn.execute(
            "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
             VALUES (?1, ?2, ?3, ?4, 'related_to', 100)",
            params![id, entity_id, source, target],
        )
        .unwrap();
    }

    /// A minimal `CommitContext` for exercising a single commit_* function
    /// directly, without going through a whole proposal.
    fn test_ctx(entity_id: &str) -> CommitContext {
        CommitContext {
            proposal_id: "prop-test".into(),
            proposal_created_at: 100,
            entity_id: entity_id.to_string(),
            entity_name: "Test Entity".into(),
            source_type: "librarian_inferred",
            now_secs: 200,
            now_ms: 200_000,
            committed: Vec::new(),
            conflicts: Vec::new(),
            dropped_edges: Vec::new(),
            accepted_count: 0,
            rejected_count: 0,
            facts_added: 0,
            facts_updated: 0,
            facts_archived: 0,
            tasks_added: 0,
            facts_duplicated: 0,
            entry_embeddings: std::collections::HashMap::new(),
        }
    }

    fn test_item(id: &str, item_type: &str, target_id: Option<&str>) -> LoadedItem {
        LoadedItem {
            id: id.into(),
            item_type: item_type.into(),
            target_id: target_id.map(|s| s.to_string()),
            payload: serde_json::json!({}),
            evidence: Vec::new(),
            edited_payload: None,
        }
    }

    #[test]
    fn archiving_a_fact_purges_its_edges() {
        let conn = open_in_memory().unwrap();
        seed_entity(&conn, "ent-1", "Existing", "Summary", 100);
        seed_fact_row(&conn, "fact_a", "ent-1", "Body A");
        seed_fact_row(&conn, "fact_b", "ent-1", "Body B");
        seed_fact_row(&conn, "fact_c", "ent-1", "Body C");
        seed_edge_row(&conn, "edge_out", "ent-1", "fact_a", "fact_b");
        seed_edge_row(&conn, "edge_in", "ent-1", "fact_c", "fact_a");
        seed_edge_row(&conn, "edge_other", "ent-1", "fact_b", "fact_c");

        // R1 (remediation): the new heterogeneous contract only purges edges
        // whose partner is also dead in every endpoint table. Soft-delete
        // fact_b and fact_c so the cascade treats their edges as purgeable.
        // Without this, edge_out and edge_in would survive because fact_b /
        // fact_c remain alive in `llm_wiki_entries`.
        conn.execute(
            "UPDATE llm_wiki_entries SET deleted_at = 100 WHERE id IN ('fact_b', 'fact_c')",
            [],
        )
        .unwrap();

        let mut ctx = test_ctx("ent-1");
        let item = test_item("item-1", "fact_archive", Some("fact_a"));
        commit_fact_archive(&conn, &mut ctx, &item).unwrap();

        // The entry is soft-deleted...
        let deleted_at: Option<i64> = conn
            .query_row(
                "SELECT deleted_at FROM llm_wiki_entries WHERE id = 'fact_a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(deleted_at, Some(200_000), "deleted_at is milliseconds");

        // ...and no edge references it any more.
        let dangling: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_edges
                  WHERE source_id = 'fact_a' OR target_id = 'fact_a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(dangling, 0, "archived entry must leave no ghost edges");

        // edge_other (fact_b → fact_c) is not touched by the cascade from
        // fact_a because fact_a is on neither endpoint. Under the R1
        // contract it survives even though both its partners are soft-
        // deleted; the broader `purge_orphan_edges` would clean it up.
        let survivors: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!(survivors, 1);
    }

    #[test]
    fn updating_a_body_replaces_the_stale_embedding() {
        let conn = open_in_memory().unwrap();
        seed_entity(&conn, "ent-1", "Existing", "Summary", 100);
        seed_fact_row(&conn, "fact_a", "ent-1", "The original body.");
        // Give it a blob describing the ORIGINAL text.
        conn.execute(
            "UPDATE llm_wiki_entries SET embedding_blob = ?1 WHERE id = 'fact_a'",
            params![vec![1u8; 32]],
        )
        .unwrap();

        let mut ctx = test_ctx("ent-1");
        // No pre-computed embedding for this item -> the stale blob must be
        // cleared, not left describing text the entry no longer has.
        let item = test_item("item-1", "fact_update", Some("fact_a"));
        let payload = serde_json::json!({ "body": "A completely different body." });
        commit_fact_update(&conn, &mut ctx, &item, &payload).unwrap();

        let blob: Option<Vec<u8>> = conn
            .query_row(
                "SELECT embedding_blob FROM llm_wiki_entries WHERE id = 'fact_a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            blob, None,
            "a changed body must never keep the vector of the old text"
        );
    }

    #[test]
    fn updating_a_body_stores_a_fresh_embedding_when_one_was_precomputed() {
        let conn = open_in_memory().unwrap();
        seed_entity(&conn, "ent-1", "Existing", "Summary", 100);
        seed_fact_row(&conn, "fact_a", "ent-1", "The original body.");
        conn.execute(
            "UPDATE llm_wiki_entries SET embedding_blob = ?1 WHERE id = 'fact_a'",
            params![vec![1u8; 32]],
        )
        .unwrap();

        let mut ctx = test_ctx("ent-1");
        ctx.entry_embeddings
            .insert("item-1".to_string(), vec![0.5_f32; 8]);
        let item = test_item("item-1", "fact_update", Some("fact_a"));
        let payload = serde_json::json!({ "body": "A completely different body." });
        commit_fact_update(&conn, &mut ctx, &item, &payload).unwrap();

        let blob: Option<Vec<u8>> = conn
            .query_row(
                "SELECT embedding_blob FROM llm_wiki_entries WHERE id = 'fact_a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            blob,
            Some(crate::wiki_graph::f32_vec_to_blob(&[0.5_f32; 8])),
            "the fresh vector replaces the stale one"
        );
    }

    #[test]
    fn archiving_pushes_no_edge_outbox_rows() {
        let conn = open_in_memory().unwrap();
        seed_entity(&conn, "ent-1", "Existing", "Summary", 100);
        seed_fact_row(&conn, "fact_a", "ent-1", "Body A");
        seed_fact_row(&conn, "fact_b", "ent-1", "Body B");
        seed_edge_row(&conn, "edge_ab", "ent-1", "fact_a", "fact_b");

        let mut ctx = test_ctx("ent-1");
        let item = test_item("item-1", "fact_archive", Some("fact_a"));
        commit_fact_archive(&conn, &mut ctx, &item).unwrap();

        // Edges are not replicated (spec §2). Only the entries delete is in the outbox.
        let edge_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_outbox WHERE table_name = 'edges'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(edge_rows, 0, "edge purges must not emit outbox rows");
    }

    fn insert_test_proposal(
        conn: &Connection,
        id: &str,
        kind: ProposalKind,
        entity_id: Option<&str>,
        items: Vec<NewProposalItem>,
        doc_id: i64,
    ) {
        insert_proposal(
            conn,
            &NewProposal {
                id: id.into(),
                kind,
                entity_id: entity_id.map(str::to_string),
                proposed_name: Some("Project X".into()),
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

    #[test]
    fn resolve_new_entity_creates_entity_and_fact_with_outbox() {
        let mut conn = open_in_memory().unwrap();
        let doc_id = seed_document(&conn, "/vault/documents/notes.pdf");
        let chunk_id = seed_chunk(&conn, doc_id);
        insert_test_proposal(
            &conn,
            "prop-1",
            ProposalKind::NewEntity,
            None,
            vec![fact_item("item-1", chunk_id, "A new fact.")],
            doc_id,
        );

        let result = resolve_proposal(
            &mut conn,
            "prop-1",
            &[ItemDecision {
                item_id: "item-1".into(),
                decision: ItemDecisionKind::Accept,
                edited_payload: None,
            }],
            None,
            ResolveOptions {
                auto_approve: false,
                embed_profile: None,
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(result.proposal_status, "approved");
        assert_eq!(result.committed.len(), 1);
        assert_eq!(result.committed[0].table, "entries");

        let entity_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM curated_entities", [], |r| r.get(0))
            .unwrap();
        assert_eq!(entity_count, 1);

        let fact_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_entries WHERE source_type = 'user_confirmed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(fact_count, 1);

        let outbox_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_outbox WHERE table_name = 'entries' AND operation = 'INSERT'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(outbox_count, 1);

        // The proposal-insert outbox payload must carry the persisted
        // lifecycle_status (defaults to "stable" for newly committed facts).
        let payload_lifecycle: String = conn
            .query_row(
                "SELECT json_extract(payload, '$.lifecycle_status')
                 FROM llm_wiki_outbox
                 WHERE table_name = 'entries' AND operation = 'INSERT'
                 ORDER BY id ASC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(payload_lifecycle, "stable");

        let event_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(event_count, 1);
    }

    #[test]
    fn partial_approval_marks_proposal_partial() {
        let mut conn = open_in_memory().unwrap();
        let doc_id = seed_document(&conn, "/vault/documents/a.pdf");
        let chunk_id = seed_chunk(&conn, doc_id);
        seed_entity(&conn, "ent-1", "Existing", "Summary", 100);
        insert_test_proposal(
            &conn,
            "prop-partial",
            ProposalKind::UpdateEntity,
            Some("ent-1"),
            vec![
                fact_item("item-a", chunk_id, "Keep me."),
                fact_item("item-b", chunk_id, "Drop me."),
            ],
            doc_id,
        );

        let result = resolve_proposal(
            &mut conn,
            "prop-partial",
            &[
                ItemDecision {
                    item_id: "item-a".into(),
                    decision: ItemDecisionKind::Accept,
                    edited_payload: None,
                },
                ItemDecision {
                    item_id: "item-b".into(),
                    decision: ItemDecisionKind::Reject,
                    edited_payload: None,
                },
            ],
            None,
            ResolveOptions {
                auto_approve: false,
                embed_profile: None,
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(result.proposal_status, "partial");
        assert_eq!(result.committed.len(), 1);

        let accepted: String = conn
            .query_row(
                "SELECT status FROM curated_proposal_items WHERE id = 'item-a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(accepted, "accepted");
    }

    #[test]
    fn edited_payload_wins_over_stored_payload() {
        let mut conn = open_in_memory().unwrap();
        let doc_id = seed_document(&conn, "/vault/documents/a.pdf");
        let chunk_id = seed_chunk(&conn, doc_id);
        seed_entity(&conn, "ent-1", "Existing", "Summary", 100);
        insert_test_proposal(
            &conn,
            "prop-edit",
            ProposalKind::UpdateEntity,
            Some("ent-1"),
            vec![fact_item("item-a", chunk_id, "Original body.")],
            doc_id,
        );

        resolve_proposal(
            &mut conn,
            "prop-edit",
            &[ItemDecision {
                item_id: "item-a".into(),
                decision: ItemDecisionKind::Accept,
                edited_payload: Some(serde_json::json!({
                    "body": "Edited body.",
                    "tags": ["edited"],
                    "confidence": "certain"
                })),
            }],
            None,
            ResolveOptions {
                auto_approve: false,
                embed_profile: None,
                ..Default::default()
            },
        )
        .unwrap();

        let body: String = conn
            .query_row("SELECT body FROM llm_wiki_entries LIMIT 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(body, "Edited body.");
    }

    #[test]
    fn fact_update_succeeds_when_source_ref_is_null() {
        // Regression: imported facts can carry a NULL source_ref. The
        // row-mapping closure in commit_fact_update used to fail to
        // deserialize NULL into String before the update ran, so the
        // proposal resolution would error and the import + manual edit
        // couldn't be reconciled.
        let mut conn = open_in_memory().unwrap();
        let doc_id = seed_document(&conn, "/vault/documents/a.pdf");
        let chunk_id = seed_chunk(&conn, doc_id);
        seed_entity(&conn, "ent-1", "Existing", "Summary", 100);

        // Seed an entry whose source_ref is NULL (the path bundle_apply
        // can produce when a fact is imported without a `resource`).
        conn.execute(
            "INSERT INTO llm_wiki_entries
                (id, entity_id, title, body, tags, confidence, source_type,
                 source_ref, created_at, updated_at)
             VALUES ('fact-imported', 'ent-1', 'Original', 'Original body.',
                     '[]', 'inferred', 'librarian_inferred',
                     NULL, 100, 100)",
            [],
        )
        .unwrap();

        insert_test_proposal(
            &conn,
            "prop-null-ref",
            ProposalKind::UpdateEntity,
            Some("ent-1"),
            vec![NewProposalItem {
                id: "item-update-null-ref".into(),
                item_type: "fact_update".into(),
                target_id: Some("fact-imported".into()),
                payload: serde_json::json!({
                    "body": "Edited body.",
                    "tags": [],
                    "confidence": "inferred",
                }),
                evidence: vec![StoredEvidenceChunk {
                    chunk_id: Some(chunk_id),
                    content_hash: String::new(),
                    quote: "x".into(),
                    start_line: Some(1),
                    end_line: Some(1),
                    source_kind: None,
                }],
            }],
            doc_id,
        );

        let result = resolve_proposal(
            &mut conn,
            "prop-null-ref",
            &[ItemDecision {
                item_id: "item-update-null-ref".into(),
                decision: ItemDecisionKind::Accept,
                edited_payload: None,
            }],
            None,
            ResolveOptions {
                auto_approve: false,
                embed_profile: None,
                ..Default::default()
            },
        )
        .expect("fact_update must succeed when source_ref is NULL");

        assert_eq!(result.proposal_status, "approved");
        let body: String = conn
            .query_row(
                "SELECT body FROM llm_wiki_entries WHERE id = 'fact-imported'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(body, "Edited body.");
        // The outbox UPDATE payload should carry the coalesced empty string
        // (not error) so downstream consumers can decode the row.
        let payload_source_ref: String = conn
            .query_row(
                "SELECT json_extract(payload, '$.source_ref')
                 FROM llm_wiki_outbox
                 WHERE record_id = 'fact-imported' AND operation = 'UPDATE'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(payload_source_ref, "");
    }

    #[test]
    fn summary_update_conflict_surfaces_for_manual_path() {
        let mut conn = open_in_memory().unwrap();
        let doc_id = seed_document(&conn, "/vault/documents/a.pdf");
        let _chunk_id = seed_chunk(&conn, doc_id);

        conn.execute(
            "INSERT INTO curated_proposals (id, kind, entity_id, model, status, created_at)
             VALUES ('prop-conflict', 'update_entity', 'ent-1', 'test', 'pending', 100)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO curated_proposal_items (id, proposal_id, item_type, payload, evidence)
             VALUES ('item-sum', 'prop-conflict', 'summary_update', '{\"summary\":\"New summary\"}', '[]')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO curated_proposal_sources (proposal_id, doc_id, role) VALUES ('prop-conflict', ?1, 'trigger')",
            [doc_id],
        )
        .unwrap();
        seed_entity(&conn, "ent-1", "Existing", "Old summary", 200);

        let result = resolve_proposal(
            &mut conn,
            "prop-conflict",
            &[ItemDecision {
                item_id: "item-sum".into(),
                decision: ItemDecisionKind::Accept,
                edited_payload: None,
            }],
            None,
            ResolveOptions {
                auto_approve: false,
                embed_profile: None,
                ..Default::default()
            },
        )
        .unwrap();

        assert!(result.conflicts.contains(&"item-sum".to_string()));
        assert_eq!(result.proposal_status, "rejected");

        let summary: String = conn
            .query_row(
                "SELECT summary FROM curated_entities WHERE id = 'ent-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(summary, "Old summary");
    }

    #[test]
    fn edge_new_name_unresolved_is_dropped() {
        let mut conn = open_in_memory().unwrap();
        let doc_id = seed_document(&conn, "/vault/documents/a.pdf");
        let chunk_id = seed_chunk(&conn, doc_id);
        seed_entity(&conn, "ent-1", "Existing", "Summary", 100);

        insert_test_proposal(
            &conn,
            "prop-edge",
            ProposalKind::UpdateEntity,
            Some("ent-1"),
            vec![NewProposalItem {
                id: "edge-1".into(),
                item_type: "edge_add".into(),
                target_id: None,
                payload: serde_json::json!({
                    "source": "self",
                    "target": { "new_name": "Missing Entity" },
                    "edge_type": "related_to"
                }),
                evidence: vec![StoredEvidenceChunk {
                    chunk_id: Some(chunk_id),
                    content_hash: String::new(),
                    quote: "x".into(),
                    start_line: Some(1),
                    end_line: Some(1),
                    source_kind: None,
                }],
            }],
            doc_id,
        );

        let result = resolve_proposal(
            &mut conn,
            "prop-edge",
            &[ItemDecision {
                item_id: "edge-1".into(),
                decision: ItemDecisionKind::Accept,
                edited_payload: None,
            }],
            None,
            ResolveOptions {
                auto_approve: false,
                embed_profile: None,
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(result.dropped_edges, vec!["edge-1".to_string()]);
        let edge_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!(edge_count, 0);
    }

    #[test]
    fn edge_insert_or_ignore_dedupes() {
        let mut conn = open_in_memory().unwrap();
        let doc_id = seed_document(&conn, "/vault/documents/a.pdf");
        let chunk_id = seed_chunk(&conn, doc_id);
        seed_entity(&conn, "ent-1", "Existing", "Summary", 100);
        seed_entity(&conn, "ent-2", "Other", "Summary", 100);

        conn.execute(
            "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
             VALUES ('edge-existing', 'ent-1', 'ent-1', 'ent-2', 'related_to', 100)",
            [],
        )
        .unwrap();

        insert_test_proposal(
            &conn,
            "prop-dedupe",
            ProposalKind::UpdateEntity,
            Some("ent-1"),
            vec![NewProposalItem {
                id: "edge-dup".into(),
                item_type: "edge_add".into(),
                target_id: None,
                payload: serde_json::json!({
                    "source": { "existing_id": "ent-1" },
                    "target": { "existing_id": "ent-2" },
                    "edge_type": "related_to"
                }),
                evidence: vec![StoredEvidenceChunk {
                    chunk_id: Some(chunk_id),
                    content_hash: String::new(),
                    quote: "x".into(),
                    start_line: Some(1),
                    end_line: Some(1),
                    source_kind: None,
                }],
            }],
            doc_id,
        );

        let result = resolve_proposal(
            &mut conn,
            "prop-dedupe",
            &[ItemDecision {
                item_id: "edge-dup".into(),
                decision: ItemDecisionKind::Accept,
                edited_payload: None,
            }],
            None,
            ResolveOptions {
                auto_approve: false,
                embed_profile: None,
                ..Default::default()
            },
        )
        .unwrap();

        assert!(result.committed.is_empty());
        let edge_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!(edge_count, 1);
    }

    #[test]
    fn failed_commit_leaves_no_outbox_rows() {
        let mut conn = open_in_memory().unwrap();
        let doc_id = seed_document(&conn, "/vault/documents/a.pdf");
        let chunk_id = seed_chunk(&conn, doc_id);
        seed_entity(&conn, "ent-1", "Existing", "Summary", 100);

        insert_test_proposal(
            &conn,
            "prop-fail",
            ProposalKind::UpdateEntity,
            Some("ent-1"),
            vec![NewProposalItem {
                id: "bad-update".into(),
                item_type: "fact_update".into(),
                target_id: Some("missing-fact".into()),
                payload: serde_json::json!({ "body": "nope", "tags": [], "confidence": "inferred" }),
                evidence: vec![StoredEvidenceChunk {
                    chunk_id: Some(chunk_id),
                    content_hash: String::new(),
                    quote: "x".into(),
                    start_line: Some(1),
                    end_line: Some(1),
                    source_kind: None,
                }],
            }],
            doc_id,
        );

        let err = resolve_proposal(
            &mut conn,
            "prop-fail",
            &[ItemDecision {
                item_id: "bad-update".into(),
                decision: ItemDecisionKind::Accept,
                edited_payload: None,
            }],
            None,
            ResolveOptions {
                auto_approve: false,
                embed_profile: None,
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("not found"));

        let outbox_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_outbox", [], |r| r.get(0))
            .unwrap();
        assert_eq!(outbox_count, 0);

        let status: String = conn
            .query_row(
                "SELECT status FROM curated_proposals WHERE id = 'prop-fail'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "pending");
    }

    #[test]
    fn resolution_events_use_approved_rejected_types() {
        let mut conn = open_in_memory().unwrap();
        let doc_id = seed_document(&conn, "/vault/documents/notes.pdf");
        let chunk_id = seed_chunk(&conn, doc_id);

        // Approve a proposal (NewEntity)
        insert_test_proposal(
            &conn,
            "prop-approve",
            ProposalKind::NewEntity,
            None,
            vec![fact_item("item-approve", chunk_id, "Approved fact.")],
            doc_id,
        );
        resolve_proposal(
            &mut conn,
            "prop-approve",
            &[ItemDecision {
                item_id: "item-approve".into(),
                decision: ItemDecisionKind::Accept,
                edited_payload: None,
            }],
            None,
            ResolveOptions {
                auto_approve: false,
                embed_profile: None,
                ..Default::default()
            },
        )
        .unwrap();

        // Reject a proposal (UpdateEntity with existing entity so the resolve path reaches write_resolution_event)
        seed_entity(&conn, "ent-1", "Project X", "Summary", 100);
        insert_test_proposal(
            &conn,
            "prop-reject",
            ProposalKind::UpdateEntity,
            Some("ent-1"),
            vec![fact_item("item-reject", chunk_id, "Rejected fact.")],
            doc_id,
        );
        resolve_proposal(
            &mut conn,
            "prop-reject",
            &[ItemDecision {
                item_id: "item-reject".into(),
                decision: ItemDecisionKind::Reject,
                edited_payload: None,
            }],
            Some("not relevant"),
            ResolveOptions {
                auto_approve: false,
                embed_profile: None,
                ..Default::default()
            },
        )
        .unwrap();

        let approved_type: String = conn
            .query_row(
                "SELECT event_type FROM llm_wiki_events WHERE summary LIKE 'Approved%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(approved_type, "approved");

        let rejected_type: String = conn
            .query_row(
                "SELECT event_type FROM llm_wiki_events WHERE event_type = 'rejected'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rejected_type, "rejected");
    }

    #[test]
    fn resolve_proposal_writes_content_hash_in_source_ref() {
        let mut conn = open_in_memory().unwrap();
        let doc_id = seed_document(&conn, "/vault/documents/note.pdf");
        // Pre-seed a chunk with a real content_hash; the commit must
        // look this up and write it into the source_ref JSON.
        let chunk_id = seed_chunk(&conn, doc_id);
        let hash =
            crate::db::chunk_hash::compute_chunk_hash("quoted", "/vault/documents/note.pdf", 0);
        conn.execute(
            "UPDATE chunks SET content_hash = ?1 WHERE id = ?2",
            params![hash, chunk_id],
        )
        .unwrap();

        insert_test_proposal(
            &conn,
            "prop-hash",
            ProposalKind::NewEntity,
            None,
            vec![NewProposalItem {
                id: "item-h".into(),
                item_type: "fact_add".into(),
                target_id: None,
                payload: serde_json::json!({ "body": "Hashed fact.", "tags": [], "confidence": "inferred" }),
                evidence: vec![StoredEvidenceChunk {
                    chunk_id: Some(chunk_id),
                    content_hash: String::new(), // commit must look up the real hash
                    quote: "quoted".into(),
                    start_line: Some(1),
                    end_line: Some(2),
                    source_kind: None,
                }],
            }],
            doc_id,
        );

        resolve_proposal(
            &mut conn,
            "prop-hash",
            &[ItemDecision {
                item_id: "item-h".into(),
                decision: ItemDecisionKind::Accept,
                edited_payload: None,
            }],
            None,
            ResolveOptions {
                auto_approve: false,
                embed_profile: None,
                ..Default::default()
            },
        )
        .unwrap();

        let source_ref: String = conn
            .query_row(
                "SELECT source_ref FROM llm_wiki_entries ORDER BY id DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&source_ref).unwrap();
        let evidence = parsed.get("evidence").unwrap().as_array().unwrap();
        let entry = evidence[0].as_object().unwrap();
        assert_eq!(
            entry.get("content_hash").and_then(|v| v.as_str()).unwrap(),
            hash,
            "commit must populate content_hash from the chunk row"
        );
    }

    fn resolve_fact(conn: &mut Connection, prop_id: &str, item_id: &str) -> CommitResult {
        resolve_proposal(
            conn,
            prop_id,
            &[ItemDecision {
                item_id: item_id.into(),
                decision: ItemDecisionKind::Accept,
                edited_payload: None,
            }],
            None,
            ResolveOptions {
                auto_approve: false,
                embed_profile: None,
                ..Default::default()
            },
        )
        .unwrap()
    }

    #[test]
    fn fact_add_identical_body_dedupes() {
        let mut conn = open_in_memory().unwrap();
        let doc_id = seed_document(&conn, "/vault/documents/a.pdf");
        let chunk_id = seed_chunk(&conn, doc_id);
        seed_entity(&conn, "ent-1", "Existing", "Summary", 100);

        insert_test_proposal(
            &conn,
            "prop-f1",
            ProposalKind::UpdateEntity,
            Some("ent-1"),
            vec![fact_item("fact-1", chunk_id, "Rust is a systems language.")],
            doc_id,
        );

        // Separate trigger doc so the second proposal is not auto-superseded.
        let doc2 = seed_document(&conn, "/vault/documents/b.pdf");
        let chunk2 = seed_chunk(&conn, doc2);
        insert_test_proposal(
            &conn,
            "prop-f2",
            ProposalKind::UpdateEntity,
            Some("ent-1"),
            vec![fact_item("fact-dup", chunk2, "Rust is a systems language.")],
            doc2,
        );

        resolve_fact(&mut conn, "prop-f1", "fact-1");
        let result = resolve_fact(&mut conn, "prop-f2", "fact-dup");

        assert!(result.committed.is_empty(), "duplicate must not commit");
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_entries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "exactly one stored fact");
        let status: String = conn
            .query_row(
                "SELECT status FROM curated_proposal_items WHERE id = 'fact-dup'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "rejected", "duplicate item recorded as skipped");
    }

    #[test]
    fn fact_add_whitespace_varied_duplicate_dedupes() {
        let mut conn = open_in_memory().unwrap();
        let doc_id = seed_document(&conn, "/vault/documents/a.pdf");
        let chunk_id = seed_chunk(&conn, doc_id);
        seed_entity(&conn, "ent-1", "Existing", "Summary", 100);

        insert_test_proposal(
            &conn,
            "prop-f1",
            ProposalKind::UpdateEntity,
            Some("ent-1"),
            vec![fact_item(
                "fact-1",
                chunk_id,
                "Rust is  a  systems\tlanguage.",
            )],
            doc_id,
        );

        // Separate trigger doc so the second proposal is not auto-superseded.
        let doc2 = seed_document(&conn, "/vault/documents/b.pdf");
        let chunk2 = seed_chunk(&conn, doc2);
        insert_test_proposal(
            &conn,
            "prop-f2",
            ProposalKind::UpdateEntity,
            Some("ent-1"),
            vec![fact_item(
                "fact-ws",
                chunk2,
                "  Rust   is a systems\nlanguage.  ",
            )],
            doc2,
        );

        resolve_fact(&mut conn, "prop-f1", "fact-1");
        let result = resolve_fact(&mut conn, "prop-f2", "fact-ws");

        assert!(result.committed.is_empty());
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_entries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn fact_add_different_body_still_commits() {
        let mut conn = open_in_memory().unwrap();
        let doc_id = seed_document(&conn, "/vault/documents/a.pdf");
        let chunk_id = seed_chunk(&conn, doc_id);
        seed_entity(&conn, "ent-1", "Existing", "Summary", 100);

        insert_test_proposal(
            &conn,
            "prop-f1",
            ProposalKind::UpdateEntity,
            Some("ent-1"),
            vec![fact_item("fact-1", chunk_id, "Rust is a systems language.")],
            doc_id,
        );

        // Separate trigger doc so the second proposal is not auto-superseded.
        let doc2 = seed_document(&conn, "/vault/documents/b.pdf");
        let chunk2 = seed_chunk(&conn, doc2);
        insert_test_proposal(
            &conn,
            "prop-f2",
            ProposalKind::UpdateEntity,
            Some("ent-1"),
            vec![fact_item(
                "fact-new",
                chunk2,
                "Rust has no garbage collector.",
            )],
            doc2,
        );

        resolve_fact(&mut conn, "prop-f1", "fact-1");
        let result = resolve_fact(&mut conn, "prop-f2", "fact-new");

        assert_eq!(result.committed.len(), 1);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_entries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn fact_add_dedupe_scoped_per_entity() {
        let mut conn = open_in_memory().unwrap();
        let doc_id = seed_document(&conn, "/vault/documents/a.pdf");
        let chunk_id = seed_chunk(&conn, doc_id);
        seed_entity(&conn, "ent-1", "Existing", "Summary", 100);
        seed_entity(&conn, "ent-2", "Other", "Summary", 100);

        insert_test_proposal(
            &conn,
            "prop-f1",
            ProposalKind::UpdateEntity,
            Some("ent-1"),
            vec![fact_item("fact-1", chunk_id, "Rust is a systems language.")],
            doc_id,
        );
        // Same normalized body, different entity — must NOT be treated as duplicate.
        insert_test_proposal(
            &conn,
            "prop-f2",
            ProposalKind::UpdateEntity,
            Some("ent-2"),
            vec![fact_item("fact-x", chunk_id, "Rust is a systems language.")],
            doc_id,
        );

        resolve_fact(&mut conn, "prop-f1", "fact-1");
        let result = resolve_fact(&mut conn, "prop-f2", "fact-x");

        assert_eq!(result.committed.len(), 1);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_entries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn fact_add_all_duplicates_includes_count_in_rejected_event() {
        let mut conn = open_in_memory().unwrap();
        let doc_id = seed_document(&conn, "/vault/documents/a.pdf");
        let chunk_id = seed_chunk(&conn, doc_id);
        seed_entity(&conn, "ent-1", "Existing", "Summary", 100);

        // Seed an existing fact so the second proposal's items dedupe.
        insert_test_proposal(
            &conn,
            "prop-original",
            ProposalKind::UpdateEntity,
            Some("ent-1"),
            vec![fact_item(
                "fact-original",
                chunk_id,
                "Rust is a systems language.",
            )],
            doc_id,
        );
        resolve_fact(&mut conn, "prop-original", "fact-original");

        // New proposal with ONLY duplicate fact_add items (different trigger doc
        // so it isn't auto-superseded).
        let doc2 = seed_document(&conn, "/vault/documents/b.pdf");
        let chunk2 = seed_chunk(&conn, doc2);
        insert_test_proposal(
            &conn,
            "prop-dup",
            ProposalKind::UpdateEntity,
            Some("ent-1"),
            vec![
                fact_item("fact-d1", chunk2, "Rust is a systems language."),
                fact_item("fact-d2", chunk2, "Rust is a systems language."),
            ],
            doc2,
        );

        let result = resolve_proposal(
            &mut conn,
            "prop-dup",
            &[
                ItemDecision {
                    item_id: "fact-d1".into(),
                    decision: ItemDecisionKind::Accept,
                    edited_payload: None,
                },
                ItemDecision {
                    item_id: "fact-d2".into(),
                    decision: ItemDecisionKind::Accept,
                    edited_payload: None,
                },
            ],
            None,
            ResolveOptions {
                auto_approve: false,
                embed_profile: None,
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(result.proposal_status, "rejected");
        assert_eq!(result.committed.len(), 0);

        // The resolution event for prop-dup must surface the duplicate count
        // so reviewers can see *why* every item was rejected. Filter by
        // event_type rather than relying on `ORDER BY created_at DESC LIMIT 1`,
        // because back-to-back resolve_proposal calls can produce identical
        // millisecond timestamps and SQLite's order is then unspecified —
        // returning whichever row the storage layer chose first.
        let (event_type, event_summary): (String, String) = conn
            .query_row(
                "SELECT event_type, summary FROM llm_wiki_events
                 WHERE event_type = 'rejected'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(event_type, "rejected");
        assert!(
            event_summary.contains("2 duplicate fact(s) skipped"),
            "rejected event summary must include duplicate count, got: {event_summary}"
        );
    }

    #[test]
    fn fact_add_stores_an_embedding_when_a_profile_is_configured() {
        temp_env::with_vars([("CURATED_EMBED_STUB", Some("constant8"))], || {
            let mut conn = open_in_memory().unwrap();
            let doc_id = seed_document(&conn, "/vault/documents/a.pdf");
            let chunk_id = seed_chunk(&conn, doc_id);
            seed_entity(&conn, "ent-1", "Existing", "Summary", 100);

            insert_test_proposal(
                &conn,
                "prop-embed",
                ProposalKind::UpdateEntity,
                Some("ent-1"),
                vec![NewProposalItem {
                    id: "fact-1".into(),
                    item_type: "fact_add".into(),
                    target_id: None,
                    payload: serde_json::json!({ "body": "A fact worth embedding." }),
                    evidence: vec![StoredEvidenceChunk {
                        chunk_id: Some(chunk_id),
                        content_hash: String::new(),
                        quote: "x".into(),
                        start_line: Some(1),
                        end_line: Some(1),
                        source_kind: None,
                    }],
                }],
                doc_id,
            );

            resolve_proposal(
                &mut conn,
                "prop-embed",
                &[ItemDecision {
                    item_id: "fact-1".into(),
                    decision: ItemDecisionKind::Accept,
                    edited_payload: None,
                }],
                None,
                ResolveOptions {
                    auto_approve: false,
                    embed_profile: Some(EmbedProfile::default()),
                    ..Default::default()
                },
            )
            .unwrap();

            let blob_len: Option<i64> = conn
                .query_row(
                    "SELECT length(embedding_blob) FROM llm_wiki_entries WHERE entity_id = 'ent-1'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(blob_len, Some(32), "constant8 gives 8 dims -> 32 bytes");
        });
    }

    #[test]
    fn fact_add_commits_with_null_embedding_when_the_provider_fails() {
        temp_env::with_vars([("CURATED_EMBED_STUB", None::<&str>)], || {
            let mut conn = open_in_memory().unwrap();
            let doc_id = seed_document(&conn, "/vault/documents/a.pdf");
            let chunk_id = seed_chunk(&conn, doc_id);
            seed_entity(&conn, "ent-1", "Existing", "Summary", 100);

            insert_test_proposal(
                &conn,
                "prop-embed-fail",
                ProposalKind::UpdateEntity,
                Some("ent-1"),
                vec![NewProposalItem {
                    id: "fact-1".into(),
                    item_type: "fact_add".into(),
                    target_id: None,
                    payload: serde_json::json!({ "body": "Curation that must survive." }),
                    evidence: vec![StoredEvidenceChunk {
                        chunk_id: Some(chunk_id),
                        content_hash: String::new(),
                        quote: "x".into(),
                        start_line: Some(1),
                        end_line: Some(1),
                        source_kind: None,
                    }],
                }],
                doc_id,
            );

            // Cloud profiles always Err -> the embed pre-pass fails.
            let result = resolve_proposal(
                &mut conn,
                "prop-embed-fail",
                &[ItemDecision {
                    item_id: "fact-1".into(),
                    decision: ItemDecisionKind::Accept,
                    edited_payload: None,
                }],
                None,
                ResolveOptions {
                    auto_approve: false,
                    embed_profile: Some(EmbedProfile::Cloud {
                        provider: crate::embedder::CloudProvider::OpenAi,
                        model: "unreachable".into(),
                        api_key: String::new(),
                    }),
                    ..Default::default()
                },
            );

            assert!(
                result.is_ok(),
                "an embed failure must never destroy the librarian's curation"
            );
            let (count, blob): (i64, Option<Vec<u8>>) = conn
                .query_row(
                    "SELECT COUNT(*), MAX(embedding_blob) FROM llm_wiki_entries
                      WHERE entity_id = 'ent-1'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(count, 1, "the fact is committed");
            assert_eq!(blob, None, "with a NULL blob for the sweep to fill later");
        });
    }

    #[test]
    fn fact_add_commits_with_null_embedding_when_provider_returns_wrong_size() {
        // R6 length-mismatch guard: `constant8_short` returns N-1 vectors for
        // an N-text batch. The pre-pass must skip that chunk instead of
        // zipping the surviving vector onto the first id — the fact still
        // commits, but with a NULL blob that the sweep will fill later.
        temp_env::with_vars([("CURATED_EMBED_STUB", Some("constant8_short"))], || {
            let mut conn = open_in_memory().unwrap();
            let doc_id = seed_document(&conn, "/vault/documents/a.pdf");
            let chunk_id = seed_chunk(&conn, doc_id);
            seed_entity(&conn, "ent-1", "Existing", "Summary", 100);

            insert_test_proposal(
                &conn,
                "prop-embed-short",
                ProposalKind::UpdateEntity,
                Some("ent-1"),
                vec![NewProposalItem {
                    id: "fact-1".into(),
                    item_type: "fact_add".into(),
                    target_id: None,
                    payload: serde_json::json!({ "body": "Body that must commit cleanly." }),
                    evidence: vec![StoredEvidenceChunk {
                        chunk_id: Some(chunk_id),
                        content_hash: String::new(),
                        quote: "x".into(),
                        start_line: Some(1),
                        end_line: Some(1),
                        source_kind: None,
                    }],
                }],
                doc_id,
            );

            let result = resolve_proposal(
                &mut conn,
                "prop-embed-short",
                &[ItemDecision {
                    item_id: "fact-1".into(),
                    decision: ItemDecisionKind::Accept,
                    edited_payload: None,
                }],
                None,
                ResolveOptions {
                    auto_approve: false,
                    embed_profile: Some(EmbedProfile::default()),
                    entry_embeddings: None,
                },
            );

            assert!(
                result.is_ok(),
                "a wrong-sized provider response must never destroy the librarian's curation"
            );
            let (count, blob): (i64, Option<Vec<u8>>) = conn
                .query_row(
                    "SELECT COUNT(*), MAX(embedding_blob) FROM llm_wiki_entries
                          WHERE entity_id = 'ent-1'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(count, 1, "the fact is committed");
            assert_eq!(
                blob, None,
                "the wrong-sized batch must not pair a vector to this row"
            );
        });
    }
}

/// D-tests for the `source_ref_is_still_grounded` consumer contract. Lives
/// in commit.rs so it shares `super::*` with the helper under test and the
/// in-memory connection. The five tests pin every branch of the helper:
/// legacy-path lookup, JSON parse, evidence-empty short-circuit, chunk-id
/// presence, and the parse-error "still-grounded" defensive policy that
/// Bug A's spec review called out.
#[cfg(test)]
mod source_ref_grounded_tests {
    use super::*;
    use crate::db::connection::open_in_memory;

    fn insert_doc_indexed(conn: &Connection, path: &str) -> i64 {
        conn.execute(
            "INSERT INTO documents (path, hash, tier, status)
             VALUES (?1, 'h', 'user_doc', 'indexed')",
            [path],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn insert_chunk_for_doc(conn: &Connection, doc_id: i64, hash: &str) -> i64 {
        conn.execute(
            "INSERT INTO chunks (doc_id, chunk_text, position, start_line, end_line,
                                 symbol_name, strategy, content_hash)
             VALUES (?1, 'ct', 0, 1, 3, NULL, 'prose', ?2)",
            rusqlite::params![doc_id, hash],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    /// D1 — Legacy path lookup: a `documents.path` match with
    /// `status='indexed'` is "still grounded". A non-indexed status or a
    /// missing path returns false.
    #[test]
    fn legacy_path_indexed_returns_true_missing_returns_false() {
        let conn = open_in_memory().unwrap();
        insert_doc_indexed(&conn, "documents/gone.md");
        // Flip the status to 'pending' so the legacy existence check fails.
        conn.execute(
            "UPDATE documents SET status = 'pending' WHERE path = ?1",
            ["documents/gone.md"],
        )
        .unwrap();
        assert!(
            !source_ref_is_still_grounded(&conn, "documents/gone.md"),
            "non-indexed status must report not-grounded"
        );

        // Set it back to indexed and re-check.
        conn.execute(
            "UPDATE documents SET status = 'indexed' WHERE path = ?1",
            ["documents/gone.md"],
        )
        .unwrap();
        assert!(
            source_ref_is_still_grounded(&conn, "documents/gone.md"),
            "indexed legacy path must report still-grounded"
        );

        // And a path that doesn't exist at all.
        assert!(
            !source_ref_is_still_grounded(&conn, "documents/missing.md"),
            "unknown legacy path must report not-grounded"
        );
    }

    /// D2 — JSON-shape happy path: at least one referenced chunk exists →
    /// still-grounded.
    #[test]
    fn json_with_live_chunk_returns_true() {
        let conn = open_in_memory().unwrap();
        let doc_id = insert_doc_indexed(&conn, "documents/notes.md");
        let chunk_id = insert_chunk_for_doc(&conn, doc_id, "h_alive");
        let src = format!(
            r#"{{"proposal_id":"p1","evidence":[{{"chunk_id":{chunk_id},"content_hash":"h_alive","quote":"q","start_line":1,"end_line":3}}]}}"#
        );
        assert!(source_ref_is_still_grounded(&conn, &src));
    }

    /// D3 — JSON-shape with all dead chunks → not-grounded. Pins the
    /// "partial evidence keeps the entry" rule's negative: every chunk_id
    /// gone means we soft-delete.
    #[test]
    fn json_with_all_dead_chunks_returns_false() {
        let conn = open_in_memory().unwrap();
        let doc_id = insert_doc_indexed(&conn, "documents/x.md");
        let chunk_id = insert_chunk_for_doc(&conn, doc_id, "h_dead");
        conn.execute("DELETE FROM chunks WHERE id = ?1", [chunk_id])
            .unwrap();
        let src = format!(
            r#"{{"proposal_id":"p1","evidence":[{{"chunk_id":{chunk_id},"content_hash":"h_dead","quote":"q","start_line":1,"end_line":3}}]}}"#
        );
        assert!(
            !source_ref_is_still_grounded(&conn, &src),
            "JSON with all-dead chunks must report not-grounded"
        );
    }

    /// D4 — Empty evidence array → still-grounded. The MANUAL_SOURCE_REF
    /// sentinel has `evidence:[]`; those rows are user_stated and the heal
    /// must never delete them based on the source_ref shape.
    #[test]
    fn json_with_empty_evidence_returns_true() {
        let conn = open_in_memory().unwrap();
        assert!(source_ref_is_still_grounded(
            &conn,
            r#"{"proposal_id":null,"evidence":[]}"#
        ));
    }

    /// D5 — Parse-error defensive branch. JSON-looking-but-malformed input
    /// returns `true` (no soft-delete) rather than false. This is the
    /// Bug A spec-review contract: a row that can't be parsed isn't
    /// *demonstrably* stale — it's a legacy path or a future producer we
    /// don't know about yet. Logging is the right response, not deletion.
    #[test]
    fn malformed_json_returns_true_defensive() {
        let conn = open_in_memory().unwrap();
        assert!(
            source_ref_is_still_grounded(&conn, "{not valid json"),
            "malformed JSON must defensively return true (no soft-delete)"
        );
        // A JSON-shaped value without an `evidence` key also returns true
        // (not a JSON parse failure, but no librarian evidence either).
        assert!(source_ref_is_still_grounded(
            &conn,
            r#"{"proposal_id":"p1"}"#
        ));
        // Empty string and whitespace-only strings also return true.
        assert!(source_ref_is_still_grounded(&conn, ""));
        assert!(source_ref_is_still_grounded(&conn, "   \t  "));
    }

    /// D6 — Defensive DB-error branch. A failing lookup (broken
    /// connection, corrupted schema, lock contention) must NOT be
    /// interpreted as "demonstrably stale" — same policy as the
    /// parse-error branch above. The previous `unwrap_or(None)`
    /// collapsed `Err(...)` into `None` and could over-delete on DB
    /// faults, which is the opposite of what the spec wants. We pin
    /// both call sites here: legacy-path lookup against `documents`
    /// and chunk lookup against `chunks`.
    ///
    /// We provoke the DB error by dropping the underlying table so
    /// the SELECT raises `SqliteFailure` (vs. `NoRows` — that's the
    /// distinction the new match arm is making).
    #[test]
    fn dropped_table_db_error_returns_true_defensive() {
        // Legacy-path branch: documents lookup against a vanished table.
        let conn = open_in_memory().unwrap();
        conn.execute("DROP TABLE documents", [])
            .expect("drop documents");
        assert!(
            source_ref_is_still_grounded(&conn, "documents/anything.md"),
            "legacy-path DB error must defensively return true (no soft-delete)"
        );

        // JSON branch: chunks lookup against a vanished table. Use a
        // shape with a real chunk_id so the code reaches the chunks
        // SELECT before failing on the missing table.
        let conn = open_in_memory().unwrap();
        conn.execute("DROP TABLE chunks", []).expect("drop chunks");
        let src = r#"{"proposal_id":"p1","evidence":[{"chunk_id":1,"content_hash":"h","quote":"q","start_line":1,"end_line":3}]}"#;
        assert!(
            source_ref_is_still_grounded(&conn, src),
            "JSON-path DB error must defensively return true (no soft-delete)"
        );
    }
}
