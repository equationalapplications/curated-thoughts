# Phase 2: Structural Awareness & Code Graph Integration — Design Spec

**Date:** 2026-05-13
**Status:** Implemented
**Depends on:** Phase 1 (Three-Tier Memory Foundation) — `kv/fixes`
**Stack:** Tauri 2.x (Rust), React 19 frontend, `@equationalapplications/core-llm-wiki` v3.3.0+, tree-sitter

---

## Overview

Transition the retrieval model from a "List of Chunks" to a **Network of Knowledge** by implementing the code graph pattern. Phase 1 gave every chunk a tier-aware weight; Phase 2 gives every chunk structural *edges* — explicit relationships (`CALLS`, `IMPORTS`, `IMPLEMENTS`) between chunks — enabling the Active Librarian to reason about impact radius and architectural consistency across the entire vault.

---

## Problem

Phase 1 retrieval is **semantically aware but structurally blind**. Two failure modes remain:

1. **Invisible dependencies** — A query for `init_db` returns the function's own chunk but not the call sites that will break if its signature changes. The Librarian cannot propose a "breaking change" Wisdom entry because it cannot see the callers.
2. **Orphaned contradictions** — A security policy in `documents/security_policy.pdf` (Fact) may forbid an anti-pattern used in several files. The Librarian detects the violation in one file but cannot enumerate every file that repeats it — they are not linked.

The root cause in both cases: the SQLite database stores *chunks* but no *edges between chunks*. Retrieval is a flat list; the knowledge graph is implicit only.

---

## Architecture: The Code Graph Pattern

```
              ┌──────────────────────────────────────────┐
              │           curated_relationships           │
              │  from_id → to_id    rel_type    symbol    │
              └─────────────────┬────────────────────────┘
                                │
     ┌──────────────────────────┼──────────────────────────┐
     ▼                          ▼                          ▼
[chunk: init_db]         [chunk: call site]        [chunk: security §4]
defined_symbol=init_db    symbol_name=init_db        tier=tier_fact
tier=tier_working         defined_symbol=NULL
```

The Linker (Pass 3) writes an edge `(call_site_id → init_db_id, CALLS, "init_db")`. The retrieval layer walks this edge in either direction: downstream to find the definition, upstream to enumerate all callers.

---

## Component 1: Schema Extension — `curated_relationships` Table

**Layer:** Rust / SQLite (same database file as `curated_chunks`)
**Migration:** Schema V5

```sql
CREATE TABLE IF NOT EXISTS curated_relationships (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    from_id     TEXT    NOT NULL,   -- chunk id of the call site / importer / implementor
    to_id       TEXT    NOT NULL,   -- chunk id of the definition / module / interface
    rel_type    TEXT    NOT NULL,   -- 'CALLS' | 'IMPORTS' | 'IMPLEMENTS'
    symbol      TEXT    NOT NULL,   -- function/class name (normalised, lowercase)
    entity_id   TEXT    NOT NULL,   -- vault namespace; must match both from_id and to_id
    created_at  INTEGER NOT NULL DEFAULT (unixepoch())
);

-- High-performance symbol lookup for the Global Resolver (Pass 3)
CREATE INDEX IF NOT EXISTS idx_rel_symbol
    ON curated_relationships (symbol, entity_id);

-- Caller traversal: "who calls this chunk?"
CREATE INDEX IF NOT EXISTS idx_rel_to_id
    ON curated_relationships (to_id, entity_id);

-- Callee traversal: "what does this chunk call?"
CREATE INDEX IF NOT EXISTS idx_rel_from_id
    ON curated_relationships (from_id, entity_id);
```

**Schema V5 guard:** the migration runner (`src-tauri/src/db/migrations.rs`) checks `PRAGMA user_version` before applying. V5 adds the table and indices; no existing columns are altered.

---

## Component 2: `defined_symbol` Column on `curated_chunks`

**Layer:** Rust / SQLite (Schema V5, same migration as §Component 1)

```sql
ALTER TABLE curated_chunks
    ADD COLUMN defined_symbol TEXT DEFAULT NULL;

-- Partial index: only index rows that are definitions, not references.
CREATE INDEX IF NOT EXISTS idx_chunks_defined_symbol
    ON curated_chunks (defined_symbol, entity_id)
    WHERE defined_symbol IS NOT NULL;
```

