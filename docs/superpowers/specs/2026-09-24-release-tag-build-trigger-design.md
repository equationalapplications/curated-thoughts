# Auto-trigger the Build workflow on semantic-release tags

- **Date:** 2026-09-24
- **Issue:** equationalapplications/curated-thoughts#207
- **Status:** Draft (spec stage)
- **Risk tier:** Low–moderate (release pipeline wiring; no app code, no security surface)

## Problem

Every semantic-release tag produces a GitHub Release with **zero assets and no
Build run**, because `release.yml` pushes the `v*` tag with the built-in
`GITHUB_TOKEN` and GitHub's anti-recursion guard deliberately drops
`GITHUB_TOKEN`-created `push` events — so `build.yml`'s
`on: push: tags: v*` trigger never fires. Assets only exist when someone
notices and manually runs `gh workflow run build.yml --ref <tag>`.

**Confirmed still live as of this spec (fresh verification, 2026-09-24):**

| Tag | Assets | Build run |
|---|---|---|
| v2.14.0 (2026-09-24 11:38Z) | **0** | none |
| v2.13.0 (2026-09-23) | 7 | manual `workflow_dispatch` |
| v2.12.1 (2026-09-16) | 7 | manual `workflow_dispatch` |

## Root cause (verified)

- `release.yml` sets `GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}` for
  semantic-release (release.yml:75), which pushes the release commit and tag.
- GitHub docs ("Triggering a workflow"): events triggered by `GITHUB_TOKEN`
  **will not create a new workflow run**, *with the exception of
  `workflow_dispatch` and `repository_dispatch`, which always create runs*
  (docs.github.com → Triggering a workflow; changelog 2022-09-08).
- Therefore the tag `push` event is dropped and `build.yml`
  (.github/workflows/build.yml:4-7) never starts for release tags. The tag
  trigger remains valid only for tags pushed by a human credential.

## Decision

**Option 2 from the issue — explicit `workflow_dispatch` chaining — with zero
new secrets.** The Release workflow's final step dispatches `build.yml` at the
tag ref. This works with `GITHUB_TOKEN` alone because `workflow_dispatch` is
explicitly exempt from the anti-recursion guard (docs citation above).
Rejected alternatives: Option 1 (PAT/App secret — new credential to rotate and
scope; unnecessary given the exemption), Option 3 (fold release+build into one
workflow — large rework of a three-OS matrix for no functional gain).

## Proposed changes

### 1. `.releaserc.json` — expose "a release was published" as a step output

Add a `successCmd` to the existing `@semantic-release/exec` config so the
version is recorded **only when a release is actually published**
(semantic-release exits 0 with no publication when there is nothing to
release; the dispatch must not fire then):

```json
[
  "@semantic-release/exec",
  {
    "prepareCmd": "node scripts/update-versions.cjs ${nextRelease.version}",
    "successCmd": "echo \"version=${nextRelease.version}\" >> \"$GITHUB_OUTPUT\""
  }
]
```

`$GITHUB_OUTPUT` is an ordinary env-var-pointed file inside the step, so
appending from `successCmd` lands the output on the semantic-release step.

### 2. `.github/workflows/release.yml` — dispatch the build after a published release

- Give the "Run semantic-release" step an `id: release`.
- Add `actions: write` to the workflow `permissions` block (required for
  `gh workflow run`; today only contents/issues/pull-requests are granted).
- Append a final step:

```yaml
- name: Trigger Build workflow at the release tag
  if: steps.release.outputs.version != ''
  env:
    GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
    VERSION: ${{ steps.release.outputs.version }}
  run: gh workflow run build.yml --ref "v${VERSION}"
```

The `--ref v<version>` target is exactly the proven manual workaround; the
step's non-zero exit (failed dispatch) fails the release run so the gap can
never silently reappear.

### 3. `.github/workflows/build.yml` — document, don't remove, the tag trigger

Keep `on: push: tags: v*` (it correctly fires for tags pushed by a human
credential) and add a comment: tags pushed with `GITHUB_TOKEN` (i.e. by
semantic-release) never fire this trigger; the automatic path is the
`workflow_dispatch` sent by `release.yml`.

## Acceptance criteria

1. The next semantic-release publication on `main` produces: tag → Release →
   an automatic Build run whose ref is the tag → all 7 platform assets on the
   Release — with **no manual step**.
2. A `main` push that produces **no release** triggers no Build dispatch
   (guarded by `successCmd`-sourced output being empty).
3. A tag pushed manually by Kurt still triggers `build.yml` via the existing
   tag trigger.
4. `actionlint` (or `python3 -c yaml.safe_load`) passes on both modified
   workflows; `.releaserc.json` parses as JSON.

## Post-merge ops (not in the diff)

- One-time backfill: `gh workflow run build.yml --ref v2.14.0` so today's
  asset-less release gets its assets.
- Close #207 via the PR merge; comment the verification evidence on the issue.

## Out of scope

- PAT / GitHub App tokens (Option 1), single-workflow fold (Option 3),
  release-item backfills older than v2.14.0, any change to the CI/CodeQL
  workflows.
