# Rust Outbox Worker — Prisma Adapter Design

**Date:** 2026-05-17
**Status:** Implemented
**Implementation:** `/subagent-driven-development` + `/test-driven-development` required on all PRs

---

## Summary

Add a native Rust outbox worker to Curated Thoughts that polls the `core-llm-wiki` SQLite outbox table and syncs events to a PostgreSQL database via `sqlx`. Inspired by `@equationalapplications/prisma-outbox` (TypeScript), ported to Rust. No Node.js sidecar. No npm package dependency.

The worker is **opt-in** — Curated Thoughts runs normally without it. When `DATABASE_URL` is set, both the Tauri desktop binary and the `curated-thoughts-mcp` headless binary auto-initialize the worker at startup. Consumer-level users are unaffected.

---

## Motivation

`core-llm-wiki` v4.9.0 introduced `enableOutbox: true`, which writes wiki mutations atomically to a SQLite `outbox` table. This design provides a general-purpose Postgres sync path usable without recompilation or forking — operators configure via `DATABASE_URL`, enterprise schemas project from a `wiki_outbox_events` table using Postgres triggers or views.

### Example deployment (illustrative)

An enterprise runs `curated-thoughts-mcp` headlessly alongside Postgres in Docker:

```yaml
services:
  curated-thoughts-mcp:
    image: equationalapplications/curated-thoughts-mcp
    environment:
      DATABASE_URL: postgresql://user:pass@postgres:5432/enterprise_db
  postgres:
    image: postgres:16
```

The enterprise DB receives all wiki events in `wiki_outbox_events`. Their team projects into their own schema via Postgres triggers or downstream ETL. No Rust changes, no custom binary.

---

## Architecture

```
[wiki mutations]
      │ wiki_exec / wiki_run (JS → Tauri)
      ▼
[SQLite: wiki tables + outbox table (atomic write)]
      │
      │ poll every 5s — dedicated worker connection (WAL, busy_timeout=5000ms)
      ▼
[OutboxWorker — tokio::spawn task]         ← defined in tauri_app_lib
      │ sqlx PgPool → $DATABASE_URL        ← used by both binaries
      ▼
[Postgres: wiki_outbox_events]
      │ triggers / views / ETL
      ▼
[Enterprise schema / downstream consumers]
```

### Two entry points, one implementation

`OutboxWorker` is defined in `tauri_app_lib` (`src-tauri/`). The MCP binary already depends on `tauri_app_lib`, so it gains the worker for free.

| Binary | Runtime | Auto-init trigger |
|---|---|---|
| `curated-thoughts` (Tauri) | multi-thread Tokio | `tauri::Builder::setup` — checks `DATABASE_URL` |
| `curated-thoughts-mcp` | `current_thread` Tokio | `main()` — checks `DATABASE_URL` |

Both: if `DATABASE_URL` is absent, worker is not spawned. If present, worker starts automatically.

The Tauri binary additionally exposes `start_outbox_worker` / `stop_outbox_worker` commands for runtime override (e.g., desktop user connecting to a different DB mid-session).

### SQLite connection strategy

The worker opens **its own dedicated SQLite connection** (read+write, WAL, `PRAGMA busy_timeout = 5000`). This connection handles both polling and ack DELETEs. It does not share or lock `DbState`. WAL mode allows concurrent connections to the same file; busy timeout handles write contention without deadlock.

All SQLite operations within the worker are wrapped in `tokio::task::spawn_blocking` — correct for both `current_thread` and `multi_thread` Tokio flavors.

---

## Components

### New files

```
src-tauri/src/outbox/
  mod.rs        — OutboxWorker, OutboxConfig, OutboxEvent, ErrorPolicy, Sink trait, sync_batch
  postgres.rs   — PgSink: wiki_outbox_events table creation, batch insert via sqlx
```

### Changed files

