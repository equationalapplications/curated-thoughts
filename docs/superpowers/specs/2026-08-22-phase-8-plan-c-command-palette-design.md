# Phase 8 Plan C — Global ⌘K Command Palette Design Spec

**Date:** 2026-08-22
**Branch:** `phase-8-plan-c-command-palette`
**Status:** Implemented 2026-08-22
**Anchored by:**
- UX vision spec line 33 (canonical): *"Search is not a mode. Global `⌘K` command palette plus a search field in each mode's sidebar."* This feature ships the **palette half only**; per-mode sidebar search fields stay deferred.
- `docs/superpowers/specs/2026-08-20-phase-8-design.md` §Scope item 4 + the §Components `<CommandPalette>` / `src/lib/commands.ts` / `AppShell` bullets — **elaborated by this spec, not superseded**.
- `docs/superpowers/plans/2026-08-20-phase-8-plan-c.md` — stage-1 implementation plan (ephemeral input artifact; deleted post-ship along with the stage-2 plan and the handoff).

---

## Goal

Pressing `⌘K` (macOS) / `Ctrl+K` (elsewhere) anywhere in the app — including while focus is inside a BlockNote editor or any input — opens a top-center command palette with client-side search over a static command registry plus the already-available entity and document lists. Arrow keys move the active row; `Enter` dispatches; `Esc`, backdrop click, or a second `⌘K` closes. Keyboard-only users can reach any mode, entity, or document without touching the mouse.

This closes the last open Phase 8 deferral; when it ships, Phase 8 is fully Implemented.

## Relationship to prior specs

- `2026-08-20-phase-8-design.md` §Scope item 4 is elaborated by this spec, not superseded: the phase-8 design named the pieces (`<CommandPalette>`, `src/lib/commands.ts`, AppShell shortcut); this spec fixes their final shapes — registry/context split, scope model, dynamic-result rules, staging.
- Status-line edits happen at ship time (stage 2), so no spec claims Implemented ahead of the focus contract:
  - Phase 8 spec Status line → Plan C shipped; if all three plans have landed, Phase 8 → Implemented.
  - UX vision spec's Phasing row for the global palette → landed (per-mode sidebar search stays deferred).

## Implementation staging

User-mandated two-stage sequence on one branch, one PR, two commit clusters:

| | Stage 1 = existing plan file | Stage 2 = harmonization plan |
|---|---|---|
| Ships | Registry, palette overlay + CSS, capture-phase `⌘K`, dispatch/close (plan Tasks 1–3 verbatim) | Focus capture/restore + test; Tab pinning + test; 8+8 result cap + test |
| Ends at | Code green (`pnpm test` / `typecheck` / `lint`) + manual smoke minus focus-return check | Full smoke (incl. focus return, both themes/modifiers) + docs polish (absorbs old Task 4 Steps 2–3) |

Rationale: every stage-2 change is purely additive — nothing in stage 1 gets rewritten. Docs status polish lands only after stage 2 so the specs never claim Implemented over a missing focus contract.

## Scope (in)

1. **`src/lib/commands.ts`** — static command registry, late-bound navigation context, `useCommands` hook (§Components).
2. **`<CommandPalette>`** — top-center overlay: search input, listbox, keyboard navigation, dispatch, close paths (§Components).
3. **Dynamic results** — entity and document open-targets built from existing Tauri data, wiki-tier excluded, capped 8+8.
4. **AppShell wiring** — always-on capture-phase `⌘K`/`Ctrl+K` toggle, conditional palette mount scoped to the active mode, command-context registration.
5. **CSS** — palette overlay styles in `src/index.css`, existing tokens only, z-index above all current overlays.
6. **Docs polish** (stage 2) — status-line edits to the two prior specs.

## Scope (out)

- **Backend/fuzzy search** — case-insensitive substring matching only; server-side or fuzzy search is a Phase 9+ decision.
- **Per-mode sidebar search fields** — the other half of ux-vision line 33; stays deferred.
- **New Rust IPC** — none; reuses `list_entities_cmd` and `list_vault_files`.
- **New design tokens** — existing tokens only (see Components/CSS).
- **Populated mode-scoped commands** — the `useCommands(scope, extras)` mechanism ships, but v1 contributes zero mode-scoped entries; it exists so future modes can add commands without touching the registry.
- **MRU/recency ranking, multi-select, actions on selected text** — not attempted.

## Components

### `src/lib/commands.ts` (new)

```ts
type CommandScope = "global" | `mode:${AppMode}`   // AppMode from src/components/shell/ModeRail.tsx
interface Command { id: string; label: string; scope: CommandScope; internal?: boolean; run: () => void }
```

- **`COMMAND_REGISTRY: Command[]`** — module-level constant. Six nav commands (`nav.brain/review/library/timeline/tasks/settings`, all `scope: "global"`), plus `palette.close` / `palette.next` / `palette.previous` with `internal: true`. The internal entries exist as the canonical id list; they are never listed in results — the palette's own key handling implements their behavior.
- **`registerCommandContext({ navigate }): () => void`** / **`commandNavigate(target)`** — late-bound navigation singleton. This is what keeps `COMMAND_REGISTRY` a compile-time constant even though `nav.navigate` only exists inside the mounted `AppShell`: nav commands' `run` closures read the registered context lazily. Unregister is idempotent-guarded (`if (context === ctx)`); calling `run` after unregistration is an inert no-op (no throw).
- **`useCommands(scope, extras?)`** — registry entries whose scope is `"global"` or matches the argument, **excluding `internal` entries**, merged with `extras` via `Map` dedupe keyed by id (the extra wins on duplicate id).

