# MCP server + retrieval façade (agent use)

**Date:** 2026-05-08  
**Status:** Implemented  
**Depends on:** Existing `search::{semantic_search, related_chunks}` and `VaultConfig` (`config.json`) + `brain.db` layout (`2026-05-07-v2-code-rag-chunking-design.md` alignment).

## 1. Summary

Agents (Cursor, Copilot, etc.) should query the **same retrieval semantics** as the desktop app—not a forked SQL layer or duplicated embed logic. This spec merges **two tracks**:

- **Track 2 — Retrieval façade:** A small Rust API that initializes from explicit paths or env, reads `EmbedProfile`, runs `semantic_search` / `related_chunks`, and returns **`SearchResult`**. Both **Tauri commands** (`search_vault`, `get_related_chunks`) and **MCP tool handlers** call only this façade.
- **Track 1 — MCP server:** A **stdio MCP** binary (v0 developer distribution) exposing tools that delegate to the façade.

## 2. Goals and non-goals

### Goals

- **Semantic parity:** MCP tool outputs match **`search::SearchResult`** fields and meanings used by Tauri (`doc_path`, `chunk_text`, `chunk_position`, `score`, `start_line`, `end_line`, `symbol_name`, `strategy`).
- **Single code path:** No second copy of “how to open DB”, “which embed profile”, or “how to embed query text” outside the façade + shared embedder helpers.
- **Works with app closed:** MCP opens **`brain.db` read-only** and reads **`config.json`** for `embed_profile`; no requirement that Tauri UI is running.
- **Inspectable v0 ops:** Logging (stderr only; never MCP stdout) acceptable for MVP.

### Non-goals (v0)

- MCP tools for **ingestion**, wiki approval, vault mutation, or re-embed pipelines.
- **Packaged** distribution for end users (no VSIX / Open VSX / signed installer requirement in this spec); `cargo run` or ad-hoc binary is enough.
- **Remote** MCP or authenticated multi-tenant MCP.
- **Settings UI** for MCP-specific options (see §4 roadmap note).

## 3. Architecture

```
┌─────────────────────────────────────────────────────────────┐
│  embedder::{embed_one, EmbedProfile}  +  VaultConfig read   │
└───────────────────────────┬─────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│  retrieval facade (NEW)                                      │
│  - open rusqlite::Connection READONLY to brain.db            │
│  - load EmbedProfile from same config.json as desktop app    │
│  - search_vault_facade(query, limit) → Vec<SearchResult>      │
│  - related_chunks_facade(doc_path, limit) → Vec<SearchResult> │
└───────────────┬─────────────────────────────┬───────────────┘
                │                             │
                ▼                             ▼
┌───────────────────────┐         ┌──────────────────────────┐
│  Tauri lib.rs          │         │  mcp_stdio binary (NEW) │
│  (thin wrappers only) │         │  tools → facade          │
└───────────────────────┘         └──────────────────────────┘
```

- **`search::semantic_search`** and **`search::related_chunks`** remain the low-level primitives; the façade wraps **connection/profile acquisition** and **`embed_one` for queries** so Tauri handlers shrink to acquiring state vs env-free façade entry points.
- Naming suggestion: module `crate::retrieval` or `crate::agent_retrieval` with `pub struct RetrievalContext { conn: Connection, profile: EmbedProfile }` (exact shape left to implementation plan).

## 4. Configuration (v0 env / CLI — Settings UI later)

**Principle:** Default to **the same filesystem layout as production** (`~/.brain/brain.db` + `~/.brain/config.json` as today’s app bootstrap), overridable for CI and agents.

### v0 mechanisms (implement all)

| Variable / flag | Purpose |
|-----------------|--------|
| `CURATED_BRAIN_DIR` | Directory containing **`brain.db`** and **`config.json`** (brain home). Default: **`$HOME/.brain`** (mirror `lib.rs`). |
| Optional override | **`CURATED_BRAIN_DB`** — explicit DB file path when tests or power users split DB from dir (must still supply embed profile via `CURATED_BRAIN_CONFIG` or co-located `config.json` next to DB parent). Prefer **single `CURATED_BRAIN_DIR`** as the documented happy path. |
| **`CURATED_BRAIN_CONFIG`** | Optional explicit path to **`config.json`** when it does not live beside `brain.db`. |

**Embed profile:** Read via existing **`VaultConfig::new(config_path).get_embed_profile()`** semantics so MCP and GUI never drift.

