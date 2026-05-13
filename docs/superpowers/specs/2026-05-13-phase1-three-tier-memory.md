# Phase 1: Three-Tier Memory Foundation — Design Spec

**Date:** 2026-05-13
**Status:** Implemented
**Branch:** `kv/fixes` (target: `main`)
**Stack:** Tauri 2.x (Rust), React 19 frontend, `@equationalapplications/core-llm-wiki` v3.x, `@equationalapplications/react-llm-wiki` v3.x

---

## Overview

Introduce a three-tier memory model into Curated Thoughts so that retrieval is **tier-aware**: documents from `documents/` are treated as immutable facts, curated wiki pages are weighted as trusted synthesis, and active workspace chunks are included as high-noise context. All retrieval through the `core-llm-wiki` layer respects these weights automatically. Background jobs (`runLibrarian`, `runHeal`) are strictly prohibited from touching the Facts tier.

---

## Problem

The current `wiki.read()` call treats every `entityId` namespace as equal. A raw AST chunk from a transient code project competes on equal footing with a manually curated wiki page or an imported reference document. This creates two failure modes:

1. **Noise drowning signal** — noisy working-memory chunks rank above stable, curated facts when the query happens to match raw code tokens.
2. **Librarian contaminating facts** — `runLibrarian()` and `runHeal()` have no mechanism to protect `documents/` content from being modified or superseded by AI proposals.

---

## Three-Tier Model

| Tier | `entityId` | Source Folder | `source_type` | Persistence | Default Weight |
|---|---|---|---|---|---|
| **Facts** | `tier_fact` | `documents/` | `immutable_document` | Permanent, read-only | `1.5×` |
| **Wisdom** | `tier_wisdom` | `wiki/` | `user_confirmed` | Permanent, human-verified | `1.0×` |
| **Working** | `tier_working::<sha256>` | everything else | `librarian_inferred` | Ephemeral, per-workspace | `0.6×` |

`tier_working::<sha256>` is the SHA-256 of the absolute vault path, truncated to 16 hex chars. It is deterministic and isolated — multiple vaults on the same machine never share working memory, and `#temp-fix` tags from one workspace cannot pollute the global Wisdom tag space.

> **Package note:** `immutable_document` and `librarian_inferred` are new `source_type` values that do not exist in `core-llm-wiki` v3.2.0. They must be added to the union type in the v3.3.0 release described below.

---

## Components

### 1. Rust: `get_workspace_id` Command

**File:** `src-tauri/src/lib.rs`

New Tauri command that accepts an absolute vault path and returns the deterministic `tier_working::<hash>` entityId. Reuses the existing `hash_bytes` from `src-tauri/src/hasher.rs`.

```rust
#[tauri::command]
fn get_workspace_id(path: String) -> String {
    // hash_bytes returns hex::encode output — [0-9a-f]{64} — safe to slice as &str.
    let hash = crate::hasher::hash_bytes(path.as_bytes());
    format!("tier_working::{}", &hash[..16])
}
```

Registered in `tauri::generate_handler![]` alongside existing commands.

**Frontend contract:** `invoke<string>("get_workspace_id", { path: vaultPath })` → `"tier_working::a3f9b2c1d4e5f607"`.

---

### 2. `core-llm-wiki` v3.3.0 Package Update

This is the highest-risk change. Four modifications are required.

#### 2a. Extend `source_type` Union

```typescript
// types.ts in core package — add two new members:
source_type:
  | 'user_stated'
  | 'agent_inferred'
  | 'user_confirmed'
  | 'user_document'
  | 'immutable_document'   // NEW — Facts tier; Librarian must never write this
  | 'librarian_inferred'   // NEW — Working tier; ephemeral, auto-prunable
```

`user_document` and `agent_inferred` remain in the union for backward compatibility with existing data.

#### 2b. Add `tierWeights` to `ReadOptions`

```typescript
export interface ReadOptions {
  // ... existing fields unchanged ...
  /**
   * Per-entity score multiplier applied after cosine similarity.
   * Keys are entityId strings. Missing keys default to 1.0.
   */
  tierWeights?: Record<string, number>;
}
```

Default: all weights `1.0`. Fully backward-compatible.

