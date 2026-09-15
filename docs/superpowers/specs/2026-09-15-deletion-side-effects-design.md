# Document deletion: preserve pending-proposal provenance + legacy cleanup

**Date:** 2026-09-15
**Status:** Proposed (rev 2)
**Branch:** spec/211-deletion-side-effects
**Issue:** #211
**Priority:** Medium (Review desk loses the identity of deleted sources and
accumulates duplicate proposals when a file moves; no index corruption)

## Revision history

- **rev 1** proposed restoring the two side effects of the removed
  `PipelineJob::Delete` worker arm: marking `wiki_pages` rows `orphaned`
  and removing `.brain/converted/<stem>.md`. Review found both targets are
  obsolete, so rev 1 would have shipped no behavior a user could see:
  - `wiki_pages` has had no production writer or status reader since the V7
    OKF migration (`db/okf_migration.rs`). That migration moved approved
    pages into `curated_entities` and marked every pending page
    `orphaned`. The remaining references are the migration itself,
    `clear_vault_tables` (`db/queries.rs:145`) and tests. The rev 1 claim
    that "the review shim writes `source_doc_ids`" was wrong:
    `review_shim.rs` builds that field for the API response from
    `curated_proposal_sources ⋈ documents` and writes no table.
  - Nothing in `src-tauri/src` or `tools/src` writes to `.brain/converted`.
    Every reference creates the directory or asserts it exists.
- **rev 2** (this document) retargets #211's intent ("deleting a document
  mishandles derived knowledge") at the tables that hold derived knowledge
  today. It also removes the legacy leftovers that misled rev 1.

## Problem

### Where derived knowledge lives today

The librarian (`librarian/synthesis.rs:1226`) turns an ingested document
into **proposals** (`curated_proposals`, `db/okf_ddl.rs:162`). Each
proposal has items (`curated_proposal_items`) and source links
(`curated_proposal_sources`, `db/okf_ddl.rs:190`):

```sql
doc_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
role   TEXT NOT NULL DEFAULT 'evidence' CHECK(role IN ('trigger','evidence')),
```

Once approved, a proposal becomes `curated_entities` plus
`llm_wiki_entries` facts. Their provenance lives in `librarian_evidence`
(V18), which references chunks by `content_hash` and has no foreign key to
`documents`.

Foreign-key cascades do fire on every connection that deletes documents.
The desktop per-event watcher connection (`lib.rs:1353`) and `ct watch`'s
`open_rw` (`tools/src/write.rs:63`) never run `PRAGMA foreign_keys=ON`, but
the bundled SQLite is compiled with `-DSQLITE_DEFAULT_FOREIGN_KEYS=1`
(`libsqlite3-sys` 0.30.1 `build.rs:123`). Deleting a `documents` row
therefore removes its `chunks`, `embeddings`, `ingest_runs` and
`curated_proposal_sources` rows everywhere.

### What goes wrong

Deleting a document silently removes the proposal's link to it. What
survives and what does not:

| Surface | After the source document is deleted |
|---|---|
| Item evidence (`hydrate_evidence`, `db/proposals.rs:446`) | Quote survives (copied into `curated_proposal_items.evidence`); chunk shows "Unknown source (deleted source)". Designed for, and tested at `proposals.rs:802`. |
| Proposal `source_doc_paths` (`source_paths_for_proposal`, `proposals.rs:352`) | The deleted path disappears. Once the last source is gone the list is empty, and no field says a source ever existed. |
| Commit event label (`trigger_source_label`, `db/commit.rs:1032`) | Falls back to `"unknown source"`. |
| Library reverse lookup (`list_proposals_for_document`) | The proposal no longer appears under any document. |
| Supersede on re-ingest (`supersede_stale_pending`, `proposals.rs:186`) | Keyed on `trigger` doc id. See below. |

**Moves are deletes.** The desktop watcher and the desktop startup reconcile
both handle a move as `Remove` of the old path plus `Create` of the new one:
- the watcher does this via `enqueue_vault_event`;
- startup reconcile (`lib.rs:1222-1234`) removes each vanished path the
  same way.

Only `ct ingest`'s `reconcile_vault` (`reconcile.rs`) re-points a row by
content hash. Each document row carries its own synthesis watermark
(`documents.synth_hash`, `librarian/mod.rs:201`), so the new row
re-synthesizes and emits a fresh proposal. That proposal cannot supersede
the old one: the old proposal's trigger link was cascaded away, and it
would have pointed at a different id anyway. **Moving a file on the desktop
leaves a duplicate pending proposal for the same target, and the orphaned
copy has no visible source.**

### Delete sites

Every production statement that deletes a `documents` row:

| # | Site | Enclosing function | Trigger |
|---|---|---|---|
| 1 | `db/queue.rs:100` | `enqueue_vault_event` (Remove) | watcher Remove; desktop startup vanished-row loop (`lib.rs:1226`) |
| 2 | `db/queue.rs:157` | `enqueue_vault_event` (NotFound) | Add/Modify for a file that vanished before read |
| 3 | `reconcile.rs:192` | `reconcile_vault` excluded pre-pass | row moved under an excluded dir |
| 4 | `reconcile.rs:218` | `reconcile_vault` vanished `None` branch | file gone, no rename match |
| 5 | `reconcile.rs:259` | `purge_brain_rows` | empty-walk `.brain` heal |
| 6 | `lib.rs:842` | `purge_excluded_rows` | startup heal of excluded-directory rows |

That is six statements in four functions. Two other deletes are out of
scope:
- `db/proposals.rs:818` is test-only and deliberately exercises the raw
  cascade.
- `db/okf_migration.rs:149` is a one-off migration bulk delete of
  `tier='wiki'` rows.

`delete_document` (`db/queries.rs:117`) has no production callers, only
its unit test at `queries.rs:296`.

## Design

### D1 — Pending proposals stay `pending`

A pending proposal that loses sources, including its last one, is **not**
auto-transitioned. It stays `pending` and is flagged (D3). A human decides
its fate from the Review desk.

Rejected alternatives:

- **Auto-`rejected`.** Every desktop file move is a delete (see Problem),
  so this would throw away valid, unreviewed work on each move or rename.
  The proposal also remains reviewable after deletion: evidence quotes are
  stored on the item, not read from chunks. Auto-rejection would also
  bypass the human-verification gate that `resolve_proposal` enforces
  (`reviewed_by`, V21).
- **New `invalid`/`stranded` status.** `curated_proposals.status` is a
  `CHECK` constraint, so SQLite needs a full table rebuild to widen it.
  `resolve_proposal` accepts only `pending` (`commit.rs:2292`), so the new
  status would make these proposals impossible to approve or dismiss
  unless every resolve path and review filter learned it. That is the
  same wedge `insert_proposal`'s empty-items guard exists to prevent
  (`proposals.rs:254`).

Non-pending proposals (`approved`, `rejected`, `partial`, `superseded`) are
historical records. D2 does not touch them.

### D2 — Record deleted sources before the cascade

New table in the next free schema version (V23 at time of writing; confirm
against the `schema_version` watermark and bump the watermark pin in
`tests/okf_migration.rs`):

```sql
CREATE TABLE IF NOT EXISTS curated_proposal_deleted_sources (
    proposal_id TEXT    NOT NULL REFERENCES curated_proposals(id) ON DELETE CASCADE,
    doc_path    TEXT    NOT NULL,
    doc_hash    TEXT    NOT NULL,
    role        TEXT    NOT NULL CHECK(role IN ('trigger','evidence')),
    deleted_at  INTEGER NOT NULL,
    PRIMARY KEY (proposal_id, doc_path)
);
```

