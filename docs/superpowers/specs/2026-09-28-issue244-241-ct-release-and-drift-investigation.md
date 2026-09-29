# Investigation — issues #244 (ct release asset) and #241 (delete self-cleaning docs + drift sweep)

**Date:** 2026-09-28
**Status:** Investigation (Step 0 of the delivery flow)
**Evidence tags:** [V] = controller-verified; [C] = child-reported

## Issue #244 — ship the `ct` headless CLI as a release asset

### Current state [V unless noted]

- `ct` is registered at `tools/Cargo.toml:81-82` (`[[bin]] name = "ct"`).
  `cargo build --release -p curated-thoughts-tools --bin ct` **works from the
  workspace root** — child ran it [C], binary 51 MB (it links `src-tauri` as a
  path dep). README.md:224 documents exactly this command.
- Release chain: `release.yml` (semantic-release on main CI success) →
  `dispatch-build` job runs `gh workflow run build.yml --ref "$TAG"`; human tag
  pushes also trigger `build.yml` directly. Either way the build runs **at the
  tag ref**, so `github.ref_name` is `vX.Y.Z`. [C, consistent with build.yml:12-15]
- Asset attachment today: `tauri-apps/tauri-action` (build.yml:147, pinned
  `1deb371b`) creates the release and uploads installers only. No `gh release
  upload` exists yet. `contents: write` already granted (build.yml:21).
- The `.deb`/AppImage do NOT bundle `ct` (verified by the issue reporter by
  extracting the v2.20.1 AppImage).
- Stale-binary incident (2026-09-27, v2.20.1): a Sep 26 local `ct ingest`
  bypassed the new `records/` exclusion, ingested 150 never-ingest files,
  required a surgical SQL purge. App and sidecar from the same .deb were correct.

### Root cause [V]

There is no release-sanctioned channel for the `ct` binary at all, so anyone
who needs it headless builds locally — and a local build silently outlives its
safety gates when the app is upgraded underneath it.

### Proposed fix direction (revised per Opus c1 — BLOCKER + M1/M2/M3 folded in)

1. New step "Build ct CLI" in `.github/workflows/build.yml`, placed after the
   MCP sidecar steps (sidecar build step at :75, `if: matrix.platform !=
   'macos-latest'`; macOS universal build at :121). Command:
   `cargo build --release -p curated-thoughts-tools --bin ct` (from root;
   NOT `--manifest-path src-tauri/Cargo.toml`, which does not build ct).
   Package as `ct_<version>_<os>_<arch>.tar.gz` (binary + short README note).
2. **Version derivation (Opus M2 — supersedes the c0 fallback sketch):**
   derive `<version>` from the stamped `src-tauri/tauri.conf.json` (jq) in ALL
   cases. `GITHUB_REF_NAME` is NOT empty on non-tag dispatches — it is the
   branch name, and branch names containing `/` would corrupt the filename.
   On tag refs (`github.ref_type == 'tag'`), additionally assert the derived
   version equals `${GITHUB_REF_NAME#v}` and fail the step on mismatch.
3. **BLOCKER fix — tauri-action must be tag-gated (Opus c1):** tauri-action
   (`build.yml:147-157`) runs on EVERY trigger with `tagName: v__VERSION__`
   and no `if:` — a PR-branch `workflow_dispatch` would upload unreleased
   installers onto the LIVE release (e.g. v2.20.1). The PR's verification
   path REQUIRES gating tauri-action with `if: github.ref_type == 'tag'`
   (or a release-free verification job). This gate is part of THIS PR — the
   ct-asset work cannot be CI-verified without it, and the current
   unprotected dispatch trigger is a latent incident regardless.
4. New post-tauri-action step: `gh release upload "$GITHUB_REF_NAME" ct_*.*
   --clobber`, `if: github.ref_type == 'tag'`, **with `env: GH_TOKEN:
   ${{ secrets.GITHUB_TOKEN }}`** (Opus M3: actions checkout runs with
   `persist-credentials: false` and `GITHUB_TOKEN` is only present on the
   tauri-action step's env today — without the explicit env the upload fails
   with no auth).
