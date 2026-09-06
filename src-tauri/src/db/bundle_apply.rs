//! Transactional OKF bundle import: preview + apply for merge/replace/clone.
//! Summary writes go to `curated_entities.summary` only (backend spec
//! addendum point 4, option (a)) — never to llm_wiki_meta.

use std::collections::HashMap;

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};

use crate::db::commit::{
    push_entries_outbox, push_tasks_outbox, wiki_fact_outbox_payload, wiki_task_outbox_payload,
};
use crate::db::edge_purge::{purge_dead_edges, purge_edges_for_hard_deleted};
use crate::db::outbox_format::OutboxOperation;
use crate::okf::bundle_read::{ParsedBundle, ParsedEntity};
use crate::okf::ids::generate_id;
use crate::okf::timefmt::ms_from_utc_date;
#[cfg(test)]
use crate::okf::types::LLM_WIKI_PROFILE_V2;

/// Per-fact synthesized `okf_sources` JSON, keyed by fact id.
/// Populated by `synthesize_sources_from_body` when a profile-1 fact
/// has no `sources` key but its body carries a `# Citations` section.
type SyntheticSources = std::collections::HashMap<String, String>;

/// v0.1 → v0.2 fallback (upstream §4.8): synthesize `okf_sources` from a
/// `# Citations` body list when a profile-1 fact has no `sources` key.
/// Captures every URL, not just the first.
fn synthesize_sources_from_body(bundle: &ParsedBundle) -> SyntheticSources {
    let mut out = SyntheticSources::new();
    for entity in &bundle.entities {
        for fact in &entity.facts {
            if fact.okf_sources.is_some() {
                continue;
            }
            if !fact.body.contains("# Citations") {
                continue;
            }
            let urls = extract_citations_urls(&fact.body);
            if urls.is_empty() {
                continue;
            }
            let entries: Vec<serde_json::Value> = urls
                .into_iter()
                .map(|u| serde_json::json!({ "resource": u }))
                .collect();
            out.insert(
                fact.id.clone(),
                serde_json::to_string(&entries).unwrap_or_default(),
            );
        }
    }
    out
}

/// Byte index of the first ASCII `http` in `s`, case-insensitive.
/// Returns `None` for empty inputs or strings shorter than `http`.
/// Used by the v0.1 → v0.2 `# Citations` fallback so URLs whose protocol
/// is upper- or mixed-case (`HTTPS://…`, `Https://…`, etc.) are still captured.
fn find_http_ci(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    if bytes.len() < 4 {
        return None;
    }
    (0..=bytes.len() - 4).find(|&i| {
        bytes[i].eq_ignore_ascii_case(&b'h')
            && bytes[i + 1].eq_ignore_ascii_case(&b't')
            && bytes[i + 2].eq_ignore_ascii_case(&b't')
            && bytes[i + 3].eq_ignore_ascii_case(&b'p')
    })
}