`defined_symbol` is non-null **only** for chunks that are the authoritative *definition* of a symbol (function body, class declaration, interface). Chunks that merely *reference* a symbol (call sites, import statements) leave `defined_symbol` NULL. The existing `symbol_name` column (schema V4) continues to hold the *referenced* symbol name for call sites.

---

## Component 3: Tree-sitter Multi-pass Indexing

**Layer:** Rust (`src-tauri/src/indexer/`)

The existing single-pass indexer is refactored into three ordered passes. Passes 1 and 2 are file-local and run in the same transaction as text chunking. Pass 3 is a cross-file background job.

### Pass 1 — Definition Extraction

For each source file, execute tree-sitter queries that match symbol *definitions* and populate `defined_symbol`:

```rust
// Language-generic — adapted per parser registration.
// Rust:       function_item, struct_item, impl_item, trait_item
// TypeScript: function_declaration, class_declaration, interface_declaration
// Python:     function_definition, class_definition

let def_query = r#"
    [
      (function_item  name: (identifier)      @def.name)
      (struct_item    name: (type_identifier) @def.name)
      (trait_item     name: (type_identifier) @def.name)
      (impl_item trait: (type_identifier)?    @def.name)
    ]
"#;

// For each match: upsert into curated_chunks with
//   defined_symbol = LOWER(TRIM(def.name))
//   symbol_name    = NULL  (definitions do not reference another symbol)
```

**Normalisation rule:** `defined_symbol` is stored as `lowercase(trimmed(name))`. This enables case-insensitive resolution without altering the raw source text stored in `content`.

### Pass 2 — Reference Extraction

Scan each file for call sites and import statements. Store the referenced symbol in `symbol_name`; leave `defined_symbol` NULL:

```rust
let ref_query = r#"
    [
      (call_expression
         function: (identifier)                          @ref.name)
      (call_expression
         function: (field_expression
           field: (field_identifier)                     @ref.name))
      (use_declaration
         argument: (scoped_identifier
           name: (identifier)                            @ref.name))
    ]
"#;

// For each match: upsert into curated_chunks with
//   symbol_name    = LOWER(TRIM(ref.name))
//   defined_symbol = NULL
```

Both passes write chunk rows in the same transaction. A file re-index **deletes all existing chunks** (and then triggers stale edge cleanup — see §Pitfalls) before re-running passes 1 and 2.

### Pass 3 — The Linker (Global Resolver)

