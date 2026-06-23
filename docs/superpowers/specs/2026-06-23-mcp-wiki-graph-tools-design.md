# MCP wiki graph tools (wiki_search, wiki_get_ontology, wiki_traverse_graph)

**Date:** 2026-06-23
**Status:** Proposed
**Depends on:** `2026-05-07-mcp-retrieval-facade-design.md` (single-code-path principle, stdio MCP server), `2026-05-23-unified-mcp-binary-spec.md` (one binary, `--mcp` flag)

## 1. Summary

`@equationalapplications/react-llm-wiki` / `core-llm-wiki` v4.17.0 ships JS tool manifests (`wikiGetOntologyManifest`, `wikiTraverseGraphManifest` from `core-llm-tools`) describing a knowledge-graph traversal capability over the "Active Librarian" memory (facts + relationships extracted from the vault). Curated Thoughts' MCP server is pure Rust (`rmcp`), with no JS runtime in the `--mcp` process — so these manifests cannot be imported directly. This spec adds **3 new Rust-native MCP tools** that read the same SQLite tables `core-llm-wiki` already creates and maintains in `brain.db`, giving Cursor/Claude Desktop/other MCP clients read access to the semantic memory graph, not just the code/doc chunk index.

This is read-only, additive work: no new tables, no JS dependency, no mutation path.

## 2. Background — verified architecture facts