5. Smoke test (Opus M1): `ct status --json` FAILS in CI — the seeded
   `~/.brain` on the runner lacks a `documents` table (no migrated DB). Use
   `./target/release/ct --help` as the smoke assertion (optionally add
   `#[command(version)]` to the clap app); a DB-backed smoke would need a
   fully migrated fixture DB — not worth it here.
6. macOS: universal pattern (both targets + `lipo`), matching the sidecar.
7. Packaging detail (Opus m2 + c2 nits): Windows asset is `.zip` — Git Bash
   has no `zip` binary, so use `pwsh Compress-Archive` (or `7z a`); linux/macOS
   keep `.tar.gz`. CI cost note: the workspace-root build recompiles the
   ~51MB src-tauri lib; rust-cache absorbs most of it.
8. **Runtime stale-CLI warning: split to a follow-up issue.** Not cheaply
   feasible: the tools crate is pinned `0.1.0`, never version-stamped by
   semantic-release (`update-versions.cjs` touches only src-tauri/package
   manifests), `ct` has zero version identity today, and no installed-app-
   version source exists in brain config/DB for it to compare against
   (`schema_version` only). Shipping release assets largely removes the staleness
   vector anyway. [C, verified reasoning]

### Verification

**#244:** Local: build + `ct --help` (on the macOS leg, run the smoke against
the lipo'd universal binary — `--target <triple>` builds land under
`target/<triple>/release/`, so `./target/release/ct` does not exist there).
CI: workflow_dispatch on the PR branch exercises the build+package steps with
the tauri-action gate in place (no release touching — that's the BLOCKER
fix); the upload path itself is proven on the next real release tag.

**#241 (aligned with the item-2 test requirements — rewritten c2 to match the
spec's drift semantics):** the classify/apply split lives in
`src-tauri/src/reconcile.rs`, so its tests stay THERE (all existing
reconcile tests green against the split, incl.
`vanished_file_is_deleted_and_chunks_cascade`); `tools/` carries only the CLI
tests: clean → 0, pending plan (gone/repoint/excluded-delete each) → 3,
ambiguous-only → 0 with warning, classify guard trip (unmounted vault) → 4.
`ct drift` reports reconcile's FULL plan (gone + repointed +
excluded-dir deletes); ambiguous rows are warnings, not drift; the
files-without-rows direction (never-ingested files) stays OUT — a hand-rolled
reverse diff would evade the empty-walk guard at `reconcile.rs:69`.

## Issue #241 — deletion-is-self-cleaning docs + drift sweep

### Current state [V]

Self-cleaning is real and multi-layered:

- **Desktop startup reconcile:** `lib.rs:1508-1565` purges rows whose backing
  file is gone (`enqueue_vault_event(Remove)`, comment at :1518) + `purge_excluded_rows`
  (:1035-1072). NOTE (superseded by the spec's M3 ruling): this path never
  repoints — it deletes rows the `ct ingest` reconcile path would repoint,
  so "self-cleaning" behaves DIFFERENTLY on desktop startup vs ingest.
- **Watcher Deleted event:** `lib.rs:1713-1715` → heal scheduler (:1630) →
  removal via `db::queries::delete_document` (`queries.rs:155`, cascades chunks;
  embeddings ride the cascade, `delete_document_chunks` at `queries.rs:50`).
- **Headless:** `ct ingest` runs `reconcile_vault` first
  (`tools/src/cmds.rs:193`, prints `reconcile: removed {p} (file is gone)` at
  :199-201). `reconcile_vault` at `src-tauri/src/reconcile.rs:50` has cascade
  proof tests (`reconcile.rs:349 vanished_file_is_deleted_and_chunks_cascade`).

Docs home: README.md — CLI section "### The `ct` Headless CLI" (~:213-246) and
the episodic-memory/watcher paragraph (:24). No in-app FAQ dir exists.

**No existing drift detector** beyond reconcile — `reconcile.rs` *is* the
prune mechanism; grep for orphan/prune/drift finds nothing else (`embed_sweep.rs`
is the embedding backlog, unrelated). The issue's "drift-detector sweep" is
genuinely additive, and thin.

### Proposed fix direction

1. **Docs (item 1 of revised scope, zero-risk):** README `ct` section — deleting
   a superseded note outright is safe and recommended; watcher/startup
   reconcile/`ct ingest` remove doc row + chunks + embeddings automatically;
   lineage lives in git history and session records. Point to `ct drift` as
   the periodic CHECK and `ct ingest --yes` as the REPAIR (Opus m3).
2. **`ct drift` headless subcommand (item 2 — reshaped per Opus c1 M4 and
   c2 spec M1/M2/M3; THIS SECTION IS SUPERSEDED BY THE SPEC where they
   differ):** a hand-rolled symmetric diff in the tool would IGNORE
   reconcile's hard-won guards — `reconcile.rs:69` treats an empty walk as
   an unmounted/misconfigured vault (a naive diff would report the whole DB
   as drift and exit nonzero forever), and the ambiguous/unrelativizable-row
   handling (:35-36, :104) exists precisely to avoid false deletes. Reshape:
   **split `reconcile_vault` (`src-tauri/src/reconcile.rs:50`) into classify
   + apply**; `ct drift` calls the CLASSIFY half as a dry run. Final scope
   per the spec: classify returns the FULL plan (gone + repointed +
   excluded-dir deletes — the Sep 27 `records/` incident was an
   excluded-delete class, so "gone-only" would have missed it); ambiguous
   rows are WARNINGS exiting 0 (ingest never clears them); guard semantics
   shared with `ct ingest` by construction (desktop startup is explicitly
   NOT in that claim — it deletes where ingest repoints).
   - New `Cmd::Drift { json: bool }` variant in `tools/src/bin/ct.rs`
     (~:14-140), handler read-only (open ro connection; NO repair — repair
     remains `ct ingest` per the spec's M3 ruling; the desktop startup path
     is NOT an equivalent repair — it deletes where ingest repoints).
   - Exit codes (Opus m1): drift found → exit 3 (exit 2 already means
     `EXIT_NO_RESULTS` in this CLI); clean → 0; classify guard trip
     (unmounted vault etc.) → exit 4 with a clear message, NOT "drift".
   - Refactor is test-covered: existing reconcile tests
     (`reconcile.rs:349 vanished_file_is_deleted_and_chunks_cascade` and
     neighbors) must stay green against the split, plus a drift-mode test
     mirroring reconcile's fixture style (tempdir + seeded sqlite) asserting
     report content and exit codes across the full matrix (clean → 0, pending
     plan gone/repoint/excluded-delete → 3, ambiguous-only → 0 with warning,
     guard trip → 4 — see the Verification section above).
3. Item 3 (wisdom-fact lifecycle on delete) stays with PR #242 territory per
   the issue; out of scope here.

### Verification

(Superseded in detail by the spec's Testing section — reproduced here with the
final exit codes so this doc no longer contradicts it:) classify tests in
`src-tauri/src/reconcile.rs` (existing suite green against the split); CLI
tests in `tools/` — clean → 0, pending plan → 3, ambiguous-only → 0 with
warning, guard trip → 4 (tempdir + seeded sqlite, reconcile fixture style).
Docs verified by reading rendered README.

## Open questions

- **OQ1 (#244 matrix scope):** include macOS universal `ct` in v1, or
  linux-x64 (+windows) first with macOS gated like the sidecar? Recommend
  include — lipo pattern is proven in the same file.
- **OQ2 (#241 split — revised c2):** the classify/apply split of
  `reconcile_vault` makes this materially bigger than the original "~100
  lines" estimate (it touches `src-tauri/src/reconcile.rs`, its test suite,
  the clap enum, and the tools handler). One PR is still the recommendation —
  the docs piece and the drift piece serve one issue and the drift shape
  depends on the split — but the plan should expect the drift half to
  dominate the diff.

## What was NOT checked

- Windows tarball layout (`ct.exe` naming) beyond noting bash shell is already
  uniform across legs; plan pins it.
- Whether GitHub's release page ordering/notes need the ct asset mentioned in
  release notes (semantic-release owns notes; asset appears alongside installers).
