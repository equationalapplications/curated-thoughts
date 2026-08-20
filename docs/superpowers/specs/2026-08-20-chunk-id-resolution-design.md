# Chunk-Id Resolution for Library Deep-Link Highlight (Design Spec)

**Date:** 2026-08-20
**Branch:** TBD
**Status:** Draft (in review)
**Anchored by:**
- `docs/superpowers/specs/2026-08-20-phase-8-design.md` §2 (line 32 caveat) + Open Questions (line 147) — the "chunk-id → block resolution" follow-up that Phase 8 Plan A deferred.
- `docs/superpowers/specs/2026-08-19-phase-7-plan-c-design.md` — Plan A+B shipped `EditorPane` anchor infrastructure at `src/components/shell/EditorPane.tsx:101+`; this spec replaces the heading-text-match mechanism with hash-based line-range overlay.
- `docs/superpowers/specs/2026-07-05-ux-vision-okf-native-design.md` §1 — the cross-link user story (Brain/Review → Library deep-link).

---

## Goal

Close the chunk-id → highlight story that Phase 8 Plan A partially landed. Plan A surfaced numeric `chunkId`s on the wire but the `EditorPane` anchor effect resolved by heading-text match, so numeric ids silently no-op'd. This spec delivers two coupled changes:

1. **Stable chunk identifiers** — content-derived hashing (SHA-256 of chunk text + document path + position) gives chunks a stable id that survives re-ingest line shifts. The current identifier (SQLite autoincrement rowid) shifts on every re-chunk.
2. **Hash-based line-range overlay** — `EditorPane` resolves the hash to a `(startLine, endLine)` range via a new Rust command and overlays a position-absolute highlight on those lines. Replaces the heading-text-match mechanism with a primitive that works for any chunk shape (paragraph, heading, code).

Phase 8 Plans B (peek panels) and C (global ⌘K command palette) are **deferred** until this spec ships. Reason: those plans reuse the same `chunkId` plumbing; introducing a stable hash while B/C are still in flight would force them to be refactored.

---

## Architecture

### Scope (in)

1. **Chunk content hashing** — `chunks.content_hash` column (TEXT, NOT NULL after migration). SHA-256 of `(chunk_text, doc_path, position_in_doc)` truncated to first 16 bytes (32 hex chars). The `(doc_path, position)` tie-break prevents collisions on duplicate chunks in the same doc; cross-doc collisions are also prevented.

2. **Bulk migration on first start** — one-time transaction that re-chunks every document, populates `content_hash`, and rewrites `llm_wiki_entries.source_ref` JSON to use the new hash. A "Optimizing your library..." splash screen blocks the UI until the migration completes. <5 seconds for ~100 docs.

3. **Hash-based read path** — `source_docs_from_ref` joins on `(doc_id, content_hash)` instead of `id`. The wire shape becomes `SourceDocRef { path, chunkId: string | null }` (was `chunkId: number | null`). The frontend flows the hash through `NavTarget.chunkId` to `EditorPane.anchorChunkId` unchanged in shape (already `string`).

4. **New Tauri command `resolve_chunk_overlay`** — `resolve_chunk_overlay(path: String, hash: String) -> Option<ChunkOverlay>` where `ChunkOverlay { startLine: u32, endLine: u32 }`. Indexed lookup on `(doc_id, content_hash)`. Registered in both `invoke_handler` lists in `src-tauri/src/lib.rs`.

5. **Line-range overlay in `EditorPane`** — new `useChunkOverlay` hook replaces the heading-text-match block at `src/components/shell/EditorPane.tsx:121–128`. Resolves the hash, builds a line-to-block map from the BlockNote document, renders a position-absolute overlay. Falls back to a "source may have moved" badge when the hash doesn't resolve or the line range can't be mapped to blocks.

6. **Content-hash on `StoredEvidenceChunk`** — `proposals.rs:48-54` gains `content_hash: String`. `commit.rs:219-225` writes the hash at evidence-commit time so facts carry the stable id from the moment they're created.

### Scope (out)

