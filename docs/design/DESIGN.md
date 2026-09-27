# Curated Thoughts — Design Language

**Status:** Living document. Describes the look shipped in PR #234 (v2.19.1) plus the light-theme pass in PR #238.
**Source of truth for values:** `src/index.css` (token block at the top, dark theme under `[data-theme="dark"]`). If this document and the CSS disagree, the CSS wins — fix this document.
**Enforced by:** `src/__tests__/a11y-contrast.test.ts` (every fg/bg pair), `eslint-plugin-jsx-a11y`, axe in Vitest. See `docs/superpowers/specs/2026-08-31-ui-ux-accessibility-design.md`.
**Preview:** `docs/a11y/palette-preview.html`.

This document has two halves:

1. **Portable language** — brand, tokens, type, shape, states, motion, accessibility. Any Curated product (e.g. Curated Journal on mobile) should adopt this as-is.
2. **Desktop patterns** — the Obsidian / VS Code shell (activity rail, sidebar, status bar, panels, palette). Specific to this Tauri app; other products translate rather than copy.

---

## Part 1 — Portable language

### 1.1 Direction: calm chrome, content forward

The reference is Obsidian and VS Code: flat, quiet, dense, and neutral, so the user's notes are the loudest thing on screen.

- **Chrome recedes.** Regions are separated by a change of surface first and a hairline second. No heavy borders, no drop shadows on resting chrome.
- **One accent, used sparingly.** `--primary` belongs on the focus ring, the primary button, the active indicator and checked controls — places where it is the only coloured thing on screen. It is *not* a fill for large areas.
- **Selection is structure, not emphasis.** Selected rows use `--primary-container`, which is deliberately desaturated (especially in dark) so selecting things does not tint the whole app.
- **Flat and small-radius.** Large radii and soft shadows read as neumorphic, which is the look this app moved away from.
- **No decorative type.** One system sans for everything; no serif display face, no italic empty states.
- **Line icons, not emoji.** Emoji were replaced app-wide in #234.
- **Every state is designed.** Disabled, empty, error and loading each have their own treatment; nothing falls back to a platform default or to `opacity: 0.5`.

### 1.2 Colour

Two themes with one shared *structure* and different *temperatures*:

- **Light — "warm paper":** cream surfaces, brown-black text, amber accent.
- **Dark — "neutral slate":** blue-grey surfaces, cool grey text, soft blue accent.

The token roles are identical in both; only values change. Always reference the role, never the hex.

#### Surfaces (climb from page to raised)

| Token | Role | Light | Dark |
|---|---|---|---|
| `--bg` | Page / editor background; text-field fill | `#fffcf9` | `#101215` |
| `--surface` | Sidebars, panels, dialogs | `#fffcf9` | `#15181c` |
| `--elev-1` | Rail, status bar, cards, list groups, dialog footer | `#f9f3f2` | `#191d22` |
| `--elev-2` | Resting button fill; hover on quiet rows | `#f5eeeb` | `#1f242a` |
| `--elev-3` | Button hover/active | `#f1e9e3` | `#262c33` |
| `--surface-variant` | Highest tonal step | `#f0e0d0` | `#2c333b` |

In light, `--bg` and `--surface` are the same value; in dark, panels sit one step above the page. For docked regions (sidebar, panels) both work because the `--separator` hairline carries the boundary. Anything that must *dim* the page cannot rely on `--surface`: see the scrim rule under Overlays.

The whole light ladder is one temperature (warm cream). A near-white with a cool or violet cast next to warm chrome reads as a seam.

#### Text (climb from strongest to quietest)

| Token | Role | Light | Dark |
|---|---|---|---|
| `--on-surface` | Body text, titles, active labels | `#1f1b16` | `#ccd3dc` |
| `--on-surface-var` | Secondary text, section labels, idle icons | `#4f4539` | `#9ea8b5` |
| `--outline` | Tertiary text: placeholders, meta, scores, file-type icons | `#6b5e50` | `#8b95a3` |

`--outline` is used as text, so it is held at ≥4.5:1 on every surface.

#### Borders — two roles, on purpose

| Token | Role | Light | Dark |
|---|---|---|---|
| `--separator` | Hairlines between chrome regions and list rows. Quiet; not a contrast requirement. | `#e0d3c4` | `#262c33` |
| `--outline-var` | The boundary of an interactive control (input, button, card, dialog). ≥3:1 on `--bg` and `--elev-3` (WCAG 1.4.11). | `#94826e` | `#6f7a87` |

Never use `--separator` to outline a control, and never use `--outline-var` for a region divider.

#### Accent

