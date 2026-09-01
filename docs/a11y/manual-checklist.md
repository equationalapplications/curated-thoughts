# Manual Accessibility Checklist — per release

**What this checklist is for:** the automated gates (jsx-a11y lint, axe smoke test, contrast
unit tests) provably cannot verify real focus order, real screen-reader output, real OS
motion settings, or rendered contrast on a live GPU-composited surface. This document lists
exactly those items. Run it against **Tauri's platform WebViews** — WKWebView (macOS),
WebView2 (Windows), WebKitGTK (Linux) — via `pnpm run tauri dev` or a built bundle.
**Never run it against `vite dev` in desktop Chrome**: the accessibility bridges between
WebView and host screen reader differ from Chrome's, and WebView2 in particular has known
inconsistencies conveying `aria-modal="true"` to NVDA.

Check the dialog surfaces (`CommandPalette`, `PeekPanel`, `EditorPane`,
`EphemeralDisclosureModal` + `MigrationDisclosureModal`, `FactPowerMenu`) with
extra care: with a dialog open, content
outside it must be unreachable by screen-reader browse mode, not merely by Tab. If a WebView
does not honor `aria-modal`, verify the `aria-hidden`/`inert` fallback applied by the
inert guard (`src/a11y/inertGuard.ts`, `applyInertGuard` — used by `PeekPanel`) engages.

---

## 1. macOS — WKWebView + VoiceOver

| # | Check | SC / criterion | Pass |
|---|---|---|---|
| M1 | Keyboard-only pass: complete onboarding wizard, review approve/edit, entity page edit, palette navigation, settings — no mouse | 2.1.1 Keyboard | ☐ |
| M2 | Skip link appears on first Tab, activates, moves focus past nav | 2.4.1 Bypass Blocks | ☐ |
| M3 | Real focus order matches visual order in every mode | 2.4.3 Focus Order | ☐ |
| M4 | Focus visible on every interactive element (focus ring on dark + light) | 2.4.7 Focus Visible | ☐ |
| M5 | Focus not obscured by sticky status bar / peek panel (scroll to keep exposed) | 2.4.11 Focus Not Obscured (Min.) | ☐ |
| M6 | In dialogs: Tab cycles inside, focus restored to trigger on close (PeekPanel/EditorPane/palette) | 2.4.3, 3.2.2 | ☐ |
| M7 | VoiceOver announces each dynamic change exactly once (review queue updates, wizard step changes, save confirmations) | 4.1.3 Status Messages | ☐ |
| M8 | Dialog open → outside content unreachable in VoiceOver browse mode (VO+Left/Right) | 4.1.2 Name, Role, Value | ☐ |
| M9 | BlockNote editor: Tab yields to page navigation (yieldTo), Shift+Tab exits; Escape path documented | 2.1.1, 2.1.2 | ☐ |
| M10 | OS Reduce Motion on → no animations run (palette, wizard transitions, activity feed) | 2.3.3 Animation from Interactions | ☐ |
| M11 | Contrast spot check vs retuned tokens: body text, muted text, text over `--elev-*` surfaces, disabled states, both themes | 1.4.3, 1.4.6, 1.4.11 | ☐ |
| M12 | 200% zoom: no loss of content or function | 1.4.4 Resize Text | ☐ |
| M13 | 320 CSS-px narrow: no 2-D scrolling | 1.4.10 Reflow | ☐ |
| M14 | Target size ≥ 24×24 CSS px on icon-only controls (palette rows, mode rail, fact menu) | 2.5.8 Target Size (Min.) | ☐ |

## 2. Windows — WebView2 + NVDA

| # | Check | SC / criterion | Pass |
|---|---|---|---|
| W1 | Keyboard-only pass across the five primary flows | 2.1.1 | ☐ |
| W2 | Skip link activates | 2.4.1 | ☐ |
| W3 | Focus order + focus restore after dialog close (PeekPanel/EditorPane/palette) | 2.4.3 | ☐ |
| W4 | **NVDA + `aria-modal` inconsistency probe**: with dialog open, verify NVDA browse mode (`NVDA+B` boundary) does not read background content; if it does, confirm the inertGuard fallback engages and re-verify | 4.1.2 | ☐ |
| W5 | NVDA announces status messages once each (queue updates, wizard steps, privacy-mode changes) | 4.1.3 | ☐ |
| W6 | BlockNote Tab yieldTo behavior under WebView2 | 2.1.1, 2.1.2 | ☐ |
| W7 | Reduce Motion honored (Windows Settings → Accessibility → Visual effects) | 2.3.3 | ☐ |
| W8 | Contrast spot check, both themes, incl. `--elev-*` surfaces + disabled states | 1.4.3, 1.4.11 | ☐ |
| W9 | 200% zoom + 320 px reflow | 1.4.4, 1.4.10 | ☐ |
| W10 | Focus never obscured (sticky status bar overlaps content while tabbing) | 2.4.11 | ☐ |

## 3. Linux — WebKitGTK + Orca

| # | Check | SC / criterion | Pass |
|---|---|---|---|
| L1 | Keyboard-only pass across the five primary flows | 2.1.1 | ☐ |
| L2 | Skip link activates | 2.4.1 | ☐ |
| L3 | Focus order + restore in dialogs | 2.4.3 | ☐ |
| L4 | Orca reads dialog only (flat review confined to dialog when open); `aria-modal`/inert fallback verified | 4.1.2 | ☐ |
| L5 | Orca announces live-region updates once each | 4.1.3 | ☐ |
| L6 | BlockNote Tab yieldTo behavior under WebKitGTK | 2.1.1, 2.1.2 | ☐ |
| L7 | Reduce Motion honored (GNOME Accessibility → Reduce Animation) | 2.3.3 | ☐ |
| L8 | Contrast spot check, both themes | 1.4.3, 1.4.11 | ☐ |
| L9 | 200% zoom + 320 px reflow | 1.4.4, 1.4.10 | ☐ |

## 4. Every-OS quick gate (any run)

- [ ] No keyboard trap outside editors (2.1.2 — Escape releases every dialog)
- [ ] Focus ring visible on `--elev-*` surfaces in dark theme (1.4.11)
- [ ] Screen-reader announcements fire once, not duplicated (4.1.3)
- [ ] Autocomplete inputs (palette, wikilink suggestions) expose role + result count (4.1.2, 4.1.3)

## Recording results

Append a dated section below per release with OS/WebView/screen-reader versions, reviewer,
and per-section pass/fail. A release ships only with all boxes checked or tracked exceptions
linked to their remediation phase (see `docs/a11y/conformance-ledger.md`).

## Results log

| Date | Release/commit | OS + WebView | Screen reader | Result | Notes |
|---|---|---|---|---|---|
| 2026-08-31 | 8f9cb2e (phase 1 foundation) | — | — | NOT YET RUN | checklist created; first run due with phase 2 (onboarding) |
