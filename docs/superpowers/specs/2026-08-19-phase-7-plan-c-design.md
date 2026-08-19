# Phase 7 Plan C — Onboarding Rework + Visual Polish (Design Spec)

**Date:** 2026-08-19
**Branch:** `phase-7-plan-c`
**Status:** Design spec — input to the Phase 7 Plan C implementation plan.
**Anchored by:**
- `docs/superpowers/specs/2026-07-05-ux-vision-okf-native-design.md` §6 (Settings & Onboarding) + §7 (Visual direction), Phasing item 7.
- `docs/superpowers/plans/2026-08-18-phase-7-discovery.md` (Phase 7 discovery memo, Plan C section).
- `docs/superpowers/plans/2026-08-18-phase-7-plan-ab.md` (Plan A+B — shipped as v1.17.0 on 2026-08-19; this spec depends on it).

---

## Goal

Ship the discovery memo's Plan C: rework the BYOI setup wizard per spec §6, add a new "Watch it think" live-ingest step, unify wizard chrome via a shared `WizardStep` shell, and apply first-run empty states to Brain / Library / Review. Visual polish uses existing design tokens — no new tokens, no app-wide rewrite.

**One spec, two parts (one design, two PRs):**
- **Part 1 — The wizard:** `WizardStep` shell, `StepIndicator`, `StepWatchItThink`, "Re-run wizard" CTA in `Settings → Vault`, and the 5 existing steps migrated to the new shell.
- **Part 2 — Empty states:** per-mode first-run empty states for Brain, Library, Review.

The split keeps the wizard diff and the empty-states diff independently reviewable. Both parts ship in the same release; either can land first.

---

## Architecture

### Scope (in)

1. **Wizard step order** (six steps):
   `[0] Welcome → [1] Privacy → [2] Fastembed (auto) → [3] Model → [4] Watch it think (skippable) → [5] Done`
2. **`WizardStep` shell** — shared card surface for all six steps. Existing steps migrate; new step adopts.
3. **`StepIndicator`** — step-name strip + 1px progress bar. 1-based display; 0-based internal state.
4. **`StepWatchItThink`** — file picker → fire-and-forget `ingestDocument` → live pipeline status → patient-path auto-routes to Review; skip-path routes to Done.
5. **"Re-run setup wizard"** button in `Settings → Vault`. Reuses the existing `nav.setMode("setup")` entry point.
6. **Per-mode first-run empty states** — Brain, Library, Review.

### Scope (out)

- Re-implementing the BYOI model picker, privacy step, or Fastembed bootstrap. Plan C reframes copy and unifies chrome; underlying implementations are unchanged.
- Settings tabs polish, status bar polish, app-wide design-token rewrite. Phase 8+.
- Group-by-source for tasks UI (Phase 8 — depends on librarian writing `okf_sources`).
- Similarity scores in Connections (Phase 8 — depends on `summary_embedding` backfill).
- Peek panels (Phase 8 — depends on EditorPane anchor work; landed in Plan A+B Task 15).
- Global `⌘K` command palette (Phase 8 — depends on peek).
- Compact density toggle (Phase 8 — depends on token system stability).

### Dependency on Plan A+B (shipped v1.17.0)

Plan C reuses three Plan A+B surfaces:
- `ProviderNotice` (A+B Task 14) — the inline notice when the embedder or generation provider is down. Surfaces on the Review / Library / Brain post-wizard if a Fastembed-init or generation failure lingers.
- `EntityFact` v0.2 fields (A+B Task 9) — irrelevant to Plan C; mentioned only because the per-fact "…" menu (A+B Task 10) already exposes the v0.2 provenance and is the *kind* of surface Plan C's empty states should match in chrome.
- Per-mode empty-state pattern (A+B Task 13) — defines the `p className="placeholder"` idiom that Plan C's empty states follow.