#### 2c. Multi-Entity `read()` Signature

```typescript
// Before
read(entityId: string, query: string, options?: ReadOptions): Promise<MemoryBundle>

// After
read(entityId: string | string[], query: string, options?: ReadOptions): Promise<MemoryBundle>
```

Internal normalization: `const entityIds = Array.isArray(entityId) ? entityId : [entityId];`

#### 2d. SQL Candidate Selection — `IN (...)` Clause

Replace `WHERE entity_id = ?` with a dynamic `IN` clause everywhere candidates are fetched:

```typescript
const placeholders = entityIds.map(() => '?').join(',');
candidateRows = await db.getAllAsync<CandidateRow>(
  `SELECT id, entity_id, updated_at, access_count
   FROM ${prefix}entries
   WHERE entity_id IN (${placeholders}) AND deleted_at IS NULL`,
  entityIds
);
```

Apply the same change to the MiniSearch pre-filter:

```typescript
filter: (r) => entityIds.includes((r as any).entity_id)
```

`preFilterLimit` applies across all entities combined, not per-entity.

#### 2e. Tier Weight Multiplier in Ranker

In `_rankWithJsCosine` (and the `VectorRanker` path), after computing the base cosine score:

```typescript
const weight = options?.tierWeights?.[row.entity_id] ?? 1.0;
const adjustedScore = cosineSimilarity * weight;
```

The adjusted score replaces the raw score in `_tieBreakSort`. Facts naturally float to the top whenever semantically competitive.

#### 2f. Immutability Guard in `runLibrarian` / `runHeal`

```typescript
// Before any write inside runLibrarian or runHeal:
if (fact.source_type === 'immutable_document') {
  // Include body as "Anchor Truth" in prompt context only.
  // NEVER target for updates, rewrites, or deletion.
  return;
}
```

**Fact Supremacy rule:** if a contradiction is detected between a `tier_working` chunk and a `tier_fact` entry, the Librarian must propose a new `tier_wisdom` entry flagging the violation — it must not attempt to reconcile by modifying the Fact.

#### 2g. Vector Cache Limit

Cap in-memory vector caching at **500 vectors per entity** (16 entities max). Call `wikiMemory.clearVectorCache()` when the app is minimized or idle to prevent RAM pressure on constrained desktop hardware.

---

### 3. Ingestion Tier Assignment

When a document is ingested into the `core-llm-wiki` layer, it receives the correct `entityId` and `source_type` based on its vault-relative path.

**Rule (priority order):**

| Path prefix | `entityId` | `source_type` |
|---|---|---|
| `documents/` | `tier_fact` | `immutable_document` |
| `wiki/` | `tier_wisdom` | `user_confirmed` |
| anything else | `tier_working::<sha256>` | `librarian_inferred` |

New helper in the TypeScript layer:

```typescript
// src/lib/wikiTiers.ts  (new file)
export function entityIdForPath(
  vaultRelativePath: string,
  workspaceId: string
): { entityId: string; sourceType: 'immutable_document' | 'user_confirmed' | 'librarian_inferred' } {
  if (vaultRelativePath.startsWith('documents/')) {
    return { entityId: 'tier_fact', sourceType: 'immutable_document' };
  }
  if (vaultRelativePath.startsWith('wiki/')) {
    return { entityId: 'tier_wisdom', sourceType: 'user_confirmed' };
  }
  return { entityId: workspaceId, sourceType: 'librarian_inferred' };
}
```

Every call to `wiki.ingestDocument()` passes the result of `entityIdForPath`. Existing ingestion hooks must be updated accordingly.

**Note:** The Rust pipeline's `documents.tier` column (`'user_doc'` / `'wiki'`) is a separate system powering `search_vault`. It is **not** changed in Phase 1.

---

### 4. Librarian Prompt Tier Separation

**File:** `src-tauri/src/librarian/mod.rs`

The current system prompt (`"You are a knowledge librarian. Summarize the document..."`) must be extended when assembling `source_text` for the Ollama call. Each chunk must carry its **source metadata** (vault-relative path, symbol name if AST chunk, and line range) so the Librarian can cite exact locations in its proposals. Chunks are labelled by tier:

```
ANCHOR TRUTH — do not propose modifications to these facts:
[source: documents/sqlite_docs.pdf | lines 12-34]
<chunk text>

[source: documents/api-ref.md | lines 5-18]
<chunk text>

CURATED WISDOM — may be updated via Wisdom proposals:
[source: wiki/auth-patterns.md | lines 1-40]
<chunk text>

WORKING CONTEXT — summarize patterns and flag contradictions only:
[source: src/db/init.rs | symbol: init_db | lines 22-45]
<chunk text>
```

**Conflict resolution directive** (append to system prompt verbatim):

> _"If Working Context contradicts Anchor Truth, do not harmonize or modify the Anchor Truth. Instead, create a new Wisdom entry titled **'Architectural Inconsistency'** that states: which Working file and symbol introduced the deviation (cite `source:` metadata), which Anchor Truth document it violates (cite `source:` metadata), and a one-sentence description of the conflict. Do not emit a Wisdom proposal for any content that is consistent with the Anchor Truth."_

**Rust implementation note** — the `source_text` assembly loop in `librarian/mod.rs` (which currently does `chunks.join("\n\n")`) must be changed to include the source metadata header per chunk:

```rust
let source_text: String = chunks
    .iter()
    .map(|(path, symbol, start_line, end_line, text)| {
        let loc = match symbol {
            Some(s) => format!("[source: {} | symbol: {} | lines {}-{}]\n", path, s, start_line, end_line),
            None    => format!("[source: {} | lines {}-{}]\n", path, start_line, end_line),
        };
        format!("{}{}", loc, text)
    })
    .collect::<Vec<_>>()
    .join("\n\n");
```

This requires the DB query in `librarian/mod.rs` to `SELECT` `symbol_name`, `start_line`, `end_line` from the `chunks` table (all three columns exist since schema V4).

---

### 5. Frontend `wiki.ts` Update

**File:** `src/lib/wiki.ts`

```typescript
import { invoke } from '@tauri-apps/api/core';
import { createWiki, WikiBusyError } from '@equationalapplications/core-llm-wiki';
import { tauriWikiAdapter } from './wikiAdapter';

let _workspaceId: string = 'tier_working::default';

export async function initWorkspaceId(vaultPath: string): Promise<void> {
  _workspaceId = await invoke<string>('get_workspace_id', { path: vaultPath });
}

export function getWorkspaceId(): string {
  return _workspaceId;
}

export const wiki = createWiki(tauriWikiAdapter, {
  llmProvider: {
    async generateText({ systemPrompt, userPrompt }) {
      return invoke<string>('ollama_generate', { systemPrompt, userPrompt });
    },
    async embed(text: string): Promise<number[]> {
      return invoke<number[]>('embed_text', { text });
    },
  },
  config: {
    hybridWeight: 0.7,
    preFilterLimit: 50,
  },
  onRetrievalFallback: (err) => {
    console.warn('[wiki] embed unavailable, using keyword search:', err.message);
  },
});

/** Tiered read: Facts (1.5×) > Wisdom (1.0×) > Working (0.6×). */
export async function tieredRead(query: string) {
  return wiki.read(
    ['tier_fact', 'tier_wisdom', _workspaceId],
    query,
    {
      tierWeights: {
        tier_fact: 1.5,
        tier_wisdom: 1.0,
        [_workspaceId]: 0.6,
      },
    }
  );
}

export { WikiBusyError };
```

`initWorkspaceId` is called from `App.tsx` (or the vault-change handler) whenever the vault path is set or changed.

---

### 6. Real-time Status Subscriptions

**Goal:** expose live ingestion/librarian/heal state to the React UI so spinners and "busy" guards stay accurate.

**Rust event emission** (already partially present via `app.emit()`):

```rust
// Emit from pipeline worker and librarian when state changes:
app.emit("wiki-status-change", serde_json::json!({
    "entity_id": entity_id,
    "ingesting": true,
    "librarian": false,
    "heal": false,
})).unwrap();
```

**TypeScript hook** `src/hooks/useWikiStatus.ts` (new file):

