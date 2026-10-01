# Release rule: first-party `chore(deps)` adoptions cut a release (issue #253)

**Date:** 2026-10-01
**Status:** Proposed — review cycle 1 adjudicated: Opus c1 REQUEST CHANGES (M1 + m1–m6, all applied); GLM self-review findings applied. Delta review pending.
**Branch:** `feat/issue-253-deps-release-rule`
**Priority:** High — release-pipeline correctness; every first-party engine adoption is silently stranded until the next unrelated `feat:`/`fix:` merge.

Investigation (live analyzer repro, Dependabot blast radius, tag-machinery audit):
`2026-10-01-issue253-deps-release-rule-investigation.md` (same directory).

## Problem

`chore(deps)` analyzes as "no release" under `.releaserc.json` (no `chore` rule in `releaseRules`;
conventionalcommits default). First-party adoptions (`@equationalapplications/*`, carrying engine
schema migrations) merge to main, the Release workflow declines to cut a version, `dispatch-build`
is skipped, and the migration is unreachable by any installed release until the next releasable
commit. Verified live: the actual `5dc58d1` adoption subject analyzes `null` through the real
`analyzeCommits` with the repo's real config; PR #252 needed a manual human tag (`v2.23.0`) to ship.

## Approach (all config + CI; no app code)

1. **D1 — separate the two `chore(deps)` classes at the source.** Dependabot is moved off the
   `deps` scope so the scope becomes human-first-party-only. `.github/dependabot.yml`: add
   `commit-message: { prefix: "chore(bot)" }` to ALL FOUR ecosystems (npm, cargo `/src-tauri`,
   cargo `/tools`, github-actions). Result: bot subjects become `chore(bot): bump …` — type
   `chore`, scope `bot`. Pins (Opus c1): (a) with an explicit `prefix`, dev-dependency bumps use
   the same prefix — `prefix-development` (hyphenated key) is NOT set, because we do not want
   `deps-dev` scope variants to reappear; (b) `include: "scope"` must NOT be set — it would make
   Dependabot append a scope after the prefix (`chore(bot)(deps): …`), which no longer parses as
   scope `bot`; (c) write the prefix WITHOUT a trailing colon — Dependabot inserts `: ` itself
   after a prefix ending in `)`, and a hand-added colon yields `chore(bot):: bump`.
   Policy note (Opus c1 m6): any OTHER automation that writes `chore(deps)` subjects (e.g.
   agent-driven lockfile maintenance) must likewise move off the `deps` scope — the scope is
   human-first-party-only by convention, not merely Dependabot-free.
2. **D2 — releasable first-party rule.** `.releaserc.json` `releaseRules` gains
   `{ "type": "chore", "scope": "deps", "release": "patch" }`.
   **Floor decision: patch, not minor.** Rationale: the schema surface itself already shipped in
   llm-wiki's own versioned releases; the adoption commit enables it in CT and must simply never be
   stranded. Majors still come from `breaking:`; a genuinely feature-level adoption can be typed
   `feat(deps)` by the author and rides the existing minor rule. (Kurt's call at spec approval;
   patch is the recommendation — the cheap, never-stranded floor.)
   Scope matching is GLOB matching (`micromatch.isMatch` in the analyzer — Opus c1 m1), but the
   literal pattern `deps` behaves exactly (`deps` ≠ `deps-dev` ≠ `bot`; never generalize the rule
   to `deps*` — that would re-match `deps-dev`), so Dependabot traffic analyzes no-release exactly
   as today. Pin: a multi-scope subject such as `chore(deps,ui): …` does NOT match and stays
   no-release — authors must use a single `deps` scope on adoption commits.
   Pin: **any human-authored `chore(deps)` is releasable by definition** — including routine
   human lockfile refreshes and dev-tool bumps, accepted at the patch floor (Opus c1 m6); the
   convention going forward is that automation never uses the `deps` scope (enforced by D1) — and
   the rule order within `releaseRules` is irrelevant (the analyzer takes the highest release type
   among ALL matching rules; see the existing no-ordering-assertion comment in
   `check-release-config.mjs`). Plain `chore` (no scope) and every other unmatched type remain
   no-release: configured rules are consulted instead of the built-in defaults whenever any of
   them matches, and the built-in default for `chore` is no-release either way (verified live:
   current config analyzes `chore(deps)` and plain-`chore` traffic as `null`).
