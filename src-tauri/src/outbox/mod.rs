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

pub mod postgres;