Plan C does not modify any Plan A+B code. If Plan A+B were ever reverted, Plan C still functions (the empty states fall back to their previous copy; the wizard works without the ProviderNotice).

### New Tauri commands

**None.** Plan C reuses:
- `@tauri-apps/plugin-dialog` `open()` (already wired in Library)
- `ingestDocument` / `readDocument` Tauri commands (already exist)
- `onEmbedInitProgress` / `onEmbedInitDone` / `onEmbedInitError` (existing; Plan A+B Task 14 added wrappers)
- `onIngestProgress` (existing)
- `useProviderHealth` (existing from Plan A+B)
- `nav.setMode` / `nav.setTarget` (existing)

### New design tokens

**None.** All new surfaces use existing tokens in `src/index.css` (`card` surface, `space-4`, `text-primary`, `text-muted`, motion tokens). Light + dark themes covered by construction.

---

## Components

### `WizardStep` (new)

```ts
interface WizardStepProps {
  title: string;            // e.g. "Choose your privacy posture"
  subtitle?: string;        // one-line outcome framing (per spec §6)
  children: ReactNode;      // step body
  onBack?: () => void;      // undefined hides the button (first step)
  onNext?: () => void;      // undefined hides the button (last step / auto-advance)
  nextLabel?: string;       // default "Continue"
  nextDisabled?: boolean;   // gating (e.g. until a privacy mode is selected)
  onSkip?: () => void;      // "Skip — take me to the app" (Watch-it-think only)
  skipLabel?: string;
  isLoading?: boolean;      // shows a thin spinner next to Next
}
```

Renders a card surface with three slots: header (`h2` title + optional `p` subtitle), body (`children`), footer (`[Back]  [Next]` or `[Skip]` on the final optional step). All existing and new steps adopt this shell.

A11y: the shell renders as `<section role="region" aria-labelledby={titleId}>`. Back / Next / Skip are real `<button>`s with visible focus rings.

### `StepIndicator` (new)

```ts
interface StepIndicatorProps {
  current: number;   // 0-based
  total: number;     // ≥ 1
  steps: string[];   // step names in order
}
```

Renders the step-name strip ("Welcome · Privacy · Fastembed · Model · Watch it think · Done") with the current step highlighted in `text-primary` and the others in `text-muted`. Below, a 1px bar showing `current / total` progress. Above the bar, a 1-based label: `"Step ${current + 1} of ${total}: ${steps[current]}"`.

A11y: the bar exposes `aria-valuenow` / `aria-valuemax` (decorative; the step-name strip is the primary navigation cue). Honors `prefers-reduced-motion`: no fill animation when set.

### `StepWatchItThink` (new)

```ts
interface StepWatchItThinkProps {
  onSkip: () => void;                 // advances the wizard to step 5
  onRouteToReview: (proposalId: string) => void;  // patient path
}
```

Local state:
```ts
{
  picked: string | null,
  phase: "idle" | "chunking" | "embedding" | "ready" | "error" | "stalled",
  errorMsg: string | null,
  proposalId: string | null,
  lastProgressAt: number,
}
```

Body composition:
- `phase === "idle"`: a single primary button "Choose a document to ingest" with `aria-label="Choose a document to ingest"`. On click, calls `open({ filters: [{ name: "Documents", extensions: ["md", "txt", "pdf"] }] })`. Cancellation is silent — the step stays in `idle`.
- `phase ∈ {"chunking", "embedding", "ready"}`: a status panel showing the live pipeline. Subscribes to `onIngestProgress`. `lastProgressAt` updates on every event; a `useEffect` watching `(now - lastProgressAt) > 60_000` flips to `stalled` and renders "Still working… this can take a few minutes."
- `phase === "ready"`: the `proposalId` is set; a `useEffect` on `proposalId` calls `onRouteToReview(proposalId)`.
- `phase === "error"`: inline error panel with the error message + a "Try again" button (reopens picker) + the persistent Skip button.
- `phase === "stalled"`: appended beneath the live status. Skip remains available at all times.

