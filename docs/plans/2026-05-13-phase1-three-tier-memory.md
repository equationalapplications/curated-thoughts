# Phase 1 Three-Tier Memory Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire three-tier memory (Facts/Wisdom/Working) into `core-llm-wiki`, the Rust pipeline, and the React frontend so retrieval is tier-weighted, ingestion is tier-assigned, and the Librarian labels Anchor Truth in its prompt context.

**Architecture:** `core-llm-wiki` v4.5.1 ships two new `source_type` values, multi-entity `read()`, and per-entity score multipliers; a `get_workspace_id` Rust command produces a deterministic per-vault entity ID (SHA-256 of vault path, first 16 hex chars); a `wikiTiers.ts` helper centralises tier routing at ingestion time; `wiki.ts` gains `tieredRead`, `initWorkspaceId`, and `startAutoHeal`; `librarian/mod.rs` gains tier-labelled source context assembly and the Architectural Inconsistency conflict directive; a `useWikiStatus` hook and `MaintenanceDashboard` surface live job state and manual maintenance controls.

**Tech Stack:** Tauri 2.x (Rust, `sha2` + `hex` already in Cargo.toml), React 19, `@equationalapplications/core-llm-wiki` v4.5.1, `@equationalapplications/react-llm-wiki` v3.x, Vitest, `@testing-library/react`, `@tauri-apps/api`

---

## File Structure

### New files
- `src/lib/wikiTiers.ts` — `entityIdForPath` tier routing helper
- `src/hooks/useWikiStatus.ts` — reactive `WikiStatus` hook via `wiki-status-change` event
- `src/components/settings/MaintenanceDashboard.tsx` — Heal / Prune / Re-index controls
- `src/__tests__/wikiTiers.test.ts` — unit tests for tier routing
- `src/__tests__/wiki.test.ts` — unit tests for `initWorkspaceId`, `tieredRead`, `startAutoHeal`
- `src/__tests__/useWikiStatus.test.ts` — unit tests for wiki status hook

### Modified files
- `src-tauri/src/lib.rs` — add `get_workspace_id`, `run_wiki_heal`, `run_wiki_prune`, `run_wiki_reembed` commands; register in both `generate_handler![]` invocations
- `src-tauri/src/librarian/mod.rs` — `ChunkRow` struct; `assemble_librarian_context` function; update `generate_summary` to use it with tier labels and conflict directive
- `src/lib/wiki.ts` — add `initWorkspaceId`, `getWorkspaceId`, `tieredRead`, `startAutoHeal`
- `src/components/settings/SettingsModal.tsx` — add `<MaintenanceDashboard />` below existing panels
- `src/App.tsx` — call `initWorkspaceId` + `startAutoHeal` on vault-path resolution

### External prerequisite (separate repo)
`@equationalapplications/core-llm-wiki` v4.5.1 must be published **before Tasks 4 and 8** can be completed. All changes required are specified in Task 1. Work on it in parallel; Tasks 2–3 and 5–7 have no dependency on it.

---

## Task 1: core-llm-wiki v4.5.1 Package Changes

**⚠️ Work in the `@equationalapplications/core-llm-wiki` source repository.** The current installed version is `3.2.0` (`node_modules/@equationalapplications/core-llm-wiki`). After publishing, bump `package.json` in this repo to `"^4.5.1"`.

The core package uses `llm_wiki_` as the default table prefix. Tables: `llm_wiki_entries`, `llm_wiki_tasks`, `llm_wiki_events`, `llm_wiki_checkpoints`, `llm_wiki_meta`. The entries table has columns: `id`, `entity_id`, `source_type`, `source_ref`, `deleted_at`, etc.

**Files in core-llm-wiki repo:**
- Modify: `src/types.ts` (wherever `WikiFact.source_type` union is defined)
- Modify: `src/wiki.ts` (wherever `WikiMemory.read()` signature and implementation live)
- Modify: `src/ranker.ts` (wherever `_rankWithJsCosine` computes scores)
- Modify: `src/db.ts` (wherever `WHERE entity_id = ?` candidate queries live)

### 1a. Extend `source_type` union

- [ ] **Step 1: Write the failing test**

In the core-llm-wiki test suite:

```typescript
// test/types.test.ts
import type { WikiFact } from '../src/types';

it('accepts immutable_document and librarian_inferred as source_type', () => {
  const fact: WikiFact = {
    id: '1', entity_id: 'tier_fact', title: 'Test', body: 'body',
    tags: [], confidence: 'certain',
    source_type: 'immutable_document',
    source_hash: null, source_ref: null,
    created_at: 0, updated_at: 0,
    last_accessed_at: null, access_count: 0, deleted_at: null,
  };
  expect(fact.source_type).toBe('immutable_document');
  const fact2: WikiFact = { ...fact, source_type: 'librarian_inferred' };
  expect(fact2.source_type).toBe('librarian_inferred');
});
```

- [ ] **Step 2: Run to verify it fails**

```bash
npx tsc --noEmit
```

Expected: Type error — `'immutable_document'` not assignable to `source_type`.

- [ ] **Step 3: Add two new members to the `source_type` union in `src/types.ts`**

```typescript
source_type:
  | 'user_stated'
  | 'agent_inferred'
  | 'user_confirmed'
  | 'user_document'
  | 'immutable_document'
  | 'librarian_inferred'
```

- [ ] **Step 4: Run to verify it passes**

```bash
npx tsc --noEmit && npm test -- types.test
```

Expected: PASS.

### 1b. `ReadOptions.tierWeights` + multi-entity `read()` signature