| File | Change |
|---|---|
| `src-tauri/src/lib.rs` | `OutboxWorkerState`, auto-init from `DATABASE_URL` in setup, `start_outbox_worker` (supports runtime `database_url` override), `stop_outbox_worker` commands |
| `src-tauri/Cargo.toml` | Add `sqlx` with `postgres`, `runtime-tokio-native-tls`, `json` features |
| `src/lib/wiki.ts` | Add `enableOutbox: true` to `createWiki` config |
| `tools/src/bin/curated_thoughts_mcp.rs` | Auto-init `OutboxWorker` from `DATABASE_URL` in `main()` |

---

## Data Structures

```rust
pub struct OutboxConfig {
    pub sqlite_path: PathBuf,    // path to the SQLite file — worker opens its own connection
    pub db_url: String,          // Postgres DATABASE_URL
    pub outbox_table: String,    // SQLite table name written by core-llm-wiki; default: "outbox"
    pub poll_interval_ms: u64,   // default: 5000
    pub batch_size: usize,       // default: 100
    pub on_error: ErrorPolicy,   // default: ErrorPolicy::Halt
}

#[derive(Clone, Copy)]
pub enum ErrorPolicy {
    Halt, // preserve ordering; stop on first failure
    Skip, // acknowledge and continue (poison-pill mitigation)
}

pub struct OutboxEvent {
    pub id: String,
    pub entity_id: String,
    pub table_name: String,
    pub record_id: String,
    pub operation: String,           // "INSERT" | "UPDATE" | "DELETE"
    pub payload: serde_json::Value,
    pub created_at: i64,
}

pub trait Sink: Send + Sync + 'static {
    fn insert_event(
        &self,
        event: &OutboxEvent,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send;
}

// Implementation uses RPITIT (Return Position Impl Trait in Trait) style,
// not async_trait. This is the correct Rust 1.75+ pattern.
```

---

## Postgres Target Schema

Auto-created by `PgSink::new()` on startup:

```sql
CREATE TABLE IF NOT EXISTS wiki_outbox_events (
    id          TEXT    PRIMARY KEY,
    entity_id   TEXT    NOT NULL,
    table_name  TEXT    NOT NULL,
    record_id   TEXT    NOT NULL,
    operation   TEXT    NOT NULL,
    payload     JSONB,
    created_at  BIGINT  NOT NULL,
    synced_at   BIGINT  NOT NULL DEFAULT (extract(epoch from now()) * 1000)
);

CREATE INDEX IF NOT EXISTS idx_woe_entity_created
    ON wiki_outbox_events (entity_id, created_at);

CREATE INDEX IF NOT EXISTS idx_woe_table_op
    ON wiki_outbox_events (table_name, operation);
```

`ON CONFLICT (id) DO NOTHING` on every insert provides idempotency: if SQLite ack fails after Postgres commits, the re-delivered event is safely skipped.

---

## `sync_batch` Logic

Mirrors `PrismaOutboxWorker.syncBatch()`:

```
sync_batch(conn: &Connection, sink: &dyn Sink, config):
  if running.swap(true, SeqCst) → return  // atomic concurrency guard

  // SQLite read via spawn_blocking
  events = SELECT * FROM outbox
           ORDER BY created_at ASC, rowid ASC
           LIMIT batch_size
  if events.empty() → return

  processed_ids = []
  halted = false

  for event in events:
    try:
      BEGIN postgres transaction
        INSERT INTO wiki_outbox_events ... ON CONFLICT (id) DO NOTHING
      COMMIT
      processed_ids.push(event.id)
    catch err:
      match config.on_error:
        Skip → processed_ids.push(event.id)   // acknowledge, continue
        Halt → halted = true; break            // preserve ordering

  // SQLite ack via spawn_blocking (same dedicated worker connection)
  DELETE FROM outbox WHERE id IN (processed_ids)

  if !halted && events.len() == batch_size:
    spawn immediate re-poll  // backlog drain

  running.store(false, SeqCst)
```

One Postgres transaction per event (not per batch). Matches JS design — partial progress on halt, per-event ordering preserved.