```typescript
import { useEffect, useState } from 'react';
import { listen } from '@tauri-apps/api/event';

export interface WikiStatus {
  ingesting: boolean;
  librarian: boolean;
  heal: boolean;
}

export function useWikiStatus(): WikiStatus {
  const [status, setStatus] = useState<WikiStatus>({
    ingesting: false,
    librarian: false,
    heal: false,
  });

  useEffect(() => {
    const unsub = listen<WikiStatus>('wiki-status-change', (e) => setStatus(e.payload));
    return () => { unsub.then(f => f()); };
  }, []);

  return status;
}
```

The `isSystemBusy` derived value (`status.ingesting || status.librarian || status.heal`) blocks destructive UI actions (vault switch, manual prune).

---

### 7. Maintenance Dashboard

**File:** `src/components/settings/MaintenanceDashboard.tsx` (new component, rendered inside Settings panel)

Three manual controls, each disabled while `isSystemBusy`:

| Button | Tauri command | Purpose |
|---|---|---|
| **Heal Database** | `run_wiki_heal` | Removes ghost notes whose source file was deleted outside the app |
| **Prune Trash** | `run_wiki_prune` | Hard-deletes `librarian_inferred` entries soft-deleted > 7 days ago |
| **Full Re-index** | `run_wiki_reembed` | Force re-chunks + re-embeds all tiers (needed after model switch) |

**Rust commands** (`src-tauri/src/lib.rs`):

```rust
#[tauri::command]
async fn run_wiki_heal(app: AppHandle, db_state: State<'_, DbState>) -> Result<(), String> {
    app.emit("wiki-status-change", serde_json::json!({"heal": true, "ingesting": false, "librarian": false})).ok();
    // ... call wiki heal logic ...
    app.emit("wiki-status-change", serde_json::json!({"heal": false, "ingesting": false, "librarian": false})).ok();
    Ok(())
}
// run_wiki_prune and run_wiki_reembed follow the same pattern
```

Label "Prune Trash" clearly as **permanent deletion** in the UI copy. Users who confuse soft-delete (removing from UI) with hard-delete (scrubbing from DB) will expect data to come back.

---

### 8. Auto-Heal After File Watcher Events

Currently `runHeal()` is manual. Phase 1 makes it automatic via a debounced listener:

```typescript
// In src/lib/wiki.ts, called once on app init:
export function startAutoHeal(): void {
  let debounce: ReturnType<typeof setTimeout> | null = null;
  listen('vault-file-changed', () => {
    if (debounce) clearTimeout(debounce);
    debounce = setTimeout(async () => {
      try {
        await wiki.runHeal('tier_fact');
        await wiki.runHeal('tier_wisdom');
        await wiki.runHeal(_workspaceId);
      } catch (err) {
        if (!(err instanceof WikiBusyError)) console.error('[auto-heal]', err);
      }
    }, 3000);
  });
}
```

---

### 9. WikiBusyError Handling on Workspace Switch

Catch `WikiBusyError` wherever maintenance jobs are invoked and surface it via the `isSystemBusy` state (§6) to block the "Change vault…" button:

```typescript
import { WikiBusyError } from '@equationalapplications/core-llm-wiki';

async function safePrune() {
  try {
    await wiki.runPrune(_workspaceId);
  } catch (err) {
    if (err instanceof WikiBusyError) {
      setWikiBusy(true); // blocks vault switch
    }
    throw err;
  }
}
```

---

## Review Queue: Grouping Strategy

**Recommendation: group by Wisdom theme, not by file.**

File-based grouping (50 files → 50 proposals) replicates what `git diff` already shows and inflicts maximum cognitive load. Theme-based grouping collapses correlated proposals into a single reviewable unit:

| Theme | What the Librarian groups together |
|---|---|
| **Architecture change** | All files touched by a structural refactor |
| **Deprecated API usage** | All call sites of the same deprecated symbol |
| **New design pattern** | Multiple files that all implement the same idiom |
| **Tech debt flag** | Contradictions between Working code and a Fact |

**Grouping heuristic for the Librarian prompt:**

> _"If more than two source chunks support the same conceptual claim, consolidate them into one Wisdom proposal. Include the supporting file list as a collapsible 'Sources' section. Do not emit a separate proposal per file."_

**Proposed UI shape for the Review Queue:**