A persistent secondary button "Skip — take me to the app" sits in the `WizardStep` footer throughout. The wizard's `next` button is not rendered for this step (`onNext` is `undefined`); the only ways forward are Skip or the patient-path auto-route.

### Reframed existing steps (5 modified)

| Step | New title | Body | Step index |
|------|-----------|------|------------|
| `StepWelcome` | "Where is your vault?" | Unchanged logic. Body shows the current vault path as read-only text. | 0 |
| `StepPrivacy` | "Choose your privacy posture" | Unchanged. | 1 |
| `StepFastembed` | "Set up local search" | Unchanged. Auto-advances on `onEmbedInitDone`. Error state from `onEmbedInitError` renders inside the `WizardStep` shell. | 2 |
| `StepModel` | "Pick your AI" | Unchanged. | 3 |
| `StepDone` | "You're ready" | Unchanged. Calls `onComplete` on click. | 5 |

The spec target is four steps (vault → privacy → AI → watch-it-think). Plan C ships six because the user opted to keep Fastembed visible as a thin auto-advance step (decision recorded in the discovery memo, locked in 2026-08-19). The indicator reflects all six honestly.

### "Re-run setup wizard" CTA in `VaultPanel`

Add a primary `<button>` below the current vault path display:

```
<button onClick={() => nav.setMode("setup")}>Re-run setup wizard</button>
```

`nav.setMode("setup")` is the existing navigation entry point that `AppShell` already mounts `<SetupWizard>` against. The wizard's `StepWelcome` body reads the current vault path so the user is reminded what they're configuring without re-picking. Privacy and Model steps default to the current active settings.

The `setup-wizard` mount state in `AppShell.tsx` is reset to `false` when the user reaches the Done step OR closes the wizard (Esc / window close). The `first_run_pending` flag is not touched on re-run.

### Per-mode first-run empty states (3 modified)

| Mode | Trigger | Copy | CTA |
|------|---------|------|-----|
| Brain | `entities.length === 0 && selectedId === null` | "No entities yet. Drop a document in Library or import a wiki bundle." | "Go to Library" — calls `nav.setMode("library")` |
| Library | `documents.length === 0 && selectedDoc === null` | "Drop your first document to get started." | File-picker button reusing the same `open()` call |
| Review | `queueLength === 0` | `"Queue clear. Librarian is watching ${watchedDocCount} document${N === 1 ? '' : 's'}."` | None (informational only) |

All three use the existing design tokens (no new colors). Light + dark themes covered by token reuse.

---

## Data flow

### Wizard step state machine

`SetupWizard.tsx` owns the step index via `useState(initialStep)`. Three navigation actions: `next`, `back`, `skip` (Watch-it-think only). `skip` on step 4 advances to step 5 directly; on Done, `onComplete` is called.

The wizard is mounted in `AppShell.tsx` whenever `wizardOpen === true`. The `wizardOpen` state is set on first-run (existing) and via `nav.setMode("setup")` from `VaultPanel` (new in Plan C).

### "Watch it think" pipeline

```
User clicks "Choose a document"
  → open({ filters: [{ name: "Documents", extensions: ["md", "txt", "pdf"] }] })
  → onPick(filePath): setLocal({ picked: filePath, phase: "chunking", lastProgressAt: now })
  → useEffect on picked: invoke('ingestDocument', { path: filePath })
  → onIngestProgress({ phase, ... }): setLocal({ phase, lastProgressAt: now })
  → onComplete: setLocal({ phase: "ready", proposalId })
  → useEffect on proposalId: onRouteToReview(proposalId)
       → nav.setMode("review")
       → nav.setTarget({ mode: "review", proposalId })
```

