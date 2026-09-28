# Ship `ct` as a release asset (#244) + deletion self-cleaning docs & `ct drift` (#241)

**Date:** 2026-09-28
**Status:** Draft
**Branch:** feat/issue-244-ct-release-and-241-drift
**Priority:** High for #244 (real stale-binary incident 2026-09-27); Medium for #241

## Problem

**#244:** There is no release-sanctioned channel for the `ct` headless CLI —
release tags ship only desktop installers, the .deb/AppImage don't bundle
`ct`, so anyone needing it headless builds locally, and a stale local build
silently outlives its safety gates (Sep 27 incident: a pre-`records/`-exclusion
`ct ingest` polluted the DB with 150 never-ingest files; surgical SQL purge
required; the .deb-installed app and sidecar were correct throughout).

**#241:** Deleting superseded vault notes is verified-safe and
self-cleaning (watcher/startup reconcile/`ct ingest` → `reconcile_vault`
remove doc+chunks+embeddings), but that is documented nowhere, and there is
no standalone drift detector as a cheap safety net for watcher hiccups (a
stale trio of rows from an older indexer was found and purged manually in the
Sep 27 audit).

Full investigation (release-chain map, reconcile guard analysis, Opus
verdict history): `2026-09-28-issue244-241-ct-release-and-drift-investigation.md`
(same directory).

## Approach

**#244 — build.yml:**
1. "Build ct CLI" step after the MCP sidecar steps:
   `cargo build --release -p curated-thoughts-tools --bin ct` (from workspace
   root; the src-tauri manifest-path does NOT build ct).
2. Version: derived from stamped `src-tauri/tauri.conf.json` (jq) in ALL
   cases; on tag refs additionally assert it equals `${GITHUB_REF_NAME#v}`
   (GITHUB_REF_NAME is the branch name on dispatches — never use it raw; `/`
   corrupts filenames).
3. Package: `ct_<version>_<os>_<arch>.tar.gz` (linux/macOS) / `.zip`
   (Windows, via `pwsh Compress-Archive` — Git Bash has no `zip`) containing
   the binary + a short README (install path, upgrade note: replace stale
   local builds).
4. **Tag-gate tauri-action** (`if: github.ref_type == 'tag'`) — prerequisite
   of this PR: tauri-action currently runs on every trigger with
   `tagName: v__VERSION__` and no gate, so a PR-branch dispatch would upload
   unreleased installers onto the LIVE release (verified BLOCKER from review).
5. Upload step: `gh release upload "$GITHUB_REF_NAME" ct_*.* --clobber`,
   `if: github.ref_type == 'tag'`, with explicit
   `env: GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}` (checkout runs with
   `persist-credentials: false`).
6. Smoke: `ct --help` on linux (`ct status` needs a migrated DB the runner
   lacks); on the macOS leg, smoke the lipo'd universal binary (target-triple
   builds land under `target/<triple>/release/`).
7. macOS: universal build (both targets + `lipo`), matching the sidecar's
   proven pattern.

**#241 — docs + drift:**
1. README `ct` section: deleting superseded notes outright is safe and
   recommended (watcher/startup reconcile/`ct ingest` cascade
   doc+chunks+embeddings; lineage lives in git history + session records);
   `ct drift` = periodic CHECK, `ct ingest --yes` = REPAIR.
2. **Split `reconcile_vault` (`src-tauri/src/reconcile.rs:50`) into classify
   + apply**; `ct drift` calls the CLASSIFY half as a dry run — guard
   semantics shared by construction (empty-walk = unmounted vault,
   ambiguous/unrelativizable rows). Drift reports reconcile's
   gone/ambiguous outputs ONLY — the files-without-rows direction is
   deliberately out (a hand-rolled second check would evade the guards).
   Read-only (ro connection; repair stays `ct ingest`/desktop startup).
3. New `Cmd::Drift { json: bool }` in `tools/src/bin/ct.rs`; exit codes:
   0 clean, 3 drift found (exit 2 is `EXIT_NO_RESULTS` — collision avoided),
   4 classify guard trip (clear message, NOT "drift").

**Rejected alternatives:** startup drift check in the desktop app
(redundant — startup reconcile already repairs); hand-rolled symmetric diff
in the tool (ignores reconcile's guards → permanent false drift);
`${GITHUB_REF_NAME#v}` as version source (branch names break filenames);
ungated tauri-action (BLOCKER above).

## Error handling

`ct drift`: guard trips are a distinct exit code with a distinct message —
an unmounted vault must never read as "drift found". No new error paths
elsewhere (build steps fail the workflow on their own).

## Testing

- #241: all existing reconcile tests stay green against the split (incl.
  `vanished_file_is_deleted_and_chunks_cascade`); classify unit tests in
  `src-tauri/src/reconcile.rs`; CLI tests in `tools/` (clean → 0, drifted →
  3, guard trip → 4) in reconcile's fixture style (tempdir + seeded sqlite).
- #244: local `cargo build --release -p curated-thoughts-tools --bin ct` +
  `ct --help`; CI verified by workflow_dispatch on the PR branch (build +
  package steps, gate keeps the release untouched); upload path proven on
  the next real release tag.
- README rendered-read check.

## Out of scope / open questions

- Runtime stale-CLI warning in `ct`: not cheaply feasible (no version
  identity in the tools crate; no installed-app-version source to compare
  against) — follow-up issue to be filed separately.
- Wisdom-fact lifecycle on delete: stays with PR #242 territory (issue text).
- OQ (resolved): macOS included in v1 (lipo pattern proven in-file); #241
  docs + drift in ONE PR (drift dominates the diff post-split).

Fixes #244. Fixes #241.
