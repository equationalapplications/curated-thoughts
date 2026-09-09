# Phase-2 provenance flip: skip+log (issue #186 §2.4)

**Date:** 2026-09-09
**Status:** Draft rev 2 (GLM 5.3 frontier review folded: C1-C3, I1-I6, M1-M7)
**Branch:** spec/issue186-phase2-flip
**Priority:** P1

## Problem (verified current state)

The #186 fix (PR #188, v2.6.0+) shipped **Phase 1**: facts whose evidence anchors no live chunk are still written, flagged `unanchored=1` in `librarian_evidence`, so the drop rate could be measured before the permanent strict policy. Phase 2 (spec §2.4) — "flipped after the baseline is measured" — has never landed, so unanchored facts continue entering the table every run.

**Baseline (measured, live brain, Sep 6–9):**
- Sep 6 run (pre-writer-fix residues same night): 34 anchored / 38 unanchored (47% anchored)
- Sep 9 run (v2.6.2 writer): **31 anchored / 1 unanchored (97% anchored)**
- Stock at spec time (live DB, read-only): 311 anchored / 356 flagged rows in `librarian_evidence`; of the flagged, **49 join to live entries (deleted_at IS NULL) and 307 join to already-soft-deleted entries** (0 orphan evidence rows). The 307 are already doomed: `prune_old_librarian_inferred` hard-deletes ≤7 days after soft-delete with the paired evidence delete, outbox Delete, and edge purge (lib.rs:1816). The operative number for this migration is **49 live flagged rows**.
- The unanchored class has collapsed from "a large fraction" (the spec's fear) to ~1 fact per run. The Phase-1 measurement purpose is served.

## Approach

Three coordinated changes, one PR. Release ordering (review criterion 2): same-PR is REQUIRED — migrate() runs at AppDb::open, strictly before the spawned heal task, so a migrated brain has no live flagged rows when post-revert heal first runs.

### 1. Insert-time gate: write-with-flag → skip+log
`commit_fact_add` (db/commit.rs:1324-1335): when `unanchored` computes true, do NOT insert the entry or evidence row. Instead: log (dual-path warn per repo convention) with proposal id, fact title, and reason `evidence anchors no live chunk`. The proposal item is marked **rejected** via a new `FactAddOutcome` variant mapped through the existing `ItemCommitOutcome::Rejected` machinery (status='rejected' + rejected_count, commit.rs ~2144-2200; the off-manifest-edge drop in `commit_edge_add` is the precedent).

**Counter placement (review C3):** `skipped_unanchored` lives in **`CommitContext`** (alongside `facts_duplicated` / `dropped_edges`), NOT in `LibrarianRunSummary` (wrong component and process boundary — skips happen at proposal commit, not per-document synthesis). Surfaced in: (a) the `ct approve` output line (cmds.rs:457, extending the `items=... dropped_edges=...` line), and (b) the rejected resolution event summary, following the duplicates precedent (commit.rs:4908 test). No run-summary plumbing in this PR.

### 2. Re-grade + purge of Phase-1 stock (§2.4) — MIGRATION_V20
New `MIGRATION_V20` (schema ladder: current max 19 → 20; bump the max_version pin test at connection.rs:480 and tests/okf_migration.rs). Following the V18 pattern in db/connection.rs (:183-290):

- **Scope (review C2):** only evidence rows joining entries with `deleted_at IS NULL` — the 49 live flagged rows. Soft-deleted entries' evidence (307) is left to `prune_old_librarian_inferred`'s existing full ceremony (already-signed death warrant); re-grading or exporting them would inflate the export and duplicate prune's work.
- Re-derive `evidence_has_live_chunk` per row: now-anchored → clear flag (`unanchored=0`); still-unanchored → export then purge.
- **Export (review C1):** `repair-export-phase2/` under the brain dir, one JSON per doomed row carrying the FULL joined row: entry fields (id, entity_id, title, body, source_ref, created_at) PLUS `evidence_json` and `proposal_id` — the entry's `source_ref` is a content-free token, so anything less destroys the only copy of the provenance data this export exists to preserve (and the chunk-selection follow-up needs the proposal ids).
- **Hard gate (review I1):** exported_count == doomed_count, else skip the destructive phase with a loud WARN and still stamp (V18 review-round-5 finding-4 posture). Brain-completeness assertion gates the destructive phase separately (§2.5 pattern). Pathless/in-memory DB arm (db_dir=None): skip export, run the re-grade but SKIP purges (destructive phase requires provable backup). Caught-error arm: WARN + skip + stamp.
- **Purge = full repair-orphan transaction (review I2):** per row, in one transaction — outbox `OutboxOperation::Delete` push, paired `delete_librarian_evidence`, entry hard-DELETE, and `purge_edges_for_hard_deleted` (parent §2.4 "same treatment as repair orphans"; evidence_repair.rs ~330-360 precedent; #132/#158 contract).
- **Recovery path (review I3):** the re-grade is exposed as an idempotent manual command — `ct evidence regrade` (new tools subcommand wrapping the same function the migration calls). Skip WARNs name it explicitly. This resolves the V18-style stamp-even-when-skipped hazard: an operator on a skipped brain has a documented, idempotent recovery route.
- Idempotent; stamped in `schema_version`.

### 3. Carve-out revert (§2.3)
`source_ref_is_still_grounded` (db/commit.rs:558-562): the `Some((_, 1)) => true` Phase-1 branch is removed — grounding for token rows becomes strictly evidence-based again. The defensive branches (missing evidence row, DB error → grounded-with-warn) stay unchanged. The existing seventh D-test `phase1_unanchored_rows_are_treated_as_grounded` (commit.rs:5407) is INVERTED to pin the new behavior (flagged + no live chunk → NOT grounded; defensive branches keep their own tests).

### Post-flip writers of unanchored=1 (review I4 — recorded posture)
Two paths will still write `unanchored=1` rows after this PR, and that is the ACCEPTED strict-policy behavior: (a) `bundle_apply` (bundle evidence chunks absent from the importing brain → flagged row; post-revert heal will soft-delete it on the next heal cycle, prune finishes it — strictly-evidence-based grounding working as designed); (b) manual `run_evidence_repair` re-derivations. This consciously relaxes parent §2.3's "re-grade is the ONLY purge path" clause to "the only EXPORTING purge path" for these two writer classes; the export guarantee applies to the one-shot migration, not to steady-state heal. No bundle-import code changes in this PR.

## Testing
- Unit: skip+log path (assert no entry row, warn emitted, CommitContext counter incremented, item status 'rejected'); anchored rows still written; counter in approve output line + resolution event summary (duplicates-precedent shape).
- Carve-out revert: flagged-but-now-anchored row survives heal; still-unanchored row soft-deleted by heal post-revert; inverted D-test; defensive branches keep their own tests.
- Migration: seeded live unanchored stock → re-grade clears anchored, exports+purges the rest; **soft-deleted stock untouched by V20** (prune still owns it); full-ceremony assertions — outbox Delete emitted per purge, `purge_edges_for_hard_deleted` invoked (I6); idempotency (second open: no-op); brain-incomplete skip path; **export-completeness skip gate** (seed N doomed, make export miss one → zero deletions + WARN + stamp) (I5); pathless-DB arm skips purges.
- `max_version` pin 19 → 20 in connection.rs + okf_migration tests (M3/M7).
- Manual command: `ct evidence regrade` idempotent on an already-migrated brain.

## Out of scope / open questions
- Chunk-selection follow-up (dangling refs at the source): the Sep 9 run's single unanchored fact's proposal id is `prop_…` (recorded in librarian_evidence; the migration export will carry it) — decide after flip whether it warrants its own issue.
- Run-summary surfacing of skips across the doc loop (needs plumbing across the approve boundary) — deferred; the commit-time surfaces are authoritative.