- **Phase 8 Plan B (peek panels)** — deferred until this spec ships.
- **Phase 8 Plan C (global ⌘K palette)** — deferred until this spec ships.
- **Removing the old `chunks.id` (rowid) column** — kept for one release; a follow-up migration drops it after the migration window.
- **Per-mode sidebar search fields, compact density toggle, group-by-source for tasks, similarity scores** — unchanged from Phase 8 design §Scope (out).
- **Resolving chunkIds inside the peek panel** — Plan B's deep-link story is Phase 8 Plan B; out of scope here.

### Dependency on prior phases

- **Phase 8 Plan A (shipped v1.18.0)** — `EntityFact.source_docs: SourceDocRef[]` shape. Plan A's `chunkId` field is replaced by `chunkId: string` (was `number`), but the field name is preserved for JSON-camelCase compat.
- **Phase 7 Plan A+B (shipped v1.17.0)** — `EditorPane` anchor infrastructure + `EntityFact` v0.2 fields. This spec replaces the heading-text-match mechanism in `EditorPane`.
- **Phase 4** — `useNavigationState` (`src/lib/navigation.ts`) + `NavTarget.chunkId` field. Phase 4's `chunkId: string` typing is preserved; the docstring updates to "SHA-256 first-16-bytes hex."

---

## New Tauri commands

**One new command:**

```rust
#[derive(serde::Serialize)]
struct ChunkOverlay { start_line: u32, end_line: u32 }

#[tauri::command]
fn resolve_chunk_overlay(path: String, hash: String) -> Option<ChunkOverlay> {
    // 1. Resolve doc_id by path.
    // 2. SELECT start_line, end_line FROM chunks WHERE doc_id = ? AND content_hash = ? LIMIT 1.
    // 3. Return Some(ChunkOverlay { ... }) or None.
}
```

Registered in both `invoke_handler` lists in `src-tauri/src/lib.rs` (~line 2204 app builder, ~line 2400 `make_test_app`). No new commands for the migration itself — the migration runs server-side at startup as a transactional step (`run_chunk_hash_migration`).

---

## New design tokens

**None.** The line-range overlay reuses the existing `accent` color token with low opacity and the existing `slide-in-right` motion token. The "source may have moved" badge uses `text-muted` for the color and `card` for the surface.

---

## Components

### Rust (`src-tauri/`)

- **`src-tauri/src/db/schema.rs`** — migration V7: `ALTER TABLE chunks ADD COLUMN content_hash TEXT NOT NULL DEFAULT ''; CREATE UNIQUE INDEX idx_chunks_doc_hash ON chunks(doc_id, content_hash);`. Migration V8 (one release later): `content_hash TEXT NOT NULL` (drop the default), then drop the old `id` index.

- **`src-tauri/src/db/chunk_hash.rs`** (new) — `compute_chunk_hash(text: &str, doc_path: &str, position: usize) -> String`. Pure function. SHA-256 over `(text || doc_path || position_as_le_u64)`; returns first 16 bytes as 32 hex chars. Unit-testable.

- **`src-tauri/src/db/queries.rs`** —
  - `insert_chunk` takes a precomputed `content_hash: &str` and writes it (was: caller did not write hash).
  - `find_chunk_overlay(conn, doc_id: i64, hash: &str) -> Option<(u32, u32)>` (new) — `SELECT start_line, end_line FROM chunks WHERE doc_id = ? AND content_hash = ? LIMIT 1`, returns `Option<(start_line, end_line)>`.

- **`src-tauri/src/db/entities.rs`** —
  - `SourceDocRef` (lines 101–110) renames `chunk_id` → `chunk_hash` with `#[serde(rename = "chunkId")]` (camelCase preserved). Type changes from `Option<i64>` to `Option<String>` so serde serializes to JSON `null` natively (no empty-string sentinel).
  - `source_docs_from_ref` (lines 191–221) reads `evidence[*].content_hash` (new field) and joins on `(doc_id, content_hash)` instead of `id`. Inner tuple is `(String, Option<String>)` (path, hash).

- **`src-tauri/src/db/proposals.rs`** — `StoredEvidenceChunk` (lines 48–54) gains `content_hash: String`.

