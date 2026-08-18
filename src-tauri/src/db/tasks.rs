//! Task CRUD operations — mirrors facts.rs structure exactly.

use crate::db::commit::{
    generate_llm_id, now_timestamps, push_tasks_outbox, wiki_task_outbox_payload,
};
use crate::db::outbox_format::OutboxOperation;
use anyhow::{bail, Result};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;

#[derive(Debug, Serialize, Clone)]
pub struct TaskRow {
    pub id: String,
    pub entity_id: String,
    pub entity_name: String,
    pub description: String,
    pub status: String,
    pub priority: i64,
    pub created_at: i64,
    pub resolved_at: Option<i64>,
}

/// List tasks filtered by status and archive state.
pub fn list_tasks(
    conn: &Connection,
    status: Option<&str>,
    include_archived: bool,
) -> Result<Vec<TaskRow>> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.entity_id, ce.name, t.description, t.status, t.priority, t.created_at, t.resolved_at
         FROM llm_wiki_tasks t
         JOIN curated_entities ce ON ce.id = t.entity_id
         WHERE (t.status = :status OR :status IS NULL)
           AND (t.deleted_at IS NULL OR :include_archived)
         ORDER BY ce.name COLLATE NOCASE ASC, t.priority DESC, t.created_at ASC",
    )?;

    let rows = stmt.query_map(
        rusqlite::named_params! {
            ":status": status,
            ":include_archived": if include_archived { 1 } else { 0 },
        },
        |r| {
            Ok(TaskRow {
                id: r.get(0)?,
                entity_id: r.get(1)?,
                entity_name: r.get(2)?,
                description: r.get(3)?,
                status: r.get(4)?,
                priority: r.get(5)?,
                created_at: r.get(6)?,
                resolved_at: r.get(7)?,
            })
        },
    )?;

    let mut tasks = Vec::new();
    for row in rows {
        tasks.push(row?);
    }
    Ok(tasks)
}

/// Create a new task with status 'pending', priority 0.
pub fn create_task(
    conn: &mut Connection,
    entity_id: &str,
    description: &str,
) -> Result<TaskRow> {
    let description = description.trim();
    if description.is_empty() {
        bail!("task description must not be empty");
    }
    let (_now_secs, now_ms) = now_timestamps();
    let task_id = generate_llm_id("task_");
    let priority = 0i64;

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

    // Verify entity exists and is not archived.
    let entity_name: Option<String> = tx
        .query_row(
            "SELECT name FROM curated_entities WHERE id = ?1 AND deleted_at IS NULL",
            [entity_id],
            |r| r.get(0),
        )
        .optional()?;
    if entity_name.is_none() {
        bail!("entity not found or archived: {entity_id}");
    }
    let entity_name = entity_name.unwrap();

    tx.execute(
        "INSERT INTO llm_wiki_tasks (
            id, entity_id, description, status, priority,
            created_at, updated_at, resolved_at, deleted_at
         ) VALUES (?1, ?2, ?3, 'pending', ?4, ?5, ?5, NULL, NULL)",
        params![task_id, entity_id, description, priority, now_ms],
    )?;

    push_tasks_outbox(
        &tx,
        entity_id,
        &task_id,
        OutboxOperation::Insert,
        wiki_task_outbox_payload(
            &task_id,
            entity_id,
            description,
            "pending",
            priority,
            now_ms,
            now_ms,
            None,
            None,
        ),
        now_ms,
    )?;

    tx.commit()?;

    Ok(TaskRow {
        id: task_id,
        entity_id: entity_id.to_string(),
        entity_name,
        description: description.to_string(),
        status: "pending".into(),
        priority,
        created_at: now_ms,
        resolved_at: None,
    })
}

/// Update task status to 'pending' or 'done'; 'done' sets resolved_at.
pub fn set_task_status(
    conn: &mut Connection,
    task_id: &str,
    status: &str,
) -> Result<()> {
    if !matches!(status, "pending" | "done") {
        bail!("status must be 'pending' or 'done'");
    }
    let (_now_secs, now_ms) = now_timestamps();

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

    // Load current task to verify it exists and get full row for outbox.
    let task = tx
        .query_row(
            "SELECT id, entity_id, description, priority, created_at
             FROM llm_wiki_tasks
             WHERE id = ?1 AND deleted_at IS NULL",
            [task_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                ))
            },
        )
        .optional()?;
    let Some((id, entity_id, description, priority, created_at)) = task else {
        bail!("task not found or archived: {task_id}");
    };

    let resolved_at = if status == "done" { Some(now_ms) } else { None };

    tx.execute(
        "UPDATE llm_wiki_tasks SET status = ?1, updated_at = ?2, resolved_at = ?3 WHERE id = ?4",
        params![status, now_ms, resolved_at, task_id],
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
            status,
            priority,
            created_at,
            now_ms,
            resolved_at,
            None,
        ),
        now_ms,
    )?;

    tx.commit()?;
    Ok(())
}

