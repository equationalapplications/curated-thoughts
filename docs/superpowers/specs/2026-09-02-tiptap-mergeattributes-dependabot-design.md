# Dependabot GHSA-cp6q-959q-f8rh — Tiptap `mergeAttributes()` Prototype-Pollution Class Vulnerability — Design

**Date:** 2026-09-02
**Status:** Draft
**Branch:** `spec/tiptap-mergeattributes-dependabot`
**Priority:** P2 (medium severity; desktop-only attack surface, exploit requires attacker-controlled JSON flowing into editor schema attributes)

## 1. Problem

Dependabot alert **#50** (GHSA-cp6q-959q-f8rh, medium, CVSS-T v4 6.4) is the only
open alert on this repository. It flags `@tiptap/core` as vulnerable to a
prototype-manipulation flaw in `mergeAttributes()`:

> `mergeAttributes()` uses ordinary bracket assignment on keys returned by
> `Object.entries()`. An own `__proto__` key from attacker-controlled JSON
> replaces the *merged object's* prototype; `Object.keys()` and own-property
> checks then show no attacker attributes. When the result is used as a
> ProseMirror `DOMOutputSpec` attribute object, `prosemirror-model`'s
> `DOMSerializer.renderSpec()` enumerates it with `for...in` and applies
> inherited values with `setAttribute()` — inherited `src` / `onerror` values
> land on a rendered `<img>` and execute. ([GHSA-cp6q-959q-f8rh](https://github.com/advisories/GHSA-cp6q-959q-f8rh))

This is per-object prototype manipulation (not global `Object.prototype`
pollution), introduced in tiptap v2.0.0-alpha.0 and fixed in **3.30.4**.
Vulnerable range: `>= 2.0.0-alpha.0, < 3.30.4`.

### 1.1 Current state of the alert surface (verified 2026-09-02)

Full alert inventory via the Dependabot API, `repos/.../dependabot/alerts`:

| State | Count | Notes |
|---|---|---|
| open | **1** | #50 `@tiptap/core` GHSA-cp6q-959q-f8rh (this spec) |
| fixed | 43 | historical; undici, vite, tar, sqlx, openssl, glib, tauri, cmov, brace-expansion, js-yaml, postcss, nanoid |
| dismissed | 2 | #1/#3 `glib` (GHSA-wrw7-89jp-8q8g) — `tolerable_risk`, rationale: "Transitive Linux-only dep via tauri→gtk→glib. Cannot upgrade without upstream tauri gtk-rs bindings update. No runtime impact on macOS." |

No action is needed for dismissed or fixed alerts. Single-alert scope.

### 1.2 How tiptap enters the tree

- `package.json` has **no direct tiptap dependency**. The editor stack is
  `@blocknote/core` / `@blocknote/mantine` / `@blocknote/react` **0.54.0**
  (direct deps), which declare `@tiptap/*@^3.29.2` transitively. BlockNote's
  latest published release is still 0.54.0 with `^3.29.2` — **no BlockNote
  release ships tiptap ≥ 3.30.4 yet**, so a dependency bump cannot fix this
  today.
- `pnpm-lock.yaml` resolves all 13 `@tiptap/*` packages at **3.30.2**
  (`@tiptap/core@3.30.2` and peers), which is inside the vulnerable range.
- `pnpm-workspace.yaml` sets `minimumReleaseAge: 20160` (14 days) to gate
  supply-chain risk, with an explicit `minimumReleaseAgeExclude` list that
  currently grandfathered the `@tiptap/*@3.30.2` set. 3.30.4 was published
  **2026-08-26** — 7 days before this spec — so it is age-blocked by default
  and must ride the same exclude mechanism, exactly as the 3.30.2 entries did.

### 1.3 Exposure assessment

The vulnerable code path requires attacker-controlled attribute objects
containing an own `__proto__` key to reach `mergeAttributes()` and then flow
through `DOMSerializer`. CT is a **local desktop app** (Tauri); the editor
renders operator-authored markdown/wiki content, and editor content does not
round-trip untrusted remote JSON into schema attributes in the current feature
set. Practical exploitability is low, but the class of flaw (attribute
injection reaching `setAttribute`) is exactly what a wiki app that later gains
shared/deposited-content rendering must not carry. Remediation is a version
bump only — no API changes — so cost is minimal and we fix it now.

## 2. Approach

Re-pin the tiptap stack from 3.30.2 → **3.30.4** via pnpm overrides + the
existing `minimumReleaseAgeExclude` mechanism. No BlockNote upgrade, no API
migration, no source changes.

**Why overrides and not a direct dependency?** Adding `@tiptap/core` as a
direct dependency of CT would pin only `core`; the other 12 `@tiptap/*`
packages are siblings resolved by BlockNote and would stay at 3.30.2.
Version-alignment across the tiptap suite is mandatory (they are
co-released and cross-typed). pnpm `overrides` pin the entire suite in one
place and are already the established pattern in this repo for transitive
security bumps (`undici`, `js-yaml`, `postcss`, `brace-expansion` — see
`package.json` → `pnpm.overrides`).

## 3. Changes

### 3.1 `package.json` — `pnpm.overrides`

Add (keep alphabetical grouping with existing entries):

```json
"@tiptap/core": "3.30.4",
"@tiptap/extension-bold": "3.30.4",
"@tiptap/extension-bubble-menu": "3.30.4",
"@tiptap/extension-code": "3.30.4",
"@tiptap/extension-floating-menu": "3.30.4",
"@tiptap/extension-italic": "3.30.4",
"@tiptap/extension-strike": "3.30.4",
"@tiptap/extension-text": "3.30.4",
"@tiptap/extension-underline": "3.30.4",
"@tiptap/extensions": "3.30.4",
"@tiptap/pm": "3.30.4",
"@tiptap/react": "3.30.4"
```

Note: `@tiptap/core` alone would satisfy the alert; the full-suite pin
prevents mixed-version resolution (`core@3.30.4` + `extension-*@3.30.2`) which
tiptap explicitly does not support and which typechecks would eventually catch
the hard way.

### 3.2 `pnpm-workspace.yaml` — `minimumReleaseAgeExclude`

Replace the `@tiptap/*@3.30.2` exclude entries (12 lines, listed under the
`@blocknote/react@0.54.0 editor stack` comment) with the `@3.30.4` equivalents.
Comment in that file documents the convention: entries are "grandfathered
status-quo versions... ages out of relevance as each package passes the 14-day
window." 3.30.4 crosses the window on **2026-09-09**; the exclude entries can
be dropped in a follow-up chore after that date (or left until the next
lockfile regen, per existing practice).

### 3.3 `pnpm-lock.yaml`

Regenerated by `pnpm install --lockfile-only` after 3.1/3.2. Expected delta:
all `@tiptap/*@3.30.2` resolutions → 3.30.4, including the peer-suffixed
entries (`@tiptap/core@3.30.2(@tiptap/pm@3.30.2)` → `...3.30.4(...)`).

## 4. Acceptance Criteria

1. `grep -c "@tiptap/core@3.30.2" pnpm-lock.yaml` returns **0** (allowing the
   peer-suffix form to be checked the same way); `grep "@tiptap/core@3.30.4"` hits > 0.
2. `pnpm install --frozen-lockfile` succeeds cleanly (lockfile consistency,
   release-age gate passes via the exclude entries).
3. `pnpm typecheck` passes — proves the full-suite pin kept tiptap/BlockNote
   types aligned.
4. `pnpm test` passes — the editor suite (`src/__tests__/`, including
   `axe-core.test.ts`) exercises the BlockNote editor on the new tiptap.
5. `pnpm lint` passes.
6. Dependabot alert #50 auto-resolves to *fixed* after merge to main
   (manifest+lockfile change on default branch re-scans). Verify the next day
   via `gh api repos/equationalapplications/curated-thoughts/dependabot/alerts --jq '.[] | select(.state=="open")'`
   → empty.
7. Editor smoke: launch dev app, open editor, type, apply bold/italic/code,
   save, reopen — rendering and persistence unchanged.

## 5. Risks & Notes

- **3.30.2 → 3.30.4 is a patch-range bump** inside `^3.29.2` that BlockNote
  already allows; semver risk is minimal. The fix commit (tiptap `01d7af8`) is
  scoped to `mergeAttributes()` key handling + regression tests.
- **Release-age policy is honored, not bypassed:** the exclude list is the
  repo's own documented mechanism for shipping pinned versions before the
  14-day window, used for the exact same tiptap packages at 3.30.2.
- **Upstream watch:** when BlockNote releases a version that bumps tiptap
  past 3.30.4 natively, the overrides (and then the exclude entries) should be
  removed in a follow-up chore to return to BlockNote's own resolution —
  overrides left stale forever will eventually fight upstream. Add a reminder
  comment in `package.json` above the override block.
- **Out of scope:** glib dismissals (documented tolerable_risk, upstream-blocked);
  any BlockNote minor/major upgrade (separate piece of work, none available).
- **CI:** repo CI runs typecheck/lint/tests on PRs; the frozen-lockfile install
  also validates the exclude-list edits. No CI changes needed.

## 6. Implementation Plan

Trivial enough to inline rather than spawn subagent-driven development:
single PR, three files, mechanical edits plus regenerated lockfile.

1. Branch `fix/tiptap-mergeattributes-3.30.4` off `main`.
2. Apply §3.1 (package.json overrides) and §3.2 (workspace exclude swap).
3. `pnpm install --lockfile-only` → regenerate `pnpm-lock.yaml`.
4. Run acceptance criteria 2–5; fix anything that surfaces.
5. Commit: `fix(deps): bump @tiptap/* 3.30.2 → 3.30.4 (GHSA-cp6q-959q-f8rh, Dependabot #50)` — regular merge commit on merge, per house convention (no squash).
6. Open PR with this spec linked; merge when green; verify criterion 6 the next day.