**Throughput note:** 100 events = 100 sequential Postgres round-trips. Acceptable for background sync. Bulk reindex of a large vault will temporarily backlog; backlog drain (immediate re-poll) keeps the queue moving.

---

## Initialization

### Tauri binary (`lib.rs` setup)

```rust
tauri::Builder::default()
    .setup({
        let db_path = db_path.clone();
        move |app| {
            // configured_database_url() trims whitespace and rejects empty strings.
            if let Some(db_url) = configured_database_url() {
                let config = OutboxConfig { sqlite_path: db_path.clone(), db_url, ..OutboxConfig::default() };
                let handle = spawn_postgres_worker(config, Some(app.app_handle().clone()));
                let state = app.state::<OutboxWorkerState>();
                *state.0.lock().unwrap() = Some(handle);
            }
            Ok(())
        }
    })
```

### MCP binary (`curated_thoughts_mcp.rs` main)

```rust
#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let p = retrieval::resolve_brain_paths();
    // ... existing setup ...

    // configured_database_url() trims whitespace and rejects empty strings.
    fn configured_database_url() -> Option<String> {
        let url = std::env::var("DATABASE_URL").ok()?;
        let url = url.trim();
        if url.is_empty() { None } else { Some(url.to_string()) }
    }

    if let Some(db_url) = configured_database_url() {
        let config = OutboxConfig {
            sqlite_path: p.db_path.clone(),
            db_url,
            ..tauri_app_lib::outbox::OutboxConfig::default()
        };
        // spawn_postgres_worker lives in tauri_app_lib::outbox::postgres — sqlx never enters tools/
        let _ = tauri_app_lib::outbox::postgres::spawn_postgres_worker(config, None);
    }

    // ... existing MCP server start ...
}
```

### Tauri runtime commands (desktop override)

```rust
#[tauri::command]
async fn start_outbox_worker(
    app_handle: AppHandle,
    database_url: Option<String>,   // runtime override; uses DATABASE_URL if None
    poll_interval_ms: Option<u64>,
    batch_size: Option<usize>,
    on_error: Option<String>,        // "halt" | "skip"
    state: State<'_, OutboxWorkerState>,
) -> Result<(), String>

#[tauri::command]
async fn stop_outbox_worker(
    app_handle: AppHandle,
    state: State<'_, OutboxWorkerState>,
) -> Result<(), String>
```

`start_outbox_worker` is idempotent with respect to config: if a worker is already
running with identical configuration, the call is a no-op. If the config differs
(different URL, interval, etc.), the existing worker is stopped and a new one
started. If `database_url` is provided, the worker connects to that database instead
of `DATABASE_URL`, enabling runtime override (e.g., desktop user connecting to a
different DB mid-session). Unknown `on_error` values return an error.

---

## Error Handling

| Tier | Trigger | Behaviour |
|---|---|---|
| Per-event | Postgres insert fails | `Halt`: stop batch, ack prior successes. `Skip`: ack event, continue. |
| Worker-level | SQLite read/ack fails | Log to stderr + emit `outbox-worker-error` Tauri event (desktop) or stderr only (MCP) |

`outbox-worker-error` payload (matches `OutboxWorkerError` struct in `postgres.rs`):
```json
{ "error": "string", "fatal": true }
```

Note: The `fatal` field is included in the actual implementation.

`fatal: true` means the worker stopped itself (SQLite open failure). `fatal: false` means a per-poll error the loop continues after. Frontend may surface this in settings UI or ignore it.

**Known limitation — halt on deterministic failure:** `ErrorPolicy::Halt` will retry the same failing event every poll cycle if the failure is deterministic (e.g., Postgres schema mismatch, constraint violation). This is intentional — matches JS package behaviour, preserves ordering, requires operator intervention to resolve. `max_retries` / circuit-breaker is deferred to a follow-up spec.

---

## `enableOutbox` JS Change

`src/lib/wiki.ts` — dynamic check for `DATABASE_URL` before enabling outbox:

```typescript
function makeWikiOptions(enableOutbox: boolean): WikiOptions & Record<string, unknown> {
  return {
    llmProvider: { ... },
    config: {
      hybridWeight: 0.7,
      preFilterLimit: 50,
      ...(enableOutbox && { enableOutbox: true }),
    },
    ...
  } as WikiOptions & Record<string, unknown>;
}

export let wiki = createWiki(tauriWikiAdapter, makeWikiOptions(false));

export async function setupWiki() {
  const outboxEnabled = await invoke<boolean>('outbox_is_configured').catch(() => false);
  const newWiki = createWiki(tauriWikiAdapter, makeWikiOptions(outboxEnabled));
  await newWiki.setup();
  wiki = newWiki;

  // Re-create wiki when a runtime outbox worker starts or stops so that
  // enableOutbox reflects the current worker state for all future mutations.
  await listen<void>('outbox-worker-started', async () => {
    const updated = createWiki(tauriWikiAdapter, makeWikiOptions(true));
    await updated.setup();
    wiki = updated;
  });
  await listen<void>('outbox-worker-stopped', async () => {
    const updated = createWiki(tauriWikiAdapter, makeWikiOptions(false));
    await updated.setup();
    wiki = updated;
  });
}
```

**Intentional improvement:** Instead of blindly setting `enableOutbox: true`, the JS layer dynamically checks whether the outbox is currently active via the `outbox_is_configured` Tauri command. That command reflects the runtime `OutboxWorkerState`, so it may return `false` if the worker has been stopped or has already finished, even when `DATABASE_URL` is present. This prevents unnecessary SQLite writes when the Postgres sync path is not actually running. The event listeners handle runtime worker start/stop (e.g., desktop user calling `start_outbox_worker` mid-session).

This causes `core-llm-wiki` to write every wiki mutation atomically to the `outbox` table alongside the primary write — but only when the outbox worker is actually active. No other JS changes required.

---

## Testing Strategy

All PRs require `/test-driven-development`. Tests written before implementation.

| Layer | Method |
|---|---|
| `OutboxEvent` SQLite deserialization | Unit — insert raw outbox row into in-memory SQLite, assert parse |
| `sync_batch` happy path | Unit — in-memory SQLite + `MockSink` |
| Concurrency guard | Unit — two concurrent `sync_batch()` calls, assert second returns immediately |
| Idempotency | Unit — deliver same event twice via `MockSink`, assert single insert attempt |
| Halt-on-error | Unit — `MockSink` returns error mid-batch, assert ordering preserved, prior events acked |
| Skip policy | Unit — `ErrorPolicy::Skip`, assert subsequent events processed after failure |
| Backlog drain | Unit — full batch returned, assert immediate re-poll triggered |
| Worker dedicated connection | Unit — assert worker opens its own connection (not DbState), busy_timeout set |
| MCP auto-init | Unit — `DATABASE_URL` set, assert `OutboxWorker::run` called in MCP main |
| Integration | Real SQLite + real Postgres; gated on `OUTBOX_TEST_DATABASE_URL` env var (CI opt-in) |

---

## Parallel PR Plan

Implementation uses `/subagent-driven-development`. PRs 1, 3 are fully independent. PR 2 is semi-parallel with PR 1. PR 4 depends on PRs 1 + 2.

### PR 1 — Outbox types + SQLite polling (`src-tauri/src/outbox/mod.rs`)

**Scope:**
- `OutboxEvent`, `OutboxConfig`, `ErrorPolicy` structs
- `Sink` trait: `async fn insert_event(&self, event: &OutboxEvent) -> anyhow::Result<()>`
- `OutboxWorker` with `sync_batch(sink: &dyn Sink)` — no Postgres dependency, no Tauri wiring
- Dedicated SQLite connection logic: open with `PRAGMA busy_timeout = 5000`, `spawn_blocking` wrappers for fetch and ack
- Unit tests: deserialization, concurrency guard, halt/skip policies, backlog drain, dedicated connection isolation

