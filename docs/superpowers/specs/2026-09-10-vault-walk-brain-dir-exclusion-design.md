# Vault walker: exclude `.brain` working directories from ingestion

**Date:** 2026-09-10
**Status:** Draft
**Branch:** docs/spec-2026-09-10-vault-walk-brain-dir-exclusion
**Priority:** Low (log noise; no data corruption)

## Problem

CT writes its own runtime log at `<vault>/.brain/errors.log`
(`src-tauri/src/pipeline/mod.rs:484`), but the vault walker does not exclude
`.brain` directories. On Kurt's ThinkPad the vault
(`~/Documents/equational-wiki`) contains nested working `.brain/` dirs —
`immutable-source-files/agents/people/.brain/errors.log` and
`immutable-source-files/agents/people/tessera/.brain/errors.log` — and both
are stuck **pending** in the `documents` queue (doc ids 6185, 6215,
verified 2026-09-10). They will never ingest cleanly (a log file is not
meaningful wiki content) and re-appear as pending noise in status checks.

Root cause: `EXCLUDED_DIRS` in `src-tauri/src/walk_vault.rs` (line 20)
covers build/VCS dirs (`.git`, `node_modules`, …) but **not `.brain`**.
`filter_entry` at line 146 prunes only names in that list, so any
`.brain/` directory inside the vault tree is walked and its files queued.
The same gap would also let CT's own top-level `<vault>/.brain/errors.log`
get queued on vaults where the brain dir lives inside the vault.

## Approach

1. **Add `".brain"` to `EXCLUDED_DIRS`** in `src-tauri/src/walk_vault.rs`.
   One-line change; the `filter_entry` prune then applies at every depth,
   which covers both the top-level brain dir and nested working dirs like
   `agents/people/.brain/`.
2. **Clean up the two stuck rows.** The already-queued docs (6185, 6215)
   pre-date the exclusion. Add a small migration-style cleanup consistent
   with existing queue hygiene: on startup reconcile, delete `documents`
   rows whose path contains a `/.brain/` segment (and their orphaned
   chunks). If reconcile-style cleanup is disproportionate, the minimal
   alternative is a one-off SQL cleanup on the affected brain + the
   exclusion alone going forward — implementer's judgment between these two
   based on how existing sweeps are structured in `pipeline/watchdog/sweep.rs`.
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
