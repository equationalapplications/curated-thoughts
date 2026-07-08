//! Task CRUD operations.

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use anyhow::{bail, Result};

use crate::db::commit::{now_timestamps, push_tasks_outbox, wiki_task_outbox_payload};
use crate::db::outbox_format::OutboxOperation;

#[derive(Debug, Clone, serde::Serialize)]
pub struct TaskRow {
    pub id: String,
    pub entity_id: String,
    pub description: String,
    pub status: String,
    pub priority: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub resolved_at: Option<i64>,
    pub deleted_at: Option<i64>,
}

pub fn list_tasks(
    conn: &Connection,
    status: Option<&str>,
    include_archived: bool,
) -> Result<Vec<TaskRow>> {
    let mut sql = String::from(
        "SELECT id, entity_id, description, status, priority, created_at, updated_at, resolved_at, deleted_at
         FROM llm_wiki_tasks WHERE 1=1"
    );
    if let Some(s) = status {
        sql.push_str(" AND status = ?1");
    }
    if !include_archived {
        sql.push_str(" AND deleted_at IS NULL");
    }
    sql.push_str(" ORDER BY created_at DESC");

    let mut stmt = conn.prepare(&sql)?;
    let rows = if let Some(s) = status {
        stmt.query_map(params![s], |r| {
            Ok(TaskRow {
                id: r.get(0)?,
                entity_id: r.get(1)?,
                description: r.get(2)?,
                status: r.get(3)?,
                priority: r.get(4)?,
                created_at: r.get(5)?,
                updated_at: r.get(6)?,
                resolved_at: r.get(7)?,
                deleted_at: r.get(8)?,
            })
        })?
    } else {
        stmt.query_map([], |r| {
            Ok(TaskRow {
                id: r.get(0)?,
                entity_id: r.get(1)?,
                description: r.get(2)?,
                status: r.get(3)?,
                priority: r.get(4)?,
                created_at: r.get(5)?,
                updated_at: r.get(6)?,
                resolved_at: r.get(7)?,
                deleted_at: r.get(8)?,
            })
        })?
    };

    let mut tasks = Vec::new();
    for row in rows {
        tasks.push(row?);
    }
    Ok(tasks)
}

pub fn create_task(conn: &mut Connection, entity_id: &str, description: &str) -> Result<TaskRow> {
    let (now_secs, now_ms) = now_timestamps();
    let id = crate::db::commit::generate_llm_id("task_");

    conn.execute(
        "INSERT INTO llm_wiki_tasks (id, entity_id, description, status, priority, created_at, updated_at)
         VALUES (?1, ?2, ?3, 'pending', 0, ?4, ?4)",
        params![id, entity_id, description, now_ms],
    )?;

    push_tasks_outbox(
        conn,
        entity_id,
        &id,
        OutboxOperation::Insert,
        wiki_task_outbox_payload(&id, entity_id, description, "pending", 0, now_ms, now_ms, None, None),
        now_ms,
    )?;

    Ok(TaskRow {
        id,
        entity_id: entity_id.to_string(),
        description: description.to_string(),
        status: "pending".into(),
        priority: 0,
        created_at: now_ms,
        updated_at: now_ms,
        resolved_at: None,
        deleted_at: None,
    })
}

pub fn resolve_task(conn: &mut Connection, task_id: &str) -> Result<()> {
    let (_now_secs, now_ms) = now_timestamps();

    let task = conn
        .query_row(
            "SELECT id, entity_id, description, status, priority, created_at, updated_at, resolved_at
             FROM llm_wiki_tasks WHERE id = ?1 AND deleted_at IS NULL",
            [task_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, i64>(6)?,
                    r.get::<_, Option<i64>>(7)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| anyhow::anyhow!("task not found: {task_id}"))?;

    let (id, entity_id, description, _status, priority, created_at, _updated_at, _resolved_at) = task;

    conn.execute(
        "UPDATE llm_wiki_tasks SET status = 'done', resolved_at = ?1, updated_at = ?1 WHERE id = ?2",
        params![now_ms, task_id],
    )?;

    push_tasks_outbox(
        conn,
        &entity_id,
        task_id,
        OutboxOperation::Update,
        wiki_task_outbox_payload(&id, &entity_id, &description, "done", priority, created_at, now_ms, Some(now_ms), None),
        now_ms,
    )?;

    Ok(())
}

pub fn archive_task(conn: &mut Connection, task_id: &str) -> Result<()> {
    let (_now_secs, now_ms) = now_timestamps();

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

    // Load current task to verify it exists and get full row for outbox.
    let task = tx
        .query_row(
            "SELECT id, entity_id, description, status, priority, created_at, updated_at, resolved_at
             FROM llm_wiki_tasks
             WHERE id = ?1 AND deleted_at IS NULL",
            [task_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, i64>(6)?,
                    r.get::<_, Option<i64>>(7)?,
                ))
            },
        )
        .optional()?;
    let Some((id, entity_id, description, status, priority, created_at, _updated_at, resolved_at)) = task else {
        bail!("task not found or already archived: {task_id}");
    };

    tx.execute(
        "UPDATE llm_wiki_tasks SET deleted_at = ?1, updated_at = ?1 WHERE id = ?2",
        params![now_ms, task_id],
    )?;

    push_tasks_outbox(
        &tx,
        &entity_id,
        task_id,
        OutboxOperation::Update,
        wiki_task_outbox_payload(
            &id,
            &entity_id,
            &description,
            &status,
            priority,
            created_at,
            now_ms,
            resolved_at,
            Some(now_ms),
        ),
        now_ms,
    )?;

    tx.commit()?;
    Ok(())
}