**No new crate dependencies.** Pure Rust + rusqlite (already in tree).

---

### PR 2 — Postgres sink (`src-tauri/src/outbox/postgres.rs`)

**Scope:**
- `sqlx` added to `src-tauri/Cargo.toml` (`postgres`, `runtime-tokio-native-tls`, `json` features; `default-features = false` to exclude unused MySQL/SQLite drivers)
- `PgSink::new(db_url)` — creates pool, runs `CREATE TABLE IF NOT EXISTS`
- `PgSink` implements `Sink` trait — single-event insert with `ON CONFLICT DO NOTHING`
- Unit tests: idempotency, `ON CONFLICT` behaviour, table creation is re-entrant

**Semi-parallel with PR 1** — requires `OutboxEvent` and `Sink` trait from PR 1 merged (or developed against PR 1 branch). Postgres logic is otherwise independent.

---

### PR 3 — JS `enableOutbox` (`src/lib/wiki.ts`)

**Scope:**
- Add dynamic `enableOutbox` check via `outbox_is_configured` Tauri command
- `makeWikiOptions(enableOutbox: boolean)` factory function with conditional `enableOutbox: true`
- `setupWiki()` invokes `outbox_is_configured` and creates wiki instance with appropriate config
- Verify existing `wiki.test.ts` passes

**Intentional improvement:** Instead of hardcoding `enableOutbox: true`, the JS layer dynamically checks whether the outbox is configured before enabling it. This prevents unnecessary SQLite writes for users who don't have Postgres configured.

**Fully independent.** No Rust changes. Ships any time.

---

### PR 4 — Tauri + MCP wiring (depends on PR 1 + PR 2)

**Scope:**
- `lib.rs`: `OutboxWorkerState`, auto-init from `DATABASE_URL` in `tauri::Builder::setup`, `start_outbox_worker` (supports runtime `database_url` override), `stop_outbox_worker` commands
- `curated_thoughts_mcp.rs`: auto-init via `tauri_app_lib::outbox::spawn_postgres_worker(config)` — no `sqlx` in `tools/Cargo.toml`; `sqlx` stays encapsulated in `src-tauri` only
- Integration test: real SQLite + real Postgres via GitHub Actions `services: postgres:` (env-var gated, `DATABASE_URL` set in CI test step only — matches existing pattern)
- `outbox-worker-error` Tauri event emission

---

## Constraints

- Single worker per SQLite file. No row-level locking or lease mechanism. Running two workers against the same file causes duplicate Postgres writes. Documented in code.
- Generic passthrough only — no per-table mapping in Rust. Enterprise projections live in Postgres.
- Worker is opt-in. No impact on users who do not set `DATABASE_URL`.
- `serde_json::Value` for payload — no schema enforcement in Rust.
- `ErrorPolicy::Halt` on deterministic failure = infinite retry loop. Operator must stop the worker and fix the root cause. `max_retries` deferred.
- Graceful shutdown on SIGTERM not implemented. Worker task is aborted; in-flight SQLite batch may be re-delivered (idempotency handles it). `CancellationToken`-based drain deferred.
- Throughput: one Postgres transaction per event. Background sync use case; not optimized for bulk throughput.
- **`database_url` webview parameter — intentional trust model:** `start_outbox_worker` accepts `database_url` from the webview so a desktop user can redirect sync to a different Postgres instance mid-session (documented use case). This is scoped to the Tauri desktop binary only; the MCP binary auto-inits exclusively from the environment. The Tauri capability system restricts which renderers can invoke `start_outbox_worker`. Operators who want to lock down the destination URL should omit the command from their capability config.
- **Connect timeout per attempt:** each `PgSink::new()` call is wrapped in a 10-second `tokio::time::timeout`. On timeout the attempt is treated as a connect failure and the retry loop checks the cancel flag immediately, keeping `stop_outbox_worker` and `switch_vault` responsive even when Postgres is unreachable.