### Roadmap — user Settings UI

- Later: persisted “brain directory” picker in-app already touches `config.json`; MCP should continue to consume **the same file** rather than introducing a parallel MCP-only store.
- Optional future: **`curated.toml` or MCP section inside `config.json`** (explicit allow-list of paths, max `limit`). Out of scope for v0 unless security review demands it sooner.

## 5. MCP tools (v0)

Two tools mirror Tauri commands:

| Tool name | Inputs | Behavior |
|-----------|--------|----------|
| **`vault_semantic_search`** | `query: string`, `limit?: number` (default 10, clamp 1–50) | `embed_one(profile, query)` then `semantic_search(&conn, &vec, limit)` |
| **`vault_related_chunks`** | `doc_path: string`, `limit?: number` (default 5, clamp 1–10) | `related_chunks(&conn, &doc_path, limit)` |

**Response payload:** JSON array of **`SearchResult`** structs (serialized with serde), identical field names/types to **`search/mod.rs`** and what the frontend receives.

**Errors:** MCP tool errors surface as **`Result`/`anyhow` strings**; include a short actionable hint (“check CURATED_BRAIN_DIR”, “no embeddings for vault”) — exact MCP error mapping left to MCP SDK conventions in the plan phase.

## 6. SearchResult parity (mandatory contract)

Agents and UI must interpret rows the same way.

- **Stable fields:** Exactly `SearchResult` in `src-tauri/src/search/mod.rs` (`doc_path`, `chunk_text`, `chunk_position`, `score`, `start_line`, `end_line`, `symbol_name`, `strategy`).
- **Strategy strings:** Continue using DB string values already written at ingest (e.g. `ast_symbol_rust`, `scanner`). No MCP-specific renaming.
- **Doc paths:** Absolute or vault-relative consistency must match what **`documents.path`** stores today; MCP returns **verbatim** paths from SQLite (same as `search_vault`).

Any future change to `SearchResult` is a **semver / migration decision** affecting Tauri IPC, MCP, and TS types together.

## 7. Security model (explicit)

- **Trust boundary:** Installing or running the MCP binary grants **any process that invokes it read access** to indexed chunk text visible in **`SearchResult`** (includes source `chunk_text`; treat as confidential as the vault).
- **Local-only:** v0 binds to **stdio** only; no network listener.
- **Read-only DB:** Open SQLite with **`OpenFlags::SQLITE_OPEN_READ_ONLY`** where the driver allows, to reduce accidental WAL side effects when the desktop app concurrently opens the DB (document caveat: SQLite locking may still block or error if incompatible modes—plan should cite platform behavior).

## 8. Testing

- **Unit / integration (Rust):**  
  - Build `RetrievalContext` against a **`tempdir`** with `brain.db` + **`config.json`** mirroring **`make_test_app`** patterns (`lib.rs` test-utils).  
  - Seed minimal `documents`, `chunks`, `embeddings` rows (or reuse helpers if present).
  - **`vault_semantic_search` façade path:** deterministic query embedding or inject fixed query vector **only through a test seam** if `embed_one` is non-deterministic across machines—for strict assertions, optional `#[cfg(test)]` embedding stub or fixture vector of known dimension documented in plan.
  - **`vault_related_chunks`:** Same fixture; assert ordering stable enough for tests (`score` desc).
- **MCP wiring smoke:** Spawn stdio MCP in process, call **`initialize`** + **`tools/call`** once per tool in CI **optional** for v1 if MCP SDK mocking is heavy; minimum bar is façade tests + thin handler unit tests calling façade.

## 9. Deliverables checklist (implementation plan inputs)

1. **`retrieval` façade module** (+ thin Tauri refactor using it).
2. **New bin crate** under workspace (or `[[bin]]` in existing package) **`curated-thoughts-mcp`** — depends on **`tauri_app_lib`** (`curated-thoughts` lib) **without** linking Tauri runtime.
3. **Env parsing** helpers (small, documented in README MCP section).
4. **Docs:** `README` or **`src-tauri/tests/README.md`** snippet for Cursor MCP config JSON pointing at compiled binary + env.
5. **No `git add -A`** in plans; explicit file lists.

## 10. Out of scope / follow-ons

- HTTP/SSE MCP transport.
- **`resource` / `prompt` templates beyond tools.
- Per-tool RBAC or secret tokens (revisit if MCP runs in shared shells).
