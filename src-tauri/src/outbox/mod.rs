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

#[derive(Debug, Clone, PartialEq, Eq)]
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
        event: &OutboxEvent,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send;
}

use rusqlite::Connection;
use std::sync::{Arc, Mutex};

fn validate_outbox_table_name(table: &str) -> anyhow::Result<&str> {
    if table.is_empty() {
        return Err(anyhow::anyhow!("outbox table name cannot be empty"));
    }
    let mut chars = table.chars();
    let first = chars.next().unwrap();
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return Err(anyhow::anyhow!("invalid outbox table name: {table}"));
    }
    if !chars.all(|c| c == '_' || c.is_ascii_alphanumeric()) {
        return Err(anyhow::anyhow!("invalid outbox table name: {table}"));
    }
    Ok(table)
}

fn is_missing_table_error(err: &rusqlite::Error) -> bool {
    matches!(err, rusqlite::Error::SqliteFailure(db_err, Some(msg))
        if db_err.code == rusqlite::ErrorCode::Unknown && msg.to_lowercase().contains("no such table"))
}

/// `table` must be an app-controlled identifier, not user-supplied input.
pub(crate) async fn fetch_pending(
    conn: Arc<Mutex<Connection>>,
    table: &str,
    batch_size: usize,
) -> anyhow::Result<Vec<OutboxEvent>> {
    let table = validate_outbox_table_name(table)?.to_string();
    tokio::task::spawn_blocking(move || {
        let guard = conn
            .lock()
            .map_err(|_| anyhow::anyhow!("SQLite mutex poisoned"))?;
        let sql = format!(
            "SELECT id, entity_id, table_name, record_id, operation, payload, created_at \
             FROM {table} ORDER BY created_at ASC, rowid ASC LIMIT ?1"
        );
        let mut stmt = match guard.prepare(&sql) {
            Ok(stmt) => stmt,
            Err(e) if is_missing_table_error(&e) => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
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
            .map(
                |(id, entity_id, table_name, record_id, operation, payload_str, created_at)| {
                    let payload: serde_json::Value =
                        serde_json::from_str(&payload_str).map_err(|e| {
                            anyhow::anyhow!("malformed outbox payload for id={id}: {e}")
                        })?;
                    // Fail loudly on null payloads from malformed data
                    if payload.is_null() && !payload_str.trim().eq_ignore_ascii_case("null") {
                        return Err(anyhow::anyhow!(
                            "outbox payload for id={id} parsed as null from: {payload_str}"
                        ));
                    }
                    Ok(OutboxEvent {
                        id,
                        entity_id,
                        table_name,
                        record_id,
                        operation,
                        payload,
                        created_at,
                    })
                },
            )
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
    let table = validate_outbox_table_name(table)?.to_string();
    tokio::task::spawn_blocking(move || {
        let guard = conn
            .lock()
            .map_err(|_| anyhow::anyhow!("SQLite mutex poisoned"))?;
        let chunk_size = 500;
        for chunk in ids.chunks(chunk_size) {
            // ?1..?N bind to params[0..N-1] by index — do not replace with anonymous ?
            let placeholders = chunk
                .iter()
                .enumerate()
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

use std::sync::atomic::{AtomicBool, Ordering};

pub struct OutboxWorker {
    pub conn: Arc<Mutex<Connection>>,
    pub running: Arc<AtomicBool>,
}

impl OutboxWorker {
    /// Opens a dedicated SQLite connection to `sqlite_path`.
    pub fn open(sqlite_path: &std::path::Path) -> anyhow::Result<Self> {
        let conn = Connection::open(sqlite_path)?;
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA busy_timeout = 5000;")?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            running: Arc::new(AtomicBool::new(false)),
        })
    }

    /// For tests: wrap an existing in-memory or temp connection.
    #[cfg(test)]
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self {
            conn,
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Run one poll cycle. Returns `true` if the batch was full (caller may
    /// immediately re-poll for backlog drain).
    pub async fn sync_batch<S: Sink>(
        &self,
        sink: &S,
        config: &OutboxConfig,
    ) -> anyhow::Result<bool> {
        if self.running.swap(true, Ordering::SeqCst) {
            return Ok(false);
        }
        let result = self.do_sync(sink, config, None).await;
        self.running.store(false, Ordering::SeqCst);
        result
    }

    async fn sync_batch_with_cancel<S: Sink>(
        &self,
        sink: &S,
        config: &OutboxConfig,
        cancel: &Arc<AtomicBool>,
    ) -> anyhow::Result<bool> {
        if self.running.swap(true, Ordering::SeqCst) {
            return Ok(false);
        }
        let result = self.do_sync(sink, config, Some(cancel)).await;
        self.running.store(false, Ordering::SeqCst);
        result
    }

    async fn do_sync<S: Sink>(
        &self,
        sink: &S,
        config: &OutboxConfig,
        cancel: Option<&Arc<AtomicBool>>,
    ) -> anyhow::Result<bool> {
        let events =
            fetch_pending(self.conn.clone(), &config.outbox_table, config.batch_size).await?;

        if events.is_empty() {
            return Ok(false);
        }

        let full_batch = events.len() == config.batch_size;
        let mut processed_ids: Vec<String> = Vec::with_capacity(events.len());
        let mut halt_error: Option<anyhow::Error> = None;
        let mut skip_error: Option<anyhow::Error> = None;

        for event in events {
            if cancel.map_or(false, |c| c.load(Ordering::SeqCst)) {
                break;
            }
            let id = event.id.clone();
            let insert_result = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                sink.insert_event(&event),
            )
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "outbox event {id} insert timed out after 10 seconds"
                )
            })?;

            match insert_result {
                Ok(()) => {
                    processed_ids.push(id);
                }
                Err(e) => match config.on_error {
                    ErrorPolicy::Skip => {
                        eprintln!("[outbox] skip event {id}: {e}");
                        if skip_error.is_none() {
                            skip_error = Some(e);
                        }
                        processed_ids.push(id);
                    }
                    ErrorPolicy::Halt => {
                        halt_error = Some(e);
                        break;
                    }
                },
            }
        }

        acknowledge(self.conn.clone(), &config.outbox_table, processed_ids).await?;
        if let Some(e) = halt_error {
            return Err(e);
        }
        if let Some(e) = skip_error {
            return Err(e);
        }
        Ok(full_batch)
    }

    fn wait_for_cancel(
        cancel: &Arc<AtomicBool>,
        interval: std::time::Duration,
    ) -> impl std::future::Future<Output = ()> {
        let cancel = cancel.clone();
        async move {
            let chunk = std::time::Duration::from_millis(250);
            let mut remaining = interval;
            while remaining > std::time::Duration::ZERO {
                if cancel.load(Ordering::SeqCst) {
                    break;
                }
                let next = std::cmp::min(chunk, remaining);
                tokio::time::sleep(next).await;
                remaining = remaining.saturating_sub(next);
            }
        }
    }

    /// Long-running poll loop. Stop via the provided cancellation token.
    /// Sleeps before the first poll intentionally — avoids thundering herd on startup.
    /// `on_error` is called for every poll/drain error; use it to emit Tauri events or log.
    pub async fn run<S: Sink, F: Fn(&anyhow::Error) + Send + 'static>(
        self,
        sink: S,
        config: OutboxConfig,
        on_error: F,
        cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) {
        let interval = std::time::Duration::from_millis(config.poll_interval_ms);
        'outer: loop {
            if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            Self::wait_for_cancel(&cancel, interval).await;
            if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            match self.sync_batch_with_cancel(&sink, &config, &cancel).await {
                Ok(true) => loop {
                    if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                        break 'outer;
                    }
                    match self.sync_batch_with_cancel(&sink, &config, &cancel).await {
                        Ok(true) => {}
                        Ok(false) => break,
                        Err(e) => {
                            on_error(&e);
                            break;
                        }
                    }
                },
                Ok(false) => {}
                Err(e) => {
                    on_error(&e);
                }
            }
        }
    }
}