A **background job** that runs after all files in a vault have been indexed (triggered on completion of the watcher's initial scan and after each batch of file-change events). The Linker is **entity-scoped**: it only resolves symbols within the same `entity_id`, preventing cross-vault contamination.

```rust
// For every reference chunk (defined_symbol IS NULL, symbol_name IS NOT NULL),
// find the definition chunk in the same entity_id:
const RESOLVER_SQL: &str = r#"
    SELECT ref.id          AS ref_chunk_id,
           ref.symbol_name,
           ref.entity_id,
           def.id          AS def_chunk_id
    FROM   curated_chunks AS ref
    JOIN   curated_chunks AS def
           ON  def.defined_symbol = ref.symbol_name    -- already normalised
           AND def.entity_id      = ref.entity_id
    WHERE  ref.defined_symbol IS NULL
      AND  ref.symbol_name    IS NOT NULL
      AND  ref.entity_id      = ?
"#;

// For each resolved pair:
//   INSERT OR REPLACE INTO curated_relationships
//   (from_id, to_id, rel_type, symbol, entity_id)
//   VALUES (ref_chunk_id, def_chunk_id, rel_type_for_ref, ref.symbol_name, entity_id)
//
// rel_type is determined by which Pass 2 query produced the reference:
//   call_expression  → 'CALLS'
//   use_declaration  → 'IMPORTS'
//   impl_item        → 'IMPLEMENTS'
```

**Stale edge cleanup** (run *before* inserting new relationships for a batch):

```rust
// Delete relationships whose source chunk was re-indexed in this pass.
db.execute(
    "DELETE FROM curated_relationships
     WHERE from_id IN (
         SELECT id FROM curated_chunks
         WHERE entity_id = ? AND updated_at >= ?
     )
     AND entity_id = ?",
    (entity_id, last_index_epoch, entity_id),
)?;
```

This prevents graph rot from accumulating during iterative development. The epoch value is recorded at the start of each watcher batch, not the real-time clock, so re-indexed files are precisely scoped.

---

## Component 4: Recursive CTE — Impact Radius Query

**Layer:** Rust / SQLite (`src-tauri/src/graph.rs`, new module)

The following pair of CTEs is exposed as the Tauri command `get_impact_radius` and called internally by the Linker for context injection.

### Callee Walk ("what does this chunk depend on?")

```sql
WITH RECURSIVE callee_walk(chunk_id, depth) AS (
    -- Seed: direct callees of the root chunk
    SELECT to_id,   1
    FROM   curated_relationships
    WHERE  from_id   = :root_chunk_id
      AND  rel_type  IN ('CALLS', 'IMPORTS')
      AND  entity_id = :entity_id

    UNION ALL

    -- Recurse: follow the call chain
    SELECT r.to_id,  cw.depth + 1
    FROM   curated_relationships r
    JOIN   callee_walk cw ON r.from_id = cw.chunk_id
    WHERE  cw.depth    < :max_depth          -- hard cap, default 5
      AND  r.rel_type  IN ('CALLS', 'IMPORTS')
      AND  r.entity_id = :entity_id
)
SELECT DISTINCT chunk_id, MIN(depth) AS min_depth
FROM   callee_walk
GROUP  BY chunk_id
ORDER  BY min_depth;
```

### Caller Walk ("what will break if this chunk changes?")

```sql
WITH RECURSIVE caller_walk(chunk_id, depth) AS (
    -- Seed: direct callers of the root chunk
    SELECT from_id, 1
    FROM   curated_relationships
    WHERE  to_id     = :root_chunk_id
      AND  rel_type  IN ('CALLS', 'IMPORTS')
      AND  entity_id = :entity_id

    UNION ALL

    -- Recurse: walk up the call chain
    SELECT r.from_id, cw.depth + 1
    FROM   curated_relationships r
    JOIN   caller_walk cw ON r.to_id = cw.chunk_id
    WHERE  cw.depth    < :max_depth
      AND  r.rel_type  IN ('CALLS', 'IMPORTS')
      AND  r.entity_id = :entity_id
)
SELECT DISTINCT chunk_id, MIN(depth) AS min_depth
FROM   caller_walk
GROUP  BY chunk_id
ORDER  BY min_depth;
```

**Guard:** `DISTINCT` + `MIN(depth)` deduplicate diamond-shaped graphs (A→B, A→C, B→D, C→D returns D exactly once at `min_depth = 2`). The `:max_depth` bind parameter is capped at `5` server-side regardless of the client payload — see §Component 6.

---

## Component 5: `core-llm-wiki` v4.6.0 — Graph-Expanded `read()`

**Package:** `@equationalapplications/core-llm-wiki`

### 5a. New `GraphExpansionOptions` Type

```typescript
export interface GraphExpansionOptions {
  /** Maximum hops to walk from each seed chunk. Default: 1. Hard max enforced: 2. */
  hops?: 1 | 2;
  /** Include callees (dependencies) of seed chunks. Default: true. */
  includeCallees?: boolean;
  /** Include callers (impact radius) of seed chunks. Default: true. */
  includeCallers?: boolean;
  /** Maximum structural neighbors to inject per seed chunk. Default: 5. */
  neighborLimit?: number;
}
```

### 5b. Extend `ReadOptions`

```typescript
export interface ReadOptions {
  // ... existing fields from v3.3.0 (tierWeights, etc.) unchanged ...
  /**
   * When present, augments semantic results with structurally linked chunks.
   * Requires a `graphAdapter` to be registered on the WikiMemory instance.
   * Omitting this field is fully backward-compatible — graph expansion is skipped.
   */
  graphExpansion?: GraphExpansionOptions;
}
```

### 5c. `GraphAdapter` Interface

`core-llm-wiki` must remain database-agnostic. Graph queries are delegated to the host app via a new adapter interface:

```typescript
export interface GraphAdapter {
  /**
   * Return chunk IDs reachable from `rootChunkId` within `entityId` up to `maxHops`.
   * Implementations execute the Recursive CTEs from §Component 4.
   */
  getNeighbors(
    rootChunkId: string,
    entityId: string,
    direction: 'callers' | 'callees' | 'both',
    maxHops: number
  ): Promise<Array<{ chunkId: string; depth: number; relType: string }>>;
}
```

Registered at construction time alongside the existing adapters:

```typescript
export const wiki = createWiki(tauriWikiAdapter, {
  // ... existing options ...
  graphAdapter: tauriGraphAdapter,   // NEW in v3.4.0
});
```

### 5d. Updated `read()` — Graph Walk After Semantic Match

```typescript
async read(
  entityId: string | string[],
  query: string,
  options?: ReadOptions
): Promise<MemoryBundle> {
  // Step 1: existing semantic retrieval (unchanged from v3.3.0)
  const semanticResults = await this._semanticRead(entityId, query, options);

  // Step 2: graph expansion (new in v3.4.0)
  if (options?.graphExpansion && this._graphAdapter) {
    const {
      hops = 1,
      includeCallees = true,
      includeCallers = true,
      neighborLimit = 5,
    } = options.graphExpansion;

    const maxHops = Math.min(hops, 2); // hard cap at 2 to protect context window
    const direction = includeCallees && includeCallers ? 'both'
                    : includeCallees ? 'callees'
                    : 'callers';

    const semanticIds = new Set(semanticResults.chunks.map(c => c.id));
    const neighborIds = new Set<string>();

    for (const seedChunk of semanticResults.chunks.slice(0, 5)) { // top-K seeds only
      const neighbors = await this._graphAdapter.getNeighbors(
        seedChunk.id,
        seedChunk.entity_id,
        direction,
        maxHops
      );
      neighbors
        .sort((a, b) => a.depth - b.depth)
        .slice(0, neighborLimit)
        .filter(n => !semanticIds.has(n.chunkId)) // no duplicates
        .forEach(n => neighborIds.add(n.chunkId));
    }

    // Step 3: fetch neighbor chunks, tag as structural
    const structuralChunks = await this._fetchChunksByIds([...neighborIds]);
    structuralChunks.forEach(c => { (c as any).structural = true; });

    return {
      ...semanticResults,
      chunks: [...semanticResults.chunks, ...structuralChunks],
    };
  }

  return semanticResults;
}
```

Semantic results are never displaced — structural chunks are appended after them.

---

## Component 6: Tauri `GraphAdapter` & `get_impact_radius` Command

**File:** `src/lib/wikiGraphAdapter.ts` (new file)

```typescript
import { invoke } from '@tauri-apps/api/core';
import type { GraphAdapter } from '@equationalapplications/core-llm-wiki';

export const tauriGraphAdapter: GraphAdapter = {
  async getNeighbors(rootChunkId, entityId, direction, maxHops) {
    return invoke<Array<{ chunkId: string; depth: number; relType: string }>>(
      'get_impact_radius',
      { rootChunkId, entityId, direction, maxHops }
    );
  },
};
```

**New Tauri command** (`src-tauri/src/lib.rs`):

```rust
#[tauri::command]
async fn get_impact_radius(
    db_state: State<'_, DbState>,
    root_chunk_id: String,
    entity_id: String,
    direction: String,     // "callers" | "callees" | "both"
    max_hops: u32,
) -> Result<Vec<NeighborRow>, String> {
    let conn = db_state.pool.get().map_err(|e| e.to_string())?;
    let max_hops = max_hops.min(5);  // server-side cap; never trust the client

    match direction.as_str() {
        "callees" => graph::get_callees(&conn, &root_chunk_id, &entity_id, max_hops),
        "callers" => graph::get_callers(&conn, &root_chunk_id, &entity_id, max_hops),
        "both"    => graph::get_both(&conn, &root_chunk_id, &entity_id, max_hops),
        _         => Err(format!("unknown direction: {}", direction)),
    }
    .map_err(|e| e.to_string())
}
```

`graph::get_callees`, `graph::get_callers`, `graph::get_both` live in the new `src-tauri/src/graph.rs` module and execute the Recursive CTEs from §Component 4.

---

## Component 7: `tieredRead` Update

**File:** `src/lib/wiki.ts`

```typescript
import type { GraphExpansionOptions } from '@equationalapplications/core-llm-wiki';

/** Tiered + optionally graph-expanded read. */
export async function tieredRead(
  query: string,
  opts: { graphExpansion?: GraphExpansionOptions } = {}
) {
  return wiki.read(
    ['tier_fact', 'tier_wisdom', _workspaceId],
    query,
    {
      tierWeights: {
        tier_fact:     1.5,
        tier_wisdom:   1.0,
        [_workspaceId]: 0.6,
      },
      graphExpansion: opts.graphExpansion,
    }
  );
}
```

Existing call sites that omit `graphExpansion` are fully backward-compatible.

---

## Component 8: React UI — "Connected" Badges

**File:** `src/components/search/SearchResult.tsx` (update)

Chunks returned with `structural: true` receive a **"Connected"** badge. Structural results are rendered after semantic results, separated by a divider:

```tsx
{chunk.structural && (
  <span
    className="badge badge-connected"
    title={relTypeLabel(chunk.relType)}
  >
    Connected
  </span>
)}
```

```typescript
function relTypeLabel(relType?: string): string {
  if (relType === 'CALLS')      return 'Calls this symbol';
  if (relType === 'IMPORTS')    return 'Imports this module';
  if (relType === 'IMPLEMENTS') return 'Implements this interface';
  return 'Structurally linked';
}
```

The divider label reads **"Structural context"** so users understand these results were retrieved via call graph, not semantic similarity.

---

## Component 9: Librarian Prompt — Structural Context Section

**File:** `src-tauri/src/librarian/mod.rs`

When structural neighbors are present in a librarian pass, a new section is appended to `source_text` after the Working Context section from Phase 1:

```
STRUCTURAL CONTEXT — linked via call graph (do not modify; use for impact analysis only):
[source: src/auth/login.rs | symbol: init_db | rel: CALLS | depth: 1]
<chunk text>

[source: src/db/migrations.rs | symbol: init_db | rel: CALLS | depth: 1]
<chunk text>
```

**Extended conflict directive** (append to system prompt after the Phase 1 "Architectural Inconsistency" directive verbatim):

> _"If a Structural Context chunk reveals that a violation in Working Context propagates to multiple callers, enumerate each caller file and symbol in the Wisdom proposal. Title the proposal **'Cascading Violation'** and list each impacted call site under an 'Affected callers' section. Do not emit separate proposals per caller — consolidate into one."_

---

## Key Reasoning Capability

With Phase 2, the Active Librarian can perform **Contextual Validation** with structural reach:

> _"The code in `main.rs` (Working Memory) calls `init_db()`, which violates the security protocol defined in `security_policy.pdf` (Fact). `init_db()` is also called by `auth/login.rs` and `db/migrations.rs` (Structural Context, depth 1). Proposing a **Cascading Violation** Wisdom entry to flag this architectural inconsistency across all three callers."_

---

## Acceptance Criteria

1. Schema V5 migration runs cleanly on an existing V4 database; existing rows may be updated to populate `entity_id` and backfill schema metadata, but no data is deleted.
2. After a full re-index, `defined_symbol` is populated for all function/class definition chunks; reference chunks have `defined_symbol = NULL`.
3. After indexing `main.rs` (which calls `init_db`) and `init_db.rs` (which defines `init_db`), `curated_relationships` contains at least one `CALLS` edge with `symbol = 'init_db'` and `entity_id` matching both chunks.
4. Editing `main.rs` and triggering a re-index deletes the old relationships for `main.rs` chunks before inserting new ones. A no-op edit produces the same edge count before and after (graph rot test).
5. `tieredRead(query, { graphExpansion: { hops: 1 } })` returns structural neighbors tagged `structural: true` alongside semantic results.
6. A structural neighbor already returned by semantic search is not duplicated in the final bundle.
7. Calling `get_impact_radius` with `maxHops: 10` from the client is silently clamped to `5` on the Rust side.
8. `getNeighbors(..., 'both', 2)` on a diamond graph (A→B, A→C, B→D, C→D) returns D exactly once with `min_depth = 2`.
9. Search results show "Connected" badges only on structural chunks; semantic-only chunks carry no badge.
10. A 30-file vault with 500 chunks completes the Linker pass (Pass 3) in under 2 seconds on the target hardware.
11. The Linker never writes relationships that cross `entity_id` boundaries, even when two vaults define symbols with identical names.

---

## Pitfalls to Avoid

| Risk | Mitigation |
|---|---|
| **Stale edges after file edit** | Delete `curated_relationships` rows where `from_id` belongs to re-indexed chunks before writing new rows (§Component 3, Stale edge cleanup) |
| **Graph bloat / context window overflow** | Hard cap: `hops ≤ 2` in TypeScript; `neighborLimit` defaults to 5 per seed; structural section is clearly delimited from semantic results in Librarian prompt |
| **Cross-vault symbol contamination** | Linker resolver uses `entity_id = ?` in every query; all three `curated_relationships` indices include `entity_id` |
| **Diamond graph deduplication** | Recursive CTE uses `DISTINCT` + `MIN(depth)`; TypeScript layer pre-filters neighbor IDs against semantic result IDs before fetching |
| **`max_hops` abuse from client** | Rust command clamps `:max_hops` to `5` regardless of client payload |
| **Linker blocking file saves** | Linker runs via Tokio `spawn_blocking`, never on the save path; triggered only after the watcher batch settles (3 s debounce inherited from Phase 1) |
| **Orphaned `to_id` references** | When a definition chunk is deleted, `runHeal` (Phase 1 §7) must also `DELETE FROM curated_relationships WHERE to_id = <deleted_chunk_id>` |
| **Symbol name collisions within a vault** | Collisions are valid — the Linker produces multiple edges; retrieval deduplication via `DISTINCT` handles this correctly |

---

## Out of Scope (Phase 3+)

- **Native vector ranker** — `sqlite-vec` / `sqlite-vss` adapter for sub-millisecond ANN search.
- **Contextual aging** — score decay for working-memory chunks not accessed recently.
- **Librarian re-weighting** — dynamic tier weight adjustment based on Review Queue ignore patterns.
- **Cross-vault graph federation** — resolving symbols across multiple open vaults.
- **LSP integration** — using a Language Server Protocol go-to-definition response instead of tree-sitter for higher-fidelity resolution in large monorepos.

---

## Files Changed

| File | Change |
|---|---|
| `src-tauri/src/db/migrations.rs` | Schema V5: `curated_relationships` table + 3 indices; `curated_chunks.defined_symbol` column + partial index |
| `src-tauri/src/graph.rs` | New: `get_callees`, `get_callers`, `get_both` executing the Recursive CTEs |
| `src-tauri/src/indexer/mod.rs` | Refactor single-pass into Pass 1 (definitions) + Pass 2 (references); populate `defined_symbol` |
| `src-tauri/src/indexer/linker.rs` | New: Pass 3 Global Resolver; stale edge cleanup; `entity_id`-scoped resolution loop |
| `src-tauri/src/lib.rs` | Add `get_impact_radius` command; register `graph` module in `generate_handler![]` |
| `src-tauri/src/librarian/mod.rs` | Structural context section in `source_text` assembly; "Cascading Violation" conflict directive |
| `src/lib/wikiGraphAdapter.ts` | New: `tauriGraphAdapter` implementing `GraphAdapter` |
| `src/lib/wiki.ts` | Update `tieredRead` to accept `graphExpansion` options; pass `tauriGraphAdapter` to `createWiki` |
| `src/components/search/SearchResult.tsx` | "Connected" badge for structural chunks; "Structural context" divider |
| `@equationalapplications/core-llm-wiki` | v3.4.0: `GraphAdapter` interface; `GraphExpansionOptions`; `ReadOptions.graphExpansion`; updated `read()` with graph walk, deduplication, and structural tagging |
