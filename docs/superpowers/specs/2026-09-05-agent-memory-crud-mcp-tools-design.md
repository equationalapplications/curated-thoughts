# Spec: Agent-memory CRUD tools on the main MCP server

**Status:** Implemented 2026-09-05 (PR #185).
**Date:** 2026-09-05
**Baseline:** `main` @ `2bf1c189` (v2.4.3)
**Scope:** Curated Thoughts `src-tauri` only (main `--mcp` server +
`tool_dispatch`). No GUI/CLI behavior changes.

## §1 — Problem

The main binary's MCP server (`/usr/bin/curated-thoughts --mcp`, the one Hermes
and other agents attach to) exposes only 8 read-mostly vault/wiki tools. The
coding-focused MCP server (`tools/src/bin/curated_thoughts_mcp.rs`, PR #137)
implements the `curated_*` memory tools but is a separate binary that agents
rarely have wired up. `curated_add_wisdom` is referenced in its setup
instructions but was never implemented anywhere. Agents therefore cannot
persist learned wisdom through the primary MCP surface.

## §2 — Tool surface

| Tool | Status | Source |
|---|---|---|
| `curated_recall_context` | port | coding server (read: wiki + ast code chunks) |
| `curated_get_wiki_entry` | port | coding server (read: full entry body) |
| `curated_search_code` | port | coding server (read: ast code chunks) |
| `curated_add_wisdom` | new | wraps `db::wisdom::add_wisdom_with_blob` |
| `curated_update_wisdom` | new | wraps `db::wisdom::update_wisdom_with_blob` |
| `curated_archive_wisdom` | new | wraps `db::wisdom::archive_wisdom` (soft delete) |

Out of scope: `graph_neighbors` (code-graph, separate concern),
`curated_superpowers_setup` (editor-specific scaffolding).

## §3 — Invariants

**Terminology alignment (domain architecture):** in EA's domain, **Facts**
are immutable reference files (the vault documents tier) while **Wisdom** is
the mutable, LLM-curated graph (the `llm_wiki_entries` layer). The existing
backend module `db::facts` writes exclusively to that wisdom layer, so its
name has been a misnomer; the implementation PR RENAMES `db::facts` →
`db::wisdom` and its functions accordingly (`add_fact_with_blob` →
`add_wisdom_with_blob`, `update_fact_with_blob` → `update_wisdom_with_blob`,
`archive_fact` → `archive_wisdom`, `EntityFact` → `EntityWisdom`), with the
spec's references already using the new nomenclature. This is a mechanical
rename of module/function identifiers only — table names (`llm_wiki_entries`),
column names, and the outbox format are unchanged.

- GUI and CLI behavior unchanged; Tauri command surface untouched.
- All DB writes go through the existing `db::wisdom` core (renamed from `db::facts` in the implementation PR) — same
  `MANUAL_SOURCE_REF` JSON shape (`{"proposal_id":null,"evidence":[]}`),
  `source_type='user_stated'`, ms timestamps, outbox rows, entity touch.
  NO raw INSERT/UPDATE/DELETE against `llm_wiki_entries` in this work.
- The readonly connection used by the existing 8 tools stays readonly; writes
  use a separate lazily-opened RW connection.
- All `#[tool]` handlers remain thin: params struct in `tool_dispatch.rs`,
  dispatch fn in `tool_dispatch.rs`, `#[tool]` wrapper in `mcp_server.rs`
  (3-location pattern — the in-repo precedent is the existing
  `vault_write_note` wiring across `tool_dispatch.rs` + `mcp_server.rs`).
- Embedding calls (blocking network) never run while holding a DB lock —
  `precompute_entry_embedding` exists for exactly this.

## §4 — Read path (ports)

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

## §5 — Write path (new)

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
  2. lock RW conn → `wisdom::add_wisdom_with_blob(conn, entity_id, body, blob)`
     (the `_with_blob` variants are the API — the caller precomputes the
     blob; `*_with_profile` wrappers are for callers holding a profile ref)
  3. return the new wisdom-entry JSON (`id`, `entity_id`, `title`, `body`)
  - Errors if entity not found/archived (core already bails with a clear msg).
- `dispatch_curated_update_wisdom(entity_id, wisdom_id, body, profile)`: same
  shape via `wisdom::update_wisdom_with_blob(conn, entity_id, wisdom_id, body, blob)`.
  NOTE: `update_wisdom_with_blob` returns `Result<()>` — after the update
  transaction commits, RELOAD the wisdom entry (SELECT by entity_id + wisdom_id; reuse
  the coding server's per-entry query shape) and return its JSON; never
  fabricate the response from the request. Signature note: `wisdom::*` writers
  take `&mut Connection`; call sites hold the lazy RW `MutexGuard` and pass
  `&mut *guard`.
- `dispatch_curated_archive_wisdom(entity_id, wisdom_id)`: wraps
  `archive_wisdom`; returns `{archived: true, wisdom_id}`.

## §6 — Identifier collision (`curated_get_wiki_entry`)

If BOTH `topic` and `entity_id` are supplied, `entity_id` takes precedence and
`topic` is ignored — matching the PR #137 coding server's existing behavior
(its `if let Some(entity_id)` branch runs first). This precedence is stated in
the tool description so agents learn the contract without round-tripping.

## §7 — Access logging

`log_agent_access` performs an INSERT; the readonly connection cannot service
it (today's `let _` swallow means read-tool logging is silently a no-op). All
six curated tools route their access-log write through the lazy RW connection,
`client` = `"local-mcp"` (existing ctx), and a failed log write FAILS the tool
call — audit logs are never bypassed (best-effort is explicitly rejected).

**SUPERSEDES existing policy — implementers must update the callers, not just
add new code.** `tool_dispatch.rs` currently documents the OPPOSITE rule in
two places: `log_agent_access`'s doc comment ("Best-effort audit log … A
failed log write must never fail the tool call", ~line 435) and the
`dispatch_tool_call` log block ("best-effort, never fail the tool call",
~line 578). Both comments AND the shared `log_agent_access` helper must be
rewritten in this PR so the code no longer contradicts this spec; the shared
helper becomes fail-closed for the curated tools (its INSERT result is
checked, not `let _`-swallowed). The existing 8 non-curated tools keep their
current behavior unchanged in this PR; migrating them to fail-closed logging
is a separate, explicitly-scoped follow-up so this PR stays
backward-compatible for existing clients.

## §8 — MCP registration

Six `#[tool]` handlers in `mcp_server.rs`, same error mapping as existing
(`retrieval::mcp_error_hint`). Descriptions follow the coding server's text,
plus explicit "writes to the live brain" wording on the three mutators, plus
the §6 precedence note on `curated_get_wiki_entry`.

## §9 — Testing

- **Test DB isolation (required):** tool_dispatch unit tests that exercise
  BOTH the RO and RW connections must NOT use bare `Connection::open_in_memory()`
  (each in-memory DB is private to its single connection — the RW connection
  would see an empty database). Two accepted patterns:
  1. shared-cache URI opened by both connections:
     `Connection::open("file::memory:?cache=shared")` with
     `sqlite_open_flags(SQLITE_OPEN_URI | SQLITE_OPEN_READ_WRITE)`; or
  2. a `TempDir`-backed file DB (preferred for write-path tests — also
     exercises the real RW-open-and-cache path).
  Read-only helpers may keep `open_in_memory()` when only one connection is involved.
- Unit (`tool_dispatch` module): recall/get/search against a seeded shared or
  file-backed DB with wiki entries + ast chunks (port the coding server's
  fixtures); add/update/archive happy path + entity-missing + wisdom-entry-missing errors.
- Integration (`src-tauri/tests/mcp_integration.rs`, feature-gated as today):
  assert `tools/list` now includes the six `curated_*` names; then seed an
  ACTIVE entity in the temp brain, capture its `entity_id`, and pass it to
  `curated_add_wisdom` (the core bails on non-existent/archived entities —
  never call write tools without a seeded active entity), followed by a
  `curated_get_wiki_entry` round-trip.
- Log-failure test: force `curated_agent_log` to be unwritable (e.g. drop the
  table on the test DB) and assert the curated tool call FAILS, proving
  audit-log writes are never silently skipped.
- Guards: existing readonly tools unchanged (their tests keep passing).

## §10 — Risks

- Write access from any MCP client raises the stakes on the brain file —
  mitigated by reusing the audited `db::wisdom` writers (outbox, ms, source_ref
  contract) rather than raw SQL.
- Local GUI running concurrently: SQLite busy-timeout handles brief lock
  contention; the GUI already tolerates external writers (outbox pattern).

## §11 — Docs

The MCP product page section "Tools (vault/wiki graph set)" gains the
curated_* list after implementation merges.
