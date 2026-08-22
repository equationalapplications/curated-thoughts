# Phase 8 Plan B — Peek Panels v2 (Chunk-Slice) Design Spec

**Date:** 2026-08-21
**Branch:** `phase-8-plan-b-peek-panels`
**Status:** Implemented 2026-08-21 (PR #44)
**Anchored by:**
- `docs/superpowers/specs/2026-08-20-phase-8-design.md` §Scope item 3 — **superseded by this spec** for the document/chunk side (that section's `isPeek`-on-`EditorPane` full-doc approach predates Phase 9; see "Relationship to prior specs").
- UX vision spec line 37 (canonical): *"Peek panels are read-only and dismiss on `Esc` or click-outside; 'Open in [mode]' inside the peek promotes it to full navigation."*
- `docs/superpowers/specs/2026-08-20-chunk-id-resolution-design.md` (shipped v1.19.0, PR #43) — supplies the content-hash chunk ids this feature fetches by, and the `resolve_chunk_overlay` command whose shape this spec mirrors.
- Phase 7 deferred item #6 (`ReviewMode.tsx:455` drops `chunkId`) — disposition updated here; see §Phase 7 item #6.

---

## Goal

Option+click a fact's source chip and a slide-over panel peeks at the **exact passage** the fact came from — the chunk text fetched from the database by content hash — without leaving the current mode. `Esc` or click-outside dismisses it; "Open ↗" promotes to full Library navigation carrying the hash, so Phase 9's overlay highlight fires on arrival.

This is the v2 of Plan B. The August 20 phase-8 design described mounting a read-only `EditorPane` (whole document, BlockNote) inside the panel; that was written before chunk-id resolution existed. v2 renders only the resolved chunk slice, which is lighter (no second live editor), more faithful (text comes straight from the `chunks` table — exactly what the librarian embedded), and free of the client-side line-map drift cases documented in `EditorPane.tsx:36–42`.

## Relationship to prior specs

- `2026-08-20-phase-8-design.md` §Scope item 3 + §Components (`<PeekPanel>` bullet) are superseded as follows: document peeks render a DB-fetched chunk slice instead of an `isPeek` mini-editor; wikilink triggers and entity-kind peeks move from "in scope" to **deferred**; review-evidence triggers move from "in scope" to **out of scope** (hashes do not exist pre-commit). Its accessibility contract (dialog role, focus trap, focus restore) carries over unchanged.
- A status-note edit to that spec's Plan B row points here (same PR as this spec's implementation).
- `docs/superpowers/plans/2026-08-20-phase-8-plan-b.md` (pre-Phase-9) is architecturally superseded and is deleted in the same PR that writes the replacement plan, per the plans-are-ephemeral convention.

## Scope (in)

1. **New read-only Tauri command `fetch_chunk_content`** returning chunk text by `(path, hash)`.
2. **`<PeekPanel>`** — right-edge slide-over owned by `AppShell`; one peek at a time; `Esc`/click-outside dismiss; focus trap; "Open ↗" promotion.
3. **Trigger wiring** — Option+click on fact source chips only (`BrainMode → EntityPage → FactCard`); plain clicks unchanged everywhere.
4. **Promotion with highlight** — promote navigates to Library with `chunkId`, reusing the existing anchor-overlay path end-to-end.
5. **Docs housekeeping** — status-note edit to `2026-08-20-phase-8-design.md`; deletion of the stale plan doc (with the new plan, not this spec).

## Scope (out)

- **Wikilink triggers and entity-kind peeks** — deferred to a follow-up; would need either whole-doc mode or an embedded read-only `EntityPage`. Recorded as an open deferral, not abandoned.
- **Review-mode peeking** — `proposal.source_doc_paths` carries paths only until commit; no hashes exist pre-commit, so no `onPeek*` prop is threaded into `ReviewMode` / `ReviewEvidencePanel` in v1. Alt+click there behaves exactly like a plain click (navigates).
- **Toast/notification infrastructure** — not added; the no-hash rule makes fallback silent-by-design (see §Dispatch rule).
- **Markdown rendering engine in the panel** — chunk text renders `pre-wrap` plain; a peek shows the passage, not a reader.
- **Any change to `useChunkOverlay` / `EditorPane`** — untouched.

## New Tauri command

```rust
// src-tauri/src/commands/chunks.rs — beside resolve_chunk_overlay_cmd
#[tauri::command]
pub fn fetch_chunk_content_cmd(db: State<DbState>, path: String, hash: String)
    -> Result<Option<String>, String>
```

- Query lives beside its sibling: `find_chunk_text(conn, path, hash) -> Result<Option<String>>` in `src-tauri/src/db/queries.rs`:

```sql
SELECT c.chunk_text
FROM chunks c JOIN documents d ON d.id = c.doc_id
WHERE d.path = ?1 AND c.content_hash = ?2
LIMIT 1
```

- **Why `(path, hash)` and not hash alone:** the only unique index is `idx_chunks_doc_hash (doc_id, content_hash)`; a hash-only `WHERE` cannot use it. The join mirrors `find_chunk_overlay` (`queries.rs:191`) — same index, same shape — and scopes the lookup to the document being peeked rather than relying on global hash uniqueness.
- `Ok(None)` means the hash no longer resolves ("source moved"); `Err` means a real backend failure. The two surface distinctly in the panel.
- Registered in **both** invoke-handler lists (`src-tauri/src/lib.rs` ~line 2346 real app, ~line 2671 `make_test_app`).
- Frontend wrapper in `src/lib/tauri.ts`: `fetchChunkContent(path: string, hash: string): Promise<string | null>`.

## Components

### `<PeekPanel>` (new — `src/components/shell/PeekPanel.tsx`)

Exports `type PeekTarget = { path: string; hash: string }`.

Props:

```ts
{
  target: PeekTarget | null;
  onDismiss: () => void;
  onPromote: (path: string, hash: string) => void;
}
```

Rendered by `AppShell` only while `target != null`, as a sibling after `.app-body` alongside `ActivityFeedPanel` (~`AppShell.tsx:300`). Conditional mount keeps every listener/lifecycle inactive when closed — the ActivityFeedPanel pattern.

**Chrome** (mirrors `ActivityFeedPanel.tsx:36–52`):

- Full-viewport backdrop `<button className="peek-backdrop">` — clicking it *is* click-outside dismiss.
- `<aside className="peek-panel" role="dialog" aria-modal="true" aria-label="Source peek: <basename>">`, fixed right edge, `width: min(420px, 92vw)`, `bottom: 26px` above the StatusBar (matching ActivityFeedPanel), z-index 50 (backdrop) / 51 (panel).

**Lifecycle:**

- On mount: `fetchChunkContent(path, hash)` → body state (below). Focus moves into the panel, to the "Open ↗" button. The opener element is captured by `AppShell` at dispatch time (`document.activeElement`) and restored on unmount.
- `Tab` / `Shift+Tab` cycle within the panel (real focus trap — required because `aria-modal="true"` promises screen readers focus stays inside).
- One `window` keydown listener while open routes `Esc` → `onDismiss`; the backdrop click uses the same `onDismiss`.

**Body states:** loading · ready (`chunk_text` rendered `white-space: pre-wrap` with card tokens) · **not-found** ("source moved" notice, same copy family as EditorPane's ~`EditorPane.tsx:231`) · error (backend failure notice). Not-found and error keep the panel open and dismissible.

**Header:** document basename (full vault-relative path as `title`), "Open ↗" button.

**Promotion:** `onPromote(path, hash)` → AppShell dismisses the panel and calls `nav.navigate({ mode: "library", docPath: path, chunkId: hash })`. The existing chain (`NavTarget.chunkId` → `LibraryMode.anchorChunkId` → `EditorPane` effect → overlay) highlights the passage on arrival. That overlay auto-dismisses after ~1.5 s today; promotion inherits exactly the deep-link behavior a plain chip click already has — deliberate consistency, not a regression.

### `AppShell` (modify)

- Owns `const [peekTarget, setPeekTarget] = useState<PeekTarget | null>(null)`.
- `handlePeekSource(path, chunkId)`: guards `if (!chunkId) return` (unreachable via the FactCard dispatch rule, but keeps `PeekTarget.hash: string` honest), captures `document.activeElement` as the opener, then `setPeekTarget({ path, hash: chunkId })`.
- `handlePromote(path, hash)`: `setPeekTarget(null)` then navigate as above.
- Threads `onPeekSource?: (path: string, chunkId: string | null) => void` down through `BrainMode → EntityPage → FactCard` (all optional props; `ReviewMode` deliberately receives none).

### `FactCard` (modify)

- Gains optional `onPeekSource?`.
- Chip `onClick` becomes the single enforcement point of the dispatch rule below.

**Dispatch rule:** `e.altKey && onPeekSource && doc.chunkId ? onPeekSource(doc.path, doc.chunkId) : onOpenSource(doc.path, doc.chunkId)`. Alt+click without a hash behaves *exactly* as today — the null-hash case can never open a degraded peek, anywhere, by construction.

### CSS (`src/index.css`)

`.peek-backdrop` / `.peek-panel` rules next to the activity-feed styles; ad-hoc `@keyframes peek-slide-in` (the codebase has no motion tokens) with a `prefers-reduced-motion` guard; existing tokens only (`--bg`, `--outline-var`, `--shadow-lg`, radius tokens).

## Error handling

| Case | Source | Panel behavior |
|---|---|---|
| Hash resolves | `fetch_chunk_content` → `Some(text)` | passage rendered |
| Source moved / chunk deleted | → `None` | not-found notice; panel stays open |
| IPC/backend failure | wrapper rejection | error notice; panel stays open |
| No hash on the ref (alt+click) | FactCard dispatch rule | never reaches the panel — plain navigation |

All failures are contained inside the panel; nothing throws upward, `Esc` always works.

## Testing

**Rust** (`queries.rs` `#[cfg(test)]` trio mirroring `find_chunk_overlay`'s at `queries.rs:347`):
- `find_chunk_text_returns_text_by_path_and_hash`
- `find_chunk_text_returns_none_for_unknown_hash`
- `find_chunk_text_returns_none_for_missing_doc`
Plus one IPC integration test via `TestApp` beside `resolve_chunk_overlay`'s (invoke → assert text payload; unknown hash → null).

**Frontend** (Vitest + Testing Library, jsdom):
- `__tests__/PeekPanel.test.tsx` (new): renders fetched text; `Esc` dismisses; backdrop click dismisses; "Open ↗" calls `onPromote(path, hash)`; focus moves to "Open ↗" on mount and returns to the opener on unmount; basic Tab-cycle trap; not-found state renders the notice.
- `__tests__/FactCard.test.tsx` additions: alt+click invokes `onPeekSource(path, chunkId)`; plain click still invokes `onOpenSource`; alt+click with `chunkId: null` falls back to `onOpenSource`; absent `onPeekSource` prop → alt+click behaves as plain click.
- `src/test-setup.ts`: add a `fetch_chunk_content` dispatch to the canned invoke mock (it currently falls through to `null`).

**Gates:** `cargo test --features test-utils,mcp-server` · `pnpm test` · `pnpm typecheck` · `pnpm lint`.

## Phase 7 deferred item #6 — disposition

Item #6 ("ReviewMode call site drops `chunkId`") stays deferred with its reason updated: the original gate — *"wait until source_docs carries chunk ids"* — is now satisfied for **committed** facts (FactCard passes the hash through since Phase 9), but review evidence gains hashes only at commit time, and v1 deliberately keeps peek out of the review queue. The `ReviewMode.tsx:455` one-arg lambda remains correct behavior. When proposals start carrying per-chunk hashes pre-commit, threading `chunkId` through `ReviewEvidencePanel → ReviewMode → onOpenSource` becomes the unblocking step — and the peek trigger can ride along.

## Non-goals

- Graph visualization, auto-approve rules, kanban/Tasks depth, settings search, reject-reason tuning (unchanged from phase-8 design §Non-goals).
- Compact density toggle, per-mode sidebar search fields, similarity scores (still Phase 9+ deferrals of the phase-8 design).

## Open Questions

None blocking. Deferred follow-ups recorded above: entity/wikilink peeks, review-queue peeking once evidence carries hashes.