/// Scan a body for a `# Citations` section and collect every URL on subsequent lines.
fn extract_citations_urls(body: &str) -> Vec<String> {
    let mut urls = Vec::new();
    let mut in_section = false;
    for line in body.lines() {
        if line.trim_start().starts_with("# Citations") {
            in_section = true;
            // Standard markdown puts a blank line between a heading and its
            // body — fall through to consume it instead of breaking out.
            continue;
        }
        if in_section {
            // A second `# …` heading ends the section. Anything else (blank
            // lines included) is part of the citation list.
            if line.trim_start().starts_with('#') && !line.trim().is_empty() {
                break;
            }
            let mut rest = line;
            while let Some(idx) = find_http_ci(rest) {
                let url_start = idx;
                let mut end = url_start;
                let bytes = rest.as_bytes();
                while end < bytes.len()
                    && !(bytes[end] as char).is_whitespace()
                    && bytes[end] != b')'
                    && bytes[end] != b']'
                {
                    end += 1;
                }
                let url = rest[url_start..end].to_string();
                // Reject prose matches such as `https headers are required`.
                // Both `http://…` and `https://…` start with `http`; the bytes
                // after position 4 are `://` or `s://` respectively (case-
                // insensitive — the leading `http` was matched case-
                // insensitively too). Anything else is not a valid URL.
                let tail_lower = url[4..].to_ascii_lowercase();
                let has_scheme = tail_lower.starts_with("://") || tail_lower.starts_with("s://");
                if url.len() > 4 && has_scheme {
                    urls.push(url);
                }
                rest = &rest[end..];
            }
        }
    }
    urls
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportMode {
    Merge,
    Replace,
    Clone,
}

#[derive(Debug, Clone, Serialize)]
pub struct EntityImportPreview {
    pub entity_id: String,
    pub name: String,
    pub entity_exists: bool,
    pub facts_new: i64,
    pub facts_existing: i64,
    pub tasks_new: i64,
    pub tasks_existing: i64,
    pub edges_total: i64,
    pub events_new: i64,
    pub events_duplicate: i64,
    pub summary_action: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportPreview {
    pub profile: Option<String>,
    pub entities: Vec<EntityImportPreview>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ImportResult {
    pub entities_touched: i64,
    pub facts_added: i64,
    pub facts_skipped: i64,
    pub tasks_added: i64,
    pub tasks_skipped: i64,
    pub edges_added: i64,
    pub events_added: i64,
    pub events_skipped: i64,
}

fn now_timestamps() -> (i64, i64) {
    let dur = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    (dur.as_secs() as i64, dur.as_millis() as i64)
}

fn row_exists(conn: &Connection, sql: &str, params: &[&dyn rusqlite::ToSql]) -> Result<bool> {
    Ok(conn
        .query_row(sql, params, |_| Ok(()))
        .optional()?
        .is_some())
}

fn fact_exists(conn: &Connection, id: &str) -> Result<bool> {
    row_exists(conn, "SELECT 1 FROM llm_wiki_entries WHERE id=?1", &[&id])
}

/// True iff `source_ref` is the normative token shape `^librarian-[0-9a-f]{32}$`
/// (spec §2.2) — the Rust mirror of `evidence_repair::TOKEN_GLOB`.
fn is_librarian_token_shaped(source_ref: &str) -> bool {
    source_ref
        .strip_prefix("librarian-")
        .map(|rest| rest.len() == 32 && rest.bytes().all(|b| b.is_ascii_hexdigit()))
        .unwrap_or(false)
}

fn task_exists(conn: &Connection, id: &str) -> Result<bool> {
    row_exists(conn, "SELECT 1 FROM llm_wiki_tasks WHERE id=?1", &[&id])
}

fn event_exists(
    conn: &Connection,
    entity_id: &str,
    event_id: Option<&str>,
    event_type: &str,
    summary: &str,
    date: &str,
) -> Result<bool> {
    if let Some(id) = event_id {
        if row_exists(conn, "SELECT 1 FROM llm_wiki_events WHERE id=?1", &[&id])? {
            return Ok(true);
        }
        return Ok(false);
    }
    row_exists(
        conn,
        "SELECT 1 FROM llm_wiki_events
         WHERE entity_id=?1 AND event_type=?2 AND summary=?3
           AND date(created_at/1000, 'unixepoch')=?4",
        &[&entity_id, &event_type, &summary, &date],
    )
}

pub fn preview_import(
    conn: &Connection,
    bundle: &ParsedBundle,
    mode: ImportMode,
) -> Result<ImportPreview> {
    let mut entities = Vec::new();
    for entity in &bundle.entities {
        let entity_exists = row_exists(
            conn,
            "SELECT 1 FROM curated_entities WHERE id=?1",
            &[&entity.entity_id],
        )?;
        let local_summary: Option<String> = conn
            .query_row(
                "SELECT summary FROM curated_entities WHERE id=?1",
                [&entity.entity_id],
                |r| r.get(0),
            )
            .ok();

        let (mut facts_new, mut facts_existing) = (0i64, 0i64);
        for fact in &entity.facts {
            if mode != ImportMode::Clone && fact_exists(conn, &fact.id)? {
                facts_existing += 1;
            } else {
                facts_new += 1;
            }
        }
        let (mut tasks_new, mut tasks_existing) = (0i64, 0i64);
        for task in &entity.tasks {
            if mode != ImportMode::Clone && task_exists(conn, &task.id)? {
                tasks_existing += 1;
            } else {
                tasks_new += 1;
            }
        }
        let (mut events_new, mut events_duplicate) = (0i64, 0i64);
        for ev in &entity.events {
            let dup = mode != ImportMode::Clone
                && event_exists(
                    conn,
                    &entity.entity_id,
                    ev.event_id.as_deref(),
                    &ev.event_type,
                    &ev.summary,
                    &ev.date,
                )?;
            if dup {
                events_duplicate += 1;
            } else {
                events_new += 1;
            }
        }

        let summary_action = match mode {
            ImportMode::Clone => "new",
            ImportMode::Replace if entity.summary.is_some() => "overwrite",
            ImportMode::Replace => "none",
            ImportMode::Merge => match (&entity.summary, local_summary.as_deref()) {
                (None, _) => "none",
                (Some(_), Some(local)) if !local.trim().is_empty() => "keep_local",
                (Some(_), _) => "fill",
            },
        }
        .to_string();

        entities.push(EntityImportPreview {
            entity_id: entity.entity_id.clone(),
            name: entity
                .display_name
                .clone()
                .unwrap_or_else(|| entity.entity_id.clone()),
            entity_exists,
            facts_new,
            facts_existing,
            tasks_new,
            tasks_existing,
            edges_total: entity.edges.len() as i64,
            events_new,
            events_duplicate,
            summary_action,
        });
    }
    Ok(ImportPreview {
        profile: bundle.profile.clone(),
        entities,
        warnings: bundle.warnings.clone(),
    })
}

pub fn apply_import(
    conn: &mut Connection,
    bundle: &ParsedBundle,
    mode: ImportMode,
) -> Result<ImportResult> {
    let (now_secs, now_ms) = now_timestamps();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut result = ImportResult::default();

    // v0.1 → v0.2 fallback (upstream §4.8): if a profile-1 fact has no `sources` key
    // but its body carries a `# Citations` list, synthesize `okf_sources` from
    // the URLs (capturing every URL, not just the first).
    let synthetic_sources = synthesize_sources_from_body(bundle);

    // Ids hard-deleted by Replace-mode `clear_entity_content`, accumulated
    // across the loop and purged from `llm_wiki_edges` once at the end.
    let mut hard_deleted_ids: Vec<String> = Vec::new();

    for entity in &bundle.entities {
        let mut id_map: HashMap<String, String> = HashMap::new();
        let target_entity_id = match mode {
            ImportMode::Clone => generate_id("ent_"),
            _ => entity.entity_id.clone(),
        };
        if mode == ImportMode::Clone {
            for fact in &entity.facts {
                id_map.insert(fact.id.clone(), generate_id("fact_"));
            }
            for task in &entity.tasks {
                id_map.insert(task.id.clone(), generate_id("task_"));
            }
        }
        let mapped = |id: &str, map: &HashMap<String, String>| -> String {
            map.get(id).cloned().unwrap_or_else(|| id.to_string())
        };

        ensure_entity(&tx, entity, &target_entity_id, mode, now_secs)?;

        if mode == ImportMode::Replace {
            hard_deleted_ids.extend(clear_entity_content(&tx, &target_entity_id, now_ms)?);
        }

        for fact in &entity.facts {
            let fact_id = mapped(&fact.id, &id_map);
            if mode != ImportMode::Clone && fact_exists(&tx, &fact_id)? {
                result.facts_skipped += 1;
                continue;
            }
            let tags_json = serde_json::to_string(&fact.tags)?;
            // v0.1 → v0.2 fallback for `timestamp` ⇄ `generated.at`: Task 1 already
            // routes timestamp → updated_at and generated → generated_by. No code
            // change needed here — the synthesized sources sidecar covers
            // the body → okf_sources fallback.
            let effective_sources: Option<&str> = fact
                .okf_sources
                .as_deref()
                .or_else(|| synthetic_sources.get(&fact.id).map(String::as_str));
            // Pre-#186 librarian facts carry a legacy JSON `source_ref` (the
            // blob the engine setup rewrite mangled). On apply they are
            // normalized to the token shape, and a ref that still parses as
            // JSON with a `proposal_id` is salvaged into the paired
            // librarian_evidence row instead of being dropped. Spec §2.3.
            let mut legacy_evidence_json: Option<&str> = None;
            let effective_source_ref = if fact.source_type == "librarian_inferred"
                && !fact
                    .source_ref
                    .as_deref()
                    .map(is_librarian_token_shaped)
                    .unwrap_or(false)
            {
                if let Some(legacy) = fact.source_ref.as_deref() {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(legacy) {
                        if v.get("proposal_id").is_some() {
                            legacy_evidence_json = Some(legacy);
                        }
                    }
                }
                Some(crate::db::commit::librarian_source_ref_token(&fact_id))
            } else {
                fact.source_ref.clone()
            };
            tx.execute(
                "INSERT INTO llm_wiki_entries (
                    id, entity_id, title, body, tags, confidence, source_type,
                    source_hash, source_ref, okf_type,
                    lifecycle_status, stale_after, generated_by,
                    okf_sources, okf_verified, okf_usage_window,
                    last_verified_at, last_verified_by,
                    created_at, updated_at, last_accessed_at,
                    access_count, deleted_at
                 ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23)",
                params![
                    fact_id,
                    target_entity_id,
                    fact.title,
                    fact.body,
                    tags_json,
                    fact.confidence,
                    fact.source_type,
                    fact.source_hash,
                    effective_source_ref,
                    fact.okf_type,
                    fact.lifecycle_status,
                    fact.stale_after,
                    fact.generated_by,
                    effective_sources,
                    fact.okf_verified,
                    fact.okf_usage_window,
                    fact.last_verified_at,
                    fact.last_verified_by,
                    fact.created_at,
                    fact.updated_at,
                    fact.last_accessed_at,
                    fact.access_count,
                    fact.deleted_at,
                ],
            )?;
            push_entries_outbox(
                &tx,
                &target_entity_id,
                &fact_id,
                OutboxOperation::Insert,
                wiki_fact_outbox_payload(
                    &fact_id,
                    &target_entity_id,
                    &fact.title,
                    &fact.body,
                    &fact.tags,
                    &fact.confidence,
                    &fact.source_type,
                    fact.source_hash.as_deref(),
                    effective_source_ref.as_deref().unwrap_or(""),
                    fact.okf_type.as_deref(),
                    effective_sources,
                    fact.okf_verified.as_deref(),
                    fact.okf_usage_window.as_deref(),
                    fact.created_at,
                    fact.updated_at,
                    fact.deleted_at,
                    Some(fact.lifecycle_status.as_str()),
                    fact.stale_after,
                    fact.generated_by.as_deref(),
                    fact.last_verified_at,
                    fact.last_verified_by.as_deref(),
                ),
                now_ms,
            )?;
            // Bundle-applied librarian facts keep their token and their evidence
            // together; a token row without evidence is treated as still-grounded by
            // the §2.3 rule, but importing one deliberately would be a silent
            // provenance loss. Spec §2.3.
            if let Some(evidence_json) = fact.evidence_json.as_deref().or(legacy_evidence_json) {
                let proposal_id = serde_json::from_str::<serde_json::Value>(evidence_json)
                    .ok()
                    .and_then(|v| {
                        v.get("proposal_id")
                            .and_then(|p| p.as_str())
                            .map(String::from)
                    })
                    .unwrap_or_default();
                // Compute the real Phase-1 flag: a bundle-applied row whose
                // blob has no live chunk anchor must be unanchored=1, or it
                // re-arms the heal-purge bait. Spec §2.4.
                let unanchored = !crate::db::commit::evidence_has_live_chunk(&tx, evidence_json)?;
                crate::db::commit::insert_librarian_evidence(
                    &tx,
                    &fact_id,
                    &proposal_id,
                    evidence_json,
                    unanchored,
                    now_ms,
                )?;
            }
            result.facts_added += 1;
        }

        for task in &entity.tasks {
            let task_id = mapped(&task.id, &id_map);
            if mode != ImportMode::Clone && task_exists(&tx, &task_id)? {
                result.tasks_skipped += 1;
                continue;
            }
            tx.execute(
                "INSERT INTO llm_wiki_tasks (
                    id, entity_id, description, status, priority,
                    created_at, updated_at, resolved_at, deleted_at, okf_type,
                    lifecycle_status, stale_after, generated_by,
                    okf_sources, okf_verified, okf_usage_window,
                    last_verified_at, last_verified_by
                 ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)",
                params![
                    task_id,
                    target_entity_id,
                    task.description,
                    task.status,
                    task.priority,
                    task.created_at,
                    task.updated_at,
                    task.resolved_at,
                    task.deleted_at,
                    task.okf_type,
                    task.lifecycle_status,
                    task.stale_after,
                    task.generated_by,
                    task.okf_sources,
                    task.okf_verified,
                    task.okf_usage_window,
                    task.last_verified_at,
                    task.last_verified_by,
                ],
            )?;
            push_tasks_outbox(
                &tx,
                &target_entity_id,
                &task_id,
                OutboxOperation::Insert,
                wiki_task_outbox_payload(
                    &task_id,
                    &target_entity_id,
                    &task.description,
                    &task.status,
                    task.priority,
                    task.created_at,
                    task.updated_at,
                    task.resolved_at,
                    task.deleted_at,
                    task.okf_type.as_deref(),
                    task.okf_sources.as_deref(),
                    task.okf_verified.as_deref(),
                    task.okf_usage_window.as_deref(),
                    Some(task.lifecycle_status.as_str()),
                    task.stale_after,
                    task.generated_by.as_deref(),
                    task.last_verified_at,
                    task.last_verified_by.as_deref(),
                ),
                now_ms,
            )?;
            result.tasks_added += 1;
        }

        for (source, target, edge_type) in &entity.edges {
            let inserted = tx.execute(
                "INSERT OR IGNORE INTO llm_wiki_edges
                    (id, entity_id, source_id, target_id, edge_type, created_at)
                 VALUES (?1,?2,?3,?4,?5,?6)",
                params![
                    generate_id("edge_"),
                    target_entity_id,
                    mapped(source, &id_map),
                    mapped(target, &id_map),
                    edge_type,
                    now_secs,
                ],
            )?;
            result.edges_added += inserted as i64;
        }

        for ev in &entity.events {
            let (event_id, is_dup) = match mode {
                ImportMode::Clone => (generate_id("evt_"), false),
                _ => {
                    let dup = event_exists(
                        &tx,
                        &target_entity_id,
                        ev.event_id.as_deref(),
                        &ev.event_type,
                        &ev.summary,
                        &ev.date,
                    )?;
                    (
                        ev.event_id.clone().unwrap_or_else(|| generate_id("evt_")),
                        dup,
                    )
                }
            };
            if is_dup {
                result.events_skipped += 1;
                continue;
            }
            let created_at = ms_from_utc_date(&ev.date).unwrap_or(now_ms);
            let related = ev.related_entry_id.as_deref().map(|id| mapped(id, &id_map));
            tx.execute(
                "INSERT OR IGNORE INTO llm_wiki_events
                    (id, entity_id, event_type, summary, related_entry_id, created_at)
                 VALUES (?1,?2,?3,?4,?5,?6)",
                params![
                    event_id,
                    target_entity_id,
                    ev.event_type,
                    ev.summary,
                    related,
                    created_at
                ],
            )?;
            result.events_added += 1;
        }

        tx.execute(
            "INSERT INTO llm_wiki_events (id, entity_id, event_type, summary, related_entry_id, created_at)
             VALUES (?1,?2,'imported',?3,NULL,?4)",
            params![
                generate_id("evt_"),
                target_entity_id,
                format!(
                    "OKF import: bundle contains {} fact(s), {} task(s)",
                    entity.facts.len(),
                    entity.tasks.len()
                ),
                now_ms,
            ],
        )?;
        result.entities_touched += 1;
    }

    // Edges stamped with any of the replaced entities' ids may point across
    // entities; purge only edges whose BOTH endpoints have no live home
    // (in llm_wiki_entries / curated_entities / llm_wiki_tasks), otherwise
    // the bundle import strands partner edges (remediation brief R1). Done
    // once after the loop instead of once per entity in
    // `clear_entity_content` — the per-entity call was an unscoped DELETE
    // over `llm_wiki_edges` per row, so for a bundle of N entities we were
    // running N scans where one suffices (CodeRabbit thread on line 641).
    // Replace mode HARD-deletes the entity's entries and tasks. Those ids are
    // gone from every endpoint table, so an edge anchored on one references
    // nothing and no later cascade could ever find it — `purge_dead_edges`
    // below needs BOTH endpoints dead and would leave it behind forever.
    // Batched once after the loop, for the same reason as `purge_dead_edges`.
    if !hard_deleted_ids.is_empty() {
        purge_edges_for_hard_deleted(&tx, &hard_deleted_ids)?;
    }

    purge_dead_edges(&tx)?;

    tx.commit()?;
    Ok(result)
}

fn ensure_entity(
    tx: &Connection,
    entity: &ParsedEntity,
    target_entity_id: &str,
    mode: ImportMode,
    now_secs: i64,
) -> Result<()> {
    let name = entity
        .display_name
        .clone()
        .unwrap_or_else(|| entity.entity_id.clone());
    let bundle_summary = entity.summary.clone().unwrap_or_default();
    let existing: Option<String> = tx
        .query_row(
            "SELECT summary FROM curated_entities WHERE id=?1",
            [target_entity_id],
            |r| r.get(0),
        )
        .ok();
    match existing {
        None => {
            tx.execute(
                "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
                 VALUES (?1, ?2, 'concept', ?3, ?4, ?4)",
                params![target_entity_id, name, bundle_summary, now_secs],
            )?;
        }
        Some(local_summary) => {
            let write_summary = match mode {
                ImportMode::Replace => entity.summary.is_some(),
                ImportMode::Merge => entity.summary.is_some() && local_summary.trim().is_empty(),
                ImportMode::Clone => false,
            };
            if write_summary {
                tx.execute(
                    "UPDATE curated_entities SET summary=?2, updated_at=?3 WHERE id=?1",
                    params![target_entity_id, bundle_summary, now_secs],
                )?;
            }
        }
    }
    Ok(())
}

/// Wipe an entity's entries, tasks and events for a Replace-mode import.
///
/// Returns the ids that were **hard**-deleted (entries + tasks) so the caller
/// can purge their edges once, after the entity loop — see the call site in
/// `apply_import`.
fn clear_entity_content(tx: &Connection, entity_id: &str, now_ms: i64) -> Result<Vec<String>> {
    let fact_ids: Vec<String> = tx
        .prepare("SELECT id FROM llm_wiki_entries WHERE entity_id=?1")?
        .query_map([entity_id], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for id in &fact_ids {
        push_entries_outbox(
            tx,
            entity_id,
            id,
            OutboxOperation::Delete,
            serde_json::json!({ "id": id }),
            now_ms,
        )?;
    }
    let task_ids: Vec<String> = tx
        .prepare("SELECT id FROM llm_wiki_tasks WHERE entity_id=?1")?
        .query_map([entity_id], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for id in &task_ids {
        push_tasks_outbox(
            tx,
            entity_id,
            id,
            OutboxOperation::Delete,
            serde_json::json!({ "id": id }),
            now_ms,
        )?;
    }
    // FK CASCADE is not relied upon (spec §2.1): brain.db has connections whose
    // `PRAGMA foreign_keys` state we do not control, so the evidence row is
    // deleted explicitly alongside its entry.
    tx.execute(
        "DELETE FROM librarian_evidence WHERE entry_id IN
             (SELECT id FROM llm_wiki_entries WHERE entity_id = ?1)",
        [entity_id],
    )?;
    tx.execute(
        "DELETE FROM llm_wiki_entries WHERE entity_id=?1",
        [entity_id],
    )?;
    tx.execute("DELETE FROM llm_wiki_tasks WHERE entity_id=?1", [entity_id])?;
    // NOTE: neither `purge_dead_edges` nor `purge_edges_for_hard_deleted` is
    // called here. Both run once after the entire entity loop (in
    // `apply_import`) so the scan happens once per import rather than once
    // per entity — see CodeRabbit review thread on `bundle_apply.rs` line 641.
    // That is why the hard-deleted ids are returned rather than acted on.
    tx.execute(
        "DELETE FROM llm_wiki_events WHERE entity_id=?1",
        [entity_id],
    )?;
    let mut hard_deleted = fact_ids;
    hard_deleted.extend(task_ids);
    Ok(hard_deleted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::open_in_memory;
    use crate::okf::bundle_read::{ParsedBundle, ParsedEntity, ParsedEvent};
    use crate::okf::types::WikiFact;

    fn sample_fact(id: &str, entity_id: &str) -> WikiFact {
        WikiFact {
            id: id.into(),
            entity_id: entity_id.into(),
            title: "A fact".into(),
            body: "Body.".into(),
            tags: vec![],
            confidence: "certain".into(),
            source_type: "user_confirmed".into(),
            source_hash: None,
            source_ref: None,
            created_at: 1719835200000,
            updated_at: 1719835200000,
            last_accessed_at: None,
            access_count: 0,
            deleted_at: None,
            okf_type: None,
            lifecycle_status: "stable".into(),
            stale_after: None,
            generated_by: None,
            okf_sources: None,
            okf_verified: None,
            okf_usage_window: None,
            last_verified_at: None,
            last_verified_by: None,
            evidence_json: None,
        }
    }

    fn sample_bundle() -> ParsedBundle {
        ParsedBundle {
            profile: Some(LLM_WIKI_PROFILE_V2.into()),
            entities: vec![ParsedEntity {
                entity_id: "ent_a".into(),
                display_name: Some("Project X".into()),
                summary: Some("Bundle summary.".into()),
                facts: vec![sample_fact("fact_1", "ent_a")],
                tasks: vec![],
                edges: vec![],
                events: vec![ParsedEvent {
                    event_id: Some("evt_1".into()),
                    event_type: "action".into(),
                    summary: "Did a thing".into(),
                    related_entry_id: Some("fact_1".into()),
                    date: "2026-07-05".into(),
                }],
            }],
            ..ParsedBundle::default()
        }
    }

    #[test]
    fn merge_creates_entity_and_inserts_new_rows() {
        let mut conn = open_in_memory().unwrap();
        let result = apply_import(&mut conn, &sample_bundle(), ImportMode::Merge).unwrap();
        assert_eq!(result.facts_added, 1);
        assert_eq!(result.events_added, 1);
        let summary: String = conn
            .query_row(
                "SELECT summary FROM curated_entities WHERE id='ent_a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(summary, "Bundle summary.");
        let outbox: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_outbox WHERE table_name='entries'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(outbox, 1);
    }

    #[test]
    fn merge_is_idempotent_id_first() {
        let mut conn = open_in_memory().unwrap();
        apply_import(&mut conn, &sample_bundle(), ImportMode::Merge).unwrap();
        let second = apply_import(&mut conn, &sample_bundle(), ImportMode::Merge).unwrap();
        assert_eq!(second.facts_added, 0);
        assert_eq!(second.facts_skipped, 1);
        assert_eq!(second.events_added, 0);
        let facts: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_entries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(facts, 1);
    }

    #[test]
    fn merge_tuple_dedup_for_profile0_events() {
        let mut conn = open_in_memory().unwrap();
        apply_import(&mut conn, &sample_bundle(), ImportMode::Merge).unwrap();
        let mut profile0 = sample_bundle();
        profile0.profile = None;
        profile0.entities[0].facts.clear();
        profile0.entities[0].events[0].event_id = None;
        let result = apply_import(&mut conn, &profile0, ImportMode::Merge).unwrap();
        assert_eq!(
            result.events_added, 0,
            "tuple (type, summary, day) already present"
        );
    }

    #[test]
    fn replace_clears_then_inserts() {
        let mut conn = open_in_memory().unwrap();
        apply_import(&mut conn, &sample_bundle(), ImportMode::Merge).unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_entries (id, entity_id, title, body, tags, confidence, source_type, created_at, updated_at)
             VALUES ('fact_local', 'ent_a', 'Local only', 'x', '[]', 'inferred', 'librarian_inferred', 1, 1)",
            [],
        )
        .unwrap();
        apply_import(&mut conn, &sample_bundle(), ImportMode::Replace).unwrap();
        let ids: Vec<String> = conn
            .prepare("SELECT id FROM llm_wiki_entries WHERE entity_id='ent_a'")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            ids,
            vec!["fact_1".to_string()],
            "local-only fact removed by replace"
        );
    }

    #[test]
    fn clone_remaps_every_id() {
        let mut conn = open_in_memory().unwrap();
        apply_import(&mut conn, &sample_bundle(), ImportMode::Merge).unwrap();
        let result = apply_import(&mut conn, &sample_bundle(), ImportMode::Clone).unwrap();
        assert_eq!(result.facts_added, 1);
        let entities: i64 = conn
            .query_row("SELECT COUNT(*) FROM curated_entities", [], |r| r.get(0))
            .unwrap();
        assert_eq!(entities, 2);
        let fact1: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_entries WHERE id='fact_1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(fact1, 1);
        let evt1: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_events WHERE id='evt_1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(evt1, 1, "clone must remap event ids too");
        let dangling: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_events e
                 WHERE e.related_entry_id IS NOT NULL
                 AND NOT EXISTS (SELECT 1 FROM llm_wiki_entries f WHERE f.id = e.related_entry_id)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(dangling, 0);
    }

    #[test]
    fn replace_keeps_edges_stamped_with_the_entity_whose_endpoints_live_elsewhere() {
        // R1 proof: `clear_entity_content` (called by Replace mode) used to
        // DELETE every llm_wiki_edges row stamped with the dying entity_id,
        // even when the edge points into another live entity. That strands the
        // partner's edges. The `entity_id` STAMP must never decide an edge's
        // fate — only its endpoints do.
        //
        // Note the distinction this test now pins, which the earlier version
        // conflated: an edge stamped `ent_a` whose endpoints both live in
        // other entities survives a Replace on `ent_a`; an edge with an
        // endpoint IN `ent_a` does not, because Replace hard-deletes that
        // endpoint and the edge would reference a row that no longer exists in
        // any table (see `replace_purges_edges_anchored_on_its_hard_deleted_facts`).
        let mut conn = open_in_memory().unwrap();

        // Two entities, each with a fact.
        conn.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
             VALUES ('ent_a', 'A', 'concept', '', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
             VALUES ('ent_b', 'B', 'concept', '', 1, 1)",
            [],
        )
        .unwrap();
        for (fact_id, entity_id) in [
            ("fact_a_1", "ent_a"),
            ("fact_b_1", "ent_b"),
            ("fact_b_2", "ent_b"),
        ] {
            conn.execute(
                "INSERT INTO llm_wiki_entries (
                    id, entity_id, title, body, tags, confidence, source_type,
                    created_at, updated_at
                 ) VALUES (?1, ?2, 'T', 'B', '[]', 'inferred', 'librarian_inferred', 1, 1)",
                params![fact_id, entity_id],
            )
            .unwrap();
        }

        // Stamped with ent_a, but BOTH endpoints are ent_b's facts. The stamp
        // is the only thing tying it to the replaced entity, so it MUST
        // survive — this is the R1 regression guard.
        conn.execute(
            "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
             VALUES ('edge_cross', 'ent_a', 'fact_b_1', 'fact_b_2', 'related_to', 1)",
            [],
        )
        .unwrap();
        // Truly dead edge: neither endpoint exists anywhere.
        conn.execute(
            "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
             VALUES ('edge_orphan', 'ent_a', 'ghost_a', 'ghost_b', 'related_to', 1)",
            [],
        )
        .unwrap();

        // Replace ent_a — clears ent_a's facts/edges/etc.
        apply_import(&mut conn, &sample_bundle(), ImportMode::Replace).unwrap();

        let surviving_ids: Vec<String> = conn
            .prepare("SELECT id FROM llm_wiki_edges ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            surviving_ids,
            vec!["edge_cross".to_string()],
            "an edge merely STAMPED with the replaced entity survives when both \
             its endpoints live elsewhere; the orphan edge is purged because \
             both endpoints are dead everywhere"
        );
    }

    #[test]
    fn replace_purges_edges_anchored_on_its_hard_deleted_facts() {
        // Replace HARD-deletes the entity's facts. An edge from one of them
        // into a live fact in another entity references a row that is gone
        // from every endpoint table — the `purge_dead_edges` contract needs
        // BOTH endpoints dead, so nothing would ever collect it and it would
        // dangle for the life of the database.
        let mut conn = open_in_memory().unwrap();
        for (entity_id, name) in [("ent_a", "A"), ("ent_b", "B")] {
            conn.execute(
                "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
                 VALUES (?1, ?2, 'concept', '', 1, 1)",
                params![entity_id, name],
            )
            .unwrap();
        }
        for (fact_id, entity_id) in [("fact_a_1", "ent_a"), ("fact_b_1", "ent_b")] {
            conn.execute(
                "INSERT INTO llm_wiki_entries (
                    id, entity_id, title, body, tags, confidence, source_type,
                    created_at, updated_at
                 ) VALUES (?1, ?2, 'T', 'B', '[]', 'inferred', 'librarian_inferred', 1, 1)",
                params![fact_id, entity_id],
            )
            .unwrap();
        }
        // One in each direction, both anchored on the fact Replace destroys.
        conn.execute(
            "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
             VALUES ('edge_out', 'ent_a', 'fact_a_1', 'fact_b_1', 'related_to', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
             VALUES ('edge_in', 'ent_b', 'fact_b_1', 'fact_a_1', 'related_to', 1)",
            [],
        )
        .unwrap();

        apply_import(&mut conn, &sample_bundle(), ImportMode::Replace).unwrap();

        let surviving: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            surviving, 0,
            "edges anchored on a hard-deleted fact must not survive the import \
             that destroyed it, in either direction"
        );
        // The surviving partner is untouched.
        let partner: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_entries WHERE id = 'fact_b_1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(partner, 1);
    }

    #[test]
    fn preview_counts_without_writing() {
        let mut conn = open_in_memory().unwrap();
        apply_import(&mut conn, &sample_bundle(), ImportMode::Merge).unwrap();
        let preview = preview_import(&conn, &sample_bundle(), ImportMode::Merge).unwrap();
        assert_eq!(preview.entities.len(), 1);
        assert_eq!(preview.entities[0].facts_new, 0);
        assert_eq!(preview.entities[0].facts_existing, 1);
        assert_eq!(preview.entities[0].events_duplicate, 1);
    }

    #[test]
    fn extract_citations_urls_handles_uppercase_protocol() {
        let body = "# Citations\nHTTPS://example.com/path\n";
        assert_eq!(
            extract_citations_urls(body),
            vec!["HTTPS://example.com/path".to_string()]
        );
    }

    #[test]
    fn extract_citations_urls_handles_titlecase_protocol() {
        let body = "# Citations\nHttps://example.com/path\n";
        assert_eq!(
            extract_citations_urls(body),
            vec!["Https://example.com/path".to_string()]
        );
    }

    #[test]
    fn extract_citations_urls_handles_mixed_case_protocol() {
        let body = "# Citations\nhTtPs://example.com/path\n";
        assert_eq!(
            extract_citations_urls(body),
            vec!["hTtPs://example.com/path".to_string()]
        );
    }

    #[test]
    fn extract_citations_urls_tolerates_blank_line_after_heading() {
        // Standard markdown: heading, blank line, then the bullet list. The
        // pre-fix implementation broke on the blank line immediately after
        // the heading, which is the exact shape every formatter emits.
        let body =
            "Some intro text.\n\n# Citations\n\n- https://example.com/a\n- https://example.com/b\n";
        assert_eq!(
            extract_citations_urls(body),
            vec![
                "https://example.com/a".to_string(),
                "https://example.com/b".to_string(),
            ]
        );
    }

    #[test]
    fn extract_citations_urls_terminates_at_next_heading() {
        // A second `# …` heading (e.g. `# Notes`) must end the citations
        // section; URLs after it must not be captured.
        let body = "# Citations\n\n- https://example.com/a\n\n# Notes\n\n- https://example.com/b\n";
        assert_eq!(
            extract_citations_urls(body),
            vec!["https://example.com/a".to_string()]
        );
    }
}