- **`src-tauri/src/db/commit.rs`** (lines 219–225) — writes `content_hash` on each evidence entry at commit time. Reads the hash from the joining chunk's `content_hash` (computed during the chunk insert step).

- **`src-tauri/src/db/migration.rs`** (new) — `run_chunk_hash_migration(conn, emit_progress: impl Fn(usize, usize))`:
  1. Wraps the entire migration in a single SQLite transaction.
  2. Iterates all documents: re-runs the chunker on each, computes hashes, writes them to `chunks.content_hash`.
  3. Rewrites `llm_wiki_entries.source_ref` JSON: for each `evidence` entry, populate `content_hash` from the chunk already linked (by old rowid).
  4. Emits `migration-progress` events with `{ current, total }`.
  5. On any error, rolls back the transaction; surfaces the error to the splash screen.
  - Idempotent: checks if all chunks for a doc have non-empty `content_hash` and skips.

- **`src-tauri/src/lib.rs`** — startup sequence:
  1. Open the database.
  2. Run schema migrations V7.
  3. Check `chunks.content_hash` population via a sentinel query.
  4. If population is incomplete, run `run_chunk_hash_migration` synchronously inside a `tauri::async_runtime::spawn_blocking` task. The frontend shows the splash screen until the emitted `migration-complete` event fires.
  5. Register the new `resolve_chunk_overlay` command in both `invoke_handler` lists.

- **`src-tauri/src/commands/chunks.rs`** (new) — thin wrapper around `find_chunk_overlay`. Resolves `path → doc_id` then calls the query. Returns `Option<ChunkOverlay>`.

- **`src-tauri/src/pipeline/mod.rs`** — **unchanged**. The lazy per-doc migration check was considered and dropped; the startup migration is atomic from the UI's perspective (see Architecture §5).

- **Tests** — see "Testing" below.

### Frontend (`src/`)

- **`src/lib/tauri.ts`** —
  - `SourceDocRef.chunkId` type: `string | null` (was `number | null`).
  - New binding: `resolveChunkOverlay(path: string, hash: string): Promise<{ startLine: number; endLine: number } | null>`.

- **`src/lib/navigation.ts`** — `NavTarget.chunkId?` is now `string` (already was `string`; docstring updated to "SHA-256 first-16-bytes hex").

- **`src/components/shell/EditorPane.tsx`** —
  - The anchor effect at lines 121–128 (heading-text match) is **replaced**.
  - New `useChunkOverlay(path, anchorChunkId)` hook:
    1. Empty hash → no-op (no badge, no overlay, no scroll).
    2. Calls `resolveChunkOverlay(path, hash)`. Receives `Option<{ startLine, endLine }>`.
    3. On `null`: renders the "source may have moved" badge. No scroll, no overlay.
    4. On `Some(...)`: builds a line-to-block map from the BlockNote document, computes the overlay's `top` and `height` (mapping markdown line numbers to BlockNote block DOM elements), renders a position-absolute overlay.
  - **BlockNote line-to-block mapping**: BlockNote doesn't natively store Markdown line numbers in its block objects. The implementation must intercept the initial Markdown-to-BlockNote parse phase (via remark/rehype or the existing AST walker) and inject `startLine`/`endLine` into each block's `props` at parse time. The map is built from those props, keyed by `BlockNoteBlock.id`, on doc-load. The map is cached in a `useRef` and reused for every overlay render.
  - **Overlay scroll tracking**: subscribes to the editor container's `scroll` event and updates the overlay's `top` to account for editor-internal scroll. Auto-dismisses after 1.5s (same as the current block-highlight timer).
  - **Line-to-block mapping failure** (e.g., `startLine` past EOF after a doc edit): the hook falls back to the "source may have moved" badge.

- **`src/components/shell/EditorPane.tsx`** — the existing `blockText` helper (lines 23–35) is **removed** (no longer used).

- **`src/components/brain/FactCard.tsx`** —
  - The `String(...)` cast on line 105 is dropped (the value is already a string).
  - The v1 docstring on `onOpenSource` (lines 19–24) is dropped (the heading-text-match limitation no longer applies).