pub mod postgres;

#[cfg(test)]
mod sync_batch_tests {
    use super::*;
    use rusqlite::Connection;
    use std::sync::atomic::{AtomicUsize, Ordering};
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
        conn.lock()
            .unwrap()
            .execute(
                "INSERT INTO outbox VALUES (?, 'tier_fact', 'entries', 'rec1', 'INSERT', '{}', ?)",
                rusqlite::params![id, created_at],
            )
            .unwrap();
    }

    fn count_remaining(conn: &Arc<Mutex<Connection>>) -> i64 {
        conn.lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM outbox", [], |r| r.get(0))
            .unwrap()
    }

    /// Sink that succeeds for all events and records call count.
    struct CountingSink(Arc<AtomicUsize>);
    impl Sink for CountingSink {
        async fn insert_event(&self, _event: &OutboxEvent) -> anyhow::Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    /// Sink that fails after N successful inserts.
    struct FailAfterSink {
        after: usize,
        count: Arc<AtomicUsize>,
    }
    impl Sink for FailAfterSink {
        async fn insert_event(&self, _event: &OutboxEvent) -> anyhow::Result<()> {
            let n = self.count.fetch_add(1, Ordering::SeqCst);
            if n >= self.after {
                anyhow::bail!("injected failure");
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn sync_batch_processes_all_events_and_acks() {
        let (_f, conn) = setup_outbox_db();
        insert_raw(&conn, "a", 1000);
        insert_raw(&conn, "b", 2000);
        let counter = Arc::new(AtomicUsize::new(0));
        let config = OutboxConfig {
            outbox_table: "outbox".into(),
            batch_size: 10,
            ..Default::default()
        };
        let worker = OutboxWorker::new(conn);
        worker
            .sync_batch(&CountingSink(counter.clone()), &config)
            .await
            .unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 2);
        assert_eq!(count_remaining(&worker.conn), 0);
    }

    #[tokio::test]
    async fn sync_batch_concurrency_guard_skips_concurrent_call() {
        let (_f, conn) = setup_outbox_db();
        insert_raw(&conn, "a", 1000);
        let counter = Arc::new(AtomicUsize::new(0));
        let config = OutboxConfig {
            outbox_table: "outbox".into(),
            batch_size: 10,
            ..Default::default()
        };
        let worker = Arc::new(OutboxWorker::new(conn));
        // Pre-set running flag to simulate in-progress call
        worker.running.store(true, Ordering::SeqCst);
        worker
            .sync_batch(&CountingSink(counter.clone()), &config)
            .await
            .unwrap();
        assert_eq!(
            counter.load(Ordering::SeqCst),
            0,
            "should skip when already running"
        );
        worker.running.store(false, Ordering::SeqCst);
    }

    #[tokio::test]
    async fn halt_policy_stops_on_first_failure_and_acks_prior() {
        let (_f, conn) = setup_outbox_db();
        insert_raw(&conn, "a", 1000);
        insert_raw(&conn, "b", 2000);
        insert_raw(&conn, "c", 3000);
        let count = Arc::new(AtomicUsize::new(0));
        let config = OutboxConfig {
            outbox_table: "outbox".into(),
            batch_size: 10,
            on_error: ErrorPolicy::Halt,
            ..Default::default()
        };
        let worker = OutboxWorker::new(conn);
        // Fails after first successful insert — Halt returns Err, prior events are acked
        let result = worker
            .sync_batch(
                &FailAfterSink {
                    after: 1,
                    count: count.clone(),
                },
                &config,
            )
            .await;
        assert!(
            result.is_err(),
            "Halt policy must propagate the insert error"
        );
        // "a" was acked, "b" and "c" remain
        assert_eq!(count_remaining(&worker.conn), 2);
    }

    #[tokio::test]
    async fn skip_policy_continues_after_failure() {
        let (_f, conn) = setup_outbox_db();
        insert_raw(&conn, "a", 1000);
        insert_raw(&conn, "b", 2000);
        insert_raw(&conn, "c", 3000);
        let count = Arc::new(AtomicUsize::new(0));
        let config = OutboxConfig {
            outbox_table: "outbox".into(),
            batch_size: 10,
            on_error: ErrorPolicy::Skip,
            ..Default::default()
        };
        let worker = OutboxWorker::new(conn);
        // Fails on second event, skip means the batch still completes and the error is reported.
        let result = worker
            .sync_batch(
                &FailAfterSink {
                    after: 1,
                    count: count.clone(),
                },
                &config,
            )
            .await;
        assert!(result.is_err(), "skip policy should still report errors");
        assert_eq!(
            count_remaining(&worker.conn),
            0,
            "all events are acknowledged including skipped ones"
        );
    }

    #[tokio::test]
    async fn skip_policy_reports_error_for_failed_insert() {
        let (_f, conn) = setup_outbox_db();
        insert_raw(&conn, "a", 1000);
        insert_raw(&conn, "b", 2000);
        let count = Arc::new(AtomicUsize::new(0));
        let config = OutboxConfig {
            outbox_table: "outbox".into(),
            batch_size: 10,
            on_error: ErrorPolicy::Skip,
            ..Default::default()
        };
        let worker = OutboxWorker::new(conn);

        let result = worker
            .sync_batch(
                &FailAfterSink {
                    after: 1,
                    count: count.clone(),
                },
                &config,
            )
            .await;

        assert!(result.is_err(), "skip policy should still report errors");
        assert_eq!(
            count_remaining(&worker.conn),
            0,
            "skipped events are acknowledged"
        );
    }

    #[tokio::test]
    async fn backlog_drain_triggered_when_full_batch_consumed() {
        let (_f, conn) = setup_outbox_db();
        for i in 0..3 {
            insert_raw(&conn, &format!("id{i}"), i as i64 * 1000);
        }
        let counter = Arc::new(AtomicUsize::new(0));
        let config = OutboxConfig {
            outbox_table: "outbox".into(),
            batch_size: 3, // exactly fills the batch
            ..Default::default()
        };
        let worker = OutboxWorker::new(conn);
        let drained = worker
            .sync_batch(&CountingSink(counter.clone()), &config)
            .await
            .unwrap();
        assert!(drained, "should signal backlog drain when batch was full");
    }
}

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
        conn.lock()
            .unwrap()
            .execute(
                "INSERT INTO outbox VALUES (?, 'tier_fact', 'entries', 'rec1', 'INSERT', '{}', ?)",
                rusqlite::params![id, created_at],
            )
            .unwrap();
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
        acknowledge(conn.clone(), "outbox", vec!["a".into()])
            .await
            .unwrap();
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

    #[tokio::test]
    async fn fetch_pending_errors_on_malformed_payload() {
        let f = NamedTempFile::new().unwrap();
        let conn = Connection::open(f.path()).unwrap();
        conn.execute_batch(
            "CREATE TABLE outbox (
               id TEXT PRIMARY KEY, entity_id TEXT, table_name TEXT,
               record_id TEXT, operation TEXT, payload TEXT, created_at INTEGER
             );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO outbox VALUES ('bad1','e','t','r','INSERT','NOT_VALID_JSON',0)",
            [],
        )
        .unwrap();
        let conn = Arc::new(Mutex::new(conn));
        let result = fetch_pending(conn, "outbox", 10).await;
        assert!(result.is_err(), "must fail on malformed JSON payload");
    }

    #[tokio::test]
    async fn fetch_pending_returns_empty_when_outbox_table_missing() {
        let f = NamedTempFile::new().unwrap();
        let conn = Connection::open(f.path()).unwrap();
        let conn = Arc::new(Mutex::new(conn));
        let events = fetch_pending(conn, "outbox", 10)
            .await
            .expect("missing outbox table should be treated as empty queue");
        assert!(events.is_empty());
    }
}

#[cfg(test)]
mod open_tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn worker_open_enables_wal_mode() {
        let f = NamedTempFile::new().unwrap();
        let worker = OutboxWorker::open(f.path()).unwrap();
        let mode: String = worker
            .conn
            .lock()
            .unwrap()
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
    }
}
