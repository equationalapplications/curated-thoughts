use crate::outbox::{OutboxEvent, Sink};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::Emitter;

pub struct PgSink {
    pool: PgPool,
}

impl PgSink {
    pub async fn new(db_url: &str) -> anyhow::Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(db_url)
            .await?;
        execute_ddl(
            &pool,
            "CREATE TABLE IF NOT EXISTS wiki_outbox_events (
                id          TEXT    PRIMARY KEY,
                entity_id   TEXT    NOT NULL,
                table_name  TEXT    NOT NULL,
                record_id   TEXT    NOT NULL,
                operation   TEXT    NOT NULL,
                payload     JSONB,
                created_at  BIGINT  NOT NULL,
                synced_at   BIGINT  NOT NULL DEFAULT (EXTRACT(EPOCH FROM now()) * 1000)::BIGINT
            )",
        )
        .await?;
        execute_ddl(
            &pool,
            "CREATE INDEX IF NOT EXISTS idx_woe_entity_created \
             ON wiki_outbox_events (entity_id, created_at)",
        )
        .await?;
        execute_ddl(
            &pool,
            "CREATE INDEX IF NOT EXISTS idx_woe_table_op \
             ON wiki_outbox_events (table_name, operation)",
        )
        .await?;
        Ok(Self { pool })
    }
}