/// Soft-delete a task by setting deleted_at.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::open_in_memory;
    use crate::db::entities::{create_entity, CreateEntityInput};
    use rusqlite::params;

    fn make_entity(conn: &Connection) -> String {
        create_entity(
            conn,
            &CreateEntityInput {
                name: "TestEntity".into(),
                entity_type: None,
                summary: None,
            },
        )
        .unwrap()
        .id
    }

    fn outbox_count(conn: &Connection, record_id: &str, operation: &str) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM llm_wiki_outbox
             WHERE record_id = ?1 AND table_name = 'tasks' AND operation = ?2",
            params![record_id, operation],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn list_tasks_groups_carry_entity_names_and_filter_by_status() {
        let conn = open_in_memory().unwrap();
        let ent1 = make_entity(&conn);
        let ent2_id = "ent_second";
        conn.execute(
            "INSERT INTO curated_entities (id, name, entity_type, created_at, updated_at)
             VALUES (?1, 'SecondEntity', 'concept', 1, 1)",
            [ent2_id],
        )
        .unwrap();

        // Task 1: pending
        conn.execute(
            "INSERT INTO llm_wiki_tasks
             (id, entity_id, description, status, priority, created_at, updated_at)
             VALUES ('task_1', ?1, 'Do something', 'pending', 0, 100, 100)",
            [&ent1],
        )
        .unwrap();

        // Task 2: done
        conn.execute(
            "INSERT INTO llm_wiki_tasks
             (id, entity_id, description, status, priority, created_at, updated_at, resolved_at)
             VALUES ('task_2', ?1, 'Finished it', 'done', 1, 200, 200, 250)",
            [&ent1],
        )
        .unwrap();

        // Task 3: pending on second entity
        conn.execute(
            "INSERT INTO llm_wiki_tasks
             (id, entity_id, description, status, priority, created_at, updated_at)
             VALUES ('task_3', ?1, 'Another task', 'pending', 0, 300, 300)",
            [ent2_id],
        )
        .unwrap();

        let pending = list_tasks(&conn, Some("pending"), false).unwrap();
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].entity_name, "SecondEntity"); // Sorted by name
        assert_eq!(pending[0].id, "task_3");
        assert_eq!(pending[1].entity_name, "TestEntity");
        assert_eq!(pending[1].id, "task_1");

        let done = list_tasks(&conn, Some("done"), false).unwrap();
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].id, "task_2");
        assert_eq!(done[0].status, "done");
        assert_eq!(done[0].resolved_at, Some(250));

        let all = list_tasks(&conn, None, false).unwrap();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn create_task_inserts_row_and_outbox() {
        let mut conn = open_in_memory().unwrap();
        let entity_id = make_entity(&conn);

        let task = create_task(&mut conn, &entity_id, "  Create a test  ").unwrap();
        assert!(task.id.starts_with("task_"));
        assert_eq!(task.description, "Create a test");
        assert_eq!(task.status, "pending");
        assert_eq!(task.priority, 0);
        assert_eq!(task.entity_name, "TestEntity");

        let row_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_tasks WHERE id = ?1",
                [&task.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(row_count, 1);

        let outbox_count_val = outbox_count(&conn, &task.id, "INSERT");
        assert_eq!(outbox_count_val, 1);
    }

    #[test]
    fn set_task_status_done_sets_resolved_at_and_outbox_update() {
        let mut conn = open_in_memory().unwrap();
        let entity_id = make_entity(&conn);
        let task = create_task(&mut conn, &entity_id, "Task to resolve").unwrap();

        set_task_status(&mut conn, &task.id, "done").unwrap();

        let (status, resolved_at): (String, Option<i64>) = conn
            .query_row(
                "SELECT status, resolved_at FROM llm_wiki_tasks WHERE id = ?1",
                [&task.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "done");
        assert!(resolved_at.is_some());

        let outbox_count_val = outbox_count(&conn, &task.id, "UPDATE");
        assert_eq!(outbox_count_val, 1);
    }

    #[test]
    fn archive_task_sets_deleted_at_and_outbox_update() {
        let mut conn = open_in_memory().unwrap();
        let entity_id = make_entity(&conn);
        let task = create_task(&mut conn, &entity_id, "Task to archive").unwrap();

        archive_task(&mut conn, &task.id).unwrap();

        let deleted_at: Option<i64> = conn
            .query_row(
                "SELECT deleted_at FROM llm_wiki_tasks WHERE id = ?1",
                [&task.id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(deleted_at.is_some());

        let archived_tasks = list_tasks(&conn, None, false).unwrap();
        assert!(archived_tasks.iter().all(|t| t.id != task.id));

        let outbox_count_val = outbox_count(&conn, &task.id, "UPDATE");
        assert_eq!(outbox_count_val, 1);
    }
}
