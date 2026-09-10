# Watcher + walker: exclude `.brain` working directories from ingestion

**Date:** 2026-09-10
**Status:** Draft (rev 4 — round-3 review findings addressed:
empty-walk reconcile contract + targeted-delete regression test,
substring-lookalike watcher/walker fixtures; vault-walk rev-1 spec
explicitly marked superseded)
**Branch:** docs/spec-2026-09-10-vault-walk-brain-dir-exclusion
**Priority:** Low (log noise; no data corruption)

## Problem

CT's vault accumulates immortal `pending` rows for `.brain` working files.
Verified on Kurt's ThinkPad (2026-09-10, re-verified by a GLM 5.3 review
pass against the live DB and source): documents ids **6185** and **6215** —
`immutable-source-files/agents/people/.brain/errors.log` and
`immutable-source-files/agents/people/tessera/.brain/errors.log` — are
stuck `pending`, never ingest, and are re-enqueued by the sweep every pass.

Root cause (rev-1 of this spec attributed this to the walker; the GLM
review of `pipeline/mod.rs` and `db/queue.rs` proved that wrong):

- The **walker** is not the vector: `collect_files` extension-gates its
  output via `should_ingest_extension` (`walk_vault.rs:169`), and `.log`
  is not an ingestable extension (`chunker/classify.rs:24-69`) — the
  pipeline also early-returns for non-ingestable extensions before
  `upsert_document` (`pipeline/mod.rs:676-685`). The walker never staged
  these rows.