impl Sink for PgSink {
    async fn insert_event(&self, event: &OutboxEvent) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO wiki_outbox_events \
             (id, entity_id, table_name, record_id, operation, payload, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) \
             ON CONFLICT (id) DO NOTHING",
        )
        .bind(&event.id)
        .bind(&event.entity_id)
        .bind(&event.table_name)
        .bind(&event.record_id)
        .bind(&event.operation)
        .bind(sqlx::types::Json(&event.payload))
        .bind(event.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

async fn execute_ddl(pool: &PgPool, sql: &str) -> anyhow::Result<()> {
    match sqlx::query(sql).execute(pool).await {
        Ok(_) => Ok(()),
        Err(e) if is_pg_duplicate_object(&e) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

fn is_pg_duplicate_object(err: &sqlx::Error) -> bool {
    if let sqlx::Error::Database(db_err) = err {
        if let Some(code) = db_err.code() {
            return code == "42710" || code == "23505";
        }
        let msg = db_err.message().to_lowercase();
        return msg.contains(
            "duplicate key value violates unique constraint \"pg_type_typname_nsp_index\"",
        ) || msg.contains("already exists");
    }
    false
}

#[derive(serde::Serialize, Clone)]
struct OutboxWorkerError {
    error: String,
    /// true when the worker stopped itself; false for per-poll errors the loop continues after.
    fatal: bool,
}

pub struct OutboxWorkerHandle {
    cancel: Arc<AtomicBool>,
    handle: tauri::async_runtime::JoinHandle<()>,
    finished: Arc<AtomicBool>,
}

impl OutboxWorkerHandle {
    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::SeqCst)
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    pub async fn stop(self) {
        self.cancel.store(true, Ordering::SeqCst);
        let _ = self.handle.await;
        self.finished.store(true, Ordering::SeqCst);
    }
}

pub fn spawn_postgres_worker(
    config: crate::outbox::OutboxConfig,
    app_handle: Option<tauri::AppHandle>,
) -> OutboxWorkerHandle {
    let cancel = Arc::new(AtomicBool::new(false));
    let finished = Arc::new(AtomicBool::new(false));
    let finished_for_task = finished.clone();
    let cancel_for_run = cancel.clone();
    // Use Tauri's async runtime to avoid panics when called from setup hooks
    // where no Tokio runtime may be entered.
    let handle = tauri::async_runtime::spawn(async move {
        let emit = |app_handle: &Option<tauri::AppHandle>, msg: String, fatal: bool| {
            eprintln!("[outbox] {msg}");
            if let Some(ref handle) = app_handle {
                let _ = handle.emit(
                    "outbox-worker-error",
                    OutboxWorkerError { error: msg, fatal },
                );
            }
        };

        // Retry initial Postgres connection with exponential backoff so transient startup
        // failures (Postgres not yet ready in Compose/CI) don't permanently disable the worker.
        let sink = {
            let mut delay_ms = 1_000u64;
            let mut connected = None;
            loop {
                if cancel_for_run.load(Ordering::SeqCst) {
                    break;
                }
                match PgSink::new(&config.db_url).await {
                    Ok(s) => {
                        connected = Some(s);
                        break;
                    }
                    Err(e) => {
                        if cancel_for_run.load(Ordering::SeqCst) {
                            break;
                        }
                        emit(
                            &app_handle,
                            format!("Postgres connect failed: {e}; retrying in {delay_ms}ms"),
                            false,
                        );
                        let mut remaining_ms = delay_ms;
                        while remaining_ms > 0 {
                            let chunk = std::cmp::min(remaining_ms, 250);
                            tokio::time::sleep(std::time::Duration::from_millis(chunk)).await;
                            if cancel_for_run.load(Ordering::SeqCst) {
                                break;
                            }
                            remaining_ms -= chunk;
                        }
                        delay_ms = (delay_ms * 2).min(30_000);
                    }
                }
            }
            connected
        };

        let sink = match sink {
            Some(s) => s,
            None => {
                finished_for_task.store(true, Ordering::SeqCst);
                return;
            }
        };

        if cancel_for_run.load(Ordering::SeqCst) {
            finished_for_task.store(true, Ordering::SeqCst);
            return;
        }

        match crate::outbox::OutboxWorker::open(&config.sqlite_path) {
            Ok(worker) => {
                let on_error = {
                    let app_handle = app_handle.clone();
                    move |e: &anyhow::Error| {
                        let msg = e.to_string();
                        eprintln!("[outbox] worker error: {msg}");
                        if let Some(ref handle) = app_handle {
                            let _ = handle.emit(
                                "outbox-worker-error",
                                OutboxWorkerError {
                                    error: msg,
                                    fatal: false,
                                },
                            );
                        }
                    }
                };
                worker.run(sink, config, on_error, cancel_for_run).await;
            }
            Err(e) => emit(&app_handle, format!("failed to open SQLite: {e}"), true),
        }
        finished_for_task.store(true, Ordering::SeqCst);
    });

    OutboxWorkerHandle { cancel, handle, finished }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db_url() -> Option<String> {
        // Check OUTBOX_TEST_DATABASE_URL first (dedicated test var), then fall back to
        // DATABASE_URL so CI jobs that set DATABASE_URL work without modification.
        std::env::var("OUTBOX_TEST_DATABASE_URL")
            .ok()
            .or_else(|| std::env::var("DATABASE_URL").ok())
    }

    #[tokio::test]
    async fn pg_sink_new_creates_table() {
        let Some(url) = db_url() else {
            eprintln!("Skipping: DATABASE_URL not set");
            return;
        };
        let sink = PgSink::new(&url).await.expect("should connect");
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM wiki_outbox_events")
            .fetch_one(&sink.pool)
            .await
            .expect("table should exist");
        let _ = count;
    }

    #[tokio::test]
    async fn pg_sink_new_is_idempotent() {
        let Some(url) = db_url() else {
            return;
        };
        PgSink::new(&url).await.unwrap();
        PgSink::new(&url).await.unwrap();
    }

    #[tokio::test]
    async fn pg_sink_insert_event_and_idempotency() {
        let Some(url) = db_url() else {
            return;
        };
        let sink = PgSink::new(&url).await.unwrap();

        let event = OutboxEvent {
            id: format!("test-{}", uuid::Uuid::new_v4()),
            entity_id: "tier_fact".into(),
            table_name: "entries".into(),
            record_id: "rec1".into(),
            operation: "INSERT".into(),
            payload: serde_json::json!({"key": "value"}),
            created_at: 1_000_000,
        };

        sink.insert_event(&event).await.unwrap();
        sink.insert_event(&event).await.unwrap();

        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM wiki_outbox_events WHERE id = $1")
                .bind(&event.id)
                .fetch_one(&sink.pool)
                .await
                .unwrap();
        assert_eq!(count, 1, "idempotent insert must produce exactly one row");

        sqlx::query("DELETE FROM wiki_outbox_events WHERE id = $1")
            .bind(&event.id)
            .execute(&sink.pool)
            .await
            .unwrap();
    }
}
