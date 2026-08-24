//! CDC outbox row format matching core-llm-wiki@4.19.0 `OutboxRepository.push`.
//!
//! Rust-authored `llm_wiki_*` writes must stage rows in `llm_wiki_outbox` with the same
//! column values and payload JSON shapes as the TypeScript package, or Postgres replication
//! and cloud sync silently miss them.
//!
//! Note: `OutboxConfig` in `crate::outbox` defaults `outbox_table` to `"outbox"`, but the
//! package writes `llm_wiki_outbox`. Worker config alignment is tracked for Task 7.

use anyhow::{Context, Result};
use rand::RngCore;
use rusqlite::{params, Connection};

pub const LLM_WIKI_OUTBOX_TABLE: &str = "llm_wiki_outbox";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboxOperation {
    Insert,
    Update,
    Delete,
}

impl OutboxOperation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Insert => "INSERT",
            Self::Update => "UPDATE",
            Self::Delete => "DELETE",
        }
    }
}

#[derive(Debug, Clone)]
pub struct OutboxPushParams {
    pub entity_id: String,
    /// Short table name without prefix: `entries` | `tasks` | `events`.
    pub table_name: String,
    pub record_id: String,
    pub operation: OutboxOperation,
    pub payload: serde_json::Value,
}

/// Generate an outbox row id matching `generateId("out_")` in core-llm-wiki (24 hex chars).
pub fn generate_outbox_id() -> String {
    let mut bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!("out_{}", hex::encode(bytes))
}

fn outbox_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_millis() as i64
}

/// Insert one CDC row into `llm_wiki_outbox`. Returns the generated outbox id.
///
/// `created_at` defaults to current time in milliseconds (matching package `Date.now()`).
pub fn push_outbox_row(
    conn: &Connection,
    params: &OutboxPushParams,
    created_at: Option<i64>,
) -> Result<String> {
    let id = generate_outbox_id();
    let created_at = created_at.unwrap_or_else(outbox_now_ms);
    let payload_str =
        serde_json::to_string(&params.payload).context("serialize outbox payload JSON")?;

    conn.execute(
        &format!(
            "INSERT INTO {LLM_WIKI_OUTBOX_TABLE} \
             (id, entity_id, table_name, record_id, operation, payload, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"
        ),
        params![
            id,
            params.entity_id,
            params.table_name,
            params.record_id,
            params.operation.as_str(),
            payload_str,
            created_at,
        ],
    )
    .with_context(|| format!("insert into {LLM_WIKI_OUTBOX_TABLE}"))?;

    Ok(id)
}

/// Golden-fixture payloads captured from core-llm-wiki@4.19.0 repository outbox pushes.
pub mod fixtures {
    use super::OutboxOperation;
    use super::OutboxPushParams;

    pub const ENTITY_ID: &str = "ent_golden_fixture";
    pub const CREATED_AT_MS: i64 = 1_719_360_000_000;
    pub const UPDATED_AT_MS: i64 = 1_719_360_600_000;
    pub const DELETED_AT_MS: i64 = 1_719_361_200_000;

    pub const FACT_ID: &str = "fact_golden_fixture001";
    pub const TASK_ID: &str = "task_golden_fixture001";
    pub const EVENT_ID: &str = "evt_golden_fixture001";

    /// EntryRepository.upsert INSERT — full WikiFact object (snake_case JSON).
    pub fn entry_insert_payload() -> serde_json::Value {
        serde_json::json!({
            "id": FACT_ID,
            "entity_id": ENTITY_ID,
            "title": "Golden fixture fact",
            "body": "Body from EntryRepository.upsert INSERT.",
            "tags": ["golden", "fixture"],
            "confidence": "user_confirmed",
            "source_type": "librarian_inferred",
            "source_hash": null,
            "source_ref": "prop-golden-001",
            "created_at": CREATED_AT_MS,
            "updated_at": UPDATED_AT_MS,
            "last_accessed_at": null,
            "access_count": 0,
            "deleted_at": null
        })
    }

    /// EntryRepository.upsert UPDATE — full WikiFact for an existing row.
    pub fn entry_update_payload() -> serde_json::Value {
        serde_json::json!({
            "id": FACT_ID,
            "entity_id": ENTITY_ID,
            "title": "Golden fixture fact (updated)",
            "body": "Updated body from EntryRepository.upsert UPDATE.",
            "tags": ["golden", "updated"],
            "confidence": "inferred",
            "source_type": "librarian_inferred",
            "source_hash": null,
            "source_ref": "prop-golden-001",
            "created_at": CREATED_AT_MS,
            "updated_at": UPDATED_AT_MS,
            "last_accessed_at": UPDATED_AT_MS,
            "access_count": 2,
            "deleted_at": null
        })
    }

