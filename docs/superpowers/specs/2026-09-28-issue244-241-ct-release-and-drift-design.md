# Ship `ct` as a release asset (#244) + deletion self-cleaning docs & `ct drift` (#241)

**Date:** 2026-09-28
**Status:** Implemented 2026-09-28 (PR #249)
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
   proven pattern; the ct build step declares explicit `--target` values and
   an ordering dependency on the sidecar steps' target placeholders.
   Tag-gating tauri-action (item 4) drops desktop-installer coverage on
   dispatch runs — accepted: the ct steps ARE the dispatch-run coverage;
   installers are covered on tag builds (Opus spec minor).

**#241 — docs + drift:**
1. README `ct` section: deleting superseded notes outright is safe and
   recommended (watcher/startup reconcile/`ct ingest` cascade
   doc+chunks+embeddings; lineage lives in git history + session records);
   `ct drift` = periodic CHECK, `ct ingest --yes` = REPAIR.
2. **Split `reconcile_vault` (`src-tauri/src/reconcile.rs:50`) into classify
   + apply;** `ct drift` calls the CLASSIFY half as a dry run. **Scope
   (Opus spec M1):** classify returns the FULL plan — gone rows, repointed
   rows, and deletes-of-excluded-rows — and drift reports ALL of it: the
   Sep 27 `records/` incident was exactly a missed excluded-dir delete, so a
   drift that only reports "gone" would have missed it too. Exit 3 on any
   pending repoint/delete/gone.
   **Ambiguous rows (Opus spec M2):** ambiguous entries are WARNINGS, not
   drift — `ct ingest --yes` never clears them (reconcile leaves them
   untouched by design), so failing on them would mean exit 3 forever after
   a "successful" repair; ambiguous-only state exits 0 with the warning
   printed (and listed in `--json`).
   **Claim scope (Opus spec M3):** "shared by construction" holds for `ct
   ingest` ONLY — the desktop startup path (`lib.rs:1524-1560`) runs its own
   purge/exists-delete and never repoints, so startup can DELETE a moved row
   where ingest would repoint it. README names `ct ingest --yes` as the
   repair tool; drift compares against ingest semantics.
   **Walk identity (Opus spec M4):** drift must consume the IDENTICAL file
   list `ct ingest` passes to reconcile (post symlink-trust, sorted, deduped
   — `cmds.rs:163-183`); a shared walk helper serves both commands so the
   two can never diverge.
   Read-only (ro connection; repair stays `ct ingest`).
3. New `Cmd::Drift { json: bool }` in `tools/src/bin/ct.rs`; exit codes:
   0 clean (ambiguous warnings don't fail), 3 pending plan found
   (repoint/delete/gone), 4 classify guard trip — an empty walk is a vault
   problem, not drift (note: that branch currently writes `.brain` deletes
   via reconcile; the exit-4 message says drift made no classification and
   no repair was attempted by drift itself).

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
  `src-tauri/src/reconcile.rs`; CLI tests in `tools/` — clean → 0, pending
  plan (gone/repoint/excluded-delete each) → 3, ambiguous-only → 0 with
  warning, guard trip → 4 — in reconcile's fixture style (tempdir + seeded
  sqlite); exit codes asserted via the CLI harness's status return, not
  stdout string matches. Walk-helper test: drift and ingest produce the
  identical file list on the same fixture.
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