| Token | Role | Light | Dark |
|---|---|---|---|
| `--primary` | Primary button, focus ring, active indicator, checked control, link-style buttons | `#835400` | `#7aa2f7` |
| `--primary-hover` | Primary hover | `#6d4600` | `#9cb9f9` |
| `--on-primary` | Label on `--primary` | `#ffffff` | `#0d1017` |
| `--primary-container` | Selected row / chosen option / drop-active fill | `#eadbc4` | `#262e3c` |
| `--on-primary-cont` | Label on `--primary-container` | `#2a1800` | `#ccd6e6` |
| `--secondary` | Secondary accent (rare) | `#705b40` | `#8fa0b4` |
| `--secondary-cont` / `--tertiary-cont` | Tonal fills (rare) | `#fbdebc` / `#d5eaba` | `#1d232a` / `#1f3029` |

#### Status

| Token | Light | Dark |
|---|---|---|
| `--error` | `#ba1a1a` | `#f07178` |
| `--success` | `#2e7d32` | `#4bbf73` |
| `--warning` | `#8a5200` | `#e0a458` |

**Status colour is applied as a tint, never a solid slab.** A solid status fill behind small text fails contrast in one theme or the other. The pattern:

```css
color: var(--error);
background: color-mix(in srgb, var(--error) 10%, transparent);
border: 1px solid color-mix(in srgb, var(--error) 30%, transparent);
```

Destructive hover uses the same idea (14–18% tint + `--error` border + `--error` glyph). Status dots (7px circles) may be solid because they carry no text.

#### Disabled — a third treatment

| Token | Light | Dark |
|---|---|---|
| `--disabled-bg` | `#faf5f0` | `#171a1f` |
| `--disabled-border` | `#ab9a84` | `#525c68` |
| `--disabled-fg` | `#6b5e50` | `#8b95a3` |

Disabled is not a dimmed copy of enabled. It is **structural**: a fill that recedes toward the page (instead of rising like `--elev-2`), a **dashed** border (no enabled control has one), a one-step-muted label that still clears 4.5:1, and a not-allowed cursor. Text fields skip the dash (a dashed field reads as a glitch) and rely on the fainter border and muted text. Borderless controls (rail icons) rely on the muted glyph alone.

#### Overlays

| Token | Use | Light | Dark |
|---|---|---|---|
| `--backdrop-modal` | Dialog the user must answer | `rgba(24,18,12,.42)` | `rgba(0,0,0,.58)` |
| `--backdrop-panel` | Side panel you can read past | `rgba(24,18,12,.30)` | `rgba(0,0,0,.40)` |
| `--backdrop-palette` | Transient way-station (command palette) | `rgba(24,18,12,.20)` | `rgba(0,0,0,.28)` |

Light scrims are tinted with the theme's own near-black, not `#000`, so the app behind does not go grey. The heavier the commitment, the darker the scrim; the palette is always lightest so the app stays legible behind it.

**Never build a scrim as a fraction of `--surface`.** In light, `--surface` equals `--bg`, so "85% surface" over the page has no visible effect. Use a backdrop token, or a tint of `--on-surface`.

### 1.3 Typography

- **Family:** the system stack, both themes.
  - `--font-ui`: `-apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, "Helvetica Neue", Arial, sans-serif`
  - `--font-mono`: `ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas, "Liberation Mono", monospace`
  - `--font-display` and `--font-body` both resolve to `--font-ui`; they exist so a display face could be added later without touching call sites.
- **No webfonts.** No network cost, works offline, and the serif/italic pairing was the loudest thing separating the app from its reference.
- **Monospace is for identifiers:** paths, ids, package names, timestamps typed by hand, numeric report values. Not for decoration.

Desktop scale (base `13px`, line-height `1.5`; this is a dense app):

| Role | Size / weight | Notes |
|---|---|---|
| Page/dialog title | 14–17px / 600 | Dialog `h2` 14px; splash 17px |
| Empty-state title | 13.5px / 500 | |
| Subsection heading | 13px / 600 | |
| Body, list rows, buttons | 12.5–13px / 400 (500 for primary / selected) | 12.5px is the workhorse |
| Secondary / description | 11.5–12px / 400, `--on-surface-var` | Descriptions capped at ~60ch |
| **Section label** | **10.5px / 600, UPPERCASE, `letter-spacing: 0.06em`, `--on-surface-var`** | The signature label: sidebars, panel headers, day headers, form legends |
| Chip / meta | 10–11px | |

Only three weights are used: 400, 500, 600. No italics for UI states.

### 1.4 Space, shape, depth

**Spacing** — a 4px base: `--sp-1` 4, `--sp-2` 8, `--sp-3` 12, `--sp-4` 16, `--sp-5` 24, `--sp-6` 32. Gaps inside a component are `--sp-2`/`--sp-3`; section gaps are `--sp-4`; pane gutters `--sp-5`. One inset per column: every control in a sidebar shares the same left and right edge, and sidebar actions stretch to that width.

