//! CRUD for `curated_entities` — OKF entity surface for Brain mode (Phase 4).

use anyhow::{bail, Context, Result};
use rand::RngCore;
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
pub struct EntityFact {
    pub id: String,
    pub title: String,
    pub body: String,
    pub tags: Vec<String>,
    pub confidence: String,
    pub source_type: String,
    pub updated_at: i64,
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
    pub facts: Vec<EntityFact>,
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
    rand::thread_rng().fill_bytes(&mut bytes);
    format!("ent_{}", hex::encode(bytes))
}

fn summary_snippet(summary: &str) -> String {
    summary.chars().take(200).collect()
}

fn parse_tags(raw: &str) -> Vec<String> {
    serde_json::from_str(raw).unwrap_or_default()
}

fn order_clause(sort: EntitySort) -> &'static str {
    match sort {
        EntitySort::UpdatedDesc => "updated_at DESC, name ASC",
        EntitySort::NameAsc => "name COLLATE NOCASE ASC",
        EntitySort::NameDesc => "name COLLATE NOCASE DESC",
        EntitySort::CreatedDesc => "created_at DESC, name ASC",
    }
}

fn fact_count(conn: &Connection, entity_id: &str) -> Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM llm_wiki_entries
         WHERE entity_id = ?1 AND deleted_at IS NULL",
        [entity_id],
        |r| r.get(0),
    )
    .map_err(Into::into)
}

fn open_task_count(conn: &Connection, entity_id: &str) -> Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM llm_wiki_tasks
         WHERE entity_id = ?1 AND status = 'pending' AND deleted_at IS NULL",
        [entity_id],
        |r| r.get(0),
    )
    .map_err(Into::into)
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
         FROM curated_entities
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
        out.push(EntitySummary {
            id: id.clone(),
            name,
            entity_type,
            summary_snippet: summary_snippet(&summary),
            fact_count: fact_count(conn, &id)?,
            open_task_count: open_task_count(conn, &id)?,
            created_at,
            updated_at,
        });
    }
    Ok(out)
}

fn load_facts(conn: &Connection, entity_id: &str) -> Result<Vec<EntityFact>> {
    let mut stmt = conn.prepare(
        "SELECT id, title, body, tags, confidence, source_type, updated_at
         FROM llm_wiki_entries
         WHERE entity_id = ?1 AND deleted_at IS NULL
         ORDER BY updated_at DESC",
    )?;
    let rows = stmt.query_map([entity_id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, i64>(6)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, title, body, tags_raw, confidence, source_type, updated_at) = row?;
        out.push(EntityFact {
            id,
            title,
            body,
            tags: parse_tags(&tags_raw),
            confidence,
            source_type,
            updated_at,
        });
    }
    Ok(out)
}

fn load_tasks(conn: &Connection, entity_id: &str) -> Result<Vec<EntityTask>> {
    let mut stmt = conn.prepare(
        "SELECT id, description, status, priority, created_at
         FROM llm_wiki_tasks
         WHERE entity_id = ?1 AND deleted_at IS NULL AND status = 'pending'
         ORDER BY priority DESC, created_at ASC",
    )?;
    let rows = stmt.query_map([entity_id], |r| {
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

fn load_events(conn: &Connection, entity_id: &str) -> Result<Vec<EntityEvent>> {
    let mut stmt = conn.prepare(
        "SELECT id, event_type, summary, related_entry_id, created_at
         FROM llm_wiki_events
         WHERE entity_id = ?1
         ORDER BY created_at DESC
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![entity_id, RECENT_EVENTS_LIMIT], |r| {
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
pub fn get_entity(conn: &Connection, entity_id: &str) -> Result<Option<EntityDetail>> {
    let row = conn
        .query_row(
            "SELECT name, entity_type, summary, created_at, updated_at, deleted_at
             FROM curated_entities WHERE id = ?1",
            [entity_id],
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

    Ok(Some(EntityDetail {
        id: entity_id.to_string(),
        name,
        entity_type,
        summary,
        created_at,
        updated_at,
        deleted_at,
        facts: load_facts(conn, entity_id)?,
        tasks: load_tasks(conn, entity_id)?,
        events: load_events(conn, entity_id)?,
    }))
}

/// Create a new curated entity (`summary_embedding` backfill deferred).
pub fn create_entity(conn: &Connection, input: &CreateEntityInput) -> Result<EntityDetail> {
    let name = input.name.trim();
    if name.is_empty() {
        bail!("entity name must not be empty");
    }
    let entity_type = input
        .entity_type
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("concept");
    let summary = input.summary.as_deref().unwrap_or("");
    let id = generate_entity_id();
    let now = now_secs();

    conn.execute(
        "INSERT INTO curated_entities (
            id, name, entity_type, summary, summary_embedding, created_at, updated_at, deleted_at
         ) VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?5, NULL)",
        params![id, name, entity_type, summary, now],
    )?;

    get_entity(conn, &id)?
        .context("entity missing immediately after insert")
}

/// Replace entity summary; clears `summary_embedding` for lazy re-embed.
pub fn update_entity_summary(conn: &Connection, entity_id: &str, summary: &str) -> Result<()> {
    let now = now_secs();
    let changes = conn.execute(
        "UPDATE curated_entities
         SET summary = ?1, summary_embedding = NULL, updated_at = ?2
         WHERE id = ?3 AND deleted_at IS NULL",
        params![summary, now, entity_id],
    )?;
    if changes == 0 {
        bail!("entity not found or archived: {entity_id}");
    }
    Ok(())
}

/// Soft-delete entity (`deleted_at` set; facts/tasks remain for audit).
pub fn archive_entity(conn: &Connection, entity_id: &str) -> Result<()> {
    let now = now_secs();
    let changes = conn.execute(
        "UPDATE curated_entities SET deleted_at = ?1, updated_at = ?1 WHERE id = ?2 AND deleted_at IS NULL",
        params![now, entity_id],
    )?;
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

    #[test]
    fn create_and_get_entity_round_trip() {
        let conn = open_in_memory().unwrap();
        let detail = create_entity(
            &conn,
            &CreateEntityInput {
                name: "Project Alpha".into(),
                entity_type: Some("project".into()),
                summary: Some("Summary prose.".into()),
            },
        )
        .unwrap();
        assert!(detail.id.starts_with("ent_"));
        assert_eq!(detail.name, "Project Alpha");
        assert_eq!(detail.entity_type, "project");
        assert_eq!(detail.summary, "Summary prose.");

        let loaded = get_entity(&conn, &detail.id).unwrap().unwrap();
        assert_eq!(loaded.name, "Project Alpha");
        assert!(loaded.facts.is_empty());
    }

    #[test]
    fn list_entities_excludes_archived_by_default() {
        let conn = open_in_memory().unwrap();
        let active = create_entity(
            &conn,
            &CreateEntityInput {
                name: "Active".into(),
                entity_type: None,
                summary: None,
            },
        )
        .unwrap();
        let archived = create_entity(
            &conn,
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
        let conn = open_in_memory().unwrap();
        let detail = create_entity(
            &conn,
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
        assert_eq!(list_entities(&conn, EntitySort::default(), &EntityListFilter::default()).unwrap()[0].fact_count, 1);
    }

    #[test]
    fn update_entity_summary_clears_embedding() {
        let conn = open_in_memory().unwrap();
        let detail = create_entity(
            &conn,
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
}