- **`src/components/shell/SplashScreen.tsx`** (new) — rendered at app root when `migration-progress` events are firing. Shows "Optimizing your library..." with a progress bar. On `migration-complete` event, unmounts. On error, renders the error message with a "Restart to retry" CTA.

- **`src/components/shell/AppShell.tsx`** — mounts `<SplashScreen>` alongside the existing `<ActivityFeedPanel>`. The splash screen controls whether the rest of the UI is rendered (or renders a `null` placeholder over the rest until `migration-complete` fires).

- **`src/index.css`** —
  - Adds `.editor-pane-line-overlay--anchor` (uses `accent` color with low opacity, animates with `slide-in-right` motion token, `position: absolute`, `pointer-events: none`).
  - Adds `.editor-pane-source-moved-notice` (thin chrome strip at the top of the editor, uses `text-muted` color, dismissable via X button).
  - Existing `.editor-pane-block--anchor-highlight` is **removed** (no longer used).

---

## Data model

### Schema

```sql
-- V7
ALTER TABLE chunks ADD COLUMN content_hash TEXT NOT NULL DEFAULT '';
CREATE UNIQUE INDEX idx_chunks_doc_hash ON chunks(doc_id, content_hash);

-- V8 (one release later)
-- content_hash TEXT NOT NULL (drop the default)
-- DROP INDEX IF EXISTS <old_id_index>;
```

### Hash function

```rust
// src-tauri/src/db/chunk_hash.rs
pub fn compute_chunk_hash(text: &str, doc_path: &str, position: usize) -> String {
    // SHA-256 over (text || doc_path || position_as_le_u64)
    // Return first 16 bytes as 32 hex chars.
}
```

`position` is the 0-indexed position of the chunk in the chunker's output array for that document (i.e., the value passed to `insert_chunk` as `i` in `src-tauri/src/pipeline/mod.rs:560`). It is **not** the markdown line number.

**Chunker determinism requirement.** The hash is stable only if the chunker enumerates chunks in the same order for a given document. The existing chunker (`src-tauri/src/chunker/mod.rs`) is deterministic for a given input — strategies (`AstSymbol`, `Prose`, `CodeLike`, `Declarative`, `Fallback`) iterate the AST/text in source order and emit chunks in iteration order. The hash short-circuit in `pipeline/mod.rs:518-520` (skip re-chunk when `documents.hash == hash_bytes`) guarantees that a chunk is only re-inserted if the file changes, so the position tie-break is reliable on stable content. If the chunker is ever changed to non-deterministic chunk-ordering, the hash strategy breaks silently — a comment on `chunk_hash.rs` flags this as a guard.

### Wire shape

```rust
// src-tauri/src/db/entities.rs
pub struct SourceDocRef {
    pub path: String,
    #[serde(rename = "chunkId")]
    pub chunk_hash: Option<String>,  // None → JSON null
}
```

```ts
// src/lib/tauri.ts
interface SourceDocRef {
  path: string;
  chunkId: string | null;  // hex SHA-256 first-16-bytes; null = no chunk
}
```

### Evidence entry

```rust
// src-tauri/src/db/proposals.rs
pub struct StoredEvidenceChunk {
    pub chunk_id: Option<i64>,         // legacy; nullable after migration
    pub content_hash: String,          // new stable id
    pub quote: String,
    pub start_line: Option<i32>,
    pub end_line: Option<i32>,
    pub source_kind: Option<String>,
}
```

### Tauri command response

```rust
#[derive(serde::Serialize)]
struct ChunkOverlay {
    start_line: u32,
    end_line: u32,
}
```

---

## Data flow at click time

1. User clicks a fact chip in `FactCard` (line 99).
2. `onOpenSource(path, chunkId)` fires, where `chunkId` is the 32-char hex hash.
3. `AppShell` (line 232) builds `NavTarget { mode: "library", docPath: path, chunkId }`.
4. `LibraryMode` (line 17) reads `nav.current.chunkId` and passes it as `anchorChunkId` to `EditorPane`.
5. `EditorPane` `useChunkOverlay(path, anchorChunkId)`:
   - Empty hash → no-op.
   - Calls `resolveChunkOverlay(path, hash)`. Receives `Option<{ startLine, endLine }>`.
   - On `null`: renders the "source may have moved" badge. No scroll, no overlay.
   - On `Some(...)`: builds a line-to-block map from the BlockNote document (built once on doc-load via markdown line numbers injected into BlockNote block props during parse). For each line in `startLine..endLine`, finds the BlockNote block whose injected `startLine`/`endLine` props contain that line. Computes the overlay's `top` (min of child block `getBoundingClientRect().top`) and `height` (max of `bottom` − min `top`). Renders the position-absolute overlay. Subscribes to editor scroll and updates `top` to account for editor-internal scroll. Auto-dismisses after 1.5s.
