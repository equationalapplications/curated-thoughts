# Phase 8 — Chunk-id Plumbing, Peek Panels, Global ⌘K Palette (Design Spec)

**Date:** 2026-08-20
**Branch:** `chore/phase-8-design-and-status-polish` (this spec is the only artifact in that branch)
**Status:** Plan A (chunk-id plumbing) shipped; Plan B (peek panels) and Plan C (global ⌘K palette) pending
**Anchored by:**
- `docs/superpowers/specs/2026-07-05-ux-vision-okf-native-design.md` §1 (line 33 — global command palette; line 37 — peek views), §"Phasing" item 7 deferral rows (lines 186–187).
- `docs/superpowers/specs/2026-08-19-phase-7-plan-c-design.md` lines 41, 43–45 (Phase 8 sub-deferrals enumerated at end of Phase 7 planning).
- `docs/superpowers/plans/2026-08-19-phase-7-plan-c.md` (shipped v1.17.0; this spec depends on its `EditorPane` anchor + `EntityFact` v0.2 surfaces).

---

## Goal

Phase 8 ships the three Phase 1/4/7 deferrals that remain open — **chunk-level Library deep-link highlight**, **peek panels**, **global ⌘K command palette** — gated by a small Rust plumbing change that surfaces `chunk_id` from `source_docs_from_ref`. Together they close the cross-link story (Brain/Review → Library deep-linking, with chunk ids delivered end-to-end to the existing highlight effect — id-to-block resolution itself is a follow-up spec; see the Scope §2 caveat) and the editorial-flow story (peek at a source without leaving the current mode). Compact density toggle remains Phase 9.

