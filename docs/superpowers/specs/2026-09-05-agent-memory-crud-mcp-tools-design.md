# Agent-memory CRUD tools on the main MCP server — design

- Date: 2026-09-05
- Status: Approved for implementation
- Scope: curated-thoughts `src-tauri` (main `--mcp` server + `tool_dispatch`)

## Problem

The main binary's MCP server (`/usr/bin/curated-thoughts --mcp`, the one Hermes
and other agents attach to) exposes only 8 read-mostly vault/wiki tools. The
coding-focused MCP server (`tools/src/bin/curated_thoughts_mcp.rs`, PR #137)
implements the `curated_*` memory tools but is a separate binary that agents
rarely have wired up. `curated_add_wisdom` is referenced in its setup
instructions but was never implemented anywhere. Agents therefore cannot
persist learned wisdom through the primary MCP surface.

## Goal

Expose a complete memory CRUD surface on the main MCP server:

| Tool | Status | Source |
|---|---|---|
| `curated_recall_context` | port | coding server (read: wiki + ast code chunks) |
| `curated_get_wiki_entry` | port | coding server (read: full entry body) |
| `curated_search_code` | port | coding server (read: ast code chunks) |
| `curated_add_wisdom` | new | wraps `db::facts::add_fact_with_profile` |
| `curated_update_wisdom` | new | wraps `db::facts::update_fact_with_profile` |
| `curated_archive_wisdom` | new | wraps `db::facts::archive_fact` (soft delete) |

Out of scope: `graph_neighbors` (code-graph, separate concern),
`curated_superpowers_setup` (editor-specific scaffolding).

## Non-goals / invariants

- GUI and CLI behavior unchanged; Tauri command surface untouched.
- All DB writes go through the existing `db::facts` core — same
  `MANUAL_SOURCE_REF` JSON shape (`{"proposal_id":null,"evidence":[]}`),
  `source_type='user_stated'`, ms timestamps, outbox rows, entity touch.
- The readonly connection used by the existing 8 tools stays readonly; writes
  use a separate lazily-opened RW connection.
- All `#[tool]` handlers remain thin: params struct in `tool_dispatch.rs`,
  dispatch fn in `tool_dispatch.rs`, `#[tool]` wrapper in `mcp_server.rs`
  (3-location pattern, see skill reference mcp-write-patterns).
- Embedding calls (blocking network) never run while holding a DB lock —
  `precompute_entry_embedding` exists for exactly this.

## Design

### Read path (ports)

The coding server's helpers (`fetch_ranked_chunks`, `rank_wiki_entries`,
`RECALL_CHUNKS_SQL_BASE`, `RECALL_CHUNKS_AST_FILTER`, row→json mappers) move
into `tool_dispatch.rs` (or a `tool_dispatch` submodule `curated_memory.rs`)
so both servers can eventually share them; the coding binary is NOT modified
in this PR (follow-up dedup later, no behavior change now).

Dispatchers:

- `dispatch_curated_recall_context(ctx, query, limit_wiki, limit_code)` →
  `{wiki_entries, code_chunks, query}`
- `dispatch_curated_get_wiki_entry(ctx, topic?, entity_id?)` →
  `{full_text, chunks, …}` (at least one of topic/entity_id required)
- `dispatch_curated_search_code(ctx, query, limit, symbol?)` →
  `{code_chunks, query, symbol_filter}`

### Write path (new)

`ToolDispatchContext` gains `db_path: PathBuf` (already known from
`resolve_brain_paths()` in `mcp_server::async_run`) and a lazy RW connection:

```rust
// inside dispatch, behind the existing conn mutex pattern
fn open_rw_if_needed(&self) -> Result<MutexGuard<Connection>> // opens once, caches
```

- RW connection opens with `SQLITE_OPEN_READWRITE | NO_MUTEX` +
  `busy_timeout(5s)`; mirrors the GUI's open flags. If the DB file is missing,
  the tool returns a clear error (never creates a brain).
- `dispatch_curated_add_wisdom(entity_id, body, profile)`:
  1. `precompute_entry_embedding(Some(profile), body)` OUTSIDE the lock
  2. lock RW conn → `add_fact_with_profile(conn, entity_id, body, blob)`
  3. return the new fact JSON (`id`, `entity_id`, `title`, `body`)
  - Errors if entity not found/archived (core already bails with a clear msg).
- `dispatch_curated_update_wisdom(entity_id, fact_id, body, profile)`: same
  shape via `update_fact_with_profile`; returns updated fact.
- `dispatch_curated_archive_wisdom(entity_id, fact_id)`: wraps
  `archive_fact`; returns `{archived: true, fact_id}`.

### MCP registration

Six `#[tool]` handlers in `mcp_server.rs`, same error mapping as existing
(`retrieval::mcp_error_hint`). Descriptions follow the coding server's text,
plus explicit "writes to the live brain" wording on the three mutators.

### Access logging

`log_agent_access` rows for each call, `client` = "local-mcp" (existing ctx).

## Testing

- Unit (`tool_dispatch` module): recall/get/search against an in-memory DB
  seeded with wiki entries + ast chunks (port the coding server's fixtures);
  add/update/archive happy path + entity-missing + fact-missing errors.
- Integration (`src-tauri/tests/mcp_integration.rs`, feature-gated as today):
  assert `tools/list` now includes the six `curated_*` names; call
  `curated_add_wisdom` + `curated_get_wiki_entry` round-trip on a temp brain.
- Guards: existing readonly tools unchanged (their tests keep passing).

## Risks

- Write access from any MCP client raises the stakes on the brain file —
  mitigated by reusing the audited `db::facts` writers (outbox, ms, source_ref
  contract) rather than raw SQL.
- Local GUI running concurrently: SQLite busy-timeout handles brief lock
  contention; the GUI already tolerates external writers (outbox pattern).

## Docs

This spec + the plan ride the PR (docs-ride-their-PRs rule). The MCP product
page section "Tools (vault/wiki graph set)" gains the curated_* list.
