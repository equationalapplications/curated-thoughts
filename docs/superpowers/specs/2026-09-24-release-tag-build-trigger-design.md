# Auto-trigger the Build workflow on semantic-release tags

- **Date:** 2026-09-24
- **Issue:** equationalapplications/curated-thoughts#207
- **Status:** Approved by Kurt 2026-09-24; implemented in PR #227 (Opus r1 approve-with-changes findings folded in; GLM pass approved)
- **Risk tier:** Low — no app code; elevated `actions: write` is confined to a dedicated dispatch job with no code checkout (see §2)

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
| v2.13.0 (2026-09-23) | 7 | manual `workflow_dispatch`, dispatched at the tag ref |
| v2.12.1 (2026-09-16) | 7 | manual `workflow_dispatch`, dispatched at `main` whose head was the release commit |

## Root cause (verified)

- `release.yml` sets `GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}` for
  semantic-release (release.yml:75), which pushes the release commit and tag.
- GitHub docs ("Triggering a workflow"): events triggered by `GITHUB_TOKEN`
  **will not create a new workflow run**, with named exceptions. The exception
  this design relies on, stated exactly: **`workflow_dispatch` events sent
  using `GITHUB_TOKEN` always create workflow runs** (docs.github.com →
  Triggering a workflow; changelog 2022-09-08). `workflow_dispatch` is not
  exempt from the guard *in general* — only when the dispatch is what the
  `GITHUB_TOKEN` sent.
- Therefore the tag `push` event is dropped and `build.yml`'s tag trigger
  (.github/workflows/build.yml:4-6) never starts for release tags. The tag
  trigger remains valid only for tags pushed by a human credential.

## Decision

**Option 2 from the issue — explicit `workflow_dispatch` chaining — with zero
new secrets.** The Release workflow dispatches `build.yml` at the tag ref from
a dedicated job. This works with `GITHUB_TOKEN` alone because
`workflow_dispatch` sent with `GITHUB_TOKEN` is explicitly exempt from the
anti-recursion guard (docs citation above). Rejected alternatives: Option 1
(PAT/App secret — new credential to rotate and scope; unnecessary given the
exemption), Option 3 (fold release + build into one workflow — large rework of
a three-OS matrix for no functional gain).

## Proposed changes

### 1. `.releaserc.json` — expose "a release was published" as step outputs

Add a `successCmd` to the existing `@semantic-release/exec` config so the
release metadata is recorded **only when a release is actually published**
(verified against the installed `@semantic-release/exec@7.1.0`,
index.js:117-120, and semantic-release core: on "no relevant changes" it
returns before the success phase, so `successCmd` never fires):

```json
[
  "@semantic-release/exec",
  {
    "prepareCmd": "node scripts/update-versions.cjs ${nextRelease.version}",
    "successCmd": "echo \"tag=${nextRelease.gitTag}\" >> \"$GITHUB_OUTPUT\""
  }
]
```

`$GITHUB_OUTPUT` is an ordinary env-var-pointed file inside the step, so
appending from `successCmd` lands the outputs on the semantic-release step.
The **tag is taken from `nextRelease.gitTag`** rather than reconstructed as
`v${version}`, so the dispatch keeps working even if `tagFormat` is ever
changed from the default. (Opus flagged the `${nextRelease.X}` vs
`${nextRelease.variables.X}` template spelling as worth double-checking at
implementation time; the GLM probe exercised the installed plugin's template
rendering and found `${nextRelease.gitTag}` renders correctly — the
implementer should still assert the rendered string equals `v<version>` in a
dry run.)

### 2. `.github/workflows/release.yml` — dispatch the build from a dedicated least-privilege job

- Give the "Run semantic-release" step an `id: release`.
- Map a **job output** on the `release` job:
  `outputs: { tag: ${{ steps.release.outputs.tag }} }`.
- Add a **second job** that owns the elevated permission, so the
  semantic-release plugin process never holds `actions: write` (which can
  also cancel/rerun workflows and delete run logs):

```yaml
jobs:
  release:
    # ... existing job, plus:
    outputs:
      tag: ${{ steps.release.outputs.tag }}

  dispatch-build:
    needs: release
    if: ${{ !cancelled() && needs.release.outputs.tag != '' }}
    runs-on: ubuntu-latest
    permissions:
      actions: write
    steps:
      - name: Trigger Build workflow at the release tag
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
          GH_REPO: ${{ github.repository }}
          TAG: ${{ needs.release.outputs.tag }}
        run: gh workflow run build.yml --ref "$TAG"
```