**One design, three plans (mirroring Phase 7's Plan A/B/C layering):**
- **Plan A — chunk-id plumbing + chunk highlight.** Rust signature change; `EntityFact.source_docs` shape change; closes Phase 7 deferred #6.
- **Plan B — peek panels.** New `<PeekPanel>` + Option+click dispatch + `isPeek` read-only mode on `EditorPane` / `EntityPage`.
- **Plan C — global ⌘K command palette.** New `<CommandPalette>` + static command registry + global key listener.

The split lets each plan's diff stay narrow enough for a focused code-review pass; Plan A unblocks B and C, B and C depend on A.

---

## Architecture

### Scope (in)

1. **Chunk-id plumbing (Plan A).** `source_docs_from_ref` returns `Vec<(String, Option<i64>)>` (path, chunk_id) instead of `Vec<String>`. `EntityFact.source_docs` becomes `Array<{ path: string; chunkId: number | null }>`. Closes Phase 7 deferred #6. The frontend target side is pre-wired — `NavTarget.chunkId` (`src/lib/navigation.ts:16`) → `LibraryMode.anchorChunkId` (line 254) → `EditorPane` effect (`src/components/shell/EditorPane.tsx:101+`) → `.editor-pane-block--anchor-highlight` (CSS at `src/index.css:1158`) — so §1 delivers ids all the way to the highlight effect. The final hop resolves by heading-text match, not by id, so numeric chunk ids do not light up yet — see the caveat in §2.

2. **Chunk-level Library deep-link highlight (Plan A + Plan B prep).** Already plumbed through the entire chain; no new UI work in Plan A. **Caveat — resolution is heading-text match, not id match.** The `EditorPane` anchor effect (`src/components/shell/EditorPane.tsx:121–128`) resolves `anchorChunkId` by exact-matching it against *heading block text* (`b.type === "heading" && blockText(b) === anchorChunkId`). That is reliable when callers pass a heading *name* as the anchor; the numeric `chunkId`s Plan A surfaces will not match any heading text and the effect silently no-ops (`if (!target) return` — no highlight, no scroll). Plan A therefore lands the id at the effect end-to-end but does not, by itself, make the highlight fire. Making numeric chunk ids resolve reliably needs something like: chunk ids resolved to line ranges, then searching for a line-number match; or storing chunk heading names alongside the id in `source_docs`; or changing the highlight mechanism from "block text match" to "line range scroll". All of those are new UI/IPC surface beyond Plan A's constraints — deferred to a follow-up spec (see "Open Questions" below). Plan A's constraints remain intact.

3. **Peek panels (Plan B).** New `<PeekPanel>` slide-over read-only surface; one peek at a time, owned by `AppShell`; `Esc` and click-outside dismiss; "Open in [mode]" button promotes to real navigation. Triggered by `Option`+click on wikilinks, fact chips, evidence chunks. Reuses `EditorPane` and `EntityPage` with an `isPeek` prop. Spec line 37 is canonical: "Peek panels are read-only and dismiss on `Esc` or click-outside; 'Open in [mode]' inside the peek promotes it to full navigation."

4. **Global �K command palette (Plan C).** New `<CommandPalette>` mounted in `AppShell`; `⌘K` (mac) / `Ctrl+K` (other) opens, `Esc` closes. Static command registry in `src/lib/commands.ts` shaped as `[{ id, label, scope, run }]`; scopes are `"global" | "mode:<mode>"`. Search is client-side over already-fetched data (entity list, document list) plus the static registry. **No new Rust IPC** — keeps the IPC surface flat. If the corpus grows large enough to need backend search, that's a Phase 9+ decision. Spec line 33 is canonical: "Search is not a mode. Global `⌘K` command palette plus a search field in each mode's sidebar." **Architecturally independent of Plan B (peek panels)** — the palette is useful on its own for navigate-to-mode, navigate-to-entity, navigate-to-doc; the plan-c spec's "depends on peek" (line 44) was a planning-time ordering hint, refined here to a parallel-track dependency. Peek can ship without the palette and vice versa.

### Scope (out)

- **Compact density toggle** (Phase 9 — depends on token system stability).
- **Per-mode sidebar search fields** (independent of palette; ship later if at all).
- **Group-by-source for tasks UI** (Phase 9 — depends on the librarian populating `okf_sources` on task writes; column exists at `src-tauri/src/db/okf_ddl.rs:81`, gap is data not schema).
- **Similarity scores in Connections** (Phase 9 — depends on `summary_embedding` backfill; `curated_entities.summary_embedding` is always NULL today per `src-tauri/src/db/entities.rs:374`).
- **OKF v0.2 write-side provenance helpers in `src-tauri/src/db/`** (Phase 9 — librarian must record provenance at commit time).
- **Settings search, graph visualization, auto-approve rules, kanban/Tasks depth, reject-reason tuning** — unchanged from spec §"Non-Goals".

### Dependency on prior phases

- **Phase 7 Plan A+B (shipped v1.17.0)** — `EditorPane` anchor highlight infrastructure (`src/components/shell/EditorPane.tsx`, `src/index.css:1158`); `EntityFact` v0.2 fields. Phase 8's §1/§2 reuse both. No modifications to Plan A+B code.
- **Phase 4** — `useNavigationState` (`src/lib/navigation.ts`), `NavTarget.chunkId` field, cross-mode navigation hook. Phase 8 reuses; §1 finally populates `chunkId` end-to-end.
- **Phase 7 Plan C (shipped v1.17.0)** — `AppShell` mode-routing conventions, `p className="placeholder"` empty-state pattern, design tokens. Phase 8 reuses for peek/palette chrome.

---

## New Tauri commands

**None.** Phase 8's only Rust change is the internal signature of `source_docs_from_ref` (returns `(path, chunk_id)` tuples instead of `Vec<String>`); the public Tauri command(s) that surface `EntityFact` (`list_entity_facts`, `get_entity`) only bubble the tuple up through their existing `source_docs` serialization. No new `#[tauri::command]` annotations. **Both `invoke_handler` registration lists in `src-tauri/src/lib.rs` (~line 2204 app builder, ~line 2400 `make_test_app`) remain unchanged.**

## New design tokens

**None for v1.** The peek panel uses the existing `card` surface + `space-*` tokens; the slide-over animation reuses an existing motion token. If a per-mode accent is needed for the peek header, add it as an extension token in `src/index.css`, not a new tier.

---

## Components

### Rust (`src-tauri/`)

- **`src-tauri/src/db/entities.rs`** — `source_docs_from_ref` (line 180) returns `Vec<(String, Option<i64>)>` (path, chunk_id) instead of `Vec<String>`. The function already reads `chunk_id` internally at line 192 to drive the path lookup — change is "do not discard the value." Internal callers (`get_entity`, `list_entity_facts`) propagate the tuple; serialization becomes `{ path, chunkId }` instead of plain string.
- **`src-tauri/src/lib.rs`** — no change (no new commands).
- **Tests** — see "Testing" below.

### Frontend (`src/`)

- **`<PeekPanel>`** (new) — slide-over from right edge, `role="dialog"`, `aria-modal="true"`, `aria-label` from `kind` (`"document" | "entity"`). Renders the existing `EditorPane` or `EntityPage` with `isPeek={true}`. **Accessibility (in addition to `role`/`aria-modal`):** the panel implements a **keyboard focus trap** — on mount, focus moves into the panel (to the "Open in [mode]" button or, if absent, the first focusable element inside the content); `Tab` and `Shift+Tab` cycle within the panel; on unmount, focus returns to the element that opened the peek (the clicked wikilink / chip / chunk). This is required because `aria-modal="true"` promises screen readers that focus stays inside, and lying about it (no actual trap) is an a11y violation. **What `isPeek` does on `EditorPane` / `EntityPage`:** disables all editing affordances (no inline-edit, no save/cancel buttons, no `aria-label="Fact body"` textareas, no per-fact "…" power menu); disables navigation actions inside the peeked content (wikilink clicks stay as no-op or show a tooltip "open in [mode]"); keeps all read-only rendering and the anchor-highlight effect. Hosts the "Open in [mode]" affordance. `Esc` handler and click-outside listener both go through a single `onDismiss` callback. Reuses `card` background, `shadow-lg`, and a motion token for the slide-in animation.

- **`<CommandPalette>`** (new) — overlay positioned at viewport top center (not modal-from-edge; palette feels modal). Listbox role; arrow keys navigate, `Enter` dispatches, `Esc` closes. Backed by the static registry consumed at mount.

- **`src/lib/commands.ts`** (new) — exports `COMMAND_REGISTRY: Command[]` (a module-level constant — "static" means compile-time-resolved, not a fetched service) and `useCommands(scope: string): Command[]` (a hook that filters the global registry by scope and merges in any per-component additions). Built-in commands: `nav.brain`, `nav.library`, `nav.review`, `nav.timeline`, `nav.tasks`, `nav.settings`, `palette.close`, `palette.next`, `palette.previous` (palette-internal). Mode-scoped commands register via a `useCommands(scope)` hook that components call to contribute their own entries.

- **`src/components/shell/AppShell.tsx`** (modify) — mount `<PeekPanel>` and `<CommandPalette>` at root (alongside existing `ActivityFeedPanel`); own `peekTarget` state (one peek at a time); wire `Option`+click to dispatch `peek.open({ kind, ...target })` instead of `nav.navigate(...)`; add a global `keydown` listener for `⌘K` (mac) / `Ctrl+K` (other) that toggles the palette. **Listener is always-on and uses the capture phase** — `window.addEventListener("keydown", handler, { capture: true })`. Capture phase matters: BlockNote (and similar nested editors) attach their own keyboard handlers on bubble phase and would otherwise swallow `⌘K` first. The palette opens from anywhere in the app, including from inside an input or BlockNote editor; `Esc` inside the palette closes it before any `Esc` semantics inside the focused control. Use `e.metaKey` on macOS (`e.ctrlKey` elsewhere) and `e.key === "k"`, `preventDefault()` to avoid Chromium's built-in ⌘K bar.

- **`src/components/brain/FactCard.tsx`** (modify) — drop the v1 docstring on `onOpenSource` (lines 19–24) which currently says "v1: `source_docs` does not yet carry chunk ids so callers pass `null` here"; `FactCard` now passes the `chunkId` it received from the enriched `EntityFact.source_docs` (line 99 changes from `onOpenSource(path, null)` to `onOpenSource(path, chunkId)`).

- **`src/components/brain/EntityPage.tsx`** (verify) — already passes `chunkId` through; no change expected, just a re-read to confirm under the new `source_docs` shape.

- **`src/components/modes/BrainMode.tsx`** / **`src/components/modes/ReviewMode.tsx`** / **`src/components/modes/LibraryMode.tsx`** (verify) — already forward `chunkId` correctly (`AppShell.tsx:232, 245`); just verify under the new shape.

- **`src/index.css`** — add a `.peek-panel` slide-over rule (reuses `card` background, `shadow-lg`, motion `slide-in-right`). No new tokens.

- **`src/lib/tauri.ts`** — `EntityFact.source_docs` type changes from `string[]` to `Array<{ path: string; chunkId: number | null }>`. Update in lockstep with the Rust serialization change.

---

## Data model

```ts
// src/lib/tauri.ts
interface EntityFact {
  // ...existing fields unchanged
  source_docs: Array<{ path: string; chunkId: number | null }>;  // was: string[]
}
```

```rust
// src-tauri/src/db/entities.rs
fn source_docs_from_ref(
    conn: &Connection,
    source_ref: Option<&str>,
) -> Vec<(String, Option<i64>)> {
    // existing loop preserved; do not discard the chunk_id value read at line 192.
    // Tuple serializes as { path: String, chunkId: Option<i64> } at the JSON boundary.
}
```

`NavTarget.chunkId` already exists (`src/lib/navigation.ts:16`) and its v1 docstring ("always undefined because `source_docs` does not yet expose chunk ids") gets updated as part of Plan A to drop the "always undefined" half.

---

## Testing

- **Rust unit tests** (`src-tauri/src/db/entities.rs`):
  - `source_docs_from_ref_returns_paths_with_chunk_ids` — evidence has 2 chunks in same document → 1 entry, `chunkId` set.
  - `source_docs_from_ref_returns_distinct_entries_per_chunk` — evidence has 2 chunks in different documents → 2 entries, each with its own `chunkId`.
  - `source_docs_from_ref_dedupes_paths` — same as today's behavior, regression guard.
  - `source_docs_from_ref_handles_missing_chunks` — `chunk_id` that doesn't resolve to any document → no entry (current behavior).
  - `source_docs_from_ref_handles_evidence_without_chunk_id` — evidence entry with no `chunk_id` → skipped (current behavior).
  - `source_docs_from_ref_handles_malformed_source_ref` — bad JSON → empty vec (current behavior).
- **Frontend component tests**:
  - `__tests__/PeekPanel.test.tsx` (new) — opens, dismisses on `Esc`, dismisses on click-outside, "Open in [mode]" promotes to `nav.navigate(...)`.
  - `__tests__/CommandPalette.test.tsx` (new) — opens on `⌘K`, registry match surfaces expected commands, `Enter` dispatches, `Esc` closes, mode-scoped commands only appear when palette is opened from that mode.
  - **Fixture updates** (existing tests): `__tests__/FactCard.test.tsx`, `__tests__/ReviewMode.test.tsx`, `__tests__/ReviewEvidencePanel.test.tsx`, `__tests__/proposalPreview.test.ts`, `__tests__/proposalEntityPreview.test.ts`, `__tests__/FactPowerMenu.test.tsx` — change `source_docs: ["documents/notes.md"]` to `source_docs: [{ path: "documents/notes.md", chunkId: null }]` (or to a real `chunkId` for tests that want to assert anchor behavior).
- **Manual verification** — every existing fact-source chip click deep-links to Library with the correct document loaded and the `chunkId` reaching `EditorPane` (block-level highlight for numeric ids is the deferred follow-up — Scope §2 caveat; verify no console errors on the no-op path); `Option`+click opens a peek instead of navigating; `⌘K` opens the palette from any mode.

---

## Non-goals (unchanged from spec §"Non-Goals")

- Graph visualization view (deferred; backlinks + links cover navigation).
- Auto-approve rules for the librarian.
- Due dates, kanban, or any project-management depth in Tasks.
- Settings search.
- Wiring reject-reasons into librarian tuning.

## Open Questions Deferred to Follow-up Specs

- **Chunk-id → block resolution for the Library deep-link highlight.** Plan A surfaces numeric `chunk_id`s, but the `EditorPane` anchor effect resolves anchors by matching the string against heading block text, so numeric ids silently no-op (see Scope §2 caveat). Candidate approaches: resolve chunk ids to line ranges and scroll/match by line number; store chunk heading names alongside the id in `source_docs`; or change the highlight mechanism from "block text match" to "line range scroll". All are new UI/IPC surface beyond Plan A's constraints.
- Per-mode sidebar search fields (independent of the global palette).
- Compact density toggle (Phase 9 — needs token system stability).
- Group-by-source for tasks UI (Phase 9 — depends on librarian populating `okf_sources` on task writes; column exists, gap is data not schema).
- Similarity scores in Connections (Phase 9 — depends on `summary_embedding` backfill).
- OKF v0.2 write-side provenance helpers in `src-tauri/src/db/` (Phase 9).
- Librarian "reasoning summary" capture for the Review evidence panel (was deferred at the phase 2/3 boundary in spec line 227; both phases done, so unblocked if desired).