3. **D3 — release notes must not go silently empty.** The conventionalcommits notes template hides
   `chore` by default, so a pure-adoption release would cut a version whose notes body is empty.
   `@semantic-release/release-notes-generator` `presetConfig` gains an explicit full `types` list —
   default conventionalcommits types re-declared verbatim (so nothing else changes section) plus
   `{ type: "chore", scope: "deps", section: "Dependencies", hidden: false }`. Pins (Opus c1 M1):
   (a) the list is FULL and explicit — the preset REPLACES the default types rather than merging
   (`writer.js` spreads `...config` over defaults); the existing `presetConfig: {}` is replaced by
   the list; (b) **ORDER IS LOAD-BEARING: the scoped `{ type: "chore", scope: "deps", … }` entry
   MUST be placed BEFORE the unscoped default `{ type: "chore", hidden: true }` entry** —
   `findTypeEntry` uses `Array.find` (first match wins) and the unscoped `chore` entry matches
   every `chore` commit including `chore(deps)`; appending the scoped entry after the defaults
   reproduces the empty-notes bug this D3 exists to prevent; (c) array position also sets section
   order (`commitGroupOrder`) — placing the scoped entry immediately before the default `chore`
   row renders "Dependencies" after "Reverts", which is accepted.
4. **D4 — pin the classification in CI.** `scripts/check-release-config.mjs` `VERSION_MATRIX`
   gains rows: first-party adoption subject (the verbatim `5dc58d1` message) → `patch`;
   `chore(bot):` Dependabot-style subject → `null`; `chore(deps-dev):` legacy dev-dependency
   subject → `null` (pins the exact-match behavior — Opus c1 m1); plain `chore:` (no scope) →
   `null` (pins D2's claim — Opus c1 m4); and the previously unpinned `{ type: "style",
   release: "patch" }` rule gets its `style:` → `patch` row, keeping the matrix's "every rule
   asserted end to end" header comment true (Opus c1 m4). Notes-render checks gain (Opus c1 m5):
   a SECOND `generateNotes` invocation whose commit set contains ONLY `chore(deps)` + `chore(bot)`
   commits — the existing single `NOTES_COMMITS` call already carries feat/breaking commits and
   cannot prove a pure-adoption release renders non-empty — asserting `notes.includes(
   "Dependencies")` and that the `chore(bot)` subject does NOT appear in the rendered notes.
   This is the pre-merge tripwire — the bug class is "config analyses differently than assumed",
   which only this smoke test can catch before merge (including the D3 ordering trap: a
   wrongly-ordered types list fails this check with empty notes).

## Transitions / roll-forward

- Old-subject Dependabot PRs already open keep `chore(deps)` subjects; if one merges before
  re-rolling, it cuts a patch release — transient, harmless (a release with a dependency bump is
  not wrong). Re-roll or close/recreate is optional hygiene, not required for correctness.
- The fix commit itself lands as `feat(ci): release first-party chore(deps) adoptions (#253)` —
  `feat` → minor release, which immediately exercises Release→tag→`dispatch-build`→Build end to
  end on merge. Post-merge pass then verifies the tag per the ops skill (never claim "shipped" off
  green checks alone).

## Error handling

N/A — workflow/config change. Failure mode after this fix is a failed Release run (visible), not a
silent no-release.

## Testing

`node scripts/check-release-config.mjs` locally (mirrors CI `release-config` job):

- New VERSION_MATRIX rows pass (first-party → patch; `chore(bot)` → null; `chore(deps-dev)` →
  null; plain `chore:` → null; `style:` → patch — Opus c1 m1/m4).
- Existing rows unchanged (feat/fix/feat!/fix!/perf/revert).
- Notes render: SECOND `generateNotes` call with an adoption-only + bot commit set renders
  non-empty with a "Dependencies" section; `chore(bot)` subject absent from rendered notes
  (Opus c1 m5).
- Config JSON parses; `actionlint`-clean workflow files untouched (no workflow change in this PR).

## Out of scope / open questions

- patch vs minor floor — decided D2 (patch), flagged for Kurt at spec approval.
- Renaming the first-party adoption convention to `feat(deps)` repo-wide — docs-level follow-up if
  Kurt prefers minor-by-default adoptions.
- Other repos sharing this release config pattern (axon, etc.) — same gap likely; separate issues.

Fixes #253.
