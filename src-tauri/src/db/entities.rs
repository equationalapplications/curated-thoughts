//! CRUD for `curated_entities` — OKF entity surface for Brain mode (Phase 4).

use crate::db::commit::{evidence_json_for_entry, is_librarian_source_ref_token};
use anyhow::{bail, Context, Result};
use rand::Rng;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

const RECENT_EVENTS_LIMIT: i64 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EntitySort {
    #[default]
    UpdatedDesc,
    NameAsc,
    NameDesc,
    CreatedDesc,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EntityListFilter {
    pub entity_type: Option<String>,
    pub include_archived: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntitySummary {
    pub id: String,
    pub name: String,
    pub entity_type: String,
    pub summary_snippet: String,
    pub fact_count: i64,
    pub open_task_count: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityWisdom {
    pub id: String, // raw OKF fact id (displayed directly by the "..." power menu)
    pub title: String,
    pub body: String,
    pub tags: Vec<String>,
    pub confidence: String,
    pub source_type: String,
    pub source_docs: Vec<SourceDocRef>,
    pub updated_at: i64,
    // OKF v0.2 fields
    pub lifecycle_status: String,
    pub stale_after: Option<i64>,
    pub generated_by: Option<String>,
    pub okf_sources: Vec<OkfSourceEntry>,
    pub okf_verified: Vec<OkfVerifiedEntry>,
    pub okf_usage_window: Option<OkfUsageWindow>,
    pub last_verified_at: Option<i64>,
    pub last_verified_by: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OkfSourceEntry {
    pub id: Option<String>,
    pub resource: String,
    pub title: Option<String>,
    pub author: Option<String>,
    pub usage_count: Option<i64>,
    pub last_modified: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OkfVerifiedEntry {
    pub by: String,
    /// epoch ms. The wire format may carry an ISO-8601 string
    /// (e.g. `2026-07-02T00:00:00.000Z`); the deserializer normalizes both
    /// shapes to `i64` so imported facts and direct writes share a single
    /// `parse_okf_verified` path.
    #[serde(deserialize_with = "deserialize_epoch_ms")]
    pub at: i64,
}

fn deserialize_epoch_ms<'de, D>(d: D) -> std::result::Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;
    match serde_json::Value::deserialize(d)? {
        serde_json::Value::Number(n) => n
            .as_i64()
            .ok_or_else(|| Error::custom("okf_verified.at: expected integer epoch ms")),
        serde_json::Value::String(s) => crate::okf::timefmt::ms_from_iso(&s)
            .ok_or_else(|| Error::custom("okf_verified.at: expected ISO-8601 timestamp")),
        _ => Err(Error::custom(
            "okf_verified.at: expected number or ISO-8601 string",
        )),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OkfUsageWindow {
    pub from: String,
    pub to: String,
}

/// One resolved source-document reference on an [`EntityWisdom`].
/// Wire shape is `{ "path": ..., "chunkId": ... }` — `chunkId` is the
/// camelCase exception to `EntityWisdom`'s snake_case fields because it
/// feeds the frontend `NavTarget.chunkId` deep-link surface. The value
/// is the stable SHA-256 first-16-bytes hex from `db::chunk_hash`, or
/// `null` when the fact's evidence did not resolve to a chunk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceDocRef {
    pub path: String,
    #[serde(rename = "chunkId")]
    pub chunk_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityTask {
    pub id: String,
    pub description: String,
    pub status: String,
    pub priority: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityEvent {
    pub id: String,
    pub event_type: String,
    pub summary: String,
    pub related_entry_id: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityDetail {
    pub id: String,
    pub name: String,
    pub entity_type: String,
    pub summary: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub deleted_at: Option<i64>,
    pub facts: Vec<EntityWisdom>,
    pub tasks: Vec<EntityTask>,
    pub events: Vec<EntityEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateEntityInput {
    pub name: String,
    pub entity_type: Option<String>,
    pub summary: Option<String>,
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn generate_entity_id() -> String {
    let mut bytes = [0u8; 12];
    rand::rng().fill_bytes(&mut bytes);
    format!("ent_{}", hex::encode(bytes))
}

fn summary_snippet(summary: &str) -> String {
    summary.chars().take(200).collect()
}

fn parse_tags(raw: &str) -> Vec<String> {
    serde_json::from_str(raw).unwrap_or_default()
}

fn parse_okf_sources(raw: Option<&str>) -> Vec<OkfSourceEntry> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    serde_json::from_str(raw).unwrap_or_default()
}

fn parse_okf_verified(raw: Option<&str>) -> Vec<OkfVerifiedEntry> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    serde_json::from_str(raw).unwrap_or_default()
}

fn parse_okf_usage_window(raw: Option<&str>) -> Option<OkfUsageWindow> {
    let raw = raw?;
    serde_json::from_str(raw).ok()
}

/// The three outcomes of the shared source-resolution core (spec R2.3.2a).
///
/// Distinct on purpose: today's empty-Vec hides all of them and lets a DB
/// fault masquerade as "no source", which would let a stale hash climb to
/// strict and get retyped (the spec L792 case). The classification table is
/// authoritative; this enum is the Rust mirror.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SourceResolution {
    /// ≥1 evidence entry resolved to a document path (the healthy case).
    Resolved(Vec<(String, Option<String>)>),
    /// Had at least one evidence entry that did NOT resolve, OR the shape was
    /// an unknown provenance shape — never a silent climb.
    HadEvidenceUnresolved,
    /// Ref was absent (`None` or an explicitly-listed non-provenance value) —
    /// nothing claimed.
    NoEvidence,
}

/// Shared source-resolution core (spec L756-764, L795-815, plan-p11-MAJOR-1).
///
/// Both the write-time gate (Task 3) and the heal census (Task 5) consume
/// THIS function. [`source_docs_from_ref`] remains a second DISPLAY wrapper
/// that degrades on error (spec L792).
///
/// ORDER pinned (R2.3.2a): librarian-token shape FIRST, then JSON-parse the
/// rest. The two orders agree today (a `librarian-<hex>` token is never
/// valid JSON) but "one resolver" must not grow a second ordering.
pub(crate) fn resolve_source_core(
    conn: &Connection,
    entry_id: &str,
    source_ref: Option<&str>,
) -> rusqlite::Result<SourceResolution> {
    let raw: String = match source_ref {
        None => return Ok(SourceResolution::NoEvidence),
        Some(r) if is_librarian_source_ref_token(r) => {
            // The two split-arm sites from entities.rs:213-217 — errors do not
            // masquerade as "no source" (spec L792).
            match evidence_json_for_entry(conn, entry_id) {
                Ok(Some(json)) => json,
                Ok(None) => return Ok(SourceResolution::HadEvidenceUnresolved),
                Err(e) => return Err(e),
            }
        }
        Some(r) => r.to_string(),
    };

    let value: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return Ok(SourceResolution::HadEvidenceUnresolved),
    };
    let Some(evidence) = value.get("evidence").and_then(|v| v.as_array()) else {
        // JSON object with NO `evidence` key → HadEvidenceUnresolved.
        return Ok(SourceResolution::HadEvidenceUnresolved);
    };
    let mut out: Vec<(String, Option<String>)> = Vec::new();
    for entry in evidence {
        let Some(hash) = entry
            .get("content_hash")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        else {
            // Pre-migration chunk-id-only entries (legacy) → unresolved.
            return Ok(SourceResolution::HadEvidenceUnresolved);
        };
        let resolved: Option<String> = conn
            .query_row(
                "SELECT d.path FROM chunks c JOIN documents d ON d.id = c.doc_id
                 WHERE c.content_hash = ?1 LIMIT 1",
                [hash],
                |r| r.get(0),
            )
            .optional()?; // propagate DB faults (entities.rs:251 swallow site)
        if let Some(path) = resolved {
            if !out.iter().any(|(existing, _)| existing == &path) {
                out.push((path, Some(hash.to_string())));
            }
        } else {
            // ≥1 entry did not resolve → HadEvidenceUnresolved.
            return Ok(SourceResolution::HadEvidenceUnresolved);
        }
    }
    if out.is_empty() {
        // JSON with `"evidence": []` (the V20 doomed-row shape) → unresolved.
        Ok(SourceResolution::HadEvidenceUnresolved)
    } else {
        Ok(SourceResolution::Resolved(out))
    }
}

/// Resolve an entry's `source_ref` evidence to `(document path, content hash)`
/// pairs, deduplicated by path.
///
/// `pub(crate)` so `wiki_graph::wiki_context` can build its provenance list
/// from the same resolution the entity reader uses — two implementations of
/// "where did this fact come from" would be free to disagree.
///
/// DISPLAY wrapper only (spec L795-815): the shared resolver core lives in
/// this module as [`resolve_source_core`] and returns a `SourceResolution`
/// with `Resolved`/`HadEvidenceUnresolved`/`NoEvidence` kept distinct. A DB
/// fault is propagated by the core (Task 2) and caught here to degrade the
/// display, not to let "no source" climb to strict.
pub(crate) fn source_docs_from_ref(
    conn: &Connection,
    entry_id: &str,
    source_ref: Option<&str>,
) -> Vec<(String, Option<String>)> {
    match resolve_source_core(conn, entry_id, source_ref) {
        Ok(SourceResolution::Resolved(paths)) => paths,
        Ok(_) => Vec::new(),
        Err(_) => Vec::new(),
    }
}

fn order_clause(sort: EntitySort) -> &'static str {
    match sort {
        EntitySort::UpdatedDesc => "updated_at DESC, name ASC",
        EntitySort::NameAsc => "name COLLATE NOCASE ASC",
        EntitySort::NameDesc => "name COLLATE NOCASE DESC",
        EntitySort::CreatedDesc => "created_at DESC, name ASC",
    }
}

/// Resolve a possibly-redirected entity id to its FINAL survivor (Task 7,
/// spec R2.7.5 / r13-MAJOR-1).
///
/// This is the ONE shared resolution helper every read AND mutate path
/// goes through — it reuses Task 6's cycle-guarded chain walk
/// (`merge_duplicates::resolve_redirect_chain`); resolution is never
/// re-implemented at a call site. A hand-crafted cycle is an error (never
/// an infinite loop, never a guess): the caller surfaces it and the row is
/// repaired by hand. `ChainResolution::None` (no redirect row) returns the
/// input id unchanged.
pub(crate) fn resolve_entity_id(conn: &Connection, entity_id: &str) -> Result<String> {
    match crate::db::merge_duplicates::resolve_redirect_chain(conn, entity_id)? {
        crate::db::merge_duplicates::ChainResolution::None => Ok(entity_id.to_string()),
        crate::db::merge_duplicates::ChainResolution::Survivor(s) => Ok(s),
        crate::db::merge_duplicates::ChainResolution::Cycle(on_loop) => bail!(
            "entity {entity_id} sits on an entity_redirects cycle (loop member \
             {on_loop}); delete the looping rows by hand — refusing to guess a \
             survivor"
        ),
    }
}

/// The survivor plus every loser whose redirect resolves to it — the
/// TRANSITIVE FACT CLOSURE id set (spec R2.7.5 / r13-m3): reads for a
/// survivor must cover `entity_id IN (survivor ∪ redirected losers)`.
///
/// Merge-time path compression keeps healthy chains one hop deep, so the
/// first reverse hop (`merged_into = survivor`) finds every merge-written
/// loser; the loop to a fixpoint additionally collects hand-crafted
/// multi-hop rows that terminate at the survivor, and is bounded by the
/// table size (each iteration must add at least one NEW id or it stops).
pub(crate) fn cluster_ids(conn: &Connection, survivor: &str) -> Result<Vec<String>> {
    let mut ids = vec![survivor.to_string()];
    loop {
        let placeholders = vec!["?"; ids.len()].join(",");
        let sql =
            format!("SELECT entity_id FROM entity_redirects WHERE merged_into IN ({placeholders})");
        let params: Vec<&str> = ids.iter().map(String::as_str).collect();
        let mut stmt = conn.prepare(&sql)?;
        let rows: Vec<String> = stmt
            .query_map(rusqlite::params_from_iter(params), |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let grew = rows.iter().any(|r| !ids.contains(r));
        for r in rows {
            if !ids.contains(&r) {
                ids.push(r);
            }
        }
        if !grew {
            return Ok(ids);
        }
    }
}

/// Build an `IN (…)` placeholder list for `ids`. The caller binds the ids
/// positionally in the same order.
fn in_placeholders(ids: &[String]) -> String {
    vec!["?"; ids.len()].join(",")
}

fn fact_count(conn: &Connection, ids: &[String]) -> Result<i64> {
    let sql = format!(
        "SELECT COUNT(*) FROM llm_wiki_entries
         WHERE entity_id IN ({}) AND deleted_at IS NULL",
        in_placeholders(ids)
    );
    let n = conn.query_row(&sql, rusqlite::params_from_iter(ids.iter()), |r| r.get(0))?;
    Ok(n)
}

fn open_task_count(conn: &Connection, ids: &[String]) -> Result<i64> {
    let sql = format!(
        "SELECT COUNT(*) FROM llm_wiki_tasks
         WHERE entity_id IN ({}) AND status = 'pending' AND deleted_at IS NULL",
        in_placeholders(ids)
    );
    let n = conn.query_row(&sql, rusqlite::params_from_iter(ids.iter()), |r| r.get(0))?;
    Ok(n)
}

/// List non-archived entities (unless `filter.include_archived`).
pub fn list_entities(
    conn: &Connection,
    sort: EntitySort,
    filter: &EntityListFilter,
) -> Result<Vec<EntitySummary>> {
    let include_archived = filter.include_archived.unwrap_or(false);
    let mut conditions = Vec::new();
    let mut bind_type: Option<String> = None;

    if !include_archived {
        conditions.push("deleted_at IS NULL".to_string());
    }
    if let Some(ref entity_type) = filter.entity_type {
        conditions.push("entity_type = ?1".to_string());
        bind_type = Some(entity_type.clone());
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };

    let sql = format!(
        "SELECT id, name, entity_type, summary, created_at, updated_at
         FROM live_entities
         {where_clause}
         ORDER BY {}",
        order_clause(sort)
    );

    let mut stmt = conn.prepare(&sql)?;
    let map_row = |r: &rusqlite::Row<'_>| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, i64>(4)?,
            r.get::<_, i64>(5)?,
        ))
    };

    let rows: Vec<_> = if let Some(ref entity_type) = bind_type {
        stmt.query_map([entity_type], map_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        stmt.query_map([], map_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };

    let mut out = Vec::with_capacity(rows.len());
    for (id, name, entity_type, summary, created_at, updated_at) in rows {
        // Transitive fact closure (r13-m3): counts cover the survivor ∪ its
        // redirected losers. `id` comes from live_entities, so it is always
        // a terminal survivor.
        let cluster = cluster_ids(conn, &id)?;
        out.push(EntitySummary {
            id: id.clone(),
            name,
            entity_type,
            summary_snippet: summary_snippet(&summary),
            fact_count: fact_count(conn, &cluster)?,
            open_task_count: open_task_count(conn, &cluster)?,
            created_at,
            updated_at,
        });
    }
    Ok(out)
}

/// Load facts for a cluster (`survivor ∪ redirected losers`, r13-m3
/// transitive closure). Callers pass [`cluster_ids`].
fn load_facts(conn: &Connection, ids: &[String]) -> Result<Vec<EntityWisdom>> {
    let sql = format!(
        "SELECT id, title, body, tags, confidence, source_type, source_ref, updated_at,
                lifecycle_status, stale_after, generated_by, okf_sources, okf_verified,
                okf_usage_window, last_verified_at, last_verified_by
         FROM llm_wiki_entries
         WHERE entity_id IN ({}) AND deleted_at IS NULL
         ORDER BY updated_at DESC",
        in_placeholders(ids)
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(ids.iter()), |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, Option<String>>(6)?,
            r.get::<_, i64>(7)?,
            r.get::<_, String>(8)?,
            r.get::<_, Option<i64>>(9)?,
            r.get::<_, Option<String>>(10)?,
            r.get::<_, Option<String>>(11)?,
            r.get::<_, Option<String>>(12)?,
            r.get::<_, Option<String>>(13)?,
            r.get::<_, Option<i64>>(14)?,
            r.get::<_, Option<String>>(15)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (
            id,
            title,
            body,
            tags_raw,
            confidence,
            source_type,
            source_ref,
            updated_at,
            lifecycle_status,
            stale_after,
            generated_by,
            okf_sources_raw,
            okf_verified_raw,
            okf_usage_window_raw,
            last_verified_at,
            last_verified_by,
        ) = row?;
        let source_docs = source_docs_from_ref(conn, &id, source_ref.as_deref());
        out.push(EntityWisdom {
            id,
            title,
            body,
            tags: parse_tags(&tags_raw),
            confidence,
            source_type,
            source_docs: source_docs
                .into_iter()
                .map(|(path, chunk_hash)| SourceDocRef { path, chunk_hash })
                .collect(),
            updated_at,
            lifecycle_status,
            stale_after,
            generated_by,
            okf_sources: parse_okf_sources(okf_sources_raw.as_deref()),
            okf_verified: parse_okf_verified(okf_verified_raw.as_deref()),
            okf_usage_window: parse_okf_usage_window(okf_usage_window_raw.as_deref()),
            last_verified_at,
            last_verified_by,
        });
    }
    Ok(out)
}

fn load_tasks(conn: &Connection, ids: &[String]) -> Result<Vec<EntityTask>> {
    let sql = format!(
        "SELECT id, description, status, priority, created_at
         FROM llm_wiki_tasks
         WHERE entity_id IN ({}) AND deleted_at IS NULL AND status = 'pending'
         ORDER BY priority DESC, created_at ASC",
        in_placeholders(ids)
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(ids.iter()), |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, i64>(4)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, description, status, priority, created_at) = row?;
        out.push(EntityTask {
            id,
            description,
            status,
            priority,
            created_at,
        });
    }
    Ok(out)
}

fn load_events(conn: &Connection, ids: &[String]) -> Result<Vec<EntityEvent>> {
    // TODO(pr-followup): `ORDER BY created_at DESC` has no secondary
    // tiebreaker (e.g. `, id DESC`). When two events for the same entity
    // share a millisecond timestamp, SQLite returns rows in rowid/insertion
    // order — call sites that rely on strict time-descending order may see
    // unpredictable results on busy entities. Lower risk than the
    // unfiltered `LIMIT 1` pattern fixed in commit 4c6ecf4 (this query
    // is scoped by entity_id and returns a list, not a single row), but
    // worth a sweep across the codebase. See
    // procedures/curated-thoughts-improvement-backlog.md.
    // RECENT_EVENTS_LIMIT is a compile-time constant, formatted not bound.
    let mut stmt = conn.prepare(&format!(
        "SELECT id, event_type, summary, related_entry_id, created_at
         FROM llm_wiki_events
         WHERE entity_id IN ({})
         ORDER BY created_at DESC
         LIMIT {RECENT_EVENTS_LIMIT}",
        in_placeholders(ids)
    ))?;
    let rows = stmt.query_map(rusqlite::params_from_iter(ids.iter()), |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, i64>(4)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, event_type, summary, related_entry_id, created_at) = row?;
        out.push(EntityEvent {
            id,
            event_type,
            summary,
            related_entry_id,
            created_at,
        });
    }
    Ok(out)
}

/// Entity + facts + open tasks + recent events.
///
/// Task 7 (spec R2.7.5 / r13-m3 / r11-M4): a loser id REDIRECTS to its
/// survivor — `EntityDetail.id` is always the SURVIVOR id, never the
/// requested loser id (else the GUI pins the loser and later mutations
/// re-hit r13-MAJOR-1). The redirect is followed REGARDLESS of the
/// survivor's `deleted_at` (r21): an archived survivor is returned exactly
/// as `get_entity(survivor)` returns it — archived detail, `deleted_at`
/// populated — never `None` (a live-only hop would make a stale loser link
/// look like a deleted entity the user never archived).
pub fn get_entity(conn: &Connection, entity_id: &str) -> Result<Option<EntityDetail>> {
    let survivor_id = resolve_entity_id(conn, entity_id)?;
    let row = conn
        .query_row(
            "SELECT name, entity_type, summary, created_at, updated_at, deleted_at
             FROM live_entities WHERE id = ?1",
            [&survivor_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, Option<i64>>(5)?,
                ))
            },
        )
        .optional()?;

    let Some((name, entity_type, summary, created_at, updated_at, deleted_at)) = row else {
        return Ok(None);
    };

    // Transitive fact closure (r13-m3): the survivor's reads cover the
    // loser rows' facts/tasks/events too.
    let cluster = cluster_ids(conn, &survivor_id)?;
    Ok(Some(EntityDetail {
        id: survivor_id,
        name,
        entity_type,
        summary,
        created_at,
        updated_at,
        deleted_at,
        facts: load_facts(conn, &cluster)?,
        tasks: load_tasks(conn, &cluster)?,
        events: load_events(conn, &cluster)?,
    }))
}

/// Create a new curated entity (`summary_embedding` backfill deferred).
///
/// Wave-1 gate (Task 3, spec R2.4.2): opens an IMMEDIATE transaction so the
/// shared insert helper's `&ImmediateTx` parameter type-checks. GUI mints
/// have NO proposal, so the §2.3 ladder starts at rung 3 (host default);
/// the resolver walks rung 1 (`ct_entity_optouts` + entity manifest) and
/// rung 4 (`tier_fact`).
pub fn create_entity(conn: &mut Connection, input: &CreateEntityInput) -> Result<EntityDetail> {
    let name = input.name.trim();
    if name.is_empty() {
        bail!("entity name must not be empty");
    }
    let proposed_label = input
        .entity_type
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let summary = input.summary.as_deref().unwrap_or("");
    let id = generate_entity_id();
    let now = now_secs();

    // Open IMMEDIATE so the shared insert helper's `&ImmediateTx` parameter
    // type-checks. Spec R2.4.2 (r1-MAJOR-3): GUI mints must go through the
    // gate; a plain `&Connection` is a compile-time impossibility now that
    // the helper is the single insert point.
    let tx = crate::db::entity_gate::ImmediateTx::begin(conn)?;
    let (decision, gate) = crate::db::entity_gate::resolve_production_gate(&tx, &id, &[]);
    let outcome = crate::db::entity_gate::shared_insert_entity(
        &tx,
        Some(&id),
        name,
        proposed_label,
        summary,
        now,
        decision.clone(),
        false,
    )?;

    // Held → refusal error surfaced to the GUI (spec §2.4.5: "no-fallback-
    // exists → refusal error shown; otherwise normal ladder" / §2.5 GUI
    // bullet). The transaction rolls back when this function returns Err.
    match &outcome {
        crate::db::entity_gate::AdmitOutcome::Held { .. } => {
            let _ = tx.rollback();
            bail!(
                "ontology gate held the entity mint: manifest is strict with no declared \
                 fallback_node_type (spec §2.4.5); facts survive, retry after naming a fallback"
            );
        }
        crate::db::entity_gate::AdmitOutcome::Skipped { .. } => {
            // SKIP path: helper did NOT insert (off folder / off host /
            // no manifest). Per §2.5 the literal `'concept'` stands and the
            // origin ledger records a `gate_skipped` row.
            tx.execute(
                "INSERT INTO curated_entities (
                    id, name, entity_type, summary, summary_embedding, created_at, updated_at, deleted_at
                 ) VALUES (?1, ?2, 'concept', ?3, NULL, ?4, ?4, NULL)",
                params![id, name, summary, now],
            )?;
        }
        _ => {
            // Helper inserted; nothing more to do.
        }
    }

    crate::db::entity_gate::write_gate_origin_ledger(
        &tx,
        &id,
        &outcome,
        decision,
        gate.source_directory.as_deref(),
    )?;

    tx.commit()?;

    get_entity(conn, &id)?.context("entity missing immediately after insert")
}