### `<CommandPalette>` (new — `src/components/shell/CommandPalette.tsx`)

Props `{ scope: CommandScope; onClose: () => void }`.

**Structure & a11y.** `role="dialog" aria-modal="true" aria-label="Command palette"`; combobox input (`aria-label="Search commands"`, `aria-expanded`, `aria-controls`, `aria-activedescendant`) over a listbox of `role="option"` rows. The backdrop is a real `<button type="button" aria-label="Close command palette">`.

**Search & results.**

- Case-insensitive substring match against labels (static) and names (dynamic). Static results always precede dynamic.
- Dynamic entries are built from `listEntities("name_asc")` + `listVaultFiles()`, fetched once per open:
  - Entity name matches → `Open entity: <name>` → `{ mode: "brain", entityId }`
  - Document filename matches → `Open document: <name>` → `{ mode: "library", docPath }`
- **Wiki-tier files never appear** as document targets — `tier === "user_doc"` only (wiki pages have their own navigation surface in Brain).
- **Cap (stage 2): first 8 matching entities + first 8 matching documents after filtering** — keeps worst-case DOM bounded on large vaults; static commands are never capped.
- Empty query shows only the six nav commands. No matches at all → single "No matching commands." row.

**Keyboard.**

- `ArrowDown`/`ArrowUp` move the active row (clamped); hover moves it too; `Enter` dispatches the active row and closes; dispatch no-ops when no row is active.
- `Esc` is handled via a window-level capture listener + `preventDefault()` + `stopPropagation()` — it closes the palette before any focused-control Esc semantics (e.g. Review's reject-reason textarea) can react. Backdrop click closes.
- **Tab pinning & focus contract (stage 2):** capture `document.activeElement` at mount, autofocus the input, restore focus to the captured element on every close path (Esc, backdrop, dispatch). While open, `Tab` on the input is `preventDefault`ed — the input is the panel's only focusable stop, which keeps the `aria-modal` promise truthful without a full focus trap.

### `AppShell.tsx` (modify)

- New `paletteOpen` state; palette mounted conditionally with `scope={"mode:" + nav.current.mode}`.
- Dedicated always-on **capture-phase** keydown listener (`{ capture: true }`, separate effect) so BlockNote-style bubble handlers can't swallow the shortcut; `navigator.platform` mac-check picks `metaKey` vs `ctrlKey`; `preventDefault()` kills Chromium's built-in `⌘K` search-bar; toggles `paletteOpen`. Toggle semantics: `⌘K` opens and closes; `Esc`/backdrop/dispatch close.
- Delete the `⌘K` placeholder branch from the existing bubble-phase `⌘1–5` listener (`AppShell.tsx:182` area).
- Effect registering `registerCommandContext({ navigate: nav.navigate })`; cleanup via the returned unregister fn.

### CSS (modify `src/index.css`)

Palette block appended near the peek/activity overlays: `.palette-backdrop` z-index 70, `.palette-panel` z-index 71 — above peek/activity (50/51) and the previous max (60). Existing tokens only (`--surface`, `--outline-var`, `--on-surface`, `--on-surface-var`, `--primary`, `--r-md`, `--r-sm`, `--shadow-lg`); backdrop tints are literal rgba values. Dark-theme backdrop override added beside `[data-theme="dark"] .peek-backdrop` (~line 1726).

## Data flow

keydown(capture) → toggle `paletteOpen` → palette mounts → fetch `listEntities("name_asc")` + `listVaultFiles()` once per open → client-side filter per keystroke → `Enter`/click dispatch builds a `NavTarget` → `commandNavigate` → registered `nav.navigate` → palette closes.

## Error handling

- List-fetch failures degrade silently to empty arrays — static commands keep working; no error UI in v1.
- No matches → "No matching commands." empty-state row.
- Dispatch guards on an undefined active row (no throw).
- Unregistered navigation context makes command `run`s inert no-ops.

## Testing

Stage-1 suites (per the plan):

- `src/__tests__/commands.test.ts` — registry ids, internals flagged, context registration/unregistration inertness, `commandNavigate`, scope filtering, extras merge + override.
- `src/__tests__/CommandPalette.test.tsx` — registry display, internals hidden, query filtering with entity/document results, wiki-tier exclusion, Enter dispatch (static/entity/doc), arrow navigation, Esc close, backdrop close.
- `src/__tests__/AppShell.test.tsx` additions — `⌘K` opens (input autofocused), Esc closes, toggle reopens, dispatch navigates and closes. Tests fire `metaKey`+`ctrlKey` together since jsdom reports an empty `navigator.platform` (both platform branches satisfied).

Three stage-2 tests pinned here:

1. **Focus restore:** focus a mode-rail button, `⌘K` open, `Esc` close → the opener button is refocused.
2. **Result cap:** seed >8 matching entities and >8 matching documents for one query → at most 8 entity rows + 8 document rows render.
3. **Tab pinning:** `Tab` keydown on the palette input while open is `preventDefault`ed.

Verification: `pnpm test && pnpm typecheck && pnpm lint`.

## Non-goals

Recency/MRU ranking · fuzzy or backend search · per-mode sidebar search fields · mode-scoped command content (mechanism only) · i18n of command labels · telemetry.
