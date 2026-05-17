use crate::outbox::{OutboxEvent, Sink};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

pub struct PgSink {
    pool: PgPool,
}

impl PgSink {
    pub async fn new(db_url: &str) -> anyhow::Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(db_url)
            .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS wiki_outbox_events (
                id          TEXT    PRIMARY KEY,
                entity_id   TEXT    NOT NULL,
                table_name  TEXT    NOT NULL,
                record_id   TEXT    NOT NULL,
                operation   TEXT    NOT NULL,
                payload     JSONB,
                created_at  BIGINT  NOT NULL,
                synced_at   BIGINT  NOT NULL DEFAULT (extract(epoch from now()) * 1000)
            )",
        )
        .execute(&pool)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_woe_entity_created \
             ON wiki_outbox_events (entity_id, created_at)",
        )
        .execute(&pool)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_woe_table_op \
             ON wiki_outbox_events (table_name, operation)",
        )
        .execute(&pool)
        .await?;
        Ok(Self { pool })
    }
}

impl Sink for PgSink {
    async fn insert_event(&self, event: OutboxEvent) -> anyhow::Result<()> {
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

pub fn spawn_postgres_worker(config: crate::outbox::OutboxConfig) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let sink = match PgSink::new(&config.db_url).await {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[outbox] failed to connect to Postgres: {e}");
                return;
            }
        };
        match crate::outbox::OutboxWorker::open(&config.sqlite_path) {
            Ok(worker) => worker.run(sink, config).await,
            Err(e) => eprintln!("[outbox] failed to open SQLite: {e}"),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db_url() -> Option<String> {
        std::env::var("DATABASE_URL").ok()
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
        let Some(url) = db_url() else { return; };
        PgSink::new(&url).await.unwrap();
        PgSink::new(&url).await.unwrap();
    }

    #[tokio::test]
    async fn pg_sink_insert_event_and_idempotency() {
        let Some(url) = db_url() else { return; };
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

        sink.insert_event(event.clone()).await.unwrap();
        sink.insert_event(event.clone()).await.unwrap();

        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM wiki_outbox_events WHERE id = $1"
        )
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