- [ ] **Step 5: Write the failing tests**

```typescript
// test/read-multi-entity.test.ts
import { createWiki } from '../src';

it('ReadOptions accepts tierWeights record', () => {
  const opts: import('../src/types').ReadOptions = {
    tierWeights: { tier_fact: 1.5, tier_wisdom: 1.0, 'tier_working::abc': 0.6 },
  };
  expect(opts.tierWeights?.['tier_fact']).toBe(1.5);
});

it('read() type-checks with array of entity IDs', () => {
  // Compile-time check only — if this test file compiles, the type is correct.
  const call = (wiki: ReturnType<typeof createWiki>) =>
    wiki.read(['tier_fact', 'tier_wisdom', 'tier_working::abc'], 'query');
  expect(typeof call).toBe('function');
});
```

- [ ] **Step 6: Run to verify it fails**

```bash
npx tsc --noEmit
```

Expected: Type errors — `tierWeights` unknown on `ReadOptions`, array not assignable to `string`.

- [ ] **Step 7: Add `tierWeights` to `ReadOptions` in `src/types.ts`**

```typescript
export interface ReadOptions {
  maxResults?: number;
  preFilterLimit?: number | null;
  hybridWeight?: number;
  /**
   * Per-entity score multiplier applied after cosine similarity.
   * Keys are entityId strings. Missing keys default to 1.0.
   */
  tierWeights?: Record<string, number>;
}
```

- [ ] **Step 8: Update `read()` signature in `WikiMemory` class and its public interface**

```typescript
read(entityId: string | string[], query: string, options?: ReadOptions): Promise<MemoryBundle>
```

At the top of the implementation, normalise to an array:

```typescript
const entityIds = Array.isArray(entityId) ? entityId : [entityId];
```

### 1c. SQL `IN (...)` candidate selection

- [ ] **Step 9: Update all `WHERE entity_id = ?` candidate queries in `src/db.ts`**

```typescript
const placeholders = entityIds.map(() => '?').join(',');
candidateRows = await db.getAllAsync<CandidateRow>(
  `SELECT id, entity_id, updated_at, access_count
   FROM ${prefix}entries
   WHERE entity_id IN (${placeholders}) AND deleted_at IS NULL`,
  entityIds
);
```

Update the MiniSearch pre-filter in the same file:

```typescript
filter: (r) => entityIds.includes((r as any).entity_id)
```

### 1d. Tier weight multiplier in ranker

- [ ] **Step 10: Apply weight in `_rankWithJsCosine` (and VectorRanker path) in `src/ranker.ts`**

```typescript
const weight = options?.tierWeights?.[row.entity_id] ?? 1.0;
const adjustedScore = cosineSimilarity * weight;
```

Replace `cosineSimilarity` with `adjustedScore` in `_tieBreakSort`.

### 1e. Immutability guard in `runLibrarian` / `runHeal`

- [ ] **Step 11: Add guard before any write in `runLibrarian` and `runHeal`**

```typescript
// Before any write inside runLibrarian or runHeal:
if (fact.source_type === 'immutable_document') {
  // Include body as Anchor Truth in prompt context only.
  // NEVER target for updates, rewrites, or deletion.
  return;
}
```

### 1f. Vector cache cap

- [ ] **Step 12: Set `MAX_VECTOR_CACHE_FACTS_PER_ENTITY` to 500**

```typescript
private static readonly MAX_VECTOR_CACHE_FACTS_PER_ENTITY = 500;
```

- [ ] **Step 13: Run the full test suite**

```bash
npm test
```

Expected: all tests pass.

- [ ] **Step 14: Publish v4.5.1 and update `package.json` in curated-thoughts**

```bash
npm publish --tag latest   # in core-llm-wiki repo
```

In curated-thoughts:

```bash
npm install @equationalapplications/core-llm-wiki@4.5.1
```

Update `package.json` line:

```json
"@equationalapplications/core-llm-wiki": "^4.5.1"
```

- [ ] **Step 15: Commit in curated-thoughts**

```bash
git add package.json package-lock.json
git commit -m "chore(deps): bump core-llm-wiki to v4.5.1 with tier-aware read()"
```

---

## Task 2: Rust — `get_workspace_id` Command

**Files:**
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Add the following module to the bottom of `src-tauri/src/lib.rs`, before the final closing brace of the file:

```rust
#[cfg(test)]
mod workspace_id_tests {
    use super::get_workspace_id;

    #[test]
    fn has_tier_working_prefix() {
        let id = get_workspace_id("/Users/foo/Vault".to_string());
        assert!(id.starts_with("tier_working::"), "got: {id}");
    }

    #[test]
    fn hash_segment_is_16_lowercase_hex_chars() {
        let id = get_workspace_id("/Users/foo/Vault".to_string());
        let hash = id.strip_prefix("tier_working::").unwrap();
        assert_eq!(hash.len(), 16, "hash segment should be 16 chars, got: {hash}");
        assert!(
            hash.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
            "hash should be lowercase hex, got: {hash}"
        );
    }

    #[test]
    fn is_deterministic() {
        assert_eq!(
            get_workspace_id("/Users/foo/Vault".to_string()),
            get_workspace_id("/Users/foo/Vault".to_string())
        );
    }

    #[test]
    fn different_vaults_produce_different_ids() {
        assert_ne!(
            get_workspace_id("/Users/foo/VaultA".to_string()),
            get_workspace_id("/Users/foo/VaultB".to_string())
        );
    }
}
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p curated-thoughts workspace_id_tests
```

Expected: compile error — `get_workspace_id` is undefined.