- `react-llm-wiki`'s `SQLiteAdapter` (`src/lib/wikiAdapter.ts`) executes raw SQL via Tauri commands `wiki_exec`/`wiki_run`/`wiki_get_all`/`wiki_get_first` against the **same `brain.db`** the Rust app owns. `core-llm-wiki` migrations create these tables there (default prefix `llm_wiki_`):
  - `llm_wiki_entries` — fact nodes: `id, entity_id, title, body, tags, confidence, source_type, source_hash, source_ref, created_at, updated_at, last_accessed_at, access_count, deleted_at, embedding, embedding_blob, okf_type`. `embedding_blob` is a `Float32Array` BLOB (4 bytes/dim, same little-endian layout as Rust's `chunks`/`embeddings` blobs in `src-tauri/src/db/queries.rs:81` and `src-tauri/src/search/mod.rs:127`).
  - `llm_wiki_edges` — `id, entity_id, source_id, target_id, edge_type, created_at`. **No `deleted_at` column** — edges are hard-deleted.
  - `llm_wiki_entity_manifests` — `entity_id, mode, manifest_json, updated_at`. `manifest_json` default `{"node_types":[],"edge_types":[]}`.
- Rust already touches `llm_wiki_entries` directly (`heal_invalid_sources` in `src-tauri/src/lib.rs`), confirming direct SQL access from Rust is an established pattern, not a new risk.
- `entity_id` is per-tier, not a single fixed value: `tier_fact` (documents/), `tier_wisdom` (wiki/), or a workspace-specific value defaulting to `tier_working::default` (`src/lib/wikiTiers.ts`, `src/lib/wiki.ts:10`). The app's own `tieredRead` (`src/lib/wiki.ts:127-145`) weights these `tier_fact: 1.5, tier_wisdom: 1.0, working: 0.6` — the new `wiki_search` tool mirrors this.
- Upstream manifest schemas (verified against `core-llm-tools@4.17.0` dist):
  - `wiki_get_ontology`: `required: ["entityId"]`.
  - `wiki_traverse_graph`: `properties: { entityId, sourceId, maxDepth (1-3), direction (inbound|outbound|both, default both), edgeTypes }`, `required: ["entityId", "sourceId"]` — **`maxDepth` is optional**, no upstream default specified (ours: default `2`).
  - Upstream `getOntologyManifest(entityId)` (from `core-llm-wiki` types) returns `{ mode, manifest: { node_types, edge_types } } | null`. We nest under `manifest` to match, but substitute `{ mode: "off", manifest: null }` for the no-row case (our own choice — bare `null` is an awkward MCP return; not an upstream fact).
- There is no existing tool that returns an `llm_wiki_entries.id` to an external agent — `wiki_traverse_graph`'s `sourceId` needs a seed, so this spec also adds `wiki_search` (not from any upstream manifest; designed here to close that gap).

## 3. Architecture

```
VaultMcpServer (src-tauri/src/mcp_server.rs)
 ├─ vault_semantic_search / vault_related_chunks   (existing — chunks/embeddings tables)
 └─ wiki_search / wiki_get_ontology / wiki_traverse_graph   (NEW — src-tauri/src/wiki_graph.rs)
                          │
                          ▼
        brain.db (read-only connection; same file the Tauri app + JS wiki adapter write to)
        tables: llm_wiki_entries, llm_wiki_edges, llm_wiki_entity_manifests
```

New module `src-tauri/src/wiki_graph.rs` owns the 3 query functions and their SQL, separate from `retrieval/mod.rs` (which stays focused on code/doc chunk retrieval — different domain, different tables). `mcp_server.rs` registers the 3 functions as `#[tool]` handlers on the same `VaultMcpServer`, same stdio process started by `--mcp` (no second binary, no second process — consistent with the unified-mcp-binary decision).

## 4. Tool contracts

### `wiki_search` (new — no upstream manifest)
- **params:** `query: string` (required), `entityIds?: string[]` (default `["tier_fact", "tier_wisdom"]`), `limit?: integer` (default 10, max 25)
- **behavior:** embed `query` via existing `embedder::embed_one(profile, text)`; cosine-similarity (`search::cosine_similarity`) against `llm_wiki_entries.embedding_blob` for rows where `entity_id IN entityIds AND deleted_at IS NULL`; multiply by tier weight (`tier_fact: 1.5`, `tier_wisdom: 1.0`, any other explicit `entity_id`: `1.0`) before sorting — mirrors `tieredRead` (`src/lib/wiki.ts:127`).
- **dimension guard:** skip rows where `length(embedding_blob) / 4 != active profile dim` (same defensive check `core-llm-wiki` already does for healing) — never errors the whole call.
- **returns:** `[{ id, entity_id, title, score }]` — no body, keeps payload small; agent calls `wiki_traverse_graph` next with the `id`.

### `wiki_get_ontology` (mirrors `wikiGetOntologyManifest`)
- **params:** `entityId: string` (required)
- **query:** `SELECT mode, manifest_json FROM llm_wiki_entity_manifests WHERE entity_id = ?`
- **returns:** `{ mode, manifest: { node_types: [...], edge_types: [...] } }` parsed from `manifest_json`; or `{ mode: "off", manifest: null }` if no row.

### `wiki_traverse_graph` (mirrors `wikiTraverseGraphManifest`)
- **params:** `entityId` (required), `sourceId` (required), `maxDepth?: 1-3` (default `2`, clamp out-of-range rather than error), `direction?: inbound|outbound|both` (default `both`), `edgeTypes?: string[]`
- **behavior:** BFS over `llm_wiki_edges` (`source_id`/`target_id`/`edge_type`) up to `maxDepth` hops, filtered by `entity_id` and optional `edgeTypes`. Joins to `llm_wiki_entries` for node titles; drops any edge whose endpoint entry has `deleted_at IS NOT NULL`. The `deleted_at IS NULL` filter applies **only** to `llm_wiki_entries` — `llm_wiki_edges` has no such column.
- **returns:** `{ nodes: [{id, title, entity_id}], edges: [{source_id, target_id, edge_type}] }`

## 5. Error handling & edge cases

- Unknown `entityId`/`sourceId` in `wiki_traverse_graph` → empty `nodes`/`edges`, not an error (matches `vault_related_chunks` precedent).
- Embedder unavailable (Ollama down) for `wiki_search` → `rmcp::ErrorData`, same mapping `vault_semantic_search` already uses.
- `maxDepth` outside 1-3 → clamp, log clamp to stderr only (stdout hygiene rule from unified-mcp-binary spec; MCP stdout must stay pure JSON-RPC).
- Deleted entries (`llm_wiki_entries.deleted_at IS NOT NULL`) excluded everywhere entries are read; `llm_wiki_edges` has no soft-delete, so edge filtering happens via its joined entry endpoints only.
- No ontology manifest row → not an error, returns `{ mode: "off", manifest: null }`.

## 6. Testing

- New `src-tauri/tests/wiki_graph.rs`, seeding `llm_wiki_entries`/`llm_wiki_edges`/`llm_wiki_entity_manifests` via direct SQL (same style as `wiki_maintenance.rs`/`folder_rules.rs`), no JS involved.
- Cases: `wiki_search` tier-weight ordering, dimension-mismatch skip, no-match empty result; `wiki_get_ontology` present/absent row; `wiki_traverse_graph` multi-hop BFS, direction filter, `edgeTypes` filter, soft-deleted-node exclusion, `maxDepth` clamp.
- Extend `src-tauri/tests/mcp_integration.rs` with end-to-end stdio calls for the 3 new tools, mirroring the existing `vault_semantic_search` integration test.

## 7. Non-goals

- No write/mutation tools (no `wiki_write`, no ontology mutation via MCP).
- No JS runtime in the `--mcp` process — all 3 tools are pure Rust/SQL.
- No visual graph UI, ontology settings UI, OKF export, or background-status UI — those are separate candidate specs (not in scope here).