- `doc_path` is the virtual path, matching the `documents.path` contract
  (#204). It exists so the UI can name what was deleted.
- `doc_hash` is `documents.hash` (sha256 of the file bytes) at deletion
  time. D4 uses it for move detection.
- The migration is additive, idempotent (`IF NOT EXISTS`) and needs no
  backfill. Sources deleted before this version are already gone and
  cannot be recovered.
- The table is CT-owned. The llm-wiki engine never reads it, and it is not
  outbox-replicated (no `curated_proposal*` table is).

### D3 — Unified helper `delete_document`

Replace the unused `db/queries.rs:117` with:

```rust
pub fn delete_document(tx: &rusqlite::Transaction<'_>, path: &str) -> anyhow::Result<usize>
```

In order, inside the caller's transaction:

1. Record deleted sources for **pending** proposals only:
   ```sql
   INSERT INTO curated_proposal_deleted_sources
       (proposal_id, doc_path, doc_hash, role, deleted_at)
   SELECT s.proposal_id, d.path, d.hash, s.role, unixepoch()
     FROM curated_proposal_sources s
     JOIN documents d          ON d.id = s.doc_id
     JOIN curated_proposals p  ON p.id = s.proposal_id
    WHERE d.path = ?1 AND p.status = 'pending'
   ON CONFLICT(proposal_id, doc_path) DO UPDATE SET
       doc_hash = excluded.doc_hash, role = excluded.role, deleted_at = excluded.deleted_at
   ```
   The upsert covers a path that is deleted, re-created and deleted again
   while the same proposal is still pending.
2. `DELETE FROM documents WHERE path = ?1`. The cascade removes chunks,
   embeddings, `ingest_runs` and the live source links. The helper returns
   **this** statement's affected-row count, so `purge_excluded_rows` keeps
   its count.

**Why `&Transaction`, not `&Connection`.** Steps 1 and 2 must commit or
roll back together. A recorded deleted source for a document that still
exists is a false flag; a delete with no record loses provenance. Taking
`&Transaction` makes atomicity a compile-time requirement instead of a
convention. `Transaction` derefs to `Connection`, so the body is ordinary
rusqlite.

Proposal-level surfaces read the new table:

- `ProposalSummary` and `ProposalDetail` (`db/proposals.rs`) gain
  `deleted_source_paths: Vec<String>` (trigger first, then by path, the
  same order as `source_doc_paths`). It feeds the CLI/MCP `proposals
  list`/`show` output (an additive JSON field) and the frontend type in
  `src/lib/tauri.ts`.
- The Review desk list and detail show a "Source deleted: `<basename>`"
  marker for each entry, and a stronger "All sources deleted" marker when
  `source_doc_paths` is empty. `insert_proposal` rejects zero-source
  proposals (`proposals.rs:247`), so an empty live list with a non-empty
  deleted list means every source was deleted.
- `trigger_source_label` (`commit.rs:1032`) falls back to the deleted
  trigger's basename before `"unknown source"`.
- `list_proposals_for_document` is unchanged. A deleted document has no id
  to look up.

### D4 — Supersede heals desktop moves

Extend both arms of `supersede_stale_pending` (`proposals.rs:186`). An older
pending proposal for the same target is also superseded when its **deleted**
trigger's hash equals the new trigger document's hash:

```sql
AND (
  EXISTS (SELECT 1 FROM curated_proposal_sources s
          WHERE s.proposal_id = curated_proposals.id
            AND s.doc_id = ?4 AND s.role = 'trigger')
  OR EXISTS (SELECT 1 FROM curated_proposal_deleted_sources ds
             WHERE ds.proposal_id = curated_proposals.id
               AND ds.role = 'trigger'
               AND ds.doc_hash = (SELECT hash FROM documents WHERE id = ?4))
)
```

Identical bytes at a new path with the same target is exactly a move.
Matching is best-effort: the target key (`entity_id`, or `proposed_name`
for `new_entity`) must still match, and re-synthesis is not deterministic.
When the model names the entity differently, the old proposal stays pending
with its "All sources deleted" marker, and the human dismisses it. That is
still strictly better than today, where the duplicate carries no marker.

### D5 — Rewire the four production delete functions

| Function | Change |
|---|---|
| `enqueue_vault_event` (sites 1, 2) | `let tx = conn.transaction()?; delete_document(&tx, &path_str)?; tx.commit()?;`. The function already takes `&mut Connection`, so use the checked `transaction()`, not `unchecked_transaction()`. |
| `reconcile_vault` (sites 3, 4) | call `delete_document(&tx, old_path)` on the existing `tx` |
| `purge_brain_rows` (site 5) | call `delete_document(&tx, path)` on the existing `tx` |
| `purge_excluded_rows` (site 6) | change the parameter to `&mut Connection` (its only caller already holds `conn_opt.as_mut()`, `lib.rs:1199`), open one `conn.transaction()` around the loop, and sum the returned counts. The return type moves from `rusqlite::Result` to `anyhow::Result`; the caller only logs `Err`. All-or-nothing is safe because the heal retries on the next launch. |

After this change, `DELETE FROM documents WHERE path` appears only inside
`delete_document`. A grep assertion in the implementation PR's verification
step pins that.

### D6 — Legacy cleanup

**L1 — Stop creating `.brain/converted`.** Remove the `create_dir_all`
calls and their error branches at:
- `lib.rs:610` (`set_vault_path`)
- `lib.rs:1544` (`switch_vault`)
- `lib.rs:3433-3436` (default vault)
- `lib.rs:3460-3464` (recovery vault)
- `vault/layout.rs:29`

Drop the matching assertions in `vault/layout.rs` `creates_all_required_directories`
and `onboard/mod.rs:306`. Nothing else in a vault depends on `<vault>/.brain`
existing; the name otherwise appears only as an excluded directory.
Existing empty `converted/` directories in users' vaults are left alone:
they are inert, already excluded from walks, and removing user filesystem
content is not worth the risk.

**L2 — Remove `Stage::Deleting`.** No code sets it. It lived only in the
removed worker arm. Remove:
- the variant, its `as_str` arm and its `from_u8` arm in
  `pipeline/watchdog/heartbeat.rs`; `8` then decodes to `Idle`, which is
  safe because the stage is an in-memory `AtomicU8` and never persisted
- the `Deleting` budget arm and its stale comment
  ("Unindexed `LIKE` scan over wiki_pages (pipeline/mod.rs:229-256)") in
  `budgets.rs:34-35`
- `Deleting` from `stage_uses_shared_sqlite` in
  `recovery.rs:90`
- the test stage lists in `budgets.rs:88` and `recovery.rs:204`
- the doc-comment mentions in `watchdog/mod.rs:51,241`

Leave the SQL comment inside `MIGRATION_V14` (`schema.rs:285`) untouched.
Migration text is historical.

**L3 — Keep the `wiki_pages` table.** Do not drop it.
`run_okf_migration` runs on every `AppDb::open` until the OKF marker is
set (`connection.rs:831`) and reads `wiki_pages`, so an older database
opened by a new build still needs the table. Dropping it safely would need
a new migration gated on the marker, which buys nothing. It stays a
documented leftover.

## Principles (apply to this PR and future delete paths)

1. **Filesystem I/O after commit.** Never perform an irreversible
   filesystem effect inside a transaction that can still roll back.
   Collect targets, commit, then act. This PR has no filesystem effect
   (L1 removes the only candidate), but the principle is recorded here
   because rev 1's shadow-copy design violated it.
2. **Atomicity in the type.** A helper whose statements must commit
   together takes `&Transaction`, not `&Connection`.
3. **Checked transactions where possible.** With `&mut Connection` in hand,
   use `transaction()`. Keep `unchecked_transaction()` for call sites that
   only hold `&Connection` (e.g. `reconcile_vault` today).

## Tests

Unit tests in `db/proposals.rs` / `db/queries.rs` unless noted. Seed with
`open_in_memory` + `upsert_document` + `insert_proposal`.

1. **Single-source strand.** Delete a proposal's only (trigger) document
   through `delete_document`. The proposal stays `pending`,
   `source_doc_paths` is empty, `deleted_source_paths == [path]`, and the
   chunks are gone.
2. **Partial strand.** Of two sources, delete the evidence document.
   `source_doc_paths` keeps the trigger, and `deleted_source_paths` lists
   only the evidence path.
3. **Non-pending untouched.** For `approved`, `rejected` and `superseded`
   proposals citing the document, no deleted-source row is written.
4. **Atomicity.** Call `delete_document` inside a transaction and drop it
   without commit. Both the document row and the deleted-source record are
   absent.
5. **Re-delete upsert.** Delete a path, re-create it, re-link it to the
   same pending proposal, and delete it again. There is one row and
   `doc_hash` is refreshed.
6. **Move supersede (D4).** P1 is triggered by doc A (hash `h`). Delete A.
   Upsert doc B at a new path with hash `h`. `insert_proposal` P2 has the
   same target, triggered by B. P1 becomes `superseded`.
7. **Different content does not supersede.** Same as 6, but B has hash
   `h2`, so P1 stays `pending`.
8. **Resolve still works.** `resolve_proposal` approve **and** reject both
   succeed on an all-sources-deleted proposal (`commit.rs` tests).
9. **Trigger label fallback.** `trigger_source_label` returns the deleted
   trigger's basename.
10. **Queue path (`db/queue.rs` tests).** Remove and NotFound branches both
    record deleted sources and delete the row.
11. **Reconcile paths (`reconcile.rs` tests).** The vanished `None` branch,
    the excluded pre-pass and `purge_brain_rows` each record deleted
    sources.
12. **`purge_excluded_rows` (`lib.rs` `excluded_row_purge_tests`).** The
    count is unchanged and deleted sources are recorded; update the
    existing tests for the `&mut Connection` signature.
13. **Migration.** The new version creates the table idempotently when run
    twice; the watermark pin is updated.
14. **Legacy.** Layout and onboard tests pass without the `converted`
    assertions; `Stage::from_u8(8) == Stage::Idle`.

Frontend: extend `src/__tests__/ReviewEvidencePanel.test.tsx` /
`ReviewMode.test.tsx` fixtures with `deleted_source_paths` and assert that
both markers render.

## Non-goals

- **Approved knowledge.** Facts and entities from approved proposals
  intentionally outlive their source. A human verified them, and
  `librarian_evidence` keeps the quote and `content_hash`. Marking them
  stale (for example via the engine's `lifecycle_status`/`stale_after`)
  is a separate product decision; file it as a follow-up if wanted.
- **Re-linking live sources on move.** D4 supersedes the duplicate; it does
  not re-attach the old proposal to the new document. Per-chunk evidence
  already re-resolves after re-ingest, because `hydrate_evidence` looks up
  by `content_hash`.
- **Rename detection in the desktop watcher / startup reconcile** (making
  them re-point rows like `reconcile_vault`). This would remove the
  Remove+Create split at its source. It is larger and orthogonal.
- **Dropping `wiki_pages`** (L3).
- **Deleting existing `.brain/converted` directories** in users' vaults
  (L1).
- **Legacy review shim** (`review_shim.rs`): `ShimReviewPage` does not gain
  the new field.
- `db/proposals.rs:818` (test-only raw delete) and `db/okf_migration.rs:149`
  (migration bulk delete).
