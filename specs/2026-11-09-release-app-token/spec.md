# Release pushes through the `main` ruleset with a GitHub App token

- **Date:** 2026-11-09
- **Status:** Implemented. End-to-end proof is the first release after merge
  (`3.11.0`), which no pre-merge check can exercise.
- **Branch:** `ci/release-app-token`
- **Priority:** High. Every release is blocked until this lands.

## Problem

The Release run for `433ebbd4` failed in `@semantic-release/git`'s prepare
step with `GH006: Protected branch update failed`, `6 of 6 required status
checks are expected`. `main` requires six checks, and the release commit is
created inside the job, so it cannot have passed them. `GITHUB_TOKEN` cannot
bypass that requirement. No tag was created, and `3.11.0` was not released.

## Already applied in GitHub settings (2026-11-09)

- GitHub App `curated-thoughts-release`, installed on this repository only,
  with Contents, Issues and Pull requests write.
- Actions secret `RELEASE_APP_PRIVATE_KEY`.
- Ruleset `main protection` (active, default branch): restrict deletions,
  block force pushes, and the same six required checks, not strict. Bypass
  list: the App and the Repository admin role, the latter preserving the
  admin override the classic rule allowed.
- The classic branch protection rule on `main` was removed.

## Change

In `.github/workflows/release.yml`:

1. Mint an App token with `actions/create-github-app-token`, pinned to the
   v3.3.0 commit, and give it to semantic-release as `GITHUB_TOKEN`.
2. Mint it after `pnpm install`, so dependency install scripts never run with
   a credential that can bypass the ruleset.
3. Check out with `persist-credentials: false`, so a persisted `GITHUB_TOKEN`
   header cannot override the App token on push. Read operations stay
   unauthenticated, which requires this repository to remain public.

## Decisions

- **`dispatch-build` stays and keeps `GITHUB_TOKEN`.** The App-token tag push
  may now fire `build.yml`, but the tag points at a `[skip ci]` commit, so the
  push trigger cannot be relied on. If both start, the `build-${{ github.ref }}`
  concurrency group cancels the older run and one build completes.
- **The Client ID is a literal in the workflow.** It is a public identifier,
  and the action has deprecated `app-id` in its favor. `RELEASE_APP_ID` is
  therefore unused and can be deleted.

## Verification

- Pre-merge: workflow YAML parses, and required CI passes.
- Post-merge: the Release run pushes `chore(release): 3.11.0 [skip ci]` and tag
  `v3.11.0`, exactly one Build run completes, and the GitHub Release has its
  assets.

## Out of scope

Dependabot cargo directories, tracked separately in the same triage.