```
[ Wisdom Proposal ]  "Auth module uses deprecated init_db() — see sqlite_docs.pdf §4"
  Status: pending_review        Sources ▾   auth.rs, db.rs, migrations.rs
  [ Approve ]  [ Edit ]  [ Reject ]
```

Individual source references live in a collapsible `<details>` block. This way a `git pull` that touches 30 files produces 3–5 actionable proposals, not 30.

---

## Acceptance Criteria

1. `invoke("get_workspace_id", { path: "/Users/foo/Vault" })` returns a string matching `/^tier_working::[0-9a-f]{16}$/`.
2. `wiki.read(['tier_fact', 'tier_wisdom', workspaceId], query, { tierWeights: … })` returns results from all three tiers, ranked by adjusted score.
3. A document at `documents/api-ref.md` ingests with `entityId: 'tier_fact'`, `source_type: 'immutable_document'`. After a librarian pass its content is unchanged.
4. A `tier_working` chunk contradicting a Fact produces a `tier_wisdom` proposal titled "Architectural Inconsistency" that cites both the Working source file+symbol and the Anchor Truth document — the Fact entry is untouched.
5. Switching vault while `runPrune` is active shows a "Database busy" indicator instead of silently failing.
6. Deleting a file from `documents/` via Finder triggers `runHeal('tier_fact')` within ~3 seconds and removes the ghost entry.
7. `useWikiStatus()` reflects live state during a `run_wiki_reembed` job (ingesting spinner visible, "Change vault" button disabled).
8. The Review Queue groups a 30-file `git pull` into ≤ 5 Wisdom proposals, each with a collapsible source list.

---

## Pitfalls to Avoid

| Risk | Mitigation |
|---|---|
| Ghost notes from out-of-app file deletions | Auto-heal on 3s debounce after watcher event (§8) |
| DB lock during full re-index blocking saves | `WikiBusyError` surfaced via `useWikiStatus`; vault switch blocked (§9) |
| Infinite embedding loop | `chunkConcurrency: 1`, `hasChanged()` check before re-ingest, debounced heal |
| `tier_working` noise ranking above Facts | `tierWeights` at ranking time; Facts start at 1.5× (§2e) |
| Workspace ID collision across vaults | SHA-256 of full absolute path; `tier_working::` prefix namespaces it (§1) |
| `#temp-fix` tags polluting global Wisdom | `entityId` isolation: Working tags never write to `tier_wisdom` namespace |
| RAM pressure from vector cache | Hard cap: 500 vectors per entity; `clearVectorCache()` on app minimise (§2g) |
| Users confusing soft-delete with hard-delete | "Prune Trash" copy explicitly says "permanently delete" (§7) |

---

## Out of Scope (Phase 2+)

- **Code graph / Edge Extraction** — `curated_relationships` table, tree-sitter call-site extraction, hop-based retrieval expansion.
- **Native vector ranker** — `sqlite-vec` / `sqlite-vss` adapter for sub-millisecond ANN search.
- **Contextual aging** — score decay for working-memory chunks not accessed recently.
- **Librarian re-weighting** — dynamic tier weight adjustment based on Review Queue ignore patterns.

---

## Files Changed

| File | Change |
|---|---|
| `src-tauri/src/lib.rs` | Add `get_workspace_id`, `run_wiki_heal`, `run_wiki_prune`, `run_wiki_reembed` commands; emit `wiki-status-change` events |
| `src-tauri/src/librarian/mod.rs` | Tier-separated prompt; source metadata header per chunk; "Architectural Inconsistency" conflict directive; immutability guard on `immutable_document` |
| `src/lib/wiki.ts` | Add `initWorkspaceId`, `getWorkspaceId`, `tieredRead`, `startAutoHeal` |
| `src/lib/wikiTiers.ts` | New: `entityIdForPath` helper |
| `src/hooks/useWikiStatus.ts` | New: reactive `WikiStatus` hook via `wiki-status-change` event |
| `src/components/settings/MaintenanceDashboard.tsx` | New: Heal / Prune / Re-index controls |
| `@equationalapplications/core-llm-wiki` | v3.3.0: `immutable_document` + `librarian_inferred` source types; `ReadOptions.tierWeights`; multi-entity `read()`; SQL `IN` clause; score multiplier in ranker; 500-vector cache cap |