The pipeline is fire-and-forget at the wizard level. The Tauri `ingestDocument` promise is independent of the React component lifecycle. If the user clicks Skip mid-flight, the backend continues; the wizard navigates away; the proposal lands in Review whenever the embedder finishes (the existing `Review` mode's queue length increments automatically — no new wiring).

### Per-mode empty state data flow

Each mode's empty state reads the existing data hooks:
- `BrainMode` — `useEntityList().entities.length === 0 && selectedId === null` → render.
- `LibraryMode` — existing document-list hook. (Hook name to verify during implementation; expected `useDocumentList`.)
- `ReviewMode` — `useWikiStatus().queueLength === 0` → render with `useWikiStatus().watchedDocCount`.

No new Tauri commands. No new data hooks. The empty state is a pure render branch on existing state.

---

## Error handling & background behavior

### Step-by-step error paths

| Step | Failure mode | Behavior |
|------|--------------|----------|
| Welcome (0) | n/a | n/a |
| Privacy (1) | No mode picked | `nextDisabled` is `true` until a mode is selected |
| Fastembed (2) | `onEmbedInitError` fires | Existing error UI renders inside the `WizardStep` shell; retry button preserved |
| Model (3) | No model selected | `nextDisabled` is `true` until a model is picked |
| Watch it think (4) | (a) File picker cancelled | Silent; step stays in `idle` |
| | (b) `ingestDocument` rejects | Inline error panel + "Try again" button (reopens picker) + persistent Skip |
| | (c) Pipeline stalls (no progress event for 60s) | "Still working… this can take a few minutes" message; Skip remains available; no timeout — the pipeline is fire-and-forget |
| Done (5) | `onComplete` rejects | Existing error path: catch in `SetupWizard.tsx` and surface a small toast |

### Dual-path routing (the "patient" vs. "impatient" paths)

The wizard's `StepWatchItThink` is the final optional step. Two observable outcomes after a document is picked:

1. **Patient path** — user waits; `onIngestProgress({ phase: "ready" })` fires; `useEffect` on `proposalId` calls `onRouteToReview(proposalId)`. The wizard's `Done` step is **bypassed**. The user lands directly in `Review` mode with their first proposal focused. This is the spec §6 "Aha!" moment.
2. **Impatient path** — user clicks Skip; `onSkip` advances the index to step 5 (Done); user clicks "Continue" on Done; `onComplete` lands them in the app's default mode (Brain or Library). The pipeline continues; when the proposal lands in Review, the existing queue length increments and the status bar / badge lights up (no new wiring).

**Forcing the user through Done after the patient path fires is explicitly out of scope.** Bypassing Done in the patient path is the spec §6-aligned reward.

### Provider health during the wizard

Plan C does not check provider health before the wizard advances. The embedder's error state is handled by `StepFastembed` (existing behavior). The generator's health is checked at usage time (existing). `ProviderNotice` (from Plan A+B) is the post-wizard surface for any provider-down state.

### Mid-wizard crash / app close

The wizard does not persist intermediate state. If the user closes the app mid-wizard, the next launch shows the same first-run gating (existing behavior — `first_run_pending` flag in local storage is only cleared when the user reaches Done). Re-run does not touch `first_run_pending`.

The Rust-side `ingestDocument` initiated during a crashed wizard continues on next launch. Because the wizard is not mounted on next launch, the resulting proposal lands in the existing Review queue silently. No special recovery needed.

### Theme + accessibility

- All new surfaces use existing design tokens. Light + dark themes covered by construction.
- `StepIndicator` honors `prefers-reduced-motion` (no fill animation).
- The `WizardStep` shell renders as `<section role="region" aria-labelledby={titleId}>`.
- The file-picker button in `StepWatchItThink` has `aria-label="Choose a document to ingest"`.
- The progress indicator exposes `aria-valuenow` / `aria-valuemax` on the bar (decorative; the step-name strip is the primary navigation cue).

---

## Testing

### Component tests (Vitest + Testing Library)

- `WizardStep.test.tsx` — renders title / subtitle / body; Back / Next / Skip buttons show or hide based on prop presence; `nextDisabled` disables the Next button; `isLoading` shows the spinner; Back / Next / Skip callbacks fire with the right args.
- `StepIndicator.test.tsx` — renders the step-name strip with the current step highlighted; renders `"Step N of M: <current-name>"` (1-based for display); the bar's `aria-valuenow` / `aria-valuemax` reflect the 1-based values; `prefers-reduced-motion` disables the fill animation.
- `StepWatchItThink.test.tsx` — clicking the picker button calls `tauri.open` (mocked); on file pick, calls `ingestDocument`; live status updates on each `onIngestProgress` event; auto-routes to Review when `proposalId` lands; Skip calls `onSkip`; file-picker cancellation stays in `idle` silently; pipeline error renders the inline error panel with the "Try again" button; the 60s stall flips `phase` to `stalled` and renders the stall message.
- `VaultPanel.test.tsx` — "Re-run setup wizard" button is present; clicking it calls `nav.setMode("setup")`.
- Empty-state tests (extend existing mode tests) — Brain empty state renders "Go to Library" CTA when no entities; CTA calls `nav.setMode("library")`. Library empty state renders the file-picker button when no docs. Review empty state renders the watched-doc count from `useWikiStatus`.

### What we explicitly do NOT test

- The reframed copy strings in the 5 existing steps (copy, not logic; verified manually).
- Visual styling (light / dark themes, animation timing) — verified manually.
- The `WizardStep` shell's CSS (token usage) — verified by theme smoke.
- The `AppShell` wizard mount state (existing behavior; Plan C reuses it).

### Test patterns

- Mock `src/lib/tauri.ts` via `vi.mock` for Tauri commands (existing Plan A+B pattern).
- Mock `src/lib/events.ts` (the `onIngestProgress` etc. listeners) via `vi.mock` — return a no-op `unlisten` and capture the callback to fire in tests.
- Mock `src/lib/navigation.ts` `nav` object with a `vi.fn()` for each method.
- For `StepWatchItThink`, the file-picker mock returns a `Promise<string | null>` synchronously resolvable from the test.

### Manual smoke checklist

`pnpm tauri dev` — first-run:
- All 6 steps render through the new `WizardStep` shell.
- Step names match the spec §6 reframe.
- StepIndicator highlights the current step; bar fills; 1-based text matches.
- Fastembed step auto-advances on `onEmbedInitDone`; on error, the existing error state renders inside the shell.
- "Watch it think": picker opens; picks a markdown file; live status updates; patient path auto-routes to Review.
- "Watch it think": picker opens; picks a file; user clicks Skip mid-ingest; lands on Done; lands in the app's default mode; the proposal appears in Review within a reasonable time.
- "Watch it think": picker cancelled → step stays in `idle` silently.

`pnpm tauri dev` — re-run from `Settings → Vault`:
- Button visible; click opens the wizard at step 0; vault path is shown read-only in Welcome body.
- Privacy and Model steps default to current settings.

`pnpm tauri dev` — first-run empty states:
- Brand-new vault, Brain mode: "No entities yet" with "Go to Library" CTA.
- Brand-new vault, Library mode: "Drop your first document" with the file picker.
- Review mode with empty queue: "Queue clear. Librarian is watching N documents." (N reflects the actual count).

Light + dark themes: walk through all of the above with the theme toggled.

---

## File inventory

### New files

- `src/components/setup/WizardStep.tsx`
- `src/components/setup/StepIndicator.tsx`
- `src/components/setup/StepWatchItThink.tsx`

### Modified files

- `src/components/setup/SetupWizard.tsx` — mount `StepIndicator`; reframe step order; add `StepWatchItThink` as step 4; bump `StepDone` to step 5.
- `src/components/setup/StepWelcome.tsx` — adopt `WizardStep` shell; reframe title and copy; show current vault path read-only.
- `src/components/setup/StepPrivacy.tsx` — adopt `WizardStep` shell; reframe title.
- `src/components/setup/StepFastembed.tsx` — adopt `WizardStep` shell; reframe title.
- `src/components/setup/StepModel.tsx` — adopt `WizardStep` shell; reframe title.
- `src/components/setup/StepDone.tsx` — adopt `WizardStep` shell; reframe title.
- `src/components/settings/VaultPanel.tsx` — add "Re-run setup wizard" primary button.
- `src/components/modes/BrainMode.tsx` — first-run empty state + "Go to Library" CTA.
- `src/components/modes/LibraryMode.tsx` — first-run empty state + file-picker CTA.
- `src/components/modes/ReviewMode.tsx` — first-run empty state with watched-doc count.
- `src/index.css` — design-token additions only if absolutely required (target: zero).
- `docs/superpowers/specs/2026-07-05-ux-vision-okf-native-design.md` — Status line + Phase 1 deferral table updates.

### New test files

- `src/__tests__/WizardStep.test.tsx`
- `src/__tests__/StepIndicator.test.tsx`
- `src/__tests__/StepWatchItThink.test.tsx`
- Extend `src/__tests__/VaultPanel.test.tsx` (or create).
- Extend mode tests in `src/__tests__/` for the empty states.

---

## Definition of Done

1. All Vitest tests green.
2. `pnpm exec tsc --noEmit` clean.
3. Manual smoke checklist above passed in both light and dark themes.
4. Spec docs updated.
5. Follow `superpowers:verification-before-completion` then `superpowers:finishing-a-development-branch` (two PRs to `main`: Part 1 = wizard, Part 2 = empty states).

---

## Open questions

**Resolved during spec review (2026-08-19):**

1. **StepFastembed treatment** — keep as thin auto-advance step (visible in the wizard step list). Polished via the `WizardStep` shell; behavior unchanged.
2. **"Watch it think" sample document** — user-supplied only (no bundled fixture). File picker via `@tauri-apps/plugin-dialog`. The first-run user with an empty vault can drop in a markdown file from anywhere on disk.
3. **"Run in background" button** — not added. The persistent "Skip — take me to the app" button serves this role; the pipeline is fire-and-forget at the Rust level and continues after Skip.
4. **Patient path bypasses Done** — yes. The patient path's `useEffect` on `proposalId` calls `onRouteToReview` and the wizard's `Done` step is skipped. This is the spec §6 "Aha!" moment.
5. **Re-run wizard entry point** — `nav.setMode("setup")` (option B). No new Tauri command. Reuses the existing `AppShell` wizard mount.
6. **Plan C scope** — one spec, two PRs. The wizard rework (Part 1) and the empty states (Part 2) ship as a single design, with the implementation plan splitting them into two independently reviewable PRs.
7. **First-run empty states in scope** — yes. Per discovery memo Plan C #7. Share the same design-token polish as the wizard shell.

No open questions remain.

---

## Out of scope (per spec §215 and Plan A+B §Non-goals — unchanged)

- Graph visualization view.
- Auto-approve rules for the librarian.
- Due dates, kanban, or project-management depth in Tasks.
- Settings search.
- Wiring reject-reasons into librarian tuning.
- A fourth privacy mode for brain-state sync.
- Group-by-source for tasks UI (Phase 8 — depends on librarian write-side change).
- Similarity scores in Connections (Phase 8 — depends on `summary_embedding` backfill).
- Peek panels (Phase 8 — depends on EditorPane anchor work; now landed in Plan A+B Task 15).
- Global `⌘K` command palette (Phase 8 — depends on peek).
- Compact density toggle (Phase 8 — depends on token system stable).
- Per-mode visual polish outside first-run empty states (Phase 8+).
- Settings tabs polish, status bar polish, app-wide design-token rewrite (Phase 8+).
- OKF v0.2 *write-side* helpers in `src-tauri/src/db/` (Phase 8 — depends on librarian learning to record provenance at commit time).