Design notes (from review):

- **`!cancelled()` is load-bearing, not cosmetic.** Plugin success steps run in
  listed order: `exec` (writes the output) *before* `@semantic-release/github`
  (comments on issues/PRs, adds labels). If that later commenting fails,
  semantic-release exits non-zero **after** the tag and Release already exist.
  With a plain `needs.release.result == 'success'` gate the dispatch would be
  skipped and the zero-asset Release would recur — the exact bug we are
  fixing. With `!cancelled() && tag != ''` the dispatch still fires, because
  the output's existence already proves the tag was pushed and the Release
  published (the tag push happens before the success phase; verified in
  semantic-release core, index.js:208-218, and get-config.js default
  `tagFormat: v${version}`). Implementation must verify that a job-level `if`
  with `!cancelled()` actually permits running when `needs.release` fails but
  its outputs still map — if GitHub's semantics are stricter than expected,
  fall back to `if: always() && needs.release.outputs.tag != ''` and re-verify
  (this is the Opus MAJOR; do not silently drop it).
- `GH_REPO` is set explicitly so the step does not depend on a checkout's git
  remote — this job has no checkout at all.
- The two jobs cannot race the `release-main` concurrency group
  (`cancel-in-progress: false` serializes releases). If a manual
  `gh workflow run build.yml` is in flight when the automatic dispatch lands,
  both runs share `build.yml`'s `build-${{ github.ref }}` group with
  `cancel-in-progress: true` (build.yml:11) — one cancels the other, which is
  benign: no duplicate asset uploads, and tauri-action uploads to the same
  existing Release.
- **Residual risk (accepted):** a *successful* dispatch followed by a
  *failed* Build run (e.g. a flaky macOS matrix leg) still yields a zero-asset
  Release while the Release workflow shows green. No workflow wiring can close
  that; the watch point moves to `build.yml` runs. This replaces the earlier
  overstatement that "the gap can never silently reappear."

### 3. `.github/workflows/build.yml` — document, don't remove, the tag trigger

Keep `on: push: tags: v*` (it correctly fires for tags pushed by a human
credential) and add a comment: tags pushed with `GITHUB_TOKEN` (i.e. by
semantic-release) never fire this trigger; the automatic path is the
`workflow_dispatch` sent by `release.yml`'s `dispatch-build` job.

## Acceptance criteria

1. The next semantic-release publication on `main` produces: tag → Release →
   an automatic Build run whose ref is the tag → all 7 platform assets on the
   Release — with **no manual step**.
2. A `main` push that produces **no release** triggers no Build dispatch, via
   **both** no-release paths: (a) the release-guard step skips
   semantic-release entirely (HEAD ≠ origin/main), and (b) semantic-release
   runs and finds nothing to release. In both, `needs.release.outputs.tag` is
   empty and `dispatch-build` skips.
3. A late failure inside semantic-release *after* publication (e.g.
   `@semantic-release/github` commenting fails) still results in the Build
   dispatch firing (the `!cancelled()` path — Opus MAJOR 1).
4. A tag pushed manually by Kurt still triggers `build.yml` via the existing
   tag trigger.
5. Static checks (runnable as written):
   - `python3 -c 'import yaml,sys; yaml.safe_load(open(sys.argv[1]))' .github/workflows/release.yml`
   - `python3 -c 'import yaml,sys; yaml.safe_load(open(sys.argv[1]))' .github/workflows/build.yml`
   - `node -e 'JSON.parse(require("fs").readFileSync(".releaserc.json","utf8"))'`

## Post-merge ops (not in the diff)

- One-time backfill: `gh workflow run build.yml --ref v2.14.0` so today's
  asset-less release gets its assets. **Do not skip this** — v2.14.0 stays
  permanently asset-less otherwise.
- Close #207 via the PR merge; comment the verification evidence on the issue.

## Out of scope

- PAT / GitHub App tokens (Option 1), single-workflow fold (Option 3),
  release backfills older than v2.14.0, any change to the CI/CodeQL workflows.
