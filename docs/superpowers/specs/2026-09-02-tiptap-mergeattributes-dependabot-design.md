# Dependabot GHSA-cp6q-959q-f8rh — Tiptap `mergeAttributes()` Prototype-Pollution Class Vulnerability — Design

**Date:** 2026-09-02
**Status:** Implemented 2026-09-02 (PR #139) — target 3.30.6 per §2.1; CodeRabbit findings incorporated
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

### 1.1 Current state of the alert surface (verified 2026-09-02 via Dependabot API)

Full alert inventory, `repos/equationalapplications/curated-thoughts/dependabot/alerts`:

| State | Count | Notes |
|---|---|---|
| open | **1** | #50 `@tiptap/core` GHSA-cp6q-959q-f8rh (this spec) |
| fixed | 43 | historical; undici, vite, tar, sqlx, openssl, glib, tauri, cmov, brace-expansion, js-yaml, postcss, nanoid |
| dismissed | 2 | #1 `glib` (GHSA-wrw7-89jp-8q8g) — `tolerable_risk`, rationale: "Transitive Linux-only dep via tauri→gtk→glib. Cannot upgrade without upstream tauri gtk-rs bindings update. No runtime impact on macOS." #3 same advisory — `no_bandwidth` |

No action is needed for dismissed or fixed alerts. **This single fix takes the
repository's open-alert count to zero** — there is no other Dependabot work
available to bundle into this PR.

### 1.2 How tiptap enters the tree

- `package.json` has **no direct tiptap dependency**. The editor stack is
  `@blocknote/core` / `@blocknote/mantine` / `@blocknote/react` **0.54.0**
  (direct deps), which declare `@tiptap/*@^3.29.2` transitively. BlockNote's
  latest published release is still 0.54.0 with `^3.29.2` (verified against the
  npm registry 2026-09-02) — **no BlockNote release ships a patched tiptap
  yet**, so a BlockNote dependency bump cannot fix this today.
- `pnpm-lock.yaml` resolves all 12 `@tiptap/*` packages at **3.30.2**
  (`@tiptap/core@3.30.2` and peers), which is inside the vulnerable range.
- `pnpm-workspace.yaml` sets `minimumReleaseAge: 20160` (14 days) to gate
  supply-chain risk, with an explicit `minimumReleaseAgeExclude` list that
  currently grandfathers the `@tiptap/*@3.30.2` set. The target version is
  likewise inside the 14-day window and must ride the same exclude mechanism,
  exactly as the 3.30.2 entries did.

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

Re-pin the tiptap stack from 3.30.2 → **3.30.6** via pnpm overrides + the
existing `minimumReleaseAgeExclude` mechanism. No BlockNote upgrade, no API
migration, no source changes.

**Why overrides and not a direct dependency?** Adding `@tiptap/core` as a
direct dependency of CT would pin only `core`; the other **11** `@tiptap/*`
packages are siblings resolved by BlockNote and would stay at 3.30.2.
Version-alignment across the tiptap suite is mandatory (they are
co-released and cross-typed). pnpm `overrides` pin the entire suite in one
place and are already the established pattern in this repo for transitive
security bumps (`undici`, `js-yaml`, `postcss`, `brace-expansion` — see
`package.json` → `pnpm.overrides`).

### 2.1 Why 3.30.6 and not 3.30.4 (revision, 2026-09-02)

The first draft of this spec targeted **3.30.4**, the advisory's
`first_patched_version`. Registry review found that choice actively harmful to
pin:

| Version | npm publish time | Contents |
|---|---|---|
| 3.30.4 | 2026-08-26T09:48Z | Fixes GHSA-cp6q-959q-f8rh (`mergeAttributes` prototype manipulation) |
| 3.30.5 | 2026-08-26T11:27Z | **Second security fix**: DoS — "crafted block or inline Markdown attributes could consume excessive CPU and block the browser or server event loop" |
| 3.30.6 | 2026-08-31T15:41Z | Patch-only: nested-list Markdown round-trip, YouTube-embed crash on missing `src`, whitespace-only mark serialization, React node-view perf |
| 3.31.0 | 2026-09-01T08:58Z | **Minor**: `@tiptap/react` realigns `selected` with ProseMirror node selections, adds `selectionInside` |

Three conclusions:

1. **3.30.4 was superseded 99 minutes after publication** by another `@tiptap/core`
   security patch. Pinning an override at 3.30.4 would deliberately freeze the
   suite on a version that carries a *known, already-fixed* DoS, and — because
   an override is a hard pin, not a range — nothing would later float us off it.
   Closing one advisory while pinning open a second published security fix is
   not a defensible outcome for a security PR.
2. **3.30.5's DoS has no Dependabot advisory**, so it will never generate an
   alert to prompt a follow-up. Alert-count-driven remediation would miss it
   entirely; this is exactly the gap a human-reviewed spec exists to catch.
3. **3.31.0 is excluded deliberately.** It is a minor that changes what
   `selected` means in `@tiptap/react` node views. BlockNote 0.54.0 is built
   against `^3.29.2` and was never tested against that semantic change; a
   security patch is the wrong PR to absorb editor-selection behavior risk.

**Target: 3.30.6** — the newest patch in the 3.30 line. It contains both
security fixes, stays within BlockNote's declared `^3.29.2` range, introduces
no public API changes, and satisfies Dependabot's `first_patched_version >=
3.30.4` so alert #50 still auto-closes. All 12 packages are confirmed published
at 3.30.6.

> **Timestamp note:** GitHub's release pages date v3.30.4/v3.30.5 to
> 2026-08-28 while the npm registry records 2026-08-26. pnpm's
> `minimumReleaseAge` gate reads **npm publish time**, so registry timestamps
> are the ones used for the age math throughout this spec.

## 3. Changes

### 3.1 `package.json` — `pnpm.overrides`

Add to the existing `pnpm.overrides` block (which currently holds `undici@6`,
`undici@7`, `js-yaml`, `postcss`, `brace-expansion@1`, `brace-expansion@5`, and
the two `@equationalapplications/*` first-party pins), preceded by the removal
reminder from §5:

```json
"@tiptap/core": "3.30.6",
"@tiptap/extension-bold": "3.30.6",
"@tiptap/extension-bubble-menu": "3.30.6",
"@tiptap/extension-code": "3.30.6",
"@tiptap/extension-floating-menu": "3.30.6",
"@tiptap/extension-italic": "3.30.6",
"@tiptap/extension-strike": "3.30.6",
"@tiptap/extension-text": "3.30.6",
"@tiptap/extension-underline": "3.30.6",
"@tiptap/extensions": "3.30.6",
"@tiptap/pm": "3.30.6",
"@tiptap/react": "3.30.6"
```

Note: `@tiptap/core` alone would satisfy the alert; the full-suite pin
prevents mixed-version resolution (`core@3.30.6` + `extension-*@3.30.2`) which
tiptap explicitly does not support and which typechecks would eventually catch
the hard way.

### 3.2 `pnpm-workspace.yaml` — `minimumReleaseAgeExclude`

Replace the 12 `@tiptap/*@3.30.2` exclude entries (listed under the
`@blocknote/react@0.54.0 editor stack` comment) with the `@3.30.6`
equivalents. The file's own header comment documents the convention: entries
are grandfathered status-quo versions that "age out of relevance as each
package passes the 14-day window."

3.30.6 was published **2026-08-31**, so it crosses the 14-day window on
**2026-09-14**; the exclude entries can be dropped in a follow-up chore after
that date (or left until the next lockfile regen, per existing practice).

Also update the stale section comment at `pnpm-workspace.yaml:26` —
`# --- @blocknote/react@0.54.0 editor stack (all @tiptap at 3.30.2) ---` — to
read 3.30.6, so no `3.30.2` reference survives the change anywhere in the file.

One caveat the header comment's wording does not cover: unlike every other
entry in that list, these entries **do** change a resolved version (that is the
point of this PR). The claim "this list changes NO resolved version" is scoped
to the entries added at `37c7db9`; do not propagate it to the new block.

### 3.3 `pnpm-lock.yaml`

Regenerated by `pnpm install --lockfile-only` after 3.1/3.2. Expected delta:
all `@tiptap/*@3.30.2` resolutions → 3.30.6, including the peer-suffixed
entries (`@tiptap/core@3.30.2(@tiptap/pm@3.30.2)` → `...3.30.6(...)`).

## 4. Acceptance Criteria

1. **No package left behind.** `grep -c '@tiptap/.*@3\.30\.2' pnpm-lock.yaml`
   returns **0** (covers the plain and peer-suffixed forms), *and* the positive
   check confirms the whole suite moved, not that packages vanished:

   ```sh
   grep -oE "@tiptap/[a-z-]+@3\.30\.6" pnpm-lock.yaml | sort -u | wc -l   # → 12
   ```

   Checking `@tiptap/core` alone is insufficient: a partial update
   (`core@3.30.6` + `extension-*@3.30.2`) would pass it while leaving exactly
   the mixed-version state §3.1 exists to prevent.
2. `grep -c '3\.30\.2' pnpm-workspace.yaml` returns **0** (catches the stale
   section comment as well as the entries).
3. **Release-age gate** — `pnpm install --lockfile-only` succeeds. This is the
   *only* command that exercises `minimumReleaseAge`: the gate is a
   resolution-time constraint, so it is evaluated during lockfile generation.
   If the `minimumReleaseAgeExclude` entries in §3.2 are wrong or incomplete,
   this is the step that fails.
4. **Lockfile consistency** — `pnpm install --frozen-lockfile` succeeds.
   `--frozen-lockfile` performs no resolution, so it does **not** re-check
   release age; it only proves the lockfile matches the manifest. It is a
   distinct check from criterion 3, not a substitute for it. (`pnpm-workspace.yaml`'s
   own header comment already records this — "frozen installs were never
   affected" — so the two are consistent.)
5. `pnpm typecheck` passes — proves the full-suite pin kept tiptap/BlockNote
   types aligned.
6. `pnpm test` passes — the only BlockNote-touching tests in the suite
   (`EditorPane.test.tsx:77-81`, `EntitySummarySection.test.tsx:19-29`) call
   `vi.mock("@blocknote/react")` and `vi.mock("@blocknote/mantine")`, so
   nothing in the automated suite loads real tiptap. A green run therefore
   proves no regression elsewhere, not that the editor works on the new
   tiptap. Real coverage lives in `src/__tests__/tiptapMergeAttributes.test.ts`
   (asserts the `mergeAttributes` security property directly) and in the
   criterion-9 manual smoke — both are required, not optional.
7. `pnpm lint` passes.
8. Dependabot alert #50 auto-resolves to *fixed* after merge to main
   (manifest+lockfile change on default branch re-scans). Verify the next day.
   Assert the alert's state **positively** — a "no open alerts" query also
   passes if #50 were merely *dismissed*, which would prove nothing about
   remediation:

   ```sh
   gh api repos/equationalapplications/curated-thoughts/dependabot/alerts --paginate \
     --jq '.[] | select(.number == 50) | .state'          # → fixed
   gh api repos/equationalapplications/curated-thoughts/dependabot/alerts --paginate \
     --jq '.[] | select(.state == "open")'                # → empty (repo-wide)
   ```
9. Editor smoke: launch dev app, open editor, type, apply bold/italic/code,
   save, reopen — rendering and persistence unchanged. Because 3.30.6 touches
   Markdown list serialization, additionally round-trip a **nested bulleted
   list** and confirm hierarchy survives save/reopen.

## 5. Risks & Notes

- **3.30.2 → 3.30.6 is a patch-range bump** inside `^3.29.2` that BlockNote
  already allows; semver risk is minimal. Two of the four intervening patches
  are security fixes; the other two are bug fixes with no API surface change.
- **The one behavioral surface worth smoking** is 3.30.6's nested-list Markdown
  serialization change — covered by acceptance criterion 9.
- **Release-age policy is honored, not bypassed:** the exclude list is the
  repo's own documented mechanism for shipping pinned versions before the
  14-day window, used for the exact same tiptap packages at 3.30.2.
- **Overrides are a hard pin, and hard pins rot.** This is the reason §2.1
  matters: an override does not float forward, so whatever version is chosen
  here is what CT runs until someone deliberately changes it. Add a reminder
  comment in `package.json` above the override block naming the advisory and
  the removal condition.
- **Upstream watch:** when BlockNote releases a version whose own range
  resolves to ≥ 3.30.6, remove the overrides (and then the exclude entries) in
  a follow-up chore to return to BlockNote's resolution. Re-check
  `npm view @blocknote/core version` — it was still 0.54.0 on 2026-09-02.
- **Deferred:** tiptap 3.31.0+ adoption, which needs a BlockNote-compatibility
  pass on the `@tiptap/react` `selected` semantics change (§2.1). Not a
  security matter; schedule with the next editor-stack upgrade.
- **Out of scope:** glib dismissals (documented tolerable_risk, upstream-blocked);
  any BlockNote minor/major upgrade (separate piece of work, none available).
- **CI does not validate the exclude list.** All five workflow install steps
  (`ci.yml` ×3, `build.yml`, `release.yml`) run `pnpm install --frozen-lockfile`,
  which skips resolution and therefore never evaluates `minimumReleaseAge`. A
  broken or incomplete `minimumReleaseAgeExclude` edit will sail through CI and
  only surface for the next person who regenerates the lockfile. Criterion 3
  (`--lockfile-only`, run locally) is the sole gate on §3.2 — do not skip it on
  the grounds that "CI will catch it". No CI changes are proposed here, but this
  is a standing blind spot worth its own chore.
- **CI otherwise:** repo CI runs typecheck/lint/tests on PRs. No CI changes needed.

## 6. Implementation Plan

Trivial enough to inline rather than spawn subagent-driven development:
single PR, three files, mechanical edits plus regenerated lockfile.

1. Branch `fix/tiptap-mergeattributes-3.30.6` off `main`.
2. Apply §3.1 (package.json overrides + removal-reminder comment) and §3.2
   (workspace exclude swap + section-comment fix).
3. `pnpm install --lockfile-only` → regenerate `pnpm-lock.yaml`.
4. Run acceptance criteria 1–7, then criterion 9 (the manual editor smoke —
   it is the only real exercise of the editor, since every BlockNote test in
   the suite mocks the library); fix anything that surfaces.
5. Commit: `fix(deps): bump @tiptap/* 3.30.2 → 3.30.6 (GHSA-cp6q-959q-f8rh, Dependabot #50)` — regular merge commit on merge, per house convention (no squash).
6. Open PR with this spec linked; merge when green; verify criterion 8 the next day.