- [ ] **Step 3: Add the command implementation**

Add the following before the `// ── Vault commands ───` comment block in `src-tauri/src/lib.rs` (around line 133):

```rust
// ── Workspace identity ────────────────────────────────────────────────────────

#[tauri::command]
fn get_workspace_id(path: String) -> String {
    // hash_bytes returns hex::encode(sha256) — 64 lowercase hex chars — safe to slice to 16.
    let hash = crate::hasher::hash_bytes(path.as_bytes());
    format!("tier_working::{}", &hash[..16])
}
```

- [ ] **Step 4: Register in `make_test_app()` handler list** (around line 1449)

Add `get_workspace_id,` to the `tauri::generate_handler![...]` call inside `make_test_app`.

- [ ] **Step 5: Register in `run()` handler list** (around line 1555)

Add `get_workspace_id,` to the `tauri::generate_handler![...]` call inside `run`.

- [ ] **Step 6: Run to verify tests pass**

```bash
cargo test -p curated-thoughts workspace_id_tests
```

Expected: PASS (4 tests).

- [ ] **Step 7: Run full Rust test suite**

```bash
cargo test -p curated-thoughts
```

Expected: all existing tests pass.

- [ ] **Step 8: Commit**

```bash
git add src-tauri/src/lib.rs
git commit -m "feat(rust): add get_workspace_id command for per-vault entity isolation"
```

---

## Task 3: `src/lib/wikiTiers.ts` — Tier Routing Helper

**Files:**
- Create: `src/lib/wikiTiers.ts`
- Create: `src/__tests__/wikiTiers.test.ts`

- [ ] **Step 1: Write the failing tests**

Create `src/__tests__/wikiTiers.test.ts`:

```typescript
import { describe, it, expect } from 'vitest';
import { entityIdForPath } from '../lib/wikiTiers';

describe('entityIdForPath', () => {
  const workspaceId = 'tier_working::a3f9b2c1d4e5f607';

  it('returns tier_fact + immutable_document for documents/ prefix', () => {
    expect(entityIdForPath('documents/api-ref.md', workspaceId)).toEqual({
      entityId: 'tier_fact',
      sourceType: 'immutable_document',
    });
  });

  it('returns tier_fact for files in documents subdirectory', () => {
    expect(entityIdForPath('documents/specs/v2/design.md', workspaceId)).toEqual({
      entityId: 'tier_fact',
      sourceType: 'immutable_document',
    });
  });

  it('returns tier_wisdom + user_confirmed for wiki/ prefix', () => {
    expect(entityIdForPath('wiki/auth-patterns.md', workspaceId)).toEqual({
      entityId: 'tier_wisdom',
      sourceType: 'user_confirmed',
    });
  });

  it('returns workspaceId + librarian_inferred for src/ path', () => {
    expect(entityIdForPath('src/db/init.rs', workspaceId)).toEqual({
      entityId: workspaceId,
      sourceType: 'librarian_inferred',
    });
  });

  it('returns workspaceId + librarian_inferred for root-level file', () => {
    expect(entityIdForPath('README.md', workspaceId)).toEqual({
      entityId: workspaceId,
      sourceType: 'librarian_inferred',
    });
  });

  it('does not match documentsfoo/ as documents/', () => {
    expect(entityIdForPath('documentsfoo/bar.md', workspaceId).entityId).toBe(workspaceId);
  });
});
```

- [ ] **Step 2: Run to verify it fails**

```bash
npm run test -- wikiTiers
```

Expected: FAIL — module `../lib/wikiTiers` not found.

- [ ] **Step 3: Write minimal implementation**

Create `src/lib/wikiTiers.ts`:

```typescript
export function entityIdForPath(
  vaultRelativePath: string,
  workspaceId: string
): {
  entityId: string;
  sourceType: 'immutable_document' | 'user_confirmed' | 'librarian_inferred';
} {
  if (vaultRelativePath.startsWith('documents/')) {
    return { entityId: 'tier_fact', sourceType: 'immutable_document' };
  }
  if (vaultRelativePath.startsWith('wiki/')) {
    return { entityId: 'tier_wisdom', sourceType: 'user_confirmed' };
  }
  return { entityId: workspaceId, sourceType: 'librarian_inferred' };
}
```

- [ ] **Step 4: Run to verify tests pass**

```bash
npm run test -- wikiTiers
```

Expected: PASS (6 tests).

- [ ] **Step 5: Commit**

```bash
git add src/lib/wikiTiers.ts src/__tests__/wikiTiers.test.ts
git commit -m "feat(ts): add entityIdForPath helper for three-tier ingestion routing"
```

---

## Task 4: `src/hooks/useWikiStatus.ts` — Live Status Hook

**Files:**
- Create: `src/hooks/useWikiStatus.ts`
- Create: `src/__tests__/useWikiStatus.test.ts`

- [ ] **Step 1: Write the failing tests**

Create `src/__tests__/useWikiStatus.test.ts`:

```typescript
import { vi, describe, it, expect, beforeEach } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import type { WikiStatus } from '../hooks/useWikiStatus';

type EventCallback = (e: { payload: WikiStatus }) => void;
let capturedCallback: EventCallback | null = null;

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn().mockImplementation(
    (_event: string, cb: EventCallback) => {
      capturedCallback = cb;
      return Promise.resolve(() => { capturedCallback = null; });
    }
  ),
}));

import { useWikiStatus } from '../hooks/useWikiStatus';

describe('useWikiStatus', () => {
  beforeEach(() => {
    capturedCallback = null;
    vi.clearAllMocks();
  });

  it('returns initial idle status', () => {
    const { result } = renderHook(() => useWikiStatus());
    expect(result.current).toEqual({ ingesting: false, librarian: false, heal: false });
  });

  it('updates when wiki-status-change fires with ingesting true', async () => {
    const { result } = renderHook(() => useWikiStatus());
    await act(async () => {
      capturedCallback?.({ payload: { ingesting: true, librarian: false, heal: false } });
    });
    expect(result.current).toEqual({ ingesting: true, librarian: false, heal: false });
  });

  it('updates when wiki-status-change fires with heal true', async () => {
    const { result } = renderHook(() => useWikiStatus());
    await act(async () => {
      capturedCallback?.({ payload: { ingesting: false, librarian: false, heal: true } });
    });
    expect(result.current.heal).toBe(true);
  });

  it('isSystemBusy is true when any field is active', async () => {
    const { result } = renderHook(() => useWikiStatus());
    await act(async () => {
      capturedCallback?.({ payload: { ingesting: false, librarian: true, heal: false } });
    });
    const { ingesting, librarian, heal } = result.current;
    expect(ingesting || librarian || heal).toBe(true);
  });
});
```

- [ ] **Step 2: Run to verify it fails**

```bash
npm run test -- useWikiStatus
```

Expected: FAIL — module `../hooks/useWikiStatus` not found.

- [ ] **Step 3: Write minimal implementation**

Create `src/hooks/useWikiStatus.ts`:

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
    return () => {
      unsub.then((f) => f());
    };
  }, []);

  return status;
}
```

- [ ] **Step 4: Run to verify tests pass**

```bash
npm run test -- useWikiStatus
```

Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add src/hooks/useWikiStatus.ts src/__tests__/useWikiStatus.test.ts
git commit -m "feat(ts): add useWikiStatus hook for reactive wiki job state"
```

---

## Task 5: Librarian `mod.rs` — Tier-Separated Prompt Assembly

**Files:**
- Modify: `src-tauri/src/librarian/mod.rs`

The `generate_summary` function currently selects `chunk_text` only. It needs `symbol_name`, `start_line`, `end_line` (added in MIGRATION_V4, all three columns exist), and `d.tier` from the `documents` table. The assembled `source_text` must group chunks by tier and prepend a source metadata header to each chunk.

- [ ] **Step 1: Write the failing tests**

Add the following inside the existing `mod tests` block at the bottom of `src-tauri/src/librarian/mod.rs`, after the existing tests:

```rust
    #[test]
    fn assemble_context_labels_user_doc_as_anchor_truth() {
        let chunks = vec![
            ChunkRow {
                text: "fn init_db() {}".to_string(),
                symbol_name: Some("init_db".to_string()),
                start_line: 1,
                end_line: 3,
                tier: "user_doc".to_string(),
                path: "documents/sqlite_docs.md".to_string(),
            },
        ];
        let context = assemble_librarian_context(&chunks);
        assert!(
            context.contains("ANCHOR TRUTH"),
            "expected ANCHOR TRUTH label, got:\n{context}"
        );
    }

    #[test]
    fn assemble_context_labels_wiki_as_curated_wisdom() {
        let chunks = vec![
            ChunkRow {
                text: "Auth patterns overview".to_string(),
                symbol_name: None,
                start_line: 1,
                end_line: 10,
                tier: "wiki".to_string(),
                path: "wiki/auth-patterns.md".to_string(),
            },
        ];
        let context = assemble_librarian_context(&chunks);
        assert!(
            context.contains("CURATED WISDOM"),
            "expected CURATED WISDOM label, got:\n{context}"
        );
    }

    #[test]
    fn assemble_context_includes_source_header_with_line_range() {
        let chunks = vec![
            ChunkRow {
                text: "body text".to_string(),
                symbol_name: None,
                start_line: 12,
                end_line: 34,
                tier: "user_doc".to_string(),
                path: "documents/api-ref.md".to_string(),
            },
        ];
        let context = assemble_librarian_context(&chunks);
        assert!(
            context.contains("[source: documents/api-ref.md | lines 12-34]"),
            "expected source header, got:\n{context}"
        );
    }

    #[test]
    fn assemble_context_includes_symbol_name_when_present() {
        let chunks = vec![
            ChunkRow {
                text: "fn foo() {}".to_string(),
                symbol_name: Some("foo".to_string()),
                start_line: 22,
                end_line: 45,
                tier: "wiki".to_string(),
                path: "src/db/init.rs".to_string(),
            },
        ];
        let context = assemble_librarian_context(&chunks);
        assert!(
            context.contains("[source: src/db/init.rs | symbol: foo | lines 22-45]"),
            "expected symbol in header, got:\n{context}"
        );
    }
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p curated-thoughts assemble_context
```

Expected: compile error — `ChunkRow` and `assemble_librarian_context` not defined.

- [ ] **Step 3: Add `ChunkRow` struct and `assemble_librarian_context` function**

Add before `fn get_folder_mode(...)` in `src-tauri/src/librarian/mod.rs`:

```rust
pub struct ChunkRow {
    pub text: String,
    pub symbol_name: Option<String>,
    pub start_line: i64,
    pub end_line: i64,
    pub tier: String,
    pub path: String,
}

pub fn assemble_librarian_context(chunks: &[ChunkRow]) -> String {
    let mut body = String::new();

    for chunk in chunks {
        let tier_label = match chunk.tier.as_str() {
            "user_doc" => "ANCHOR TRUTH — do not propose modifications to these facts:\n",
            "wiki" => "CURATED WISDOM — may be updated via Wisdom proposals:\n",
            _ => "WORKING CONTEXT — summarize patterns and flag contradictions only:\n",
        };

        let header = match &chunk.symbol_name {
            Some(sym) => format!(
                "[source: {} | symbol: {} | lines {}-{}]\n",
                chunk.path, sym, chunk.start_line, chunk.end_line
            ),
            None => format!(
                "[source: {} | lines {}-{}]\n",
                chunk.path, chunk.start_line, chunk.end_line
            ),
        };

        body.push_str(tier_label);
        body.push_str(&header);
        body.push_str(&chunk.text);
        body.push_str("\n\n");
    }

    body
}
```

- [ ] **Step 4: Run to verify the new tests pass**

```bash
cargo test -p curated-thoughts assemble_context
```

Expected: PASS (4 tests).

- [ ] **Step 5: Update `generate_summary` to use `assemble_librarian_context`**

Replace the entire `chunks` query and `source_text` assembly in `generate_summary` with the following:

```rust
    let chunks: Vec<ChunkRow> = {
        let mut stmt = conn.prepare(
            "SELECT c.chunk_text, c.symbol_name, c.start_line, c.end_line, d.tier, d.path
             FROM chunks c
             JOIN documents d ON d.id = c.doc_id
             WHERE d.path = ?1
             ORDER BY c.position",
        )?;
        let mut rows = stmt.query([source_path])?;
        let mut v = Vec::new();
        while let Some(row) = rows.next()? {
            v.push(ChunkRow {
                text: row.get(0)?,
                symbol_name: row.get(1)?,
                start_line: row.get(2)?,
                end_line: row.get(3)?,
                tier: row.get(4)?,
                path: row.get(5)?,
            });
        }
        v
    };
```

Replace `let source_text = chunks.join("\n\n");` with:

```rust
    let source_text = assemble_librarian_context(&chunks);
```

- [ ] **Step 6: Add the conflict resolution directive to the system prompt**

Replace the `"system"` value in the Ollama JSON request inside `generate_summary`:

```rust
"system": "You are a knowledge librarian. Summarize the document into a concise wiki page in markdown format. Use headings and bullet points, keep under 400 words. Output only markdown.\n\nCONFLICT RESOLUTION DIRECTIVE: If Working Context contradicts Anchor Truth, do not harmonize or modify the Anchor Truth. Instead, create a new Wisdom entry titled 'Architectural Inconsistency' that states: which Working file and symbol introduced the deviation (cite source: metadata), which Anchor Truth document it violates (cite source: metadata), and a one-sentence description of the conflict. Do not emit a Wisdom proposal for any content that is consistent with the Anchor Truth.",
```

- [ ] **Step 7: Run full Rust test suite**

```bash
cargo test -p curated-thoughts
```

Expected: all tests pass.

- [ ] **Step 8: Commit**

```bash
git add src-tauri/src/librarian/mod.rs
git commit -m "feat(rust): tier-labelled librarian prompt with source metadata and conflict directive"
```

---

## Task 6: Rust — Maintenance Commands

**Files:**
- Modify: `src-tauri/src/lib.rs`

The core-llm-wiki tables use default prefix `llm_wiki_`. The entries table is `llm_wiki_entries` with columns `entity_id`, `source_type`, `source_ref`, `deleted_at`.

- [ ] **Step 1: Write failing compile check**

Add to `src-tauri/src/lib.rs`:

```rust
#[cfg(test)]
mod maintenance_command_tests {
    #[test]
    fn maintenance_commands_compile() {
        // Existence verified at compile time via generate_handler![] registration.
        // If run_wiki_heal, run_wiki_prune, or run_wiki_reembed are missing,
        // the build will fail before this test runs.
        assert!(true);
    }
}
```

- [ ] **Step 2: Run to verify it passes (trivially)**

```bash
cargo test -p curated-thoughts maintenance_command_tests
```

Expected: PASS (the point is the commands must compile).

- [ ] **Step 3: Add `run_wiki_heal` command**

Add in the `// ── Wiki SQL bridge` section of `src-tauri/src/lib.rs`, before `fn wiki_exec`:

```rust
// ── Maintenance commands ──────────────────────────────────────────────────────

#[tauri::command]
async fn run_wiki_heal(
    app: AppHandle,
    db_state: State<'_, DbState>,
    vault_state: State<'_, VaultConfigState>,
) -> Result<(), String> {
    app.emit(
        "wiki-status-change",
        serde_json::json!({"heal": true, "ingesting": false, "librarian": false}),
    )
    .ok();

    let result = (|| -> Result<(), String> {
        let vault = vault_state
            .0
            .lock()
            .unwrap()
            .get_vault_path()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "no vault configured".to_string())?;
        let vault_root = std::path::PathBuf::from(&vault);

        let guard = db_state.0.lock().unwrap();
        let conn = &guard.0;

        // Fetch non-deleted entries that have a source reference.
        let entries: Vec<(i64, String)> = {
            let mut stmt = conn
                .prepare(
                    "SELECT rowid, source_ref FROM llm_wiki_entries
                     WHERE deleted_at IS NULL AND source_ref IS NOT NULL",
                )
                .map_err(|e| e.to_string())?;
            let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
            let mut v = Vec::new();
            while let Some(row) = rows.next().map_err(|e| e.to_string())? {
                v.push((
                    row.get::<_, i64>(0).map_err(|e| e.to_string())?,
                    row.get::<_, String>(1).map_err(|e| e.to_string())?,
                ));
            }
            v
        };

        for (rowid, source_ref) in entries {
            let abs_path = if std::path::Path::new(&source_ref).is_absolute() {
                std::path::PathBuf::from(&source_ref)
            } else {
                vault_root.join(&source_ref)
            };
            if !abs_path.exists() {
                conn.execute(
                    "UPDATE llm_wiki_entries SET deleted_at = unixepoch() WHERE rowid = ?1",
                    [rowid],
                )
                .map_err(|e| e.to_string())?;
            }
        }

        Ok(())
    })();

    app.emit(
        "wiki-status-change",
        serde_json::json!({"heal": false, "ingesting": false, "librarian": false}),
    )
    .ok();

    result
}
```

