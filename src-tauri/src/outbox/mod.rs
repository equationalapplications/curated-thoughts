use std::path::PathBuf;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OutboxEvent {
    pub id: String,
    pub entity_id: String,
    pub table_name: String,
    pub record_id: String,
    pub operation: String,
    pub payload: serde_json::Value,
    pub created_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorPolicy {
    Halt,
    Skip,
}

#[derive(Debug, Clone)]
pub struct OutboxConfig {
    pub sqlite_path: PathBuf,
    pub db_url: String,
    /// SQLite table name written by core-llm-wiki. Default: "outbox".
    pub outbox_table: String,
    pub poll_interval_ms: u64,
    pub batch_size: usize,
    pub on_error: ErrorPolicy,
}

impl Default for OutboxConfig {
    /// Sentinel defaults only — callers must set `sqlite_path` and `db_url` before use.
    fn default() -> Self {
        Self {
            sqlite_path: PathBuf::new(),
            db_url: String::new(),
            outbox_table: "outbox".into(),
            poll_interval_ms: 5000,
            batch_size: 100,
            on_error: ErrorPolicy::Halt,
        }
    }
}

/// Abstracts the remote-write destination. Generic parameter avoids async-trait.
pub trait Sink: Send + Sync + 'static {
    fn insert_event(
        &self,
        event: OutboxEvent,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send;
}

use rusqlite::Connection;
use std::sync::{Arc, Mutex};

pub(crate) async fn fetch_pending(
    conn: Arc<Mutex<Connection>>,
    table: &str,
    batch_size: usize,
) -> anyhow::Result<Vec<OutboxEvent>> {
    let table = table.to_string();
    tokio::task::spawn_blocking(move || {
        let guard = conn.lock().map_err(|_| anyhow::anyhow!("SQLite mutex poisoned"))?;
        let sql = format!(
            "SELECT id, entity_id, table_name, record_id, operation, payload, created_at \
             FROM {table} ORDER BY created_at ASC, rowid ASC LIMIT ?1"
        );
        let mut stmt = guard.prepare(&sql)?;
        let events = stmt
            .query_map([batch_size as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        events
            .into_iter()
            .map(|(id, entity_id, table_name, record_id, operation, payload_str, created_at)| {
                let payload = serde_json::from_str(&payload_str)
                    .unwrap_or(serde_json::Value::Null);
                Ok(OutboxEvent { id, entity_id, table_name, record_id, operation, payload, created_at })
            })
            .collect()
    })
    .await?
}

pub(crate) async fn acknowledge(
    conn: Arc<Mutex<Connection>>,
    table: &str,
    ids: Vec<String>,
) -> anyhow::Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let table = table.to_string();
    tokio::task::spawn_blocking(move || {
        let guard = conn.lock().map_err(|_| anyhow::anyhow!("SQLite mutex poisoned"))?;
        let chunk_size = 500;
        for chunk in ids.chunks(chunk_size) {
            let placeholders = chunk.iter().enumerate()
                .map(|(i, _)| format!("?{}", i + 1))
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!("DELETE FROM {table} WHERE id IN ({placeholders})");
            let params: Vec<&dyn rusqlite::ToSql> =
                chunk.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
            guard.execute(&sql, params.as_slice())?;
        }
        Ok(())
    })
    .await?
}

pub mod postgres;

#[cfg(test)]
mod sqlite_tests {
    use super::*;
    use rusqlite::Connection;
    use std::sync::{Arc, Mutex};
    use tempfile::NamedTempFile;

    fn setup_outbox_db() -> (NamedTempFile, Arc<Mutex<Connection>>) {
        let f = NamedTempFile::new().unwrap();
        let conn = Connection::open(f.path()).unwrap();
        conn.execute_batch(
            "PRAGMA busy_timeout = 5000;
             CREATE TABLE outbox (
               id TEXT PRIMARY KEY,
               entity_id TEXT NOT NULL,
               table_name TEXT NOT NULL,
               record_id TEXT NOT NULL,
               operation TEXT NOT NULL,
               payload TEXT NOT NULL,
               created_at INTEGER NOT NULL
             );",
        )
        .unwrap();
        (f, Arc::new(Mutex::new(conn)))
    }

    fn insert_raw(conn: &Arc<Mutex<Connection>>, id: &str, created_at: i64) {
        conn.lock().unwrap().execute(
            "INSERT INTO outbox VALUES (?, 'tier_fact', 'entries', 'rec1', 'INSERT', '{}', ?)",
            rusqlite::params![id, created_at],
        ).unwrap();
    }

    #[tokio::test]
    async fn fetch_pending_returns_events_ordered_by_created_at_asc() {
        let (_f, conn) = setup_outbox_db();
        insert_raw(&conn, "id2", 2000);
        insert_raw(&conn, "id1", 1000);
        let events = fetch_pending(conn, "outbox", 10).await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].id, "id1");
        assert_eq!(events[1].id, "id2");
    }

    #[tokio::test]
    async fn fetch_pending_respects_batch_size() {
        let (_f, conn) = setup_outbox_db();
        for i in 0..5 {
            insert_raw(&conn, &format!("id{i}"), i as i64 * 1000);
        }
        let events = fetch_pending(conn, "outbox", 3).await.unwrap();
        assert_eq!(events.len(), 3);
    }

    #[tokio::test]
    async fn acknowledge_deletes_by_id() {
        let (_f, conn) = setup_outbox_db();
        insert_raw(&conn, "a", 1000);
        insert_raw(&conn, "b", 2000);
        acknowledge(conn.clone(), "outbox", vec!["a".into()]).await.unwrap();
        let remaining = fetch_pending(conn, "outbox", 10).await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, "b");
    }

    #[tokio::test]
    async fn acknowledge_empty_ids_is_noop() {
        let (_f, conn) = setup_outbox_db();
        insert_raw(&conn, "a", 1000);
        acknowledge(conn.clone(), "outbox", vec![]).await.unwrap();
        let remaining = fetch_pending(conn, "outbox", 10).await.unwrap();
        assert_eq!(remaining.len(), 1);
    }
}