    /// EntryRepository.upsert DELETE — full WikiFact with `deleted_at` set.
    pub fn entry_delete_payload() -> serde_json::Value {
        serde_json::json!({
            "id": FACT_ID,
            "entity_id": ENTITY_ID,
            "title": "Golden fixture fact (deleted)",
            "body": "Soft-deleted via upsert path.",
            "tags": ["golden"],
            "confidence": "user_confirmed",
            "source_type": "librarian_inferred",
            "source_hash": null,
            "source_ref": "prop-golden-001",
            "created_at": CREATED_AT_MS,
            "updated_at": DELETED_AT_MS,
            "last_accessed_at": null,
            "access_count": 0,
            "deleted_at": DELETED_AT_MS
        })
    }

    /// TaskRepository.upsert INSERT — full WikiTask object.
    pub fn task_insert_payload() -> serde_json::Value {
        serde_json::json!({
            "id": TASK_ID,
            "entity_id": ENTITY_ID,
            "description": "Golden fixture task",
            "status": "pending",
            "priority": 2,
            "created_at": CREATED_AT_MS,
            "updated_at": UPDATED_AT_MS,
            "resolved_at": null,
            "deleted_at": null
        })
    }

    /// TaskRepository.upsert UPDATE — full WikiTask object.
    pub fn task_update_payload() -> serde_json::Value {
        serde_json::json!({
            "id": TASK_ID,
            "entity_id": ENTITY_ID,
            "description": "Golden fixture task (in progress)",
            "status": "in_progress",
            "priority": 3,
            "created_at": CREATED_AT_MS,
            "updated_at": UPDATED_AT_MS,
            "resolved_at": null,
            "deleted_at": null
        })
    }

    /// TaskRepository.softDelete DELETE — minimal payload `{ id, entity_id, deleted_at }`.
    pub fn task_delete_payload() -> serde_json::Value {
        serde_json::json!({
            "id": TASK_ID,
            "entity_id": ENTITY_ID,
            "deleted_at": DELETED_AT_MS
        })
    }

    /// WikiEvent shape for Rust-authored resolution events (package EventRepository.add has no outbox push).
    pub fn event_insert_payload() -> serde_json::Value {
        serde_json::json!({
            "id": EVENT_ID,
            "entity_id": ENTITY_ID,
            "event_type": "action",
            "summary": "Approved: 2 facts added to Project X from notes.pdf",
            "related_entry_id": FACT_ID,
            "created_at": CREATED_AT_MS
        })
    }

    pub fn entry_insert_params() -> OutboxPushParams {
        OutboxPushParams {
            entity_id: ENTITY_ID.into(),
            table_name: "entries".into(),
            record_id: FACT_ID.into(),
            operation: OutboxOperation::Insert,
            payload: entry_insert_payload(),
        }
    }

    pub fn entry_update_params() -> OutboxPushParams {
        OutboxPushParams {
            entity_id: ENTITY_ID.into(),
            table_name: "entries".into(),
            record_id: FACT_ID.into(),
            operation: OutboxOperation::Update,
            payload: entry_update_payload(),
        }
    }

    pub fn entry_delete_params() -> OutboxPushParams {
        OutboxPushParams {
            entity_id: ENTITY_ID.into(),
            table_name: "entries".into(),
            record_id: FACT_ID.into(),
            operation: OutboxOperation::Delete,
            payload: entry_delete_payload(),
        }
    }

    pub fn task_insert_params() -> OutboxPushParams {
        OutboxPushParams {
            entity_id: ENTITY_ID.into(),
            table_name: "tasks".into(),
            record_id: TASK_ID.into(),
            operation: OutboxOperation::Insert,
            payload: task_insert_payload(),
        }
    }

    pub fn task_update_params() -> OutboxPushParams {
        OutboxPushParams {
            entity_id: ENTITY_ID.into(),
            table_name: "tasks".into(),
            record_id: TASK_ID.into(),
            operation: OutboxOperation::Update,
            payload: task_update_payload(),
        }
    }

    pub fn task_delete_params() -> OutboxPushParams {
        OutboxPushParams {
            entity_id: ENTITY_ID.into(),
            table_name: "tasks".into(),
            record_id: TASK_ID.into(),
            operation: OutboxOperation::Delete,
            payload: task_delete_payload(),
        }
    }

