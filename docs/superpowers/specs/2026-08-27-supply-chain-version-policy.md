# Supply-Chain Version Policy: exact pins + 14-day release-age gate

**Date:** 2026-08-27
**Status:** ON HOLD — awaiting external architect review (Kurt, 2026-08-27)
**Author:** Hermes Agent (Tessera) at Kurt's direction
**External guidance:** Senior architect (multi-national audit firm), relayed by Kurt:
> "We've encountered two such attacks and our current guidance is one of two options.
> 1: pin your versions. 2: only accept dependencies older than 2 weeks."

## Problem

Curated Thoughts declares 37 of 39 direct dependencies on flexible semver ranges
(`^`/`~`); only `react`/`react-dom` are exact. `pnpm-lock.yaml` + CI
`--frozen-lockfile` pin installs, but every lockfile-regen moment (Dependabot PRs,
manual `pnpm update`, dependency adds) re-resolves those ranges against the
registry. The 2026 npm attacks all landed at exactly that moment:

- **axios (2026-03-31):** poisoned 1.14.1/0.30.4 lived ~3 h; `^1.x` consumers who
  regenerated in-window pulled a RAT (`GHSA-fw8c-xr5c-95f9`).
- **keyv/cacheable (2026-08-04):** ~444 packages / ~2,236 malicious versions;
  patch-bumped releases rode `^`/`~` ranges into lockfiles.
- **TanStack (2026-05-11, CVE-2026-45321):** 84 versions in 6 minutes, published
  via the legitimate trusted-publisher binding — provenance checks passed.

Root cause: `^`/`~` are standing authorizations for future, unreviewed code; the
window between "malicious version published" and "community detects it" is
typically hours-to-days, and we currently have no time-based control.

## Proposed change (both of the architect's options, implemented together)

### Control 1 — 14-day release-age gate (architect's option 2)

New file `pnpm-workspace.yaml`:

```yaml
minimumReleaseAge: 20160          # 14 days in minutes
minimumReleaseAgeExclude:
  - '@equationalapplications/*'   # first-party: published and consumed same-day
```

**Verified on this machine under the repo's pinned pnpm 10.33.2** (test session,
2026-08-27):

| Scenario | Result |
|---|---|
| `pnpm add @mantine/core@9.5.2` (published 5 days ago) | **Blocked**: `ERR_PNPM_NO_MATURE_MATCHING_VERSION` |
| `pnpm add @equationalapplications/core-llm-wiki@6.0.1` (published 2 days ago) | **Allowed** via scope exclusion |
| `pnpm install --frozen-lockfile` with lockfile pinning a young version | **Unaffected** — gate is not evaluated on frozen installs |

The frozen-lockfile result means **CI is untouched** (all 5 CI install sites use
`--frozen-lockfile`); the gate only governs regen — precisely the attack surface.
Note: pnpm 10.x does not default this setting (pnpm 11 defaults to 1 day), so it
must be set explicitly. Verified pnpm docs: exclusion works by name/glob, and
per-version pins are supported (`pkg@x.y.z`) since 10.19.

### Control 2 — Dependabot cooldown, aligned to 14 days

`.github/dependabot.yml`: add to all four ecosystem entries (npm, cargo ×2,
github-actions):

```yaml
    cooldown:
      default-days: 14
```

GitHub's default cooldown (since 2026-07-14) is 3 days and applies to version
updates only; security updates are never delayed. This aligns Dependabot's PR
generation with the same 14-day window so no automated path can propose a
freshly-published version. (Verified: GitHub Docs, Dependabot options reference.)

### Control 3 — exact-pin direct dependencies (architect's option 1)

Rewrite every `^`/`~` specifier in `package.json` `dependencies` and
`devDependencies` to the exact version currently resolved in `pnpm-lock.yaml`
(no version changes — this is a manifest-only tightening). Rationale beyond the
letter of the guidance: exact pins move every future version bump into the
reviewable `package.json` diff, instead of burying it in a 6.7k-line lockfile
diff where a poisoned transitive bump is easy to miss.

**Scope decision needed from Kurt (see Decisions):** all 39 deps vs the 15
production deps only.

### Control 4 — exact-pin the advisory-driven `pnpm.overrides`

The six existing overrides (`undici@6/7`, `js-yaml`, `postcss`,
`brace-expansion@1/@5`) currently use `>=fixed <next-major` ranges — themselves
flexible. Pin each to the exact version the lockfile resolves today (e.g.
`undici@6: 6.28.0`), so advisory responses can't silently drift at regen.

## Files touched

1. `pnpm-workspace.yaml` (new) — gate + first-party exclusion
2. `.github/dependabot.yml` — `cooldown.default-days: 14` ×4 entries
3. `package.json` — exact pins for direct deps (scope per Decision 1); overrides
   exact-pinned (Control 4)
4. `pnpm-lock.yaml` — regenerated; **must be diff-clean for versions** (only
   specifier/comment churn, no version changes) — this is the acceptance proof
   that pinning changed nothing at install time
5. `docs/superpowers/specs/2026-08-27-supply-chain-version-policy.md` (this file)

## Test plan

1. `pnpm install --frozen-lockfile` still succeeds locally (lockfile compatible
   with tightened manifest — specifiers must match resolved versions).
2. `git diff` on `pnpm-lock.yaml` shows **zero version changes** — only
   specifier rewrites.
3. Negative test of the gate on the branch: attempt `pnpm add` of a version
   published <14 days ago → expect `ERR_PNPM_NO_MATURE_MATCHING_VERSION`.
4. CI green on the PR (frontend job exercises install + build + vitest).
5. Dependabot config validity: next scheduled run opens no immediate-regen PRs
   (cooldown applies going forward).

## Risks / trade-offs

- **Maintenance**: exact pins mean every update edits `package.json`. Dependabot
  already automates this (it updates manifest + lockfile together); net burden
  is one extra reviewable line per bump.
- **First-party cadence**: `@equationalapplications/*` is excluded from the gate;
  those packages are published from `expo-llm-wiki` and consumed same-day.
  Exact pins (Control 3) still apply to them — first-party version bumps become
  explicit manifest edits, which is correct: they're the most-trusted path and
  the one where we want deliberate motion.
- **Slower security fixes**: the gate delays *all* new versions 14 days,
  including patched ones. Mitigations: security updates via Dependabot are
  exempt from cooldown; a human can always bump explicitly (write the exact
  version into `package.json` — the pin IS the override of the gate), and the
  `minimumReleaseAgeExclude` list can admit a specific `pkg@version` for
  emergency patching.
- **Lockfile-regen edge**: `pnpm install --fix-lockfile` has a known bug
  (pnpm#10361) bypassing the gate; not in our CI paths, noted for awareness.

## Decisions

1. **Control 3 scope — DECIDED (Kurt, 2026-08-27): (a) all 39 direct deps** exact-pinned
   (prod + dev). Dev deps executed the axios/keyv postinstall vectors too.
2. **Control 4 (overrides exact-pinned):** PROPOSED yes — awaiting architect review.
3. **Gate length:** 14 days per the architect's guidance (20160 min) — awaiting
   architect confirmation (7-day alternative noted in Risks).

## Relation to prior analysis

This spec supersedes the "no meaningful risk / no action needed" verdict of the
earlier investigation (2026-08-27 Discord thread). The technical facts stand
(frozen CI is safe; attacks bite at regen), but the risk verdict is overruled by
Kurt's risk-acceptance decision and the external auditor guidance: mitigate.