6. The line-to-block map is built on doc-load (the existing `useEffect` at lines 53–95) by reading the BlockNote document model's per-block `startLine`/`endLine` props. The map is cached in `useRef` and reused across overlay renders.

---

## Error handling

- **Hash lookup returns `null`** (chunk no longer exists with that hash): the editor renders the "source may have moved" badge. No scroll, no overlay. The badge is a thin chrome strip at the top of the editor — `<div className="editor-pane-source-moved-notice">` — using `text-muted` color, dismissable by clicking the X.
- **Line range cannot be mapped to blocks** (e.g., `startLine` past EOF after a doc edit): the editor falls back to the "source may have moved" badge. No overlay, no scroll.
- **Rust invoke fails** (e.g., IPC error, app crash): `useChunkOverlay` falls back to the "source may have moved" badge. No silent no-op.
- **Migration fails mid-flight**: the splash screen stays up with an error message ("Migration failed: `<details>`. Please restart to retry."). The transaction is rolled back; the corpus is unchanged. The user can retry by relaunching.
- **Migration is slow (>5s for ~100 docs)**: code review flags the implementation — likely an N+1 query or per-row commit bug. The user with 5,000 notes would otherwise be staring at the splash screen for an hour.
- **Empty content_hash** (transient state during the migration window): treated as `null` on the wire; UI no-ops. Only possible during the brief window between V7 schema apply and migration commit.

---

## Testing

### Rust unit tests (`src-tauri/src/db/chunk_hash.rs`)

- `compute_chunk_hash_is_deterministic_for_same_input` — `(text, path, 0)` → same hash every call.
- `compute_chunk_hash_differs_on_text_change` — same `path` + `position`, different text → different hash.
- `compute_chunk_hash_differs_on_position_change` — same text + `path`, different position → different hash. (Guards the position tie-break.)
- `compute_chunk_hash_differs_on_path_change` — same text + position, different path → different hash. (Guards cross-doc collisions.)
- `compute_chunk_hash_returns_32_hex_chars` — output is the right shape.

### Rust unit tests (`src-tauri/src/db/entities.rs`)

- `source_docs_from_ref_uses_content_hash_for_join` — fixtures with `content_hash` populated; `source_docs_from_ref` reads from `evidence[*].content_hash` and joins on `(doc_id, content_hash)`, not `id`.
- Existing tests updated to populate `content_hash` on evidence entries and on the joining chunks table rows. The `source_docs_from_ref_dedupes_paths` test asserts first-seen-hash stability (replaces the old rowid-based assertion).
- `source_docs_from_ref_skips_evidence_with_empty_content_hash` — pre-migration writes have empty hashes; they are skipped.

### Rust integration tests (`src-tauri/src/db/queries.rs`)

- `find_chunk_overlay_returns_line_range_by_hash` — insert a chunk with known `(start_line, end_line)`; query by `(doc_id, content_hash)`; assert equality.
- `find_chunk_overlay_returns_none_for_unknown_hash` — query with a hash that doesn't exist → `None`.
- `find_chunk_overlay_returns_none_for_missing_doc` — query with a path not in `documents` → `None`.

### Rust migration tests (`src-tauri/src/db/migration.rs`)