- The **filesystem watcher** is the vector: `enqueue_vault_event`
  (`db/queue.rs:26-108`) hashes and upserts **any** in-vault Add/Modify
  event. Its own comment concedes the gap ("The walker has always
  filtered these; the watcher never did", `queue.rs:70-73`). Its
  exclusion check does not cover `.brain` directories, so every write to
  a `.brain/errors.log` (CT writes its own at `<vault>/.brain/errors.log`,
  `pipeline/mod.rs:484`) stages a `documents` row that ingestion then
  silently skips forever (`pipeline/mod.rs` early-returns non-ingestable
  extensions at lines ~676–685, before `upsert_document`).

`errors.log` is actively appended (the top-level one was modified today),
so rows deleted without a watcher gate would be re-staged immediately —
the watcher path must be fixed or the bug recurs.

## Approach

1. **Watcher gate (the actual fix).** In `enqueue_vault_event`
   (`db/queue.rs`), before staging a new row, reject any path containing
   an `EXCLUDED_DIRS`-named component. Implement as a small shared helper
   (e.g. `path_has_excluded_component(&Path) -> bool` in `walk_vault.rs`,
   reusing the `EXCLUDED_DIRS` const via `is_excluded_dir` per component)
   so walker and watcher can never drift. Placement must respect the
   existing ordering: **after** the `EventKind::Remove` handling
   (`queue.rs:62-68` — deletes must stay ungated so pre-existing rows can
   still heal), alongside the existing `is_excluded_file` call.
2. **Walker exclusion (defense in depth).** Add `".brain"` to
   `EXCLUDED_DIRS` in `src-tauri/src/walk_vault.rs` (line 20). The
   `filter_entry` prune at every depth then guarantees `.brain` content
   can never enter walker output even if extensions change — and, via
   item 3, makes the existing rows deletable.
3. **Cleanup of existing rows happens via reconcile — and requires an
   explicit `ct ingest` run.** Rows 6185/6215 are *already* absent from
   every walker output today (the extension gate predates this spec), so
   once the watcher gate stops re-staging them they are deletable as
   "vanished" by `reconcile_vault` — its absence-driven delete arm chunks
   cascade, and its "must not delete what it cannot match" tests pin the
   behavior. **But `reconcile_vault`'s only production caller is the
   `ct ingest` CLI path** (`tools/src/cmds.rs:193`); the desktop app's
   automatic startup pass (`lib.rs` ~995–1110) never calls it and only
   purges rows whose file no longer exists on disk — and both
   `errors.log` files exist. **Operational requirement:** clearing the two
   stuck rows requires one `ct ingest` run after this change ships; the
   app does not self-heal them. No new delete code is added, avoiding the
   race-with-watcher hazards a bespoke cleanup would have (rev-1's
   implementer's-choice cleanup is dropped per review finding 5).

   **Empty-walk sub-case (round-3 review finding 6).**
   `reconcile_vault` (`src-tauri/src/reconcile.rs:42-151`) short-circuits
   when `walked.is_empty()` (lines 48-51) to protect against a
   misconfigured or unmounted vault root — reconciling against an empty
   walk would otherwise delete the entire index on a transient mount
   failure. That protection must stay. **But** a row whose path matches
   the new exclusion pattern (an `EXCLUDED_DIRS` component on every
   segment) is *structurally* guaranteed absent from any walk — the
   exclusion is applied at every depth in `filter_entry`, so the walker
   can never emit such a path regardless of mount state. **Contract:**
   on `walked.is_empty()`, `reconcile_vault` must still attempt a
   targeted delete of rows whose path contains an excluded-directory
   component (after resolving the path into components and comparing
   against `EXCLUDED_DIRS`); non-excluded rows continue to be preserved
   untouched (the mount-failure safety net is narrowed, not removed).
   The targeting helper (`path_has_excluded_component` from item 1) is
   reused so the watcher gate and this reconcile contract cannot drift.
   This unblocks the "vault whose only walker-visible content is
   excluded" case — e.g. a small vault containing only `.brain/errors.log`
   — without giving up the mount-failure guarantee for user-visible rows.
4. **`.brain/proposed` is intentionally excluded too.** `vault/safe_path.rs`
   sanctions `.brain/proposed` as a write location for proposed content
   operations — but proposed documents reach the wiki through the
   proposals pipeline and OKF export, never through vault ingestion.
   Excluding the whole `.brain` tree from ingestion is therefore intended
   for `proposed` as well; stated explicitly so reviewers don't flag it
   as a regression.

### Rejected alternatives

- **Exclude only `errors.log` by suffix.** Rejected: `.brain/` holds the
  brain DB, embeddings, and conversion shadow copies
  (`pipeline/mod.rs:274-286`) — none are vault content.
- **Extend `folder_rules` with an `exclude` mode.** Rejected: per-folder
  rules are a user-facing librarian-policy feature; CT-owned working dirs
  should be excluded unconditionally, like `.git`. (The schema CHECK at
  `db/schema.rs:62-63` confirms there is no exclude mode today.)
- **Substring path-segment matching** (`EXCLUDED_PATH_SEGMENTS`-style
  `contains`). Rejected in rev 1 and re-rejected: over-matches lookalikes
  (`my.brain.notes/`); component-exact matching is the correct semantics.
- **Gating Remove events.** Rejected: would strand pre-existing rows
  (see `queue.rs:62-68` comment).

## Error handling

- Watcher gate: pure rejection before any hashing/DB work — no new
  failure modes; the existing containment and Remove-ordering behavior is
  untouched.
- Walker exclusion: only removes entries from the walk, in `filter_entry`
  before any file I/O.
- Cleanup: no new code; reuses reconcile's tested delete arm.

## Testing

- **Watcher unit test:** emit Add/Modify events for
  `<vault>/.brain/errors.log`, `<vault>/nested/.brain/x.log`, and control
  paths (`<vault>/notes.md`, `<vault>/brain/x.md` — lookalike dir must
  still stage, plus `<vault>/my.brain.notes/x.md` and
  `<vault>/.brainish/x.md` — these are the substring-lookalike controls:
  a faulty `contains(".brain")` check would incorrectly exclude them, so
  asserting they stage pins component-exact semantics). Assert staged
  rows exist only for control paths.
- **Remove-ordering regression:** a pre-staged `.brain` row is still
  deleted when its Remove event arrives (gate must not block deletes).
- **Walker unit test:** fixture vault containing `notes.md`,
  `.brain/errors.log`, `nested/.brain/errors.log`, `brain/` lookalike,
  `my.brain.notes/x.md`, and `.brainish/x.md` (the latter two are
  substring-lookalike controls — must be walked). Assert only intended
  files are walked.
- **Reconcile test (non-empty walk):** with a `.brain`-excluded walk, a
  pre-staged `.brain/errors.log` row is deleted and chunks cascade.
- **Reconcile test (empty-walk, round-3 review finding 6):** vault whose
  only content is `.brain/errors.log` (walker returns empty once the
  exclusion lands); DB has both a `.brain/errors.log` row AND an
  unrelated `notes.md` row. Reconcile. Assert: `.brain/errors.log`
  deleted (and chunks cascade), `notes.md` row preserved (the
  mount-failure safety net must not be widened).
- Existing walker/queue/reconcile test suites stay green.

## Out of scope

- **The generalized defect class:** the watcher stages rows for *any*
  non-ingestable extension (`.log` today; images, binaries, etc.
  tomorrow), all of which become immortal pending rows re-enqueued by the
  sweep. This spec fixes only the `.brain` instance; a general
  extension-gate in `enqueue_vault_event` (mirroring
  `should_ingest_extension`) is a sensible follow-up and should be filed
  as an issue rather than folded in here.
- ct_doctor import-preflight live-row scoping (companion spec in
  curated-thoughts-integrations:
  `2026-09-10-doctor-preflight-live-scope-design.md`).
- folder_rules exclude mode.

## Open questions

None. Rev-1's misattribution (walker vs watcher), the recurrence gap, the
`.brain/proposed` interaction, and the cleanup-mechanism ambiguity (all
from the GLM 5.3 round-1 review), plus the empty-walk reconcile contract
and the substring-lookalike fixtures (round-3 review), are resolved above.
