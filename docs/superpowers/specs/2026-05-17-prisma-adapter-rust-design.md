# Rust Outbox Worker — Prisma Adapter Design

**Date:** 2026-05-17
**Status:** Draft
**Implementation:** `/subagent-driven-development` + `/test-driven-development` required on all PRs

---

## Summary

Add a native Rust outbox worker to Curated Thoughts that polls the `core-llm-wiki` SQLite outbox table and syncs events to a PostgreSQL database via `sqlx`. Inspired by `@equationalapplications/prisma-outbox` (TypeScript), ported to Rust. No Node.js sidecar. No npm package dependency.

The worker is **opt-in** — Curated Thoughts runs normally without it. Enterprise operators enable it by supplying `DATABASE_URL` and calling `start_outbox_worker` at vault-open time. Consumer-level users are unaffected.

---

## Motivation

`core-llm-wiki` v4.9.0 introduced `enableOutbox: true`, which writes wiki mutations atomically to a SQLite `outbox` table. This design provides a general-purpose Postgres sync path usable without recompilation or forking — operators configure via environment variable, enterprise schemas project from a `wiki_outbox_events` table using Postgres triggers or views.

### Example deployment (illustrative)

An enterprise runs Curated Thoughts headlessly alongside Postgres in Docker:

```yaml
services:
  curated-thoughts:
    image: equationalapplications/curated-thoughts
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
      │ poll every 5s — dedicated read connection (WAL)
      ▼
[OutboxWorker — tokio::spawn task]
      │ sqlx PgPool → $DATABASE_URL
      ▼
[Postgres: wiki_outbox_events]
      │ triggers / views / ETL
      ▼
[Enterprise schema / downstream consumers]
```

SQLite runs in WAL mode (already enabled). Two connections to the same file are legal:
- **Write conn** — existing `DbState` `Mutex<AppDb>` (used to DELETE acknowledged events)
- **Read conn** — dedicated second connection opened by the worker for polling

`OutboxWorker` is managed as `OutboxWorkerState(Mutex<Option<tokio::task::JoinHandle<()>>>)` in Tauri state.

---

## Components

### New files

```
src-tauri/src/outbox/
  mod.rs        — OutboxWorker, OutboxConfig, OutboxEvent, ErrorPolicy, sync_batch
  postgres.rs   — PgSink: wiki_outbox_events table creation, batch insert via sqlx
```

### Changed files

| File | Change |
|---|---|
| `src-tauri/src/lib.rs` | `OutboxWorkerState`, `DbPathState` (SQLite file path), `start_outbox_worker`, `stop_outbox_worker` commands |
| `src-tauri/Cargo.toml` | Add `sqlx` with `postgres`, `runtime-tokio-native-tls`, `json` features |
| `src/lib/wiki.ts` | Add `enableOutbox: true` to `createWiki` config |

`DbPathState(Mutex<Option<PathBuf>>)` — new Tauri state set when a vault is opened. The outbox worker reads this to open its dedicated SQLite read connection.

---

## Data Structures

```rust
pub struct OutboxConfig {
    pub db_url: String,
    pub poll_interval_ms: u64,  // default: 5000
    pub batch_size: usize,      // default: 100
    pub on_error: ErrorPolicy,  // default: ErrorPolicy::Halt
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
```

---

## Postgres Target Schema

Auto-created by worker on `start()`:

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
sync_batch(db_state, sink: &dyn Sink, config):
  if running.swap(true, SeqCst) → return  // atomic concurrency guard

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

  DELETE FROM outbox WHERE id IN (processed_ids)  // via DbState write conn

  if !halted && events.len() == batch_size:
    spawn immediate re-poll  // backlog drain

  running.store(false, SeqCst)
```

One Postgres transaction per event (not per batch). Matches JS design — partial progress on halt, per-event ordering preserved.

---

## Tauri Commands

```rust
#[tauri::command]
async fn start_outbox_worker(
    db_url: String,
    poll_interval_ms: Option<u64>,
    batch_size: Option<usize>,
    on_error: Option<String>,           // "halt" | "skip"
    state: State<'_, OutboxWorkerState>,
    db_state: State<'_, DbState>,
    db_path: State<'_, DbPathState>,    // path to SQLite file for read conn
) -> Result<(), String>

