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

/// Scan a body for a `# Citations` section and collect every URL on subsequent lines.
fn extract_citations_urls(body: &str) -> Vec<String> {
    let mut urls = Vec::new();
    let mut in_section = false;
    for line in body.lines() {
        if line.trim_start().starts_with("# Citations") {
            in_section = true;
            continue;
        }
        if in_section && line.trim().is_empty() {
            break;
        }
        if in_section {
            let mut rest = line;
            while let Some(idx) = rest.find("http") {
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
                if url.len() > 4 {
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
            clear_entity_content(&tx, &target_entity_id, now_ms)?;
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
                    fact.source_ref,
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
                    fact.source_ref.as_deref().unwrap_or(""),
                    fact.created_at,
                    fact.updated_at,
                    fact.deleted_at,
                ),
                now_ms,
            )?;
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
                        ev.event_id
                            .clone()
                            .unwrap_or_else(|| generate_id("evt_")),
                        dup,
                    )
                }
            };
            if is_dup {
                result.events_skipped += 1;
                continue;
            }
            let created_at = ms_from_utc_date(&ev.date).unwrap_or(now_ms);
            let related = ev
                .related_entry_id
                .as_deref()
                .map(|id| mapped(id, &id_map));
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

fn clear_entity_content(tx: &Connection, entity_id: &str, now_ms: i64) -> Result<()> {
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
    tx.execute("DELETE FROM llm_wiki_entries WHERE entity_id=?1", [entity_id])?;
    tx.execute("DELETE FROM llm_wiki_tasks WHERE entity_id=?1", [entity_id])?;
    tx.execute("DELETE FROM llm_wiki_edges WHERE entity_id=?1", [entity_id])?;
    tx.execute("DELETE FROM llm_wiki_events WHERE entity_id=?1", [entity_id])?;
    Ok(())
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
                ..ParsedEntity::default()
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
    fn preview_counts_without_writing() {
        let mut conn = open_in_memory().unwrap();
        apply_import(&mut conn, &sample_bundle(), ImportMode::Merge).unwrap();
        let preview = preview_import(&conn, &sample_bundle(), ImportMode::Merge).unwrap();
        assert_eq!(preview.entities.len(), 1);
        assert_eq!(preview.entities[0].facts_new, 0);
        assert_eq!(preview.entities[0].facts_existing, 1);
        assert_eq!(preview.entities[0].events_duplicate, 1);
    }
}
