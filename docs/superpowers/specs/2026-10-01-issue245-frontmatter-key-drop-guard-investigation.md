# Investigation — issue #245: If-Match edits can silently drop frontmatter keys

**Date:** 2026-10-01
**Status:** Investigated 2026-10-01 (review loop)
**Branch:** `feat/issue-245-frontmatter-key-drop-guard` (to be created)
**Priority:** High — silent data loss on the `vault_write_note` edit path (same failure class as #240).

## Summary

`check_round_trip` (`src-tauri/src/okf/write.rs:606`) validates the write path by comparing the
RENDERED document against the INCOMING frontmatter only. It never reads the existing file's
frontmatter keys. An If-Match edit whose payload silently drops keys (e.g. `tags`, `supersedes`,
`entity_type`) passes every current gate — typed frontmatter parse, If-Match staleness, exact
key-set round-trip — and replaces the note with a key-reduced version. The agent that sent the
mangled payload receives success and the data is gone.

## Evidence [V — controller-verified against live main `b7fc952`]

### 1. The guard's signature proves the blind spot

`fn check_round_trip(effective_fm: &OkfFrontmatter, document: &str)` (write.rs:606) — the
parameters are the incoming (token-updated) frontmatter and the rendered document. There is no
parameter through which the existing file's key-set could be compared. Its exact-set check
(rendered keys vs `KNOWN_KEYS` filtered by the incoming struct's `Option` fields, write.rs:643–675)
can only confirm "what arrived is what renders" — not "nothing that was there disappeared".

### 2. The existing content is already in scope at the call site

`write_note_core` reads the full existing file at write.rs:513 (`let existing =
std::fs::read_to_string(&target)`) and already threads it into `enforce_staleness` (523) and
`enforce_size_drop` (559). Plumbing the existing key-set into a new check requires no new file I/O
and no new failure modes — the file is guaranteed read or the write already refused
(`existing_unparsable:parse` at 516–520, `existing_unparsable:no_fence` via the shared fence
helper).

### 3. The #240 body guard does not cover this axis

`enforce_size_drop` (write.rs:291) compares **body** bytes only (`body_bytes(existing)` vs
`body_bytes(document)`); a key-drop that leaves the body intact shrinks the document by only the
frontmatter lines (typically < `MIN_GUARDED_BODY_BYTES` worth of drift relative to a large body)
and, regardless, `allow_shrink` is an intentional opt-out for legitimate body rewrites — not a key
guarantee. `enforce_compaction_markers` (272) is marker-scoped. Neither reads frontmatter keys.

### 4. Failure simulation (reasoned from code paths; all gates pass for a key-dropping edit)

For an existing note with `tags: [x]` + `supersedes: …` and an incoming payload without them:
`validate_frontmatter` passes (both fields are `Option`), `enforce_staleness` passes (token
supplied matches), `check_round_trip` passes (incoming keys render exactly — the dropped keys are
simply absent from both sides of its comparison), `enforce_size_drop` passes (body unchanged).
Write succeeds; keys gone. This matches the issue's claim and Opus cycle-1 finding MAJOR M1 from
the #240 investigation (refuted "the round-trip guard already blocks key drops" against `eab433f`).

## Root cause

The round-trip guard's contract (PR #232) is deliberately scoped to rendered-vs-incoming
consistency; pre-write loss protection on the **existing-vs-incoming** axis was never implemented,
and #240 addressed only the body axis.

## Proposed fix direction (carried into the design doc)

On the If-Match EDIT path only, compare the incoming frontmatter key-set against the existing
file's key-set and refuse unexplained drops with a dedicated error variant naming the dropped
keys, unless an explicit confirm flag (mirroring #240's `allow_shrink`) is passed. Design
questions resolved in the design doc: reuse `collect_frontmatter_fence` for the existing-side
key-set read (yes — the same 64-line-cap fence view the token reader uses); which keys count
(load-bearing set vs all keys); interaction with intentional key removal.

**Corrigendum (Opus design-c1 M1, 2026-10-01):** this doc's and the issue's "e.g. `tags`,
`supersedes`, `entity_type`" example overstates the droppable set — `entity_type` (with
`okf_version`, `profile`, `title`, `created_at`) is a REQUIRED field of `OkfFrontmatter`
(`okf/mod.rs:35-47`); a payload omitting it fails struct parse before `write_note` runs. The
droppable keys are exactly `tags`, `supersedes`, and keys outside `KNOWN_KEYS`. The design doc
carries the corrected analysis.