#[tauri::command]
async fn stop_outbox_worker(
    state: State<'_, OutboxWorkerState>,
) -> Result<(), String>
```

`start_outbox_worker` is idempotent: calling it when already running is a no-op.

---

## Error Handling

| Tier | Trigger | Behaviour |
|---|---|---|
| Per-event | Postgres insert fails | `Halt`: stop batch, ack prior successes. `Skip`: ack event, continue. |
| Worker-level | SQLite read/ack fails | Log to stderr + emit `outbox-worker-error` Tauri event |

`outbox-worker-error` payload:
```json
{ "error": "string", "fatal": true }
```

`fatal: true` means the worker stopped itself. Frontend may surface this in settings UI or ignore it.

---

## enableOutbox JS Change

`src/lib/wiki.ts` — one-line change to `createWiki` call:

```typescript
export const wiki = createWiki(tauriWikiAdapter, {
  llmProvider: { ... },
  config: {
    hybridWeight: 0.7,
    preFilterLimit: 50,
    enableOutbox: true,   // ← new
  },
  ...
} as WikiOptions & Record<string, unknown>);
```

This causes `core-llm-wiki` to write every wiki mutation atomically to the `outbox` table alongside the primary write. No other JS changes required.

---

## Testing Strategy

All PRs require `/test-driven-development`. Tests written before implementation.

| Layer | Method |
|---|---|
| `OutboxEvent` SQLite deserialization | Unit — `open_in_memory()`, insert raw outbox row, assert parse |
| `sync_batch` happy path | Unit — in-memory SQLite + sqlx test pool |
| Concurrency guard | Unit — two concurrent `sync_batch()` calls, assert one returns immediately |
| Idempotency | Unit — deliver same event twice, assert single Postgres row |
| Halt-on-error | Unit — inject Postgres failure mid-batch, assert ordering preserved, prior events acked |
| Skip policy | Unit — `ErrorPolicy::Skip`, assert subsequent events processed after failure |
| Backlog drain | Unit — full batch returned, assert immediate re-poll triggered |
| Integration | `DATABASE_URL` env var required; skipped when absent (CI opt-in) |

---

## Parallel PR Plan

Implementation uses `/subagent-driven-development`. PRs 1–3 are independent and ship in parallel. PR 4 merges after PRs 1 and 2.

### PR 1 — Outbox types + SQLite polling (`src-tauri/src/outbox/mod.rs`)

**Scope:**
- `OutboxEvent`, `OutboxConfig`, `ErrorPolicy` structs
- `Sink` trait: `async fn insert_event(&self, event: &OutboxEvent) -> anyhow::Result<()>`
- `OutboxWorker` with `sync_batch(sink: &dyn Sink)` — no Postgres dependency, no Tauri wiring yet
- SQLite fetch and ack helpers (`fetch_pending`, `acknowledge`)
- Unit tests: deserialization, concurrency guard, halt/skip policies, backlog drain logic (using a `MockSink`)

**No new crate dependencies.** Pure Rust + rusqlite (already in tree).

---

### PR 2 — Postgres sink (`src-tauri/src/outbox/postgres.rs`)

**Scope:**
- `sqlx` dependency added to `Cargo.toml` (`postgres`, `runtime-tokio-native-tls`, `json`, `macros` features)
- `PgSink::new(db_url)` — creates pool, runs `CREATE TABLE IF NOT EXISTS`
- `PgSink::insert_event(pool, event)` — single-event insert with `ON CONFLICT DO NOTHING`
- Unit tests: idempotency, `ON CONFLICT` behaviour, table creation is re-entrant

**Semi-parallel with PR 1** — requires `OutboxEvent` and `Sink` trait from PR 1 to be merged (or developed against the PR 1 branch). Postgres logic is otherwise independent.

---

### PR 3 — JS `enableOutbox` (`src/lib/wiki.ts`)

**Scope:**
- Add `enableOutbox: true` to `createWiki` config
- Verify existing `wiki.test.ts` passes
- No new tests required (behaviour owned by `core-llm-wiki`)

**Fully independent.** No Rust changes. Ships any time.

---

### PR 4 — Tauri wiring + integration (depends on PR 1 + PR 2)

**Scope:**
- `lib.rs`: `OutboxWorkerState`, `DbPathState`, `start_outbox_worker`, `stop_outbox_worker` commands
- Wire `OutboxWorker` + `PgSink` together inside commands
- Integration test: real SQLite + real Postgres (env-var gated)
- `outbox-worker-error` Tauri event emission

---

## Constraints

- Single worker per SQLite file. No row-level locking. Documented in code and README.
- `mapEvent` equivalent is a fixed generic passthrough — not pluggable at runtime. Enterprise projections live in Postgres, not in Rust.
- Worker is opt-in. No impact on users who do not call `start_outbox_worker`.
- `serde_json::Value` used for payload — no schema enforcement in Rust.