    pub fn event_insert_params() -> OutboxPushParams {
        OutboxPushParams {
            entity_id: ENTITY_ID.into(),
            table_name: "events".into(),
            record_id: EVENT_ID.into(),
            operation: OutboxOperation::Insert,
            payload: event_insert_payload(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{
        self, entry_delete_params, entry_insert_params, entry_update_params, event_insert_params,
        task_delete_params, task_insert_params, task_update_params, CREATED_AT_MS, ENTITY_ID,
    };
    use super::*;
    use crate::db::connection::open_in_memory;

    struct StoredOutboxRow {
        id: String,
        entity_id: String,
        table_name: String,
        record_id: String,
        operation: String,
        payload: serde_json::Value,
        created_at: i64,
    }

    fn read_outbox_row(conn: &Connection, id: &str) -> StoredOutboxRow {
        conn.query_row(
            &format!(
                "SELECT id, entity_id, table_name, record_id, operation, payload, created_at \
                 FROM {LLM_WIKI_OUTBOX_TABLE} WHERE id = ?1"
            ),
            [id],
            |row| {
                let payload_str: String = row.get(5)?;
                let payload: serde_json::Value =
                    serde_json::from_str(&payload_str).expect("outbox payload must be valid JSON");
                Ok(StoredOutboxRow {
                    id: row.get(0)?,
                    entity_id: row.get(1)?,
                    table_name: row.get(2)?,
                    record_id: row.get(3)?,
                    operation: row.get(4)?,
                    payload,
                    created_at: row.get(6)?,
                })
            },
        )
        .expect("outbox row must exist")
    }

    fn assert_row_matches_params(
        stored: &StoredOutboxRow,
        params: &OutboxPushParams,
        created_at: i64,
    ) {
        assert_eq!(stored.entity_id, params.entity_id);
        assert_eq!(stored.table_name, params.table_name);
        assert!(!stored.table_name.starts_with("llm_wiki_"));
        assert_eq!(stored.record_id, params.record_id);
        assert_eq!(stored.operation, params.operation.as_str());
        assert_eq!(stored.payload, params.payload);
        assert_eq!(stored.created_at, created_at);
        assert!(stored.id.starts_with("out_"));
        assert_eq!(stored.id.len(), 28, "out_ + 24 hex chars");
        assert!(stored.id[4..].chars().all(|c| c.is_ascii_hexdigit()));
    }

    fn push_and_assert(conn: &Connection, params: OutboxPushParams) {
        let id = push_outbox_row(conn, &params, Some(CREATED_AT_MS)).expect("push outbox row");
        let stored = read_outbox_row(conn, &id);
        assert_row_matches_params(&stored, &params, CREATED_AT_MS);
    }

    #[test]
    fn generate_outbox_id_matches_package_prefix_and_length() {
        let id = generate_outbox_id();
        assert!(id.starts_with("out_"));
        assert_eq!(id.len(), 28);
        assert!(id[4..].chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn push_entry_insert_matches_golden_fixture() {
        let conn = open_in_memory().unwrap();
        push_and_assert(&conn, entry_insert_params());
    }

    #[test]
    fn push_entry_update_matches_golden_fixture() {
        let conn = open_in_memory().unwrap();
        push_and_assert(&conn, entry_update_params());
    }

    #[test]
    fn push_entry_delete_matches_golden_fixture() {
        let conn = open_in_memory().unwrap();
        push_and_assert(&conn, entry_delete_params());
    }

    #[test]
    fn push_task_insert_matches_golden_fixture() {
        let conn = open_in_memory().unwrap();
        push_and_assert(&conn, task_insert_params());
    }

    #[test]
    fn push_task_update_matches_golden_fixture() {
        let conn = open_in_memory().unwrap();
        push_and_assert(&conn, task_update_params());
    }

    #[test]
    fn push_task_delete_matches_golden_fixture() {
        let conn = open_in_memory().unwrap();
        push_and_assert(&conn, task_delete_params());
    }

    #[test]
    fn push_event_insert_matches_golden_fixture() {
        let conn = open_in_memory().unwrap();
        push_and_assert(&conn, event_insert_params());
    }

    #[test]
    fn table_name_values_have_no_llm_wiki_prefix() {
        for params in [
            entry_insert_params(),
            entry_update_params(),
            entry_delete_params(),
            task_insert_params(),
            task_update_params(),
            task_delete_params(),
            event_insert_params(),
        ] {
            assert!(!params.table_name.starts_with("llm_wiki_"));
            assert!(matches!(
                params.table_name.as_str(),
                "entries" | "tasks" | "events"
            ));
        }
        assert_eq!(fixtures::ENTITY_ID, ENTITY_ID);
    }
}
