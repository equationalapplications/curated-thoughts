# Release-Toolchain Guard (CI smoke test + Dependabot grouping) — Design

**Date:** 2026-09-03
**Status:** Proposed
**Branch:** `spec/release-toolchain-guard`
**Priority:** P2 (release-pipeline reliability; prevents a repeat of PR #155)

## 1. Problem (verified on main @ 30dfa33)

Dependabot PR #155 (`conventional-changelog-conventionalcommits` 9.3.1 →
10.4.0) passed **every** CI check — `rust-ubuntu`, `rust-macos`,
`frontend`, both CodeQL analyses, `CodeRabbit` — and was `MERGEABLE` /
`CLEAN`. It also breaks the release.

Running the repo's real `.releaserc.json` plugin chain with v10 installed
throws before producing any notes:

```
Error: Missing helper: "conventional-changelog-conventionalcommits requires
conventional-changelog-writer@9 or newer (conventional-changelog@8 or newer).
Your changelog tooling loaded an older writer which cannot render this preset.
Update the tooling or use an older major version of the preset."
```

The repo resolves `conventional-changelog-writer@8.4.0`. Nothing in CI
noticed.

### Why CI cannot catch this today

`release.yml` triggers on `workflow_run` of **CI**, `branches: [main]`,
`types: [completed]` — i.e. **after** merge. No PR check executes any part
of the release pipeline, so a broken release config is only discovered when
it runs for real, on `main`, during an auto-release. `@semantic-release/git`
commits `CHANGELOG.md` and the version files, so a mid-pipeline failure
lands with the release half-applied.

### Why Dependabot split the family in the first place

`conventional-changelog-writer` is a **transitive** dependency — pulled in
by `@semantic-release/release-notes-generator@14.1.1` (`^8.0.0`) and
`@semantic-release/commit-analyzer@13.0.1`, not declared in `package.json`.
Dependabot only bumps direct dependencies, and the `minor-and-patch` group
in `.github/dependabot.yml` covers `minor`/`patch` only, so **majors are
raised individually**. That let the preset move to v10 while the writer
that renders it stayed on v8. No configuration tied them together.

This is the same class of failure as the fastembed bump closed in #138:
a dependency's real constraint lives outside what CI exercises.

## 2. Approach

Three pieces forming two independent defences: a smoke-test script (Layer
1) wired into CI (Layer 2), plus Dependabot grouping (Layer 3). Grouping
prevents *this* failure; the smoke test catches the general class. Either
alone would have missed something — see "Why not the alternatives".

### Layer 1 — `scripts/check-release-config.mjs`

A standalone Node script that loads the **real** `.releaserc.json` (never a
copy — a fixture would drift from the config it is meant to protect),
extracts each plugin's options, and drives the two pure plugins over
synthetic commits:

```js
const rc  = JSON.parse(readFileSync('.releaserc.json', 'utf8'));
const cfgOf = (name) => {
  const p = rc.plugins.find((p) => Array.isArray(p) && p[0] === name);
  return p ? p[1] : {};
};
const type  = await analyzeCommits(cfgOf('@semantic-release/commit-analyzer'), ctx);
const notes = await generateNotes(cfgOf('@semantic-release/release-notes-generator'), ctx);
```

Only `commit-analyzer` and `release-notes-generator` are exercised. They
are pure functions of (config, commits) — no network, no token, no git
writes. `changelog`, `npm`, `exec`, `git` and `github` are deliberately
excluded: they perform side effects and need credentials, which is what
makes a full `semantic-release --dry-run` unattractive here (it also fails
`ERELEASEBRANCHES` on PR head refs, since the branch must exist on the
remote).

**Assertions.**

*Notes rendering* — the check that catches #155:

- `generateNotes` resolves without throwing
- notes are non-empty
- a `feat` subject, a `fix` subject and a breaking-change section all appear
- commit links contain the repository URL
- no `[object Object]` and no bare `undefined` leaked into the output

*Version-bump matrix* — `analyzeCommits` returns, for the real config:

| Commit | Expected |
|---|---|
| `feat: …` | `minor` |
| `fix: …` | `patch` |
| `perf: …` | `null` (suppressed) |
| `revert: …` | `null` (suppressed) |

Exit non-zero on any failure, printing a `PASS`/`FAIL` line per assertion.

**Breaking-change rows are deliberately absent.** They belong to issue
#160 — see §4.

### Layer 2 — `release-config` job in `ci.yml`

```yaml
release-config:
  runs-on: ubuntu-latest
  steps:
    - uses: actions/checkout@…
    - uses: ./.github/actions/setup-node-pnpm
    - run: pnpm install --frozen-lockfile
    - run: node scripts/check-release-config.mjs
```