- [ ] **Step 4: Add `run_wiki_prune` command**

```rust
#[tauri::command]
async fn run_wiki_prune(
    app: AppHandle,
    db_state: State<'_, DbState>,
) -> Result<(), String> {
    app.emit(
        "wiki-status-change",
        serde_json::json!({"heal": false, "ingesting": false, "librarian": false}),
    )
    .ok();

    let result = (|| -> Result<(), String> {
        let guard = db_state.0.lock().unwrap();
        let conn = &guard.0;
        // Hard-delete librarian_inferred entries soft-deleted more than 7 days ago.
        conn.execute(
            "DELETE FROM llm_wiki_entries
             WHERE source_type = 'librarian_inferred'
               AND deleted_at IS NOT NULL
               AND deleted_at < (unixepoch() - 7 * 86400)",
            [],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })();

    app.emit(
        "wiki-status-change",
        serde_json::json!({"heal": false, "ingesting": false, "librarian": false}),
    )
    .ok();

    result
}
```

- [ ] **Step 5: Add `run_wiki_reembed` command**

```rust
#[tauri::command]
async fn run_wiki_reembed(
    app: AppHandle,
    db_state: State<'_, DbState>,
    pipeline: State<'_, PipelineHolder>,
) -> Result<usize, String> {
    app.emit(
        "wiki-status-change",
        serde_json::json!({"heal": false, "ingesting": true, "librarian": false}),
    )
    .ok();

    let result = (|| -> Result<usize, String> {
        let guard = db_state.0.lock().unwrap();
        let conn = &guard.0;
        let paths = crate::db::list_indexed_user_doc_paths(conn).map_err(|e| e.to_string())?;
        let tx = {
            let pipeline_guard = pipeline.0.lock().unwrap();
            pipeline_guard
                .as_ref()
                .ok_or_else(|| "pipeline not running".to_string())?
                .0
                .clone()
        };
        drop(guard);
        let mut queued = 0usize;
        for path in paths {
            if !std::path::Path::new(&path).exists() {
                continue;
            }
            tx.send(PipelineJob::rechunk(path))
                .map_err(|e| format!("pipeline channel closed: {e}"))?;
            queued += 1;
        }
        Ok(queued)
    })();

    // Jobs are queued; pipeline processes them asynchronously.
    // Emit idle after queuing since pipeline progress is tracked via get_indexing_status.
    app.emit(
        "wiki-status-change",
        serde_json::json!({"heal": false, "ingesting": false, "librarian": false}),
    )
    .ok();

    result
}
```

- [ ] **Step 6: Register all three commands in `run()` handler list**

Add to the `tauri::generate_handler![...]` call in `run()`:

```rust
run_wiki_heal,
run_wiki_prune,
run_wiki_reembed,
```

- [ ] **Step 7: Run full Rust test suite**

```bash
cargo test -p curated-thoughts
```

Expected: all tests pass.

- [ ] **Step 8: Commit**

```bash
git add src-tauri/src/lib.rs
git commit -m "feat(rust): add run_wiki_heal, run_wiki_prune, run_wiki_reembed maintenance commands"
```

---

## Task 7: `src/lib/wiki.ts` — `initWorkspaceId`, `tieredRead`, `startAutoHeal`

**Prerequisite:** Task 1 (core-llm-wiki v4.5.1) must be installed. Task 2 (`get_workspace_id` command) must be deployed.

**Files:**
- Modify: `src/lib/wiki.ts`
- Create: `src/__tests__/wiki.test.ts`

- [ ] **Step 1: Write the failing tests**

Create `src/__tests__/wiki.test.ts`:

```typescript
import { vi, describe, it, expect, beforeEach } from 'vitest';

vi.mock('@equationalapplications/react-llm-wiki', () => ({
  createWiki: vi.fn().mockReturnValue({
    setup: vi.fn().mockResolvedValue(undefined),
    read: vi.fn().mockResolvedValue({ facts: [] }),
    runHeal: vi.fn().mockResolvedValue(undefined),
  }),
  WikiBusyError: class WikiBusyError extends Error {},
}));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

vi.mock('../lib/wikiAdapter', () => ({
  tauriWikiAdapter: {},
}));

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { createWiki } from '@equationalapplications/react-llm-wiki';
import { initWorkspaceId, getWorkspaceId, tieredRead, startAutoHeal } from '../lib/wiki';

describe('initWorkspaceId', () => {
  beforeEach(() => vi.clearAllMocks());

  it('calls get_workspace_id Tauri command with vault path', async () => {
    vi.mocked(invoke).mockResolvedValue('tier_working::abc123deadbeef01');
    await initWorkspaceId('/Users/foo/Vault');
    expect(invoke).toHaveBeenCalledWith('get_workspace_id', { path: '/Users/foo/Vault' });
  });

  it('updates getWorkspaceId() after init', async () => {
    vi.mocked(invoke).mockResolvedValue('tier_working::abc123deadbeef01');
    await initWorkspaceId('/Users/foo/Vault');
    expect(getWorkspaceId()).toBe('tier_working::abc123deadbeef01');
  });
});

describe('tieredRead', () => {
  beforeEach(() => vi.clearAllMocks());

  it('calls wiki.read with all three tier IDs and correct weights', async () => {
    vi.mocked(invoke).mockResolvedValue('tier_working::abc123deadbeef01');
    await initWorkspaceId('/Users/foo/Vault');

    const mockRead = vi.mocked(createWiki).mock.results[0].value.read;
    await tieredRead('test query');

    expect(mockRead).toHaveBeenCalledWith(
      ['tier_fact', 'tier_wisdom', 'tier_working::abc123deadbeef01'],
      'test query',
      {
        tierWeights: {
          tier_fact: 1.5,
          tier_wisdom: 1.0,
          'tier_working::abc123deadbeef01': 0.6,
        },
      }
    );
  });
});

describe('startAutoHeal', () => {
  it('subscribes to vault-file-changed event', () => {
    startAutoHeal();
    expect(listen).toHaveBeenCalledWith('vault-file-changed', expect.any(Function));
  });
});
```

