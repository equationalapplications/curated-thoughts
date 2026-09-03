# Restore `major` Releases for Breaking Changes — Design

**Date:** 2026-09-03
**Status:** Proposed
**Branch:** `fix/release-rules-breaking-major`
**Priority:** P2 (release correctness; closes issue #160)

> **Prerequisite — read before starting.** This change edits
> `scripts/check-release-config.mjs`, which does **not exist on `main`
> yet**. It is created by the release-toolchain guard
> (`2026-09-03-release-toolchain-guard-design.md`, branch
> `spec/release-toolchain-guard`). **That PR must be merged first.** If
> `scripts/check-release-config.mjs` is absent when you start, stop and
> say so rather than creating the file yourself — creating it here would
> duplicate the guard and the two copies would diverge.

## 1. Problem (verified on main @ 30dfa33)

`.releaserc.json`'s custom `releaseRules` make a **major** release
unreachable. Every breaking change this project has shipped was versioned
as minor or patch.

Measured against the real config (semantic-release 25.0.9,
`@semantic-release/commit-analyzer` 13.0.1):

| Commit | Today | Correct |
|---|---|---|
| `feat(api)!: x` + `BREAKING CHANGE:` footer | `minor` | `major` |
| `feat(api): x` + `BREAKING CHANGE:` footer | `minor` | `major` |
| `fix(api): x` + `BREAKING CHANGE:` footer | `patch` | `major` |
| the same `feat!` commit under **default** rules | `major` | `major` |

The last row is the control: the commit shape is well-formed and
semantic-release detects breaking changes correctly on its own. The custom
configuration is what suppresses it.

### Root cause

`@semantic-release/commit-analyzer` evaluates the configured
`releaseRules` **before** the built-in defaults and stops at the first
match. `.releaserc.json` currently declares:

```json
"releaseRules": [
  { "type": "feat", "release": "minor" },
  { "type": "fix",  "release": "patch" },
  { "type": "perf", "release": false },
  { "type": "revert", "release": false }
]
```

A `feat!` commit matches `{ "type": "feat" }` first and resolves to
`minor`, so the default `{ "breaking": true, "release": "major" }` rule is
never reached. Because the config declares custom rules, it takes on the
obligation of restating the breaking rule — and does not.

The consequence is silent: nothing errors, no warning is logged, the
release simply carries the wrong version. Consumers reading semver get no
signal that an interface changed.

## 2. Approach

Prepend an explicit breaking-change rule so it is matched before the
type-based rules.

**File:** `.releaserc.json`, inside the `@semantic-release/commit-analyzer`
plugin options.

```json
"releaseRules": [
  { "breaking": true, "release": "major" },
  { "type": "feat", "release": "minor" },
  { "type": "fix",  "release": "patch" },
  { "type": "perf", "release": false },
  { "type": "revert", "release": false }
]
```

**Order is the entire fix.** The new entry must be **first**. Appending it
after `{ "type": "feat" }` changes nothing, because `feat` still matches
first — an append would look correct in review and leave the bug in place.

**Verified outcome** (run against the real config during design):

| Commit | Result |
|---|---|
| `feat(api)!: x` + `BREAKING CHANGE:` | `major` |
| `fix(api)!: x` + `BREAKING CHANGE:` | `major` |
| `feat: x` | `minor` |
| `fix: x` | `patch` |
| `perf: x` | `null` (suppressed) |
| `revert: x` | `null` (suppressed) |

All four existing rules keep their current behavior; only breaking commits
change outcome. No other file needs to change for the fix itself.

### Why not the alternatives

- **Delete `releaseRules` entirely** and rely on defaults. The defaults do
  handle breaking changes correctly, but they also release on `perf` and
  `revert`, which this project deliberately suppresses. Removing the block
  would fix one bug and introduce unwanted releases.
- **Set `presetConfig` instead.** `presetConfig` tunes how the preset
  parses and renders commits; it does not decide release type.
  `releaseRules` is the correct lever.

## 3. Testing

Add the two breaking-change rows to the version-bump matrix in
`scripts/check-release-config.mjs` (created by the guard PR — see the
prerequisite note above). The guard's matrix deliberately omits these rows
so that CI stays green while this issue is open; adding them here is what
closes that gap.

**Atomicity requirement.** The `.releaserc.json` edit and the two new
matrix rows must land in the **same commit**. Splitting them across
commits leaves one commit where CI asserts behavior the config does not
yet produce (red CI) or where the config claims behavior nothing asserts
(silent regression risk).

Rows to add:

| Commit fixture | Expected `analyzeCommits` result |
|---|---|
| `feat(api)!: drop legacy path` + `BREAKING CHANGE:` footer | `major` |
| `fix(api)!: drop legacy path` + `BREAKING CHANGE:` footer | `major` |

Keep the existing four rows (`feat` → `minor`, `fix` → `patch`, `perf` →
`null`, `revert` → `null`) unchanged; this change must not alter them.

**Sabotage check:** move `{ "breaking": true, "release": "major" }` from
first position to last in `releaseRules` → both new rows fail with `minor`
/ `patch`. This specifically proves the test is sensitive to *ordering*,
not merely to the rule's presence — the failure mode the fix exists to
prevent.

**Gate:** `node scripts/check-release-config.mjs` exits 0.

## 4. Out of scope

- **The guard itself** — script, CI job, and Dependabot grouping all belong
  to `2026-09-03-release-toolchain-guard-design.md`. This change only adds
  two assertions to a matrix that already exists by then.
- **Retroactively correcting past version numbers.** Releases already
  published under the wrong version stay as they are; rewriting released
  tags would break consumers that pinned them. This fix is forward-only.
- **Auditing which historical releases should have been major.** Possibly
  useful for a changelog note, but it does not change any code and is not
  required to close #160.
- **`perf` and `revert` suppression.** Deliberate existing behavior,
  preserved exactly.