Node-only, so it reuses the existing `setup-node-pnpm` composite action
and needs no Rust toolchain, no system packages, and no secrets. It does
not touch the `ubuntu-22.04` pin that `rust-ubuntu` carries for glibc
reasons (see `.github/dependabot.yml`, fastembed note) — `ubuntu-latest`
is correct here.

Runtime is dominated by `pnpm install`; the script itself is
sub-second.

### Layer 3 — `release-toolchain` group in `.github/dependabot.yml`

```yaml
groups:
  release-toolchain:
    patterns:
      - "semantic-release"
      - "@semantic-release/*"
      - "conventional-changelog-*"
    update-types: ["major", "minor", "patch"]
```

Added to the existing `npm` ecosystem entry, alongside `minor-and-patch`.
Including `major` is the point: it is the major bumps that must move
together, and it is exactly what `minor-and-patch` fails to cover.

A grouped PR that still breaks the pipeline is now caught by Layer 2
before merge, rather than after.

### Why not the alternatives

- **Full `semantic-release --dry-run` in CI.** Most faithful, but needs a
  `GITHUB_TOKEN`, is roughly 2 minutes, and fails the branch-existence
  check on PR head refs (`ERELEASEBRANCHES`, observed while investigating
  #155). The failure mode being guarded against is config/preset
  incompatibility, which the pure plugins reproduce exactly and far more
  cheaply.
- **Grouping alone, no CI job.** Prevents only the split-bump. Any other
  release-config regression — a bad `presetConfig`, a plugin option
  removed in a major, a hand edit to `.releaserc.json` — still reaches
  `main` unguarded. Notably it would never have surfaced #160, which no
  dependency bump caused.
- **Pinning `conventional-changelog-writer` directly.** Adds a direct
  dependency the project does not otherwise use, and silently fights the
  plugins' own ranges on the next bump. Grouping expresses the real
  constraint.

## 3. Testing

The script *is* the test; it runs in CI on every PR. Its own correctness is
established by sabotage rather than by unit tests over it.

**Sabotage checks (all run against the real repo):**

| Sabotage | Expected failure |
|---|---|
| Install `conventional-changelog-conventionalcommits@10.4.0` (i.e. merge #155) | notes assertions fail — `generateNotes` throws the writer-version guard |
| Remove `{ "type": "perf", "release": false }` from `.releaserc.json` | matrix assertion fails — `perf` yields `patch`, not `null` |
| Point `preset` at a nonexistent name | script fails on preset load |

The first row is the acceptance criterion: **this guard must fail on PR
#155's dependency set.** Verified manually during design — v9 renders notes
correctly, v10 throws and produces nothing.

**Gate:** `node scripts/check-release-config.mjs` exits 0 on `main` as it
stands today.

## 4. Out of scope

- **Issue #160 — `releaseRules` shadow the default breaking-change rule.**
  Configured `releaseRules` are consulted instead of the built-in defaults
  whenever any of them matches; without an explicit `breaking` rule, a
  `feat!` commit matches only `{ "type": "feat" }` and releases as minor
  (`fix!` → patch; the same commits yield `major` under default rules).
  The analyzer does take the highest release type among all matching
  configured rules — so the breaking rule works from any position — but
  its *presence* is what restores `major`. Found while building this
  guard's matrix. Split out because it **changes version-numbering
  behavior for every future release** and deserves its own review and
  release note.

  **Sequencing contract:** this spec's matrix omits breaking-change rows on
  purpose, so CI stays green while #160 is open. The #160 PR must add
  `{ "breaking": true, "release": "major" }` to `releaseRules` **and** the
  two matching matrix rows (`feat!` → `major`, `fix!` → `major`) in the
  **same commit**, so the rule and its assertion land atomically and
  neither can regress alone.

- **PR #155.** Close it, do not merge — it breaks the release for the
  reason recorded in §1. It can be reopened once the family bumps as a
  group with a writer ≥ 9.
- **PR #156** (`@semantic-release/git` 10.0.1 → 11.0.1) — verified safe and
  unaffected by this work: peer `semantic-release >=20.1.0` satisfied by
  25.0.9, plugin loads, `verifyConditions` passes with all 7 configured
  assets, notes output byte-identical to baseline. Worth noting as a
  standing risk rather than a fix: v11 raises `engines` to
  `^22.22.2 || >=24.15`, and the workflows' `node-version: 22` currently
  floats to 22.23.2 — satisfied, but by one minor.
- **Validating the side-effecting plugins** (`changelog`, `npm`, `exec`,
  `git`, `github`). They need credentials and mutate state; covering them
  means the full dry-run rejected above.
- **The `ubuntu-22.04` pin** on `rust-ubuntu` — unrelated, and load-bearing
  for the apt-mirror workaround and glibc floor.
