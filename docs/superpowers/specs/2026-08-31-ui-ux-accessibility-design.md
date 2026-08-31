# Spec: UI/UX & Accessibility — WCAG 2.2 AA Foundation

**Date:** 2026-08-31
**Status:** IMPLEMENTED — phase 1 (PR #127, 2026-08-31)
**Type:** Cross-cutting frontend foundation + phased remediation. Phase 1 is the deliverable of the first implementation plan; phases 2–6 each get their own plan.
**Related:** `2026-07-05-ux-vision-okf-native-design.md` (the north-star UX vision this makes accessible), `2026-08-22-phase-8-plan-c-command-palette-design.md` (⌘K palette — a focus-management surface), `2026-08-21-phase-8-plan-b-peek-panels-design.md` (peek panels — a focus-management surface)

## Problem

The Curated Thoughts frontend has accessibility *intent* but no accessibility *enforcement*, and the gap shows in measurable ways. As of this spec, across 107 `.tsx` components and 2,434 lines of hand-written CSS:

- **Focus is invisible.** There are zero `:focus-visible` rules in the entire stylesheet, and two `outline: none` declarations (`src/App.css:91`, `src/index.css:130`) that remove the browser default without replacing it. A keyboard user cannot see where they are. This fails SC 2.4.7 (Focus Visible) and SC 2.4.11 (Focus Not Obscured) outright.
- **Contrast is unverified and at least partly failing.** `--outline` is defined as a border token but is used as a *text* color in at least eight places (`src/index.css:56,137,176,263,323,428,488` and others). In light theme that is `#817568` on `#fffbff` — roughly 4.0:1, below the 4.5:1 required by SC 1.4.3. Nothing tests any pair, so other failures are unknown.
- **Motion is unguarded.** 24 `transition`/`animation` declarations exist with no `@media (prefers-reduced-motion: reduce)` block anywhere, failing SC 2.3.3.
- **No skip link.** There is no bypass mechanism into main content past the rail and sidebar (SC 2.4.1).
- **Announcements are ad hoc.** 33 separate `aria-live` / `role="status"` / `role="alert"` usages are scattered across components with no coordination, so concurrent announcements (librarian status, save confirmations, error toasts) can interrupt or silence one another.
- **Dialog semantics are uneven.** 13 `role="dialog"` / `aria-modal` usages exist across the modal, peek-panel, palette and activity-feed surfaces, but there is no shared focus trap or focus-restore primitive, so correctness is per-component and unverified.
- **Nothing enforces any of it.** `eslint-plugin-jsx-a11y` is not installed. No axe assertions run in the 
Vitest suite. The CI `frontend` job runs typecheck, build, and `pnpm test` only. The two `outline: none` declarations shipped because nothing was watching.

The good news, which shapes the approach: the codebase is not structurally hostile to accessibility. There are **zero** `<div onClick>` handlers — interactive elements are already real buttons and inputs — and 41 of 107 components already carry `aria-*` attributes. This is a codebase that needs a foundation and a ratchet, not a rewrite.

## Goal

Bring Curated Thoughts to **WCAG 2.2 Level AA** conformance, and make conformance durable by making regressions fail CI.

Non-goals: a visual redesign, a component-library migration, AAA conformance, and mobile/touch support (this is a desktop Tauri app).

## Decisions Made During Brainstorming

- **Target:** WCAG 2.2 AA conformance is the success criterion. Visual change is in scope only where accessibility requires it.
- **Scope shape:** foundation + full audit ledger in phase 1; component fixes land in prioritized phases 2–6, with the CI gate ratcheting up as they land. Not a single big-bang conformance PR.
- **Verification:** `eslint-plugin-jsx-a11y` as a hard lint gate plus `axe-core` assertions inside the existing Vitest/jsdom suite. **No browser harness** (no Playwright) — the cost of new CI infra plus mocking Tauri IPC for a web target was judged not worth it now. What jsdom cannot see (contrast in situ, real focus order, screen-reader output, zoom/reflow) is covered by a documented manual checklist run per release.
- **Palette:** retune the failing tokens in place, in both light and dark themes, keeping the warm Material-ish identity. No separate high-contrast theme, no role-splitting of `--outline`. A unit test locks the ratios in.
- **Component layer:** keep the hand-rolled CSS. Adopting Mantine (already present transitively via BlockNote) was considered and rejected: it would turn an accessibility spec into a 107-component migration and discard the existing visual identity.

## Architecture

Four layers, built in this order. Each is independently testable and has one job.

```
  Enforcement   eslint-plugin-jsx-a11y  ·  vitest-axe  ·  contrast unit test  ·  CI gate
       ↑
  Tokens/CSS    retuned palette  ·  --focus-ring  ·  :focus-visible  ·  reduced-motion
       ↑
  Primitives    src/a11y/  — VisuallyHidden, SkipLink, focus trap/restore,
                             roving tabindex, single announcer
       ↑
  Components    107 .tsx files, fixed in phases 2–6 against the ledger
```

The dependency arrow points upward: components consume primitives, primitives consume tokens, enforcement observes everything. Nothing below reaches up.

### 1. Enforcement layer

**Lint.** Add `eslint-plugin-jsx-a11y` to `eslint.config.js` under the existing `**/*.{js,jsx,ts,tsx}` block, at `error` severity, using the plugin's `recommended` rule set plus these `strict`-tier additions: `jsx-a11y/no-autofocus`, `jsx-a11y/prefer-tag-over-role`, `jsx-a11y/control-has-associated-label`.

No legacy allowlist. Because there are zero `<div onClick>` handlers, the expectation is that the codebase is at or near clean already. If the initial run produces violations, they are fixed in phase 1 — not suppressed. If any single violation proves genuinely expensive to fix, it gets a file-scoped `eslint-disable-next-line` with a comment citing the ledger row that tracks it; a blanket disable is not acceptable.

**Automated axe.** Add `vitest-axe` (and its `axe-core` peer). Add a shared helper:

```ts
// src/a11y/testing/expectNoA11yViolations.ts
export async function expectNoA11yViolations(container: HTMLElement): Promise<void>
```

It runs `axe-core` over the container with the `wcag2a`, `wcag2aa`, `wcag21aa`, `wcag22aa` tags enabled, and the `color-contrast` rule **disabled** — jsdom has no layout or computed colors, so that rule cannot produce a meaningful result there; contrast is covered by the dedicated token test in layer 2 instead.

Every component fixed in phases 2–6 gains an `expectNoA11yViolations` assertion in its test file.

**Scripts and CI.** Add `"test:a11y": "vitest run --dir src/a11y"` for the primitive/token tests. The per-component axe assertions live in the normal suite and run under `pnpm test`. In `.github/workflows/ci.yml`, the `frontend` job gains a `Lint` step (`pnpm run lint`) before `Vitest` — note the repo currently defines a `lint` script but never runs it in CI, so this closes a second gap — and a `Vitest (a11y)` step running `pnpm run test:a11y`.

**The ratchet.** After phase 1, the lint gate and the contrast test apply repo-wide and unconditionally. The per-component axe assertions accumulate: a component is "done" when its test file asserts no violations, and the ledger records which are done. New components must ship with the assertion.

### 2. Token & contrast layer

**Retune.** Adjust the failing color tokens in `:root` (light, `src/index.css:3–21`) and `[data-theme="dark"]` (`src/index.css:1754–1773`) until every documented foreground/background pair meets its threshold. The known offender is `--outline` used as text; the audit will identify the rest. Retuning stays within the existing warm hue family — the change should read as a slight deepening, not a new palette.

**New tokens.** Add to both themes:

```css
--focus-ring:        /* high-contrast accent, ≥3:1 against every surface it lands on */
--focus-ring-width:  2px;
--focus-ring-offset: 2px;
```

**Focus styling.** Replace both `outline: none` declarations, and add a global rule:

```css
:focus-visible {
  outline: var(--focus-ring-width) solid var(--focus-ring);
  outline-offset: var(--focus-ring-offset);
}
```

Any component that needs a different ring shape overrides the rule; none may remove it. `:focus:not(:focus-visible)` may suppress the ring for mouse interaction, which is the point of using `:focus-visible` rather than `:focus`.

**Reduced motion.** A single global block, placed after all animation declarations:

```css
@media (prefers-reduced-motion: reduce) {
  *, *::before, *::after {
    animation-duration: 0.01ms !important;
    animation-iteration-count: 1 !important;
    transition-duration: 0.01ms !important;
    scroll-behavior: auto !important;
  }
}
```

This covers all 24 existing declarations and any added later, which is why it is a wildcard rule rather than 24 targeted ones.

**Contrast test.** `src/a11y/__tests__/contrast.test.ts` reads `src/index.css`, parses the custom-property declarations out of the `:root` and `[data-theme="dark"]` blocks, and asserts a declared table of pairs. The table is data, checked into the test file, and each row states foreground token, background token, and required ratio:

| Kind | Threshold | SC |
|---|---|---|
| Body and secondary text | 4.5:1 | 1.4.3 |
| Large text (≥18.66px bold / ≥24px) | 3:1 | 1.4.3 |
| UI component boundaries, icons, focus ring | 3:1 | 1.4.11 |

The test computes WCAG relative luminance directly (a dozen lines — no new dependency) and fails with the offending pair and its actual ratio. Adding a color token without adding its row is caught by a completeness assertion: every token matching `--on-*`, `--outline*`, `--primary*`, `--error` must appear in at least one row.

The test guards the *token pairs*, not their usage. A component that puts `--on-surface-var` on `--primary` without a row for it is a ledger/audit finding, not a test failure — this boundary is deliberate, because parsing usage out of hand-written CSS is not reliable.

### 3. Primitives layer — `src/a11y/`

Each primitive exists because the same problem appears in three or more places. Each is unit-tested in isolation with `user-event`.

- **`<VisuallyHidden>`** — renders children in a `.sr-only` clip-rect span. Plus the `.sr-only` utility class in `index.css`. (Neither exists today.)
- **`<SkipLink>`** — first focusable element in `AppShell`; visually hidden until focused, then visible; targets `<main id="main-content" tabindex="-1">`. SC 2.4.1.
- **`useFocusTrap(ref, { active, yieldTo })`** — cycles Tab/Shift+Tab within a container, handles `Escape`. Consumers: the two privacy modals, `EditorPane`'s dialog, `CommandPalette`, `ActivityFeedPanel`, and the peek panels — the 13 existing dialog-role surfaces. SC 2.1.2.

  **Editor yielding.** BlockNote is ProseMirror underneath, and ProseMirror claims `Tab` and deep cursor focus for its own key handling. A naive trap fights the editor for focus control, and the user loses. The trap therefore takes an explicit `yieldTo` predicate, defaulting to *any element matching `[contenteditable="true"]` or inside one*: when the event target satisfies it, the trap does not intercept `Tab` at all and lets the editor handle the key. `Escape` remains trapped so the surface is always dismissable. This is a mechanism, not an exception — it means `EditorPane` can carry a real trap rather than the documented opt-out the Risks section previously contemplated, and it is tested by asserting that `Tab` inside a `contenteditable` region does not move focus to the trap's first element.
- **`useRestoreFocus({ active })`** — captures `document.activeElement` on open and restores it on close. Paired with the trap at every call site; a dismissed palette must return focus to where the user was, not to `<body>`. SC 2.4.3.
- **`useRovingTabIndex(items, { orientation })`** — one tab stop per composite widget, arrow keys to move within it, Home/End to jump. Consumers: the mode rail, the Brain entity list, the review queue list, and the fact-card list. Without it these are long tab-stop runs that make keyboard navigation impractical.
- **`useAnnouncer()`** — returns `announce(message, politeness)`. A single `<Announcer>` mounted once in `AppShell` owns exactly two live regions (one `polite`, one `assertive`) and serializes messages into them. This replaces the 33 scattered live regions; consolidating is the point, because independent regions announcing at once is how announcements get lost. Also used to announce mode changes on rail navigation (SC 4.1.3).

  **Queueing is part of the primitive, not an afterthought.** React batches state updates, so two components calling `announce()` in the same tick would otherwise overwrite the region's text before a screen reader ever reads the first message — the consolidation would silently make the problem it is meant to fix worse. The announcer therefore holds an internal FIFO queue per politeness level and drains it with a minimum gap of **150ms** between DOM writes. The gap is a floor, not a fixed delay: a message arriving when the queue is empty and no write has happened within the last 150ms is written immediately, so the common single-message case takes no latency penalty. Only colliding messages are spaced. The queue also collapses consecutive identical messages, since repeated identical text in a live region is either ignored by the screen reader or read twice, and neither is what the caller wanted.

  Two unit tests cover this specifically: two `announce()` calls in one tick must both reach the DOM in order with the gap between them, and a lone `announce()` must reach the DOM synchronously on flush.

### 4. Conformance ledger and manual checklist

**`docs/a11y/conformance-ledger.md`** — the audit artifact and the work queue for phases 2–6. One row per component, with columns: component path, mode/surface, applicable SCs, status (`pass` / `fail` / `n/a`), findings, and assigned fix phase. Produced in phase 1 by a systematic sweep of all 107 components; a component moves to `pass` only when it has both an `expectNoA11yViolations` assertion and a keyboard-interaction test.

**`docs/a11y/manual-checklist.md`** — the honest complement, listing what the automated gate provably cannot check, to be run before each release:

1. Real focus order matches visual order in every mode (jsdom has no layout).
2. Focus is never obscured by a sticky status bar or peek panel (SC 2.4.11).
3. Screen-reader pass on the five primary flows — onboarding, review approve/edit, entity page edit, palette navigation, settings — with VoiceOver (macOS) and NVDA (Windows).

   **Test the WebViews, not a browser.** Curated Thoughts renders in Tauri's platform WebViews — WKWebView on macOS, WebView2 on Windows, WebKitGTK on Linux — not in desktop Chrome, and their accessibility bridges to the host screen reader differ from Chrome's. WebView2 in particular has known inconsistencies in how `aria-modal="true"` is conveyed to NVDA, which is precisely the attribute the newly-trapped dialog surfaces depend on. So this step runs against `pnpm run tauri dev` or a built bundle on each OS, never against `vite dev` in a browser, and dialog surfaces get explicit attention: with the dialog open, content outside it must be unreachable by screen-reader browse mode, not merely by Tab. If a WebView proves not to honor `aria-modal`, the fallback is `aria-hidden` / `inert` on the background content, applied by the focus-trap primitive.
4. 200% browser zoom with no loss of content or function (SC 1.4.4), and 320 CSS-px reflow (SC 1.4.10).
5. Rendered contrast spot-check against the retuned palette in both themes, including text over `--elev-*` surfaces and disabled states.
6. `prefers-reduced-motion: reduce` enabled at the OS level — no animation runs.
7. Keyboard-only completion of each primary flow, mouse unplugged.

The checklist is a required, dated artifact per release, not advisory. A release note may claim AA conformance only for surfaces whose ledger rows are `pass` *and* whose checklist run is recorded.

## Phasing

Phase 1 is the foundation and is the whole first implementation plan. Phases 2–6 are ordered by user impact — onboarding first because it is the one flow every user must complete, and a user who cannot finish it never reaches anything else.

| Phase | Scope | Exit criterion |
|---|---|---|
| **1** | Enforcement, tokens, primitives, ledger, checklist, CI gate | Lint + contrast + primitive tests green in CI; ledger complete for all 107 components |
| **2** | Onboarding wizard (`components/setup/`, 11 files) | Ledger rows `pass`; wizard completable keyboard-only |
| **3** | Review editorial desk (`components/review/`, 7 files) | Ledger rows `pass`; approve/edit/reject keyboard-only |
| **4** | Brain mode (`components/brain/`, 8 files) + `CommandPalette` + peek panels | Ledger rows `pass`; trap/restore verified at every dialog surface |
| **5** | Settings (`components/settings/`, 12 files) + privacy modals | Ledger rows `pass` |
| **6** | Timeline, Tasks, Library, shell remainder, `components/health/` | Ledger rows `pass`; full manual checklist run recorded |

Each of phases 2–6 is a separate PR with its own plan.

## Testing Strategy

Test-driven, per the repo's existing practice. For every component fixed in phases 2–6, in this order:

1. Write the failing keyboard-interaction test with `user-event` — tab order, arrow keys where a roving index applies, `Escape` dismissal, focus restoration.
2. Write the failing `expectNoA11yViolations` assertion.
3. Fix the component.

Layer-specific tests:

- **Tokens:** the contrast test, which fails on any regressing token edit.
- **Primitives:** each of the six primitives gets its own unit test — the focus trap tested by tabbing off both ends, restore tested by asserting the previously-focused element regains focus, the roving index tested through a full arrow traversal including Home/End wraparound, the announcer tested by asserting message serialization into the correct politeness region.
- **Lint:** enforced by CI, not by a test.

No snapshot tests: they record markup rather than behavior, and would lock in the very markup being changed.

## Risks

- **Retuning tokens shifts the app's look.** Mitigated by staying in the warm hue family and by the change being small — but it is a visible change, and the phase-1 PR should show before/after screenshots of both themes.
- **jsdom cannot verify the most user-visible criteria.** Focus visibility, contrast in situ, and focus order all pass through the manual checklist. This is a deliberate accepted trade-off, recorded here so no one later mistakes a green CI run for a conformance claim.
- **The three platform WebViews may not behave identically.** The automated gate runs in jsdom and the design is written against standards, but conformance is ultimately delivered through WKWebView, WebView2, and WebKitGTK. A criterion can pass everywhere in CI and still fail for an NVDA user on Windows. This is why the manual checklist is per-OS and required, and why the `aria-modal` fallback is specified in advance rather than discovered late.
- **The 33→1 live-region consolidation changes announcement behavior.** Some messages that currently announce may become queued or dropped by serialization. Each converted call site needs its announcement verified during the phase's manual pass.
- **`useFocusTrap` retrofitted into BlockNote-hosting surfaces** (`EditorPane`) conflicts with ProseMirror's own `Tab` and cursor handling. The `yieldTo` predicate above is the designed answer; if it proves insufficient in practice, that surface takes a documented exception in the ledger rather than a forced trap.
