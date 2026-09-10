# Vault walker: exclude `.brain` working directories from ingestion

**Date:** 2026-09-10
**Status:** ~~Draft~~ **Superseded by
[`2026-09-10-watcher-walker-brain-dir-exclusion-design.md`](./2026-09-10-watcher-walker-brain-dir-exclusion-design.md)
(rev 5).** This rev-1 spec misattributes the cause to the vault walker; the
filesystem watcher in `src-tauri/src/db/queue.rs` is the actual staging
vector (see the watcher-walker spec §"Problem" for the corrected causal
story and §"Approach" for the watcher gate as the primary fix). The
walker `.brain` exclusion is retained there as defense in depth. This
file is kept for historical traceability of the analysis evolution; the
canonical specification is the watcher-walker file.
**Branch:** docs/spec-2026-09-10-vault-walk-brain-dir-exclusion
**Priority:** Low (log noise; no data corruption)

## Problem (rev 1 — superseded; see watcher-walker spec for corrected analysis)

CT writes its own runtime log at `<vault>/.brain/errors.log`
(`src-tauri/src/pipeline/mod.rs:484`), and CT's vault walker does not
exclude `.brain` directories. On Kurt's ThinkPad the vault
(`~/Documents/equational-wiki`) contains nested working `.brain/` dirs —
`immutable-source-files/agents/people/.brain/errors.log` and
`immutable-source-files/agents/people/tessera/.brain/errors.log` — and
both are stuck **pending** in the `documents` queue (doc ids 6185, 6215,
verified 2026-09-10).

> **rev-1 attribution (wrong):** this section originally named the vault
> walker as the vector — `EXCLUDED_DIRS` in `src-tauri/src/walk_vault.rs`
> (line 20) covers build/VCS dirs but not `.brain`, so the walker queues
> `.brain/errors.log` files. **rev-3 finding:** the walker actually
> extension-gates its output via `should_ingest_extension`
> (`walk_vault.rs:169` / `chunker/classify.rs:24-69`), so `.log` files
> are *never* emitted by the walker. The watcher is the actual vector —
> see the watcher-walker spec for the full causal story.

## Approach (rev 1 — superseded; canonical version is in the watcher-walker spec)

1. **Add `".brain"` to `EXCLUDED_DIRS`** in `src-tauri/src/walk_vault.rs`.
   Retained in the watcher-walker spec as defense in depth: the
   `filter_entry` prune applies at every depth, covering the top-level
   brain dir and nested working dirs like `agents/people/.brain/`.
2. **Clean up the two stuck rows.** The already-queued docs (6185, 6215)
   pre-date the watcher gate. **rev-1 left this as implementer's choice**
   (startup reconcile vs. one-off SQL); **rev-3 pins one contract:**
   clearing the stuck rows requires one explicit `ct ingest` run after
   the watcher gate ships. No new delete code is added — the existing
   `reconcile_vault` absence-driven delete arm in
   `src-tauri/src/reconcile.rs:42-151` heals them, its chunks cascade,
   and its "must not delete what it cannot match" tests pin the
   behavior. **rev-5 correction:** this paragraph originally cited
   `curated-thoughts-integrations/2026-09-10-doctor-preflight-live-scope-design.md`
   as "the authoritative statement of where reconcile runs from". That is
   factually wrong — that spec covers the `ct_doctor.py check` `source_ref`
   census and mentions neither reconcile nor `ct ingest`. Reconcile's call
   sites are pinned in the watcher-walker spec, items 3–5, which also
   supersedes the `ct ingest`-only cleanup contract with a desktop
   self-heal.
3. **Precedent check for future brain-location layouts:** the
   `folder_rules` table offers index/summarize/synthesize modes but no
   exclude mode; we deliberately do NOT extend folder_rules in this change
   (global structural exclusion is the right level for CT-owned working
   directories).

### Rejected alternatives

- **Exclude only `errors.log` by suffix.** Rejected: `.brain/` holds other
  working artifacts (brain.db, embeddings, converted/ shadow copies — see
  `pipeline/mod.rs:273-282`); none of them are vault content. Excluding
  the directory is the structural fix.
- **Extend `folder_rules` with an `exclude` mode.** Rejected: per-folder
  rules are a user-facing librarian-policy feature; CT-owned working dirs
  should be excluded unconditionally, like `.git`.
- **Filter by path segment in `EXCLUDED_PATH_SEGMENTS`.** Rejected:
  segment matching uses substring `contains`, which can over-match
  (e.g. `my.brain.notes/`); directory-name exclusion is exact.

## Error handling

No new failure modes: the exclusion only removes entries from the walk.
Removal happens in `filter_entry` before any file I/O.

## Testing

- Unit test in the walk_vault test module: fixture vault containing
  `notes.md`, `.brain/errors.log`, `nested/.brain/errors.log`, and
  `.brainignore-like` lookalike dir `brain/` (must still be ingested).
  Assert only `notes.md` and the lookalike's files are walked.
- Existing walker tests stay green (no behavior change outside `.brain`).

## Out of scope

- ct_doctor import-preflight live-row scoping (companion spec in
  curated-thoughts-integrations:
  `2026-09-10-doctor-preflight-live-scope-design.md`).
- folder_rules exclude mode (potential future feature, not needed here).
- Retroactive re-index of previously skipped files (none exist — `.brain`
  content was never meant to be indexed).

## Open questions

None — scope is mechanical and fully grounded in the current code.