**Radii** — `--r-sm` 4px (controls, rows, chips' containers, list groups), `--r-md` 6px, `--r-lg` 10px (dialogs, palette, drop targets, icon tiles), `--r-pill` 999px (chips, badges, progress bars, scrollbar thumb). Nothing larger than 10px outside pills.

**Shadows** — only on things that float above the page (dialogs, panels, palette, drop overlay). Resting chrome has none.

| Token | Light | Dark |
|---|---|---|
| `--shadow-sm` | `0 1px 2px rgba(31,27,22,.07)` | `0 1px 2px rgba(0,0,0,.40)` |
| `--shadow-md` | `0 4px 10px rgba(31,27,22,.09)` | `0 4px 10px rgba(0,0,0,.45)` |
| `--shadow-lg` | `0 14px 34px rgba(31,27,22,.16)` | `0 14px 34px rgba(0,0,0,.55)` |

In dark, the "this is in front" signal comes from the floating surface's border and shadow more than from the scrim.

### 1.5 Iconography

- Inline SVG line icons, `viewBox="0 0 24 24"`, `fill: none`, `stroke: currentColor`, `stroke-width: 1.5`, round caps and joins (Lucide-style).
- Sizes: 14px (`icon--sm`), 16px (default), 18px (`icon--lg`).
- Icons inherit the text colour beside them, so an icon is never darker than its label. Idle icons are `--on-surface-var`; file-type glyphs are `--outline`.
- Icon-only buttons are transparent, square, and always carry an accessible label.
- "Icon tile" (empty states, drop target, dialog header): the icon centred in a 28–34px `--r-lg` (or `--r-sm` for dialogs) box on `--elev-1`/`--elev-2` or `--primary-container`.

### 1.6 Components (portable shapes)

**Buttons**

| Variant | Fill | Border | Label | Hover |
|---|---|---|---|---|
| Default (secondary) | `--elev-2` | `--outline-var` | `--on-surface` | `--elev-3` |
| Primary | `--primary` | `--primary` | `--on-primary`, 500 | `--primary-hover` |
| Ghost | transparent | transparent | `--on-surface-var` | `--elev-2` fill, `--on-surface` |
| Link-style | transparent | transparent | `--primary` | `--elev-2` fill |
| Danger | as default | `--outline-var` | `--error` | 14% error tint, `--error` border |
| Disabled | see 1.2 Disabled | dashed | `--disabled-fg` | none |

One primary action per surface. In a dialog footer, the confirming action is primary and Cancel is default; they must never look identical.

**Inputs** — `--bg` fill, `--outline-var` border, `--r-sm`; hover darkens the border to `--outline`; placeholder `--outline`. Checkboxes and radios are drawn in the theme's tones (checked = `--primary` fill with an `--on-primary` mark), never the platform widget. Selects use a custom caret.

**Selection & lists** — rows are flush, `--r-sm`, transparent at rest; hover `--elev-1`/`--elev-2`; selected `--primary-container` + `--on-primary-cont`, weight 500. Row lists inside a bordered group (`--elev-1`, `--outline-var` border) are divided by `--separator` hairlines.

**Chips** — pill, 10px text, 1px 6px padding, `--elev-1` fill, `--outline-var` border, `--on-surface-var` text. A "highlighted" chip uses `--primary-container`. A score/meta chip drops its border and fill entirely.

**Cards / option cards** — `--elev-1`, `--outline-var` border, `--r-sm`, `--sp-3` padding; hover `--elev-2`; chosen = `--primary` border + `--primary-container` fill.

**Action row** — "a named operation, its consequence, and the button that runs it": title (12.5/500) and description (11.5, muted, ≤60ch) stacked on the left, the control on the right aligned to the first line. Rows are grouped in a bordered list with hairlines between them.

**Dialog** — one shape for every modal: `--surface`, `--outline-var` border, `--r-lg`, `--shadow-lg`, max 480px wide. Header (optional icon tile, 14/600 title, 12px muted subtitle, 26px ghost close) / body (12.5px, line-height 1.6, muted; `strong` goes to `--on-surface`) / footer (`--elev-1`, right-aligned actions). Header and footer separated by `--separator`.

**Empty state** — one component, centred: 34px icon tile, 13.5/500 headline, 12.5px muted hint (≤42ch), then actions. Never italic, never a lone floating line.

**Error banner** — tinted status pattern (1.2), `--r-sm`, 12px text.

**Progress** — 4px pill track on `--elev-2`, `--primary` fill.

**Third-party components** — anything that ships its own palette (e.g. BlockNote's `--bn-colors-*`) has its chrome variables mapped onto our tokens in both themes: transparent editor background, `--on-surface` text, `--surface` menus, `--primary-container` selection. The user's own content colours (highlights) are left alone.

**Scrollbars** — thin, transparent track, `--scrollbar-thumb` capsule that only reaches the outline value on hover. Chrome, not a seam.

### 1.7 Motion

- Short and functional: 100–150ms colour/background/border transitions on hover; 160ms `ease-out` slide for side panels; 300ms for progress width.
- Nothing bounces, nothing scales on hover.
- `prefers-reduced-motion: reduce` collapses all animation and transition durations (must stay the last rule in `index.css`).

### 1.8 Accessibility (non-negotiable)

- **WCAG 2.2 AA** across both themes. Text ≥4.5:1, control boundaries and focus ≥3:1 — asserted in tests, not eyeballed.
- **Focus is always visible:** `2px solid var(--primary)`, 1px offset. If a composite control draws its own focus indicator, the inner element's ring moves to the container; it never simply disappears.
- **State is never carried by colour alone:** disabled uses a dash, selection uses fill *and* weight, status uses text or icon plus colour.
- Icon-only controls are labelled; decorative SVGs get `aria-hidden="true" focusable="false"` (some older call sites still lack `aria-hidden`).
- Announcements go through the shared announcer, not ad-hoc live regions.

---

## Part 2 — Desktop patterns (this app)

These are the Obsidian / VS Code shell pieces. They depend on a pointer, a wide window and a keyboard.

| Pattern | Spec |
|---|---|
| **Density** | `--control-h` 28px; body 13px; list rows 24–26px. |
| **Activity rail** | `--rail-w` 48px, `--elev-1`, `--separator` on the right. 40px icon buttons, borderless. Active mode = `--primary` glyph + a 2px `--primary` left-edge bar (VS Code style), not a filled pill. Count badge: 15px pill, `--primary`/`--on-primary`, 9.5px/600. Alert: 7px `--error` dot. |
| **Sidebar** | `--side-w` 264px, `--surface`, `--separator` on the right, `--sp-3` inset, `--sp-3`–`--sp-4` gaps, uppercase section labels. |
| **Main pane** | `--bg`; toolbars are a single `--separator`-bottomed row with `--sp-5` side gutters. |
| **Status bar** | `--status-h` 26px, `--elev-1`, `--separator` on top, 11px text, three-column grid (left / centre / right). Segments are 18px transparent buttons that gain `--elev-2` on hover. |
| **Side panels** (Activity, Peek) | Fixed right, 380–440px, `--surface`, `--outline-var` left border, `--shadow-lg`, `--backdrop-panel`. 40px header with section label or title and a 24px ghost close. Slides in 160ms. |
| **Command palette** | 560px, 12vh from top, `--r-lg`, `--backdrop-palette`. 44px borderless input row with a search glyph and a 2px `--primary` inset underline on focus. 26px options; active = `--primary-container`. Footer on `--elev-1` with key hints (`--elev-2` keycaps). |
| **Sticky day headers** | Section-label type on `--bg`, followed by a hairline. |
| **Hover-revealed actions** | Row actions (e.g. delete) are hidden until row hover. *Desktop only.* |

---

## Part 3 — Porting to another Curated product

Adopt Part 1 unchanged: the token names and roles, both palettes, the tint rule for status, the disabled treatment, the type roles, the radii, the icon style, the dialog/empty/action-row shapes, and the contrast test. Port the contrast test along with the tokens.

Translate Part 2. For a touch/mobile app (Curated Journal), specifically:

| Desktop | Mobile translation |
|---|---|
| 28px controls, 24px rows | ≥44pt touch targets (iOS) / 48dp (Android). Visual height may stay compact if the hit area is padded. |
| 13px base, 10.5px section labels | Respect Dynamic Type / font scaling. Base ~15–17pt body; section label ~12–13pt uppercase 600 with the same tracking. Keep the *ratios* and roles, not the pixel values. |
| Activity rail + active left bar | Bottom tab bar on `--elev-1` with a `--separator` top hairline; active tab = `--primary` glyph and label. No filled pill. |
| Sidebar + main pane | Stack navigation; list screens on `--bg`, grouped lists in `--elev-1` bordered groups with hairlines. |
| Side panels / dialogs | Bottom sheets or native modals using the dialog shape (header / body / footer), `--r-lg` top corners, `--backdrop-modal` or `--backdrop-panel`. |
| Command palette | Search field at the top of the relevant list; no global palette required. |
| Status bar | Not needed; surface status inline (status dot + label) or as a tinted banner. |
| Hover states | None. Use pressed states (`--elev-2` / `--elev-3`) and haptics sparingly. Hover-revealed actions become swipe actions or an explicit overflow menu. |
| `:focus-visible` ring | Still required for keyboard / switch access on iPad and web. |
| `prefers-reduced-motion` | Honour the OS Reduce Motion setting (e.g. Reanimated's `useReducedMotion`). |
| System font stack | Platform system font (SF Pro / Roboto); `ui-monospace` / `monospace` for identifiers. |