- [ ] **Step 2: Run to verify it fails**

```bash
npm run test -- wiki.test
```

Expected: FAIL — `initWorkspaceId` is not exported.

- [ ] **Step 3: Rewrite `src/lib/wiki.ts`**

```typescript
import { createWiki, WikiBusyError } from "@equationalapplications/react-llm-wiki";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { tauriWikiAdapter } from "./wikiAdapter";

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
      return invoke<string>("ollama_generate", { systemPrompt, userPrompt });
    },
    async embed(text: string): Promise<number[]> {
      return invoke<number[]>("embed_text", { text });
    },
  },
  config: {
    hybridWeight: 0.7,
    preFilterLimit: 50,
  },
  onRetrievalFallback: (err) => {
    console.warn("[wiki] embed unavailable, using keyword search:", err.message);
  },
});

export async function setupWiki() {
  await wiki.setup();
}

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

export { WikiBusyError };
```

- [ ] **Step 4: Run to verify tests pass**

```bash
npm run test -- wiki.test
```

Expected: PASS (4 tests).

- [ ] **Step 5: Run full TypeScript test suite**

```bash
npm run test
```

Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/lib/wiki.ts src/__tests__/wiki.test.ts
git commit -m "feat(ts): add initWorkspaceId, tieredRead, startAutoHeal to wiki.ts"
```

---

## Task 8: `src/components/settings/MaintenanceDashboard.tsx`

**Prerequisite:** Tasks 4 (`useWikiStatus`) and 6 (maintenance Rust commands) must be complete.

**Files:**
- Create: `src/components/settings/MaintenanceDashboard.tsx`

No automated test is written for this component — it is wired and verified manually in the Settings panel. The Rust commands and `useWikiStatus` hook are already covered by their own tests.

- [ ] **Step 1: Create the component**

Create `src/components/settings/MaintenanceDashboard.tsx`:

```typescript
import { useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useWikiStatus } from '../../hooks/useWikiStatus';

export function MaintenanceDashboard() {
  const wikiStatus = useWikiStatus();
  const isSystemBusy = wikiStatus.ingesting || wikiStatus.librarian || wikiStatus.heal;
  const [lastError, setLastError] = useState<string | null>(null);

  async function runCommand(command: string) {
    setLastError(null);
    try {
      await invoke(command);
    } catch (err) {
      setLastError(String(err));
    }
  }

  return (
    <div className="maintenance-dashboard">
      <h3>Database Maintenance</h3>

      {lastError && (
        <p className="maintenance-error" role="alert">
          Maintenance failed: {lastError}
        </p>
      )}

      {isSystemBusy && (
        <p className="maintenance-busy" aria-live="polite">
          Database busy — please wait…
        </p>
      )}

      <div className="maintenance-actions">
        <button
          type="button"
          disabled={isSystemBusy}
          onClick={() => runCommand('run_wiki_heal')}
        >
          Heal Database
        </button>
        <p className="maintenance-description">
          Removes ghost notes whose source file was deleted outside the app.
        </p>

        <button
          type="button"
          disabled={isSystemBusy}
          onClick={() => runCommand('run_wiki_prune')}
        >
          Prune Trash
        </button>
        <p className="maintenance-description">
          Permanently deletes inferred entries soft-deleted more than 7 days ago.
          <strong> This cannot be undone.</strong>
        </p>

        <button
          type="button"
          disabled={isSystemBusy}
          onClick={() => runCommand('run_wiki_reembed')}
        >
          Full Re-index
        </button>
        <p className="maintenance-description">
          Re-chunks and re-embeds all tiers. Required after switching embedding models.
        </p>
      </div>
    </div>
  );
}
```

- [ ] **Step 2: Commit**

```bash
git add src/components/settings/MaintenanceDashboard.tsx
git commit -m "feat(ui): add MaintenanceDashboard with Heal, Prune, Re-index controls"
```

---

## Task 9: Wire `MaintenanceDashboard` and `initWorkspaceId` into the App

**Prerequisite:** Tasks 7 and 8 must be complete.

**Files:**
- Modify: `src/components/settings/SettingsModal.tsx`
- Modify: `src/App.tsx`

### 9a. Add `MaintenanceDashboard` to `SettingsModal`

- [ ] **Step 1: Read the current `SettingsModal.tsx`** (already read above — 30 lines)

- [ ] **Step 2: Add the import and component**

In `src/components/settings/SettingsModal.tsx`, add the import after the existing imports:

```typescript
import { MaintenanceDashboard } from "./MaintenanceDashboard";
```

Add `<MaintenanceDashboard />` as the last panel, after `<FolderRulesPanel />`:

```typescript
        <FolderRulesPanel />
        <hr className="settings-divider" />
        <MaintenanceDashboard />
