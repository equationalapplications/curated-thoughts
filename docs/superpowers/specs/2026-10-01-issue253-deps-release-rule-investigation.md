# Investigation — issue #253: `chore(deps)` first-party adoptions don't trigger a release

**Date:** 2026-10-01
**Status:** Investigated 2026-10-01 (review loop)
**Branch:** `feat/issue-253-deps-release-rule` (to be created)
**Priority:** High — release-pipeline correctness; every first-party engine adoption is silently stranded until the next unrelated `feat:`/`fix:` merge.

## Summary

Conventional commits typed `chore(deps)` analyze as **no release** in semantic-release under the
repo's `.releaserc.json`. First-party package adoptions (`@equationalapplications/*`) are written as
`chore(deps)` because they *look* like dependency housekeeping — but they carry engine schema
migrations that `schema_guard` pins to. Result: an adoption merges to main, the Release workflow
runs, semantic-release says "no release", `dispatch-build` is skipped, and the migration sits on
main unreachable by any installed release.

## Evidence [V — controller-verified against live main `b7fc952`, 2026-10-01]

### 1. Live analyzer repro (real plugin, real config)

Ran `@semantic-release/commit-analyzer`'s `analyzeCommits` with the repo's actual
`.releaserc.json` commit-analyzer options over the actual commit subjects:

| Commit subject (source) | Analyzed release |
|---|---|
| `chore(deps): adopt @equationalapplications llm-wiki 7.9.0 (engine migration 13 schema sync) (#252)` — merge 5dc58d1, verbatim | **null (no release)** |
| `chore(deps): bump the minor-and-patch group with 11 updates (#251)` — merge 46f4563, verbatim | **null (no release)** |
| `feat(walker): exclude archive/ and backups/ at vault root (#250)` | minor |
| `fix(okf): refuse shrunk bodies` (synthetic) | patch |

### 2. Config gap

`.releaserc.json` `releaseRules` (lines 11–36) has rules for `breaking`/`feat`/`fix`/`perf`/`style`/
`revert` — **no `chore` rule**. The conventionalcommits default then applies: `chore` → no release.

### 3. The stranded migration was rescued manually, the gap was not

PR #252 merged 2026-09-30 (`5dc58d1`). The Release workflow logged
`The commit should not trigger a release` → `Analysis of 1 commits complete: no release`;
`dispatch-build` was skipped. A **manual human tag push** (`v2.23.0`, 2026-09-30 23:37 EDT, on
release commit `b7fc952`) fired Build correctly — the tag→build machinery is intact
(`.github/workflows/release.yml` tag fallback + `build.yml` tag trigger). The gap is purely the
commit-type classification. CI's `release-config` job (`scripts/check-release-config.mjs`) passes
today because its `VERSION_MATRIX` has no `chore(deps)` row — the smoke test faithfully tests a
config that has the bug.

### 4. Dependabot blast radius (constrains the fix)

`.github/dependabot.yml` runs **4 ecosystems weekly** (npm, cargo src-tauri, cargo tools,
github-actions), all emitting `chore(deps)`/`chore(deps-dev)` style subjects (verified: PR #251
merged as `chore(deps): bump the minor-and-patch group with 11 updates`). A blanket
`{ "type": "chore", "scope": "deps", "release": "patch" }` rule would cut a release roughly every
week from routine Dependabot bumps alone.

## Root cause

Commit-type classification treats first-party adoptions as housekeeping. The commit *author* knows
it is releasable content; the *pipeline* cannot tell first-party from third-party `chore(deps)`
because both use the same type+scope.

## Proposed fix direction (carried into the design doc)

1. Distinguish the two classes at the commit-subject level: move Dependabot to an inert prefix
   (e.g. `chore(bot)`) via `dependabot.yml` `commit-message.prefix` per ecosystem.
2. Add a `releaseRules` entry making first-party `chore(deps)` at least a **patch** release.
3. Add `chore(deps)` rows (both classes) to `scripts/check-release-config.mjs` `VERSION_MATRIX` so
   the classification is pinned pre-merge by CI.

Open questions (answered in the design doc / for reviewers): patch vs minor as the floor for
first-party adoptions; whether the Dependabot prefix change is acceptable (old open PRs keep old
titles); exact `scope` semantics for `chore(deps-dev)`.

## Ops-note cross-reference

`curated-thoughts-ops` skill §Post-merge status pass already documents this exact behavior as a
known hazard ("green Release run does NOT imply a tag… `chore(deps)` — even first-party package
adoptions carrying schema migrations — analyzes as 'no release'"). This issue closes the hazard at
the pipeline level instead of compensating manually.