- `run_chunk_hash_migration_populates_content_hash_on_all_chunks` — N docs, each with M chunks; after migration, every chunk has a non-empty `content_hash`.
- `run_chunk_hash_migration_rewrites_source_ref_json` — pre-migration facts have `evidence[*].chunk_id` populated (old); post-migration, the same facts have `evidence[*].content_hash` populated (new).
- `run_chunk_hash_migration_is_idempotent` — run twice; assertions hold on the second pass.
- `run_chunk_hash_migration_emits_progress_events` — mock the event sink; assert `{ current, total }` events fire in increasing order.
- `run_chunk_hash_migration_rolls_back_on_failure` — inject a failure mid-corpus; assert that `chunks.content_hash` is unchanged and `source_ref` is unchanged (rolled back).
- `run_chunk_hash_migration_completes_under_5_seconds_for_100_docs` — performance guard; fails the test if the implementation regresses.

### Frontend component tests (`src/__tests__/`)

- **`FactCard.test.tsx`** — update the existing `source chip passes the enriched chunkId to onOpenSource` test to assert the hash string is passed through unchanged (no `String(...)` cast).
- **`EditorPane.test.tsx`** — replace the heading-text-match tests with hash-based tests:
  - `renders line-range overlay when resolveChunkOverlay returns line range` — mock returns `Some({ startLine: 10, endLine: 15 })`; assert overlay is positioned with `top`/`height` computed from the line-to-block map.
  - `renders source-moved-notice when resolveChunkOverlay returns null` — mock returns `None`; assert the badge is visible, no overlay, no scroll.
  - `renders nothing when anchorChunkId is null` — mock is never called; no badge, no overlay.
  - `renders source-moved-notice when line range cannot be mapped to blocks` — mock returns `Some({ startLine: 900, endLine: 910 })` against a doc with 50 lines; assert the badge renders, no broken overlay.
  - `auto-dismisses overlay after 1.5s` — using `jest.useFakeTimers()`. Reuses the existing test pattern.
- **`useChunkOverlay.test.ts`** (new) — hook-level tests:
  - `calls resolveChunkOverlay on mount with the path and hash` — asserts the Tauri invoke is made with the right args.
  - `re-fetches when anchorChunkId changes` — re-render with a new hash; assert the second invoke fires.
  - `falls back to source-moved-notice on invoke error` — invoke rejects; assert the badge renders.
- **`SplashScreen.test.tsx`** (new) — listens to `migration-progress` events; renders progress bar; renders error state when the event reports failure.

### Fixture updates (existing tests)

- All `source_docs: ["documents/notes.md"]` literals → `source_docs: [{ path: "documents/notes.md", chunkId: null }]` (already done by Phase 8 Plan A).
- All `source_docs: [{ path: "documents/notes.md", chunkId: 42 }]` → `source_docs: [{ path: "documents/notes.md", chunkId: "<hash-string>" }]`. Fixtures that previously asserted behavior with `chunkId: 42` are updated to use a real hash from the test chunk fixture.

### Manual verification

- Bulk migration on a corpus of ~100 docs completes in **<5 seconds** on a dev machine. Anything slower is flagged in code review.
- After migration, clicking a fact chip scrolls to the chunk's line range and overlays the highlight.
- After editing a doc (changing chunk text), the original fact's hash no longer resolves; the badge appears.
- Migrating twice does not double-populate the column.
- Editing a doc after migration does not revert its chunks to empty `content_hash`.

### E2E (Playwright, if available)

- Click a fact chip in Brain → navigate to Library → see the line overlay → click dismiss → see the doc as normal.

---

## Non-goals (unchanged from Phase 8 spec §Non-goals)

- Graph visualization view (deferred; backlinks + links cover navigation).
- Auto-approve rules for the librarian.
- Due dates, kanban, or any project-management depth in Tasks.
- Settings search.
- Wiring reject-reasons into librarian tuning.

## Open Questions / Deferred

- **Removing the old `chunks.id` rowid column** — V8 migration, one release after this spec ships.
- **Phase 8 Plan B (peek panels)** — deferred until this spec ships. Architecturally independent but blocked by chunkId-shape stability.
- **Phase 8 Plan C (global ⌘K command palette)** — deferred until this spec ships. Same reason.
- **Per-mode sidebar search fields, compact density toggle, group-by-source for tasks, similarity scores** — unchanged from Phase 8 design.
- **Resolving chunkIds inside the peek panel** — emerges as a follow-up when Plan B lands.