/// Replace entity summary; clears `summary_embedding` for lazy re-embed.
///
/// Task 7 (r13-MAJOR-1 / r15-m3): a loser id resolves to the survivor
/// BEFORE acting — a mutator keyed by a stale loser link edits the
/// SURVIVOR (rejecting loser ids was rejected as hostile to stale GUI
/// state).
pub fn update_entity_summary(conn: &Connection, entity_id: &str, summary: &str) -> Result<()> {
    let resolved = resolve_entity_id(conn, entity_id)?;
    let now = now_secs();
    let changes = conn.execute(
        "UPDATE curated_entities
         SET summary = ?1, summary_embedding = NULL, updated_at = ?2
         WHERE id = ?3 AND deleted_at IS NULL",
        params![summary, now, resolved],
    )?;
    if changes == 0 {
        bail!("entity not found or archived: {entity_id}");
    }
    Ok(())
}

/// Soft-delete entity (`deleted_at` set; facts/tasks remain for audit).
///
/// Task 7 (r13-MAJOR-1): archiving a SURVIVOR archives the whole cluster —
/// the survivor AND its redirected losers — so bundle export (which maps
/// loser facts onto the survivor, r2-M8) consistently excludes the entire
/// archived cluster with zero orphaned facts. `entity_redirects` rows are
/// KEPT (r21): archiving touches only `curated_entities.deleted_at`, so a
/// stale loser link still resolves to the (archived) survivor and the
/// loser never reappears as a standalone row. Archive via a loser id acts
/// on the survivor's cluster (r15-m3).
pub fn archive_entity(conn: &Connection, entity_id: &str) -> Result<()> {
    let resolved = resolve_entity_id(conn, entity_id)?;
    let cluster = cluster_ids(conn, &resolved)?;
    let now = now_secs();
    let mut changes = 0usize;
    for id in &cluster {
        changes += conn.execute(
            "UPDATE curated_entities SET deleted_at = ?1, updated_at = ?1
             WHERE id = ?2 AND deleted_at IS NULL",
            params![now, id],
        )?;
    }
    if changes == 0 {
        bail!("entity not found or already archived: {entity_id}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::open_in_memory;

    fn seed_fact(conn: &Connection, entity_id: &str, fact_id: &str, body: &str) {
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type, created_at, updated_at
             ) VALUES (?1, ?2, 'Title', ?3, '[]', 'inferred', 'user_confirmed', 100, 100)",
            params![fact_id, entity_id, body],
        )
        .unwrap();
    }

    fn seed_task(conn: &Connection, entity_id: &str, task_id: &str, status: &str) {
        conn.execute(
            "INSERT INTO llm_wiki_tasks (
                id, entity_id, description, status, priority, created_at, updated_at
             ) VALUES (?1, ?2, 'Do thing', ?3, 0, 100, 100)",
            params![task_id, entity_id, status],
        )
        .unwrap();
    }

    fn seed_event(conn: &Connection, entity_id: &str, event_id: &str, summary: &str) {
        conn.execute(
            "INSERT INTO llm_wiki_events (id, entity_id, event_type, summary, created_at)
             VALUES (?1, ?2, 'action', ?3, 200)",
            params![event_id, entity_id, summary],
        )
        .unwrap();
    }

    /// Seed a document with `count` prose chunks; returns `(chunk_rowid, content_hash)` pairs.
    fn seed_doc_with_chunks(conn: &Connection, path: &str, count: usize) -> Vec<(i64, String)> {
        conn.execute(
            "INSERT INTO documents (path, hash, tier, status) VALUES (?1, 'h', 'user_doc', 'indexed')",
            params![path],
        )
        .unwrap();
        let doc_id = conn.last_insert_rowid();
        let mut ids_and_hashes = Vec::new();
        for i in 0..count {
            let text = format!("chunk text {i}");
            let hash = crate::db::chunk_hash::compute_chunk_hash(&text, path, i);
            conn.execute(
                "INSERT INTO chunks (doc_id, chunk_text, position, start_line, end_line, symbol_name, strategy, content_hash)
                 VALUES (?1, ?2, ?3, 1, 3, NULL, 'prose', ?4)",
                params![doc_id, text, i as i64, hash],
            )
            .unwrap();
            ids_and_hashes.push((conn.last_insert_rowid(), hash));
        }
        ids_and_hashes
    }

    fn source_ref_json(hashes: &[String]) -> String {
        let evidence: Vec<String> = hashes
            .iter()
            .map(|h| format!(r#"{{"content_hash":"{h}","quote":"q","start_line":1,"end_line":3}}"#))
            .collect();
        format!(
            r#"{{"proposal_id":"prop_1","evidence":[{}]}}"#,
            evidence.join(",")
        )
    }

    #[test]
    fn source_docs_from_ref_returns_paths_with_chunk_ids() {
        // Two chunks in the SAME document → 1 entry (path dedup), chunkId set.
        let conn = open_in_memory().unwrap();
        let chunks = seed_doc_with_chunks(&conn, "documents/notes.md", 2);
        let hashes: Vec<String> = chunks.iter().map(|(_, h)| h.clone()).collect();
        let source_ref = source_ref_json(&hashes);
        let docs = source_docs_from_ref(&conn, "fact_t", Some(&source_ref));
        assert_eq!(
            docs,
            vec![("documents/notes.md".to_string(), Some(chunks[0].1.clone()))]
        );
    }

    #[test]
    fn source_docs_from_ref_returns_distinct_entries_per_chunk() {
        // Two chunks in DIFFERENT documents → 2 entries, each with its own chunkId.
        let conn = open_in_memory().unwrap();
        let chunks_a = seed_doc_with_chunks(&conn, "documents/a.md", 1);
        let chunks_b = seed_doc_with_chunks(&conn, "documents/b.md", 1);
        let hashes = vec![chunks_a[0].1.clone(), chunks_b[0].1.clone()];
        let source_ref = source_ref_json(&hashes);
        let docs = source_docs_from_ref(&conn, "fact_t", Some(&source_ref));
        assert_eq!(
            docs,
            vec![
                ("documents/a.md".to_string(), Some(chunks_a[0].1.clone())),
                ("documents/b.md".to_string(), Some(chunks_b[0].1.clone())),
            ]
        );
    }

    #[test]
    fn source_docs_from_ref_dedupes_paths() {
        // Regression guard for today's path-dedup: interleaved evidence keeps
        // first-seen order, one entry per path, first occurrence's chunk id.
        let conn = open_in_memory().unwrap();
        let chunks_a = seed_doc_with_chunks(&conn, "documents/a.md", 2);
        let chunks_b = seed_doc_with_chunks(&conn, "documents/b.md", 1);
        let hashes = vec![
            chunks_b[0].1.clone(),
            chunks_a[0].1.clone(),
            chunks_a[1].1.clone(),
        ];
        let source_ref = source_ref_json(&hashes);
        let docs = source_docs_from_ref(&conn, "fact_t", Some(&source_ref));
        assert_eq!(
            docs,
            vec![
                ("documents/b.md".to_string(), Some(chunks_b[0].1.clone())),
                ("documents/a.md".to_string(), Some(chunks_a[0].1.clone())),
            ]
        );
    }

    #[test]
    fn source_docs_from_ref_handles_missing_chunks() {
        // content_hash that doesn't resolve to any document → no entry.
        let conn = open_in_memory().unwrap();
        let bogus_hash = "0".repeat(32);
        let source_ref = source_ref_json(&[bogus_hash]);
        assert!(source_docs_from_ref(&conn, "fact_t", Some(&source_ref)).is_empty());
    }

    #[test]
    fn source_docs_from_ref_handles_evidence_without_chunk_id() {
        // Spec R2.3.2a (Task 2): ANY evidence entry that lacks content_hash is
        // an unresolved entry — the WHOLE ref resolves to
        // `SourceResolution::HadEvidenceUnresolved`, which the display
        // wrapper degrades to `[]`. Pre-wave-1 semantics used to skip the
        // entry and keep the sibling; the spec r10-MINOR-4 consolidated both
        // cases (chunk-id legacy, malformed JSON, empty hash) under one
        // report-only outcome, so a sibling's resolution cannot be trusted
        // when one entry is unverifiable.
        let conn = open_in_memory().unwrap();
        let chunks = seed_doc_with_chunks(&conn, "documents/notes.md", 1);
        let source_ref = format!(
            r#"{{"proposal_id":"prop_1","evidence":[{{"quote":"no chunk id","start_line":1,"end_line":3}},{{"content_hash":"{}","quote":"q","start_line":1,"end_line":3}}]}}"#,
            chunks[0].1
        );
        assert!(source_docs_from_ref(&conn, "fact_t", Some(&source_ref)).is_empty());
    }

    #[test]
    fn source_docs_from_ref_handles_malformed_source_ref() {
        let conn = open_in_memory().unwrap();
        assert!(source_docs_from_ref(&conn, "fact_t", Some("not json")).is_empty());
        assert!(source_docs_from_ref(&conn, "fact_t", None).is_empty());
    }

    #[test]
    fn source_docs_from_ref_routes_strict_tokens_to_librarian_evidence() {
        // A real `librarian-<32hex>` token resolves through the CT-owned
        // `librarian_evidence` row — pinning that the §2.2 strict shape test
        // accepts what `librarian_source_ref_token` generates.
        let conn = open_in_memory().unwrap();
        let chunks = seed_doc_with_chunks(&conn, "documents/notes.md", 1);
        let token = crate::db::commit::librarian_source_ref_token("fact_lib");
        conn.execute(
            "INSERT INTO llm_wiki_entries (id, entity_id, title, body, tags, confidence,
                 source_type, source_ref, created_at, updated_at, access_count)
             VALUES ('fact_lib','ent','t','b','[]','inferred','librarian_inferred',?1,1,1,0)",
            params![token],
        )
        .unwrap();
        crate::db::commit::insert_librarian_evidence(
            &conn,
            "fact_lib",
            "prop_1",
            &source_ref_json(&[chunks[0].1.clone()]),
            false,
            1,
        )
        .unwrap();
        let docs = source_docs_from_ref(&conn, "fact_lib", Some(&token));
        assert_eq!(
            docs,
            vec![("documents/notes.md".to_string(), Some(chunks[0].1.clone()))]
        );
    }

    #[test]
    fn source_docs_from_ref_rejects_prefix_lookalike_refs() {
        // `librarian-notes.md` shares the token prefix but is not a §2.2
        // token: it must keep the document branch (here: unparseable as
        // evidence JSON → empty), never surface `librarian_evidence` through
        // the entry-id lookup.
        let conn = open_in_memory().unwrap();
        let chunks = seed_doc_with_chunks(&conn, "librarian-notes.md", 1);
        conn.execute(
            "INSERT INTO llm_wiki_entries (id, entity_id, title, body, tags, confidence,
                 source_type, source_ref, created_at, updated_at, access_count)
             VALUES ('fact_doc','ent','t','b','[]','inferred','document','librarian-notes.md',1,1,0)",
            [],
        )
        .unwrap();
        crate::db::commit::insert_librarian_evidence(
            &conn,
            "fact_doc",
            "prop_1",
            &source_ref_json(&[chunks[0].1.clone()]),
            false,
            1,
        )
        .unwrap();
        assert!(source_docs_from_ref(&conn, "fact_doc", Some("librarian-notes.md")).is_empty());
    }

    #[test]
    fn source_docs_from_ref_skips_evidence_with_empty_content_hash() {
        // Spec R2.3.2a: empty content_hash is an unresolved entry — the WHOLE
        // ref degrades to `[]`. The pre-wave-1 "skip and continue" behavior is
        // gone: spec r10-MINOR-4 unified chunk-id legacy / malformed JSON /
        // empty hash under one report-only outcome.
        let conn = open_in_memory().unwrap();
        let chunks = seed_doc_with_chunks(&conn, "documents/notes.md", 1);
        let source_ref = format!(
            r#"{{"proposal_id":"prop_1","evidence":[{{"content_hash":"","quote":"empty","start_line":1,"end_line":3}},{{"content_hash":"{}","quote":"q","start_line":1,"end_line":3}}]}}"#,
            chunks[0].1
        );
        assert!(
            source_docs_from_ref(&conn, "fact_t", Some(&source_ref)).is_empty(),
            "any unresolved evidence entry must degrade the whole ref"
        );
    }

    #[test]
    fn create_and_get_entity_round_trip() {
        let mut conn = open_in_memory().unwrap();
        let detail = create_entity(
            &mut conn,
            &CreateEntityInput {
                name: "Project Alpha".into(),
                entity_type: Some("project".into()),
                summary: Some("Summary prose.".into()),
            },
        )
        .unwrap();
        assert!(detail.id.starts_with("ent_"));
        assert_eq!(detail.name, "Project Alpha");
        // Wave-1 (spec §2.5 GUI bullet, r2-M2a): on a SKIP (no manifest
        // row → no vocabulary to violate) the literal `'concept'` stands
        // regardless of the proposed entity_type. Pre-wave-1 stored the
        // proposed label verbatim; the gate now keeps the LITERAL
        // constant so a later strict flip surfaces the row to heal as
        // ledger-tagged (not as an invented type).
        assert_eq!(detail.entity_type, "concept");
        assert_eq!(detail.summary, "Summary prose.");

        let loaded = get_entity(&conn, &detail.id).unwrap().unwrap();
        assert_eq!(loaded.name, "Project Alpha");
        assert!(loaded.facts.is_empty());
    }

    #[test]
    fn list_entities_excludes_archived_by_default() {
        let mut conn = open_in_memory().unwrap();
        let active = create_entity(
            &mut conn,
            &CreateEntityInput {
                name: "Active".into(),
                entity_type: None,
                summary: None,
            },
        )
        .unwrap();
        let archived = create_entity(
            &mut conn,
            &CreateEntityInput {
                name: "Gone".into(),
                entity_type: None,
                summary: None,
            },
        )
        .unwrap();
        archive_entity(&conn, &archived.id).unwrap();

        let list = list_entities(&conn, EntitySort::NameAsc, &EntityListFilter::default()).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, active.id);

        let with_archived = list_entities(
            &conn,
            EntitySort::NameAsc,
            &EntityListFilter {
                include_archived: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(with_archived.len(), 2);
    }

    #[test]
    fn get_entity_hydrates_facts_tasks_events() {
        let mut conn = open_in_memory().unwrap();
        let detail = create_entity(
            &mut conn,
            &CreateEntityInput {
                name: "Hydrated".into(),
                entity_type: None,
                summary: None,
            },
        )
        .unwrap();
        seed_fact(&conn, &detail.id, "fact-1", "A fact.");
        seed_task(&conn, &detail.id, "task-1", "pending");
        seed_event(&conn, &detail.id, "evt-1", "Something happened.");

        let loaded = get_entity(&conn, &detail.id).unwrap().unwrap();
        assert_eq!(loaded.facts.len(), 1);
        assert_eq!(loaded.facts[0].body, "A fact.");
        assert_eq!(loaded.tasks.len(), 1);
        assert_eq!(loaded.events.len(), 1);
        assert_eq!(
            list_entities(&conn, EntitySort::default(), &EntityListFilter::default()).unwrap()[0]
                .fact_count,
            1
        );
    }

    #[test]
    fn update_entity_summary_clears_embedding() {
        let mut conn = open_in_memory().unwrap();
        let detail = create_entity(
            &mut conn,
            &CreateEntityInput {
                name: "Edit me".into(),
                entity_type: None,
                summary: Some("Old".into()),
            },
        )
        .unwrap();
        conn.execute(
            "UPDATE curated_entities SET summary_embedding = X'01020304' WHERE id = ?1",
            [&detail.id],
        )
        .unwrap();

        update_entity_summary(&conn, &detail.id, "New summary").unwrap();

        let summary: String = conn
            .query_row(
                "SELECT summary FROM curated_entities WHERE id = ?1",
                [&detail.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(summary, "New summary");
        let embedding: Option<Vec<u8>> = conn
            .query_row(
                "SELECT summary_embedding FROM curated_entities WHERE id = ?1",
                [&detail.id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(embedding.is_none());
    }

    #[test]
    fn fact_source_docs_resolved_from_source_ref() {
        let mut conn = open_in_memory().unwrap();
        let detail = create_entity(
            &mut conn,
            &CreateEntityInput {
                name: "Sourced".into(),
                entity_type: None,
                summary: None,
            },
        )
        .unwrap();

        conn.execute(
            "INSERT INTO documents (path, hash, tier, status) VALUES ('documents/notes.md', 'h1', 'user_doc', 'indexed')",
            [],
        )
        .unwrap();
        let doc_id = conn.last_insert_rowid();
        let text = "quoted text";
        let hash = crate::db::chunk_hash::compute_chunk_hash(text, "documents/notes.md", 0);
        conn.execute(
            "INSERT INTO chunks (doc_id, chunk_text, position, start_line, end_line, symbol_name, strategy, content_hash)
             VALUES (?1, ?2, 0, 1, 3, NULL, 'prose', ?3)",
            params![doc_id, text, hash],
        )
        .unwrap();
        let source_ref = format!(
            r#"{{"proposal_id":"prop_1","evidence":[{{"content_hash":"{hash}","quote":"quoted text","start_line":1,"end_line":3}}]}}"#
        );
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type, source_ref, created_at, updated_at
             ) VALUES ('fact-src', ?1, 'T', 'B', '[]', 'inferred', 'user_confirmed', ?2, 100, 100)",
            params![detail.id, source_ref],
        )
        .unwrap();
        // A fact with NULL source_ref must yield an empty list, not an error.
        seed_fact(&conn, &detail.id, "fact-plain", "No source.");

        let loaded = get_entity(&conn, &detail.id).unwrap().unwrap();
        let sourced = loaded.facts.iter().find(|f| f.id == "fact-src").unwrap();
        assert_eq!(
            sourced.source_docs,
            vec![SourceDocRef {
                path: "documents/notes.md".to_string(),
                chunk_hash: Some(hash),
            }],
        );
        let plain = loaded.facts.iter().find(|f| f.id == "fact-plain").unwrap();
        assert!(plain.source_docs.is_empty());
    }

    #[test]
    fn fact_v02_fields_populated_from_okf_sources_column() {
        // Seed a fact with okf_sources / okf_verified populated, then load via get_entity
        // and assert the parsed Vec<OkfSourceEntry> / Vec<OkfVerifiedEntry> round-trip.
        let mut conn = open_in_memory().unwrap();
        let detail = create_entity(
            &mut conn,
            &CreateEntityInput {
                name: "V02".into(),
                entity_type: None,
                summary: None,
            },
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_entries (id, entity_id, title, body, tags, confidence, source_type,
                created_at, updated_at, lifecycle_status, okf_sources, okf_verified, okf_usage_window)
             VALUES ('fact-v02', ?1, 'T', 'B', '[]', 'certain', 'user_stated', 100, 100,
                     'stable',
                     '[{\"resource\":\"documents/notes.md\",\"usage_count\":3}]',
                     '[{\"by\":\"process:nightly\",\"at\":1700000000000}]',
                     '{\"from\":\"2026-07-01\",\"to\":\"2026-12-31\"}')",
            params![detail.id],
        ).unwrap();
        let loaded = get_entity(&conn, &detail.id).unwrap().unwrap();
        let fact = loaded.facts.iter().find(|f| f.id == "fact-v02").unwrap();
        assert_eq!(fact.lifecycle_status, "stable");
        assert_eq!(fact.okf_sources.len(), 1);
        assert_eq!(fact.okf_sources[0].resource, "documents/notes.md");
        assert_eq!(fact.okf_sources[0].usage_count, Some(3));
        assert_eq!(fact.okf_verified.len(), 1);
        assert_eq!(fact.okf_verified[0].by, "process:nightly");
        assert_eq!(fact.okf_usage_window.as_ref().unwrap().from, "2026-07-01");
    }

    #[test]
    fn fact_v02_okf_verified_accepts_iso_at_string() {
        // The OKF v0.2 frontmatter reader writes `at` as an ISO-8601 string
        // (e.g. `2026-07-02T00:00:00.000Z`); the importer round-trips that
        // JSON into `llm_wiki_entries.okf_verified` verbatim. `parse_okf_verified`
        // must normalize the ISO form to epoch ms so the UI sees the record.
        let mut conn = open_in_memory().unwrap();
        let detail = create_entity(
            &mut conn,
            &CreateEntityInput {
                name: "Iso".into(),
                entity_type: None,
                summary: None,
            },
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_entries (id, entity_id, title, body, tags, confidence, source_type,
                created_at, updated_at, lifecycle_status, okf_verified)
             VALUES ('fact-iso', ?1, 'T', 'B', '[]', 'certain', 'user_stated', 100, 100,
                     'stable',
                     '[{\"by\":\"process:nightly\",\"at\":\"2026-07-02T00:00:00.000Z\"}]')",
            params![detail.id],
        ).unwrap();
        let loaded = get_entity(&conn, &detail.id).unwrap().unwrap();
        let fact = loaded.facts.iter().find(|f| f.id == "fact-iso").unwrap();
        assert_eq!(
            fact.okf_verified.len(),
            1,
            "ISO at must round-trip into a record"
        );
        assert_eq!(fact.okf_verified[0].by, "process:nightly");
        // 2026-07-02T00:00:00.000Z = 1782950400000 ms; exact value locked in to
        // catch silent deserializer regressions.
        assert_eq!(fact.okf_verified[0].at, 1782950400000);
    }

    /// `resolve_source_core` distinguishes all three outcomes for the matrix.
    #[test]
    fn resolve_source_core_distinguishes_three_outcomes() {
        let conn = open_in_memory().unwrap();

        // 1. Resolved — librarian token + valid evidence + matching chunk.
        conn.execute(
            "INSERT INTO documents (path, hash, tier, status) VALUES ('/v/a.md', 'h', 'user_doc', 'indexed')",
            [],
        )
        .unwrap();
        let doc_id: i64 = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO chunks (doc_id, chunk_text, position, start_line, end_line, strategy, entity_id)
             VALUES (?1, 'c', 0, 1, 1, 'prose', 'tier_wisdom')",
            [doc_id],
        )
        .unwrap();
        let chunk_id: i64 = conn.last_insert_rowid();
        conn.execute(
            "UPDATE chunks SET content_hash = 'deadbeef' WHERE id = ?1",
            [chunk_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_entries (id, entity_id, title, body, tags, confidence, source_type, source_ref, created_at, updated_at, access_count)
             VALUES ('fact1', 'ent1', 't', 'b', '[]', 'inferred', 'librarian_inferred',
                     'librarian-deadbeefdeadbeefdeadbeefdeadbeef', 1, 1, 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO librarian_evidence (entry_id, proposal_id, evidence_json, unanchored, created_at)
             VALUES ('fact1', 'p1', '{\"proposal_id\":\"p1\",\"evidence\":[{\"content_hash\":\"deadbeef\"}]}', 0, 1)",
            [],
        )
        .unwrap();
        let r = resolve_source_core(
            &conn,
            "fact1",
            Some("librarian-deadbeefdeadbeefdeadbeefdeadbeef"),
        )
        .unwrap();
        assert!(matches!(r, SourceResolution::Resolved(_)), "got {r:?}");

        // 2. HadEvidenceUnresolved — librarian token but evidence row missing.
        let r = resolve_source_core(
            &conn,
            "fact_missing",
            Some("librarian-deadbeefdeadbeefdeadbeefdeadbeef"),
        )
        .unwrap();
        assert_eq!(r, SourceResolution::HadEvidenceUnresolved);

        // 3. NoEvidence — source_ref is None.
        let r = resolve_source_core(&conn, "fact1", None).unwrap();
        assert_eq!(r, SourceResolution::NoEvidence);
    }

    /// Plain path ref (non-librarian, non-JSON) → HadEvidenceUnresolved
    /// (spec R2.3.2a: not a NoEvidence).
    #[test]
    fn resolve_source_core_plain_path_is_unresolved() {
        let conn = open_in_memory().unwrap();
        let r = resolve_source_core(&conn, "fact1", Some("documents/notes.md")).unwrap();
        assert_eq!(r, SourceResolution::HadEvidenceUnresolved);
    }

    /// JSON with `evidence: []` → HadEvidenceUnresolved (the V20 doomed-row
    /// shape).
    #[test]
    fn resolve_source_core_empty_evidence_is_unresolved() {
        let conn = open_in_memory().unwrap();
        let r = resolve_source_core(
            &conn,
            "fact1",
            Some(r#"{"proposal_id":null,"evidence":[]}"#),
        )
        .unwrap();
        assert_eq!(r, SourceResolution::HadEvidenceUnresolved);
    }

    /// DB fault in the chunks query → propagates the error (R2.3.2: a DB
    /// fault must NOT masquerade as "no source").
    #[test]
    fn resolve_source_core_propagates_db_faults() {
        // Open an in-memory connection WITHOUT running migrations to force a
        // fault on the chunk join. The helper expects a librarian token, so
        // the chunks query is the first DB call.
        let conn = Connection::open_in_memory().unwrap();
        let r = resolve_source_core(
            &conn,
            "fact1",
            Some("librarian-deadbeefdeadbeefdeadbeefdeadbeef"),
        );
        // No documents table at all → the chunks query errors.
        assert!(r.is_err(), "a DB fault must propagate, not swallow");
    }

    // ---- Task 7 (spec R2.7.5): read/write redirect resolution ----------

    fn redirect(conn: &Connection, loser: &str, survivor: &str) {
        conn.execute(
            "INSERT INTO entity_redirects (entity_id, merged_into, created_at)
             VALUES (?1, ?2, 1)",
            params![loser, survivor],
        )
        .unwrap();
    }

    fn seed_pair() -> (rusqlite::Connection, String, String) {
        let mut conn = open_in_memory().unwrap();
        let a = create_entity(
            &mut conn,
            &CreateEntityInput {
                name: "Adrian".into(),
                entity_type: None,
                summary: Some("same".into()),
            },
        )
        .unwrap();
        let b = create_entity(
            &mut conn,
            &CreateEntityInput {
                name: "Adrian".into(),
                entity_type: None,
                summary: Some("same".into()),
            },
        )
        .unwrap();
        (conn, a.id, b.id)
    }

    /// r11-M4 (§6 item 10): after a merge, `get_entity(loser)` returns the
    /// SURVIVOR (`EntityDetail.id` = survivor), the survivor's reads cover
    /// the loser's facts/tasks (transitive closure, r13-m3), counts include
    /// them, and the loser never appears in `list_entities`.
    #[test]
    fn get_entity_loser_redirects_to_survivor_with_transitive_closure() {
        let (conn, surv, loser) = seed_pair();
        seed_fact(&conn, &loser, "fact-loser", "Loser fact.");
        seed_fact(&conn, &surv, "fact-surv", "Survivor fact.");
        seed_task(&conn, &loser, "task-loser", "pending");
        redirect(&conn, &loser, &surv);

        // get_entity(loser) → survivor detail (r13-m3: id is the SURVIVOR).
        let via_loser = get_entity(&conn, &loser).unwrap().unwrap();
        assert_eq!(via_loser.id, surv, "EntityDetail.id must be the survivor");
        assert_eq!(via_loser.facts.len(), 2, "closure covers both facts");
        assert_eq!(via_loser.tasks.len(), 1);
        assert!(via_loser.facts.iter().any(|f| f.id == "fact-loser"));

        // get_entity(survivor) covers the loser's facts too.
        let via_surv = get_entity(&conn, &surv).unwrap().unwrap();
        assert_eq!(via_surv.id, surv);
        assert!(via_surv.facts.iter().any(|f| f.id == "fact-loser"));

        // list_entities: loser never listed; survivor's counts include the
        // loser's fact + open task.
        let list = list_entities(&conn, EntitySort::default(), &EntityListFilter::default())
            .unwrap()
            .into_iter()
            .filter(|s| s.id == surv)
            .collect::<Vec<_>>();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].fact_count, 2, "{:?}", list[0]);
        assert_eq!(list[0].open_task_count, 1);
        let listed_ids: Vec<String> =
            list_entities(&conn, EntitySort::default(), &EntityListFilter::default())
                .unwrap()
                .into_iter()
                .map(|s| s.id)
                .collect();
        assert!(!listed_ids.contains(&loser), "loser is never returned");
    }

    /// r21: the redirect hop is followed REGARDLESS of the survivor's
    /// `deleted_at` — an archived survivor returns archived detail, never
    /// `None` (a stale loser link must not look like a deleted entity).
    #[test]
    fn get_entity_loser_follows_redirect_to_archived_survivor() {
        let (conn, surv, loser) = seed_pair();
        redirect(&conn, &loser, &surv);
        archive_entity(&conn, &surv).unwrap();

        let detail = get_entity(&conn, &loser).unwrap();
        let detail = detail.expect("archived survivor is returned, never None");
        assert_eq!(detail.id, surv);
        assert!(detail.deleted_at.is_some(), "archived detail as-is");
    }

    /// r13-MAJOR-1 / r15-m3: a mutator keyed by a loser id acts on the
    /// SURVIVOR — editing via a stale loser link edits the survivor.
    #[test]
    fn update_entity_summary_via_loser_hits_survivor() {
        let (conn, surv, loser) = seed_pair();
        redirect(&conn, &loser, &surv);
        update_entity_summary(&conn, &loser, "Edited via loser").unwrap();
        let summary: String = conn
            .query_row(
                "SELECT summary FROM curated_entities WHERE id = ?1",
                [&surv],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(summary, "Edited via loser");
    }

    /// r13-MAJOR-1: archiving a survivor archives the WHOLE cluster
    /// (survivor + losers); redirect rows are KEPT (r21) so a stale loser
    /// link still resolves.
    #[test]
    fn archive_entity_archives_whole_cluster_and_keeps_redirects() {
        let (conn, surv, loser) = seed_pair();
        redirect(&conn, &loser, &surv);
        // Archive via the LOSER link (r15-m3): acts on the survivor cluster.
        archive_entity(&conn, &loser).unwrap();
        let surv_deleted: Option<i64> = conn
            .query_row(
                "SELECT deleted_at FROM curated_entities WHERE id = ?1",
                [&surv],
                |r| r.get(0),
            )
            .unwrap();
        let loser_deleted: Option<i64> = conn
            .query_row(
                "SELECT deleted_at FROM curated_entities WHERE id = ?1",
                [&loser],
                |r| r.get(0),
            )
            .unwrap();
        assert!(surv_deleted.is_some(), "survivor archived with the cluster");
        assert!(loser_deleted.is_some(), "loser archived with the cluster");
        let redirects: i64 = conn
            .query_row("SELECT COUNT(*) FROM entity_redirects", [], |r| r.get(0))
            .unwrap();
        assert_eq!(redirects, 1, "archive keeps the redirect row (r21)");
    }

    /// r21 merge reversal: deleting the loser's redirect row fully restores
    /// it as a standalone entity — reads, recall, and export exactly as
    /// before the merge (the merge's only state is the redirect row).
    #[test]
    fn deleting_redirect_row_restores_loser() {
        let (conn, surv, loser) = seed_pair();
        seed_fact(&conn, &loser, "fact-loser", "Loser fact.");
        redirect(&conn, &loser, &surv);
        // Merged state: loser hidden.
        assert_eq!(get_entity(&conn, &loser).unwrap().unwrap().id, surv);

        conn.execute(
            "DELETE FROM entity_redirects WHERE entity_id = ?1",
            [&loser],
        )
        .unwrap();

        let detail = get_entity(&conn, &loser).unwrap().unwrap();
        assert_eq!(detail.id, loser, "loser reads as itself again");
        assert_eq!(detail.facts.len(), 1);
        assert_eq!(detail.facts[0].id, "fact-loser");
        let listed: Vec<String> =
            list_entities(&conn, EntitySort::default(), &EntityListFilter::default())
                .unwrap()
                .into_iter()
                .map(|s| s.id)
                .collect();
        assert!(listed.contains(&loser), "loser lists again");
        // Export ships it standalone again with its facts.
        let exported = crate::db::bundle_io::load_export_entities(&conn, None).unwrap();
        assert_eq!(exported.len(), 2);
        let restored = exported.iter().find(|e| e.entity_id == loser).unwrap();
        assert_eq!(restored.facts.len(), 1);
    }

    /// r2-m6 read-side cycle guard: a hand-crafted cycle errors (reported),
    /// never loops, never returns a guessed survivor.
    #[test]
    fn redirect_cycle_errors_not_loops() {
        let (conn, a, b) = seed_pair();
        redirect(&conn, &a, &b);
        redirect(&conn, &b, &a);
        assert!(get_entity(&conn, &a).is_err());
        assert!(resolve_entity_id(&conn, &a).is_err());
        // A self-cycle too.
        conn.execute(
            "INSERT OR REPLACE INTO entity_redirects (entity_id, merged_into, created_at)
             VALUES (?1, ?1, 1)",
            params![b],
        )
        .unwrap();
        assert!(resolve_entity_id(&conn, &b).is_err());
    }
}