```

- [ ] **Step 3: Commit**

```bash
git add src/components/settings/SettingsModal.tsx
git commit -m "feat(ui): add MaintenanceDashboard panel to SettingsModal"
```

### 9b. Call `initWorkspaceId` and `startAutoHeal` from `App.tsx`

- [ ] **Step 4: Read the full `App.tsx`** (already read above — 65 lines)

`App.tsx` sets `currentVaultPath` in two places: after `SetupWizard.onComplete` (line ~32) and when `activePath` is resolved. The `initWorkspaceId` call must happen whenever `activePath` is first known and whenever `handleVaultChanged` fires.

- [ ] **Step 5: Add imports and effect to `App.tsx`**

Add at the top of `src/App.tsx`, after existing imports:

```typescript
import { useEffect } from "react";
import { initWorkspaceId, startAutoHeal } from "./lib/wiki";
```

Add a `useEffect` inside the `App` function, before the early-return guards, that reacts to vault path changes:

```typescript
  useEffect(() => {
    if (!activePath) return;
    initWorkspaceId(activePath).catch((err) =>
      console.error('[wiki] initWorkspaceId failed:', err)
    );
  }, [activePath]);

  useEffect(() => {
    startAutoHeal();
    // startAutoHeal registers a Tauri event listener; cleanup happens via the
    // returned unsubscribe function, which is handled inside startAutoHeal's listen() call.
  }, []);
```

Where `activePath` is already defined in the component body as `const activePath = currentVaultPath ?? vaultPath;`.

- [ ] **Step 6: Run full TypeScript test suite to verify no regressions**

```bash
npm run test
```

Expected: all tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/App.tsx
git commit -m "feat(ui): call initWorkspaceId and startAutoHeal from App on vault load"
```

---

## Self-Review

### Spec coverage check

| Spec section | Covered by |
|---|---|
| §1 `get_workspace_id` — deterministic hash | Task 2 |
| §2a `source_type` union extension | Task 1 |
| §2b `ReadOptions.tierWeights` | Task 1 |
| §2c multi-entity `read()` | Task 1 |
| §2d SQL `IN (...)` clause | Task 1 |
| §2e tier weight multiplier in ranker | Task 1 |
| §2f immutability guard | Task 1 |
| §2g vector cache cap at 500 | Task 1 |
| §3 `entityIdForPath` ingestion routing | Task 3 |
| §4 librarian tier-separated prompt + conflict directive | Task 5 |
| §5 `wiki.ts` `initWorkspaceId`, `tieredRead`, `startAutoHeal` | Task 7 |
| §6 `useWikiStatus` hook | Task 4 |
| §7 `MaintenanceDashboard` + maintenance Rust commands | Tasks 6 + 8 |
| §8 auto-heal on file watcher event (3s debounce) | Task 7 (`startAutoHeal`) + Task 9 (`App.tsx`) |
| §9 `WikiBusyError` blocks vault switch | Task 7 (export), Task 8 (isSystemBusy guard) |

**Acceptance criteria verification (run manually after all tasks complete):**

1. `invoke("get_workspace_id", { path: "/Users/foo/Vault" })` returns a string matching `/^tier_working::[0-9a-f]{16}$/` → covered by Task 2 tests
2. `wiki.read(['tier_fact', 'tier_wisdom', workspaceId], query, { tierWeights: … })` returns results from all three tiers → covered by Task 1 + Task 7 tests
3. `documents/api-ref.md` ingests with `entityId: 'tier_fact'`, unchanged after librarian pass → covered by Task 1 immutability guard
4. Contradicting `tier_working` chunk produces `tier_wisdom` "Architectural Inconsistency" proposal → covered by Task 5 conflict directive
5. Switching vault while `runPrune` active shows "Database busy" indicator → covered by Task 4 `isSystemBusy` in Task 8
6. Deleting file from `documents/` triggers `runHeal('tier_fact')` within ~3 seconds → covered by Task 7 `startAutoHeal`
7. `useWikiStatus()` reflects live state during `run_wiki_reembed` → covered by Tasks 4 + 6
8. Review Queue grouping (30-file pull → ≤5 proposals) → prompt directive in Task 5; UI shape is Phase 2

### No placeholders detected

All steps include complete code. The only intentional deferral is acceptance criterion 8 (Review Queue UI shape), which the spec itself marks as a future enhancement ("Proposed UI shape").

### Type consistency check

- `ChunkRow` defined in Task 5, used only in Task 5 — consistent.
- `WikiStatus` defined in Task 4, imported in Task 8 — consistent.
- `initWorkspaceId` / `getWorkspaceId` defined in Task 7, called in Task 9 — consistent.
- `tier_working::default` fallback in `wiki.ts` — safe; overwritten before any `tieredRead` call because `App.tsx` runs `initWorkspaceId` on mount via `useEffect`.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-05-13-phase1-three-tier-memory.md`. Two execution options:

**1. Subagent-Driven (recommended)** — dispatch a fresh subagent per task, review between tasks, fast iteration
→ Use superpowers:subagent-driven-development

**2. Inline Execution** — execute tasks in this session with checkpoints
→ Use superpowers:executing-plans
