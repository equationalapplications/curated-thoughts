# Document deletion: preserve pending-proposal provenance + legacy cleanup

**Date:** 2026-09-15
**Status:** Implemented (rev 4)
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
- **rev 2** retargeted #211's intent ("deleting a document mishandles
  derived knowledge") at the tables that hold derived knowledge today, and
  removed the legacy leftovers that misled rev 1.
- **rev 3** (this document) folds in a verification review of rev 2:
  - **Seventh delete path.** `clear_vault_tables` (`switch_vault` without
    backup restore) deletes every `documents` row and was missing from the
    inventory. It now records provenance too (D5, D7), and the grep pin
    covers every `DELETE FROM documents` form.
  - **Migration stamping.** A literal "next free version" V23 would stamp
    on rootless opens and permanently skip V22 path unification. The table
    is now created on every open, and 23 is stamped only after 22 (D2).
  - **L1 regression.** `write_error_log` depends on `<vault>/.brain`
    existing; it now creates the directory itself (D6 L1).
  - **L2 justification.** The stage *is* persisted, as a string, to
    `pipeline_heartbeat`. It has no reader, so removal is still safe (D6 L2).
  - Minors: markers for proposals stranded before V23, the second review
    queue surface, test fixtures that lack the curated tables, and the two
    constraints the upsert depends on.
  - **Amendment (during plan writing).** Approving a fully stranded
    proposal commits no facts and resolves to `rejected`, because of the
    Phase-2 unanchored gate. D3 documents this and adds it to the marker
    copy; D7 no longer claims these proposals are approvable; test 8 pins
    the outcome and the re-anchor path. Re-anchoring requires the same
    virtual path, because the chunk `content_hash` includes it. That
    corrects rev 2's move non-goal.
- **rev 4** records one supersession. Issue #213 decided per-vault-vs-global —
  the question D7 explicitly deferred — and the answer is **per-vault**, so
  D7's vault-switch stranding rule no longer holds: `clear_vault_tables` now
  clears pending proposals along with the rest of the knowledge layer, and the
  destruction is confirmed in the switch UI rather than being a silent side
  effect. See
  `docs/superpowers/specs/2026-09-15-issue213-per-vault-brain-design.md` D4.
  **Everything else in this spec stands**: D3/D5 provenance recording on
  single-document deletion, the V23 `curated_proposal_deleted_sources` table
  and its grep pin all serve the document-deletion path and are unaffected.

## Problem

### Where derived knowledge lives today

The librarian (`librarian/synthesis.rs:1226`) turns an ingested document
into **proposals** (`curated_proposals`, `db/okf_ddl.rs:162`). Each
proposal has items (`curated_proposal_items`) and source links
(`curated_proposal_sources`, `db/okf_ddl.rs:190`):

```sql
doc_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
role   TEXT NOT NULL DEFAULT 'evidence' CHECK(role IN ('trigger','evidence')),
PRIMARY KEY (proposal_id, doc_id)
```

Once approved, a proposal becomes `curated_entities` plus
`llm_wiki_entries` facts. Their provenance lives in `librarian_evidence`
(V18), which references chunks by `content_hash` and has no foreign key to
`documents`.

Foreign-key cascades fire on every connection that deletes documents.
Connections opened through `migrate()` run `PRAGMA foreign_keys=ON`
(`db/connection.rs:220`). The desktop per-event watcher connection
(`lib.rs:1353`), `ct watch`'s `open_rw` (`tools/src/write.rs:63`) and
`switch_vault`'s raw connection (`lib.rs:1602`) never set the pragma, but
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
| Proposal `source_doc_paths` (`source_paths_for_proposal`, `proposals.rs:352`) | The deleted path disappears. Once the last source is gone the list is empty, and no field says a source ever existed. The evidence panel then claims "No source documents cited." (`ReviewEvidencePanel.tsx`). |
| MCP review queue `source_docs` (`pending_review_queue`, `proposals_review.rs:391`) | Same helper, same loss. |
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

Every production statement that deletes `documents` rows:

| # | Site | Enclosing function | Trigger |
|---|---|---|---|
| 1 | `db/queue.rs:100` | `enqueue_vault_event` (Remove) | watcher Remove; desktop startup vanished-row loop (`lib.rs:1226`) |
| 2 | `db/queue.rs:157` | `enqueue_vault_event` (NotFound) | Add/Modify for a file that vanished before read |
| 3 | `reconcile.rs:192` | `reconcile_vault` excluded pre-pass | row moved under an excluded dir |
| 4 | `reconcile.rs:218` | `reconcile_vault` vanished `None` branch | file gone, no rename match |
| 5 | `reconcile.rs:259` | `purge_brain_rows` | empty-walk `.brain` heal |
| 6 | `lib.rs:842` | `purge_excluded_rows` | startup heal of excluded-directory rows |
| 7 | `db/queries.rs:144` | `clear_vault_tables` (unfiltered `DELETE FROM documents;`) | `switch_vault` without backup restore (`lib.rs:1603`) |

That is seven statements in five functions. Three other deletes are out of
scope:
- `db/proposals.rs:818` is test-only and deliberately exercises the raw
  cascade.
- `db/okf_migration.rs:149` is a one-off migration bulk delete of
  `tier='wiki'` rows.
- `db/connection.rs:142-163` is V22's one-off collision delete. It removes
  a canonical-path `user_doc` row only when a walker row for the same file
  already exists at the virtual path. It runs once, before any build that
  records provenance, and the surviving row is the same file.

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

New CT-owned table:

```sql
CREATE TABLE IF NOT EXISTS curated_proposal_deleted_sources (
    proposal_id TEXT    NOT NULL REFERENCES curated_proposals(id) ON DELETE CASCADE,
    doc_path    TEXT    NOT NULL,
    doc_hash    TEXT    NOT NULL,
    role        TEXT    NOT NULL CHECK(role IN ('trigger','evidence')),
    deleted_at  INTEGER NOT NULL,
    PRIMARY KEY (proposal_id, doc_path)
);
CREATE INDEX IF NOT EXISTS idx_curated_proposal_deleted_sources_hash
    ON curated_proposal_deleted_sources(doc_hash);
```

- `doc_path` is the virtual path, matching the `documents.path` contract
  (#204). It exists so the UI can name what was deleted.
- `doc_hash` is `documents.hash` (sha256 of the file bytes) at deletion
  time. D4 uses it for move detection; the index serves that lookup.
- No backfill. Sources deleted before this version are already gone and
  cannot be recovered (see D3's marker rule for those proposals).
- The llm-wiki engine never reads the table, and it is not
  outbox-replicated: pushes cover only `entries`, `tasks` and `events`
  (`outbox_format.rs:36`). `schema_guard` checks for named tables
  (`schema_guard.rs:145`), so an extra table does not trip it.

**Migration placement: create always, stamp after 22.** `migrate()` gates
every step on `MAX(version)` (`connection.rs:227`). V22 deliberately
refuses to stamp on rootless opens (`open_in_memory`, `migrate_open_db`,
the MCP server) so that `MAX < 22` stays a "recovery pending" marker
(`connection.rs:656-663`). A conventional `if version < 23 { …; stamp 23 }`
would stamp 23 on the first rootless open. Every later rooted open would
then read `MAX = 23`, skip `if version < 22`, and never unify paths. The
FATAL re-warn inside that block would be skipped too, so nothing would
report it.

So V23 runs in two parts, placed after the V22 block:

1. **Every open:** `execute_batch` the `CREATE TABLE IF NOT EXISTS` and
   `CREATE INDEX IF NOT EXISTS` above, unconditionally. This follows the
   precedent of the ungated `idx_curated_agent_log_created_at`
   (`connection.rs:726-729`). Rootless and rooted opens both get the table,
   so nothing that writes provenance depends on V22's state.
2. **Stamp:** re-read `MAX(version)`, and `INSERT OR IGNORE` 23 only when
   it is ≥ 22.

Consequences for the version pins:
- `tests/okf_migration.rs:233` and `connection.rs:889-892` stay at **21**.
  Rootless opens still stop at 21. Extend both comments to say V23 keeps
  that cap.
- `connection.rs:2790` (`migrate(Some(VaultRoots))` asserts 22) moves to
  **23**.

**Principle 4 (below)** generalizes this for V24 and later.

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

**Why the upsert is well-formed.** SQLite rejects an `INSERT … SELECT …
ON CONFLICT DO UPDATE` that tries to update the same row twice in one
statement. Two constraints make that impossible here:
- `documents.path` is `UNIQUE` (`schema.rs`, `MIGRATION_V1`), so each path
  matches at most one document.
- The `curated_proposal_sources` primary key is `(proposal_id, doc_id)`,
  so a proposal has at most one source row for that document.

Together they give at most one selected row per `(proposal_id, doc_path)`.
The unfiltered form in D7 relies on the same pair: distinct documents have
distinct paths. If either constraint is ever relaxed, this statement must
aggregate first.

**Why `&Transaction`, not `&Connection`.** Steps 1 and 2 must commit or
roll back together. A recorded deleted source for a document that still
exists is a false flag; a delete with no record loses provenance. Taking
`&Transaction` makes atomicity a compile-time requirement instead of a
convention. `Transaction` derefs to `Connection`, so the body is ordinary
rusqlite.

Proposal-level surfaces read the new table through one shared helper,
`deleted_source_paths_for_proposal`. It sits beside
`source_paths_for_proposal` for the same reason that helper is shared: the
same proposal should never report different sources on different surfaces
(PR #201 finding 10). The helper orders trigger first, then by path.

- `ProposalSummary` and `ProposalDetail` (`db/proposals.rs`) gain
  `deleted_source_paths: Vec<String>`. `PendingReviewItem`
  (`proposals_review.rs:61`, the MCP review queue) gains the matching
  `deleted_source_docs: Vec<String>`. The JSON fields are additive; update
  the frontend types in `src/lib/tauri.ts`. `ct proposals show`'s text
  output (`tools/src/cmds.rs:1590`) prints a `deleted source: <path>` line
  for each entry after the `source:` lines.
- The Review desk list (`ReviewQueueList.tsx`) and evidence panel
  (`ReviewEvidencePanel.tsx`) render markers from two independent
  conditions:
  - **Per deleted source:** a "Source deleted: `<basename>`" entry, with
    the full path as the title. It is not a button, because
    `onSourceClick` would open a file that no longer exists.
  - **Stranded:** when `source_doc_paths` is empty, an "All sources
    deleted" marker replaces the "No source documents cited." placeholder.
    The condition is the empty **live** list alone, never "live empty and
    deleted non-empty". `insert_proposal` rejects zero-source proposals
    (`proposals.rs:246`), so an empty live list always means every source
    was deleted. That includes proposals stranded before V23, which have
    no recorded names and still need the marker.
- `trigger_source_label` (`commit.rs:1032`) falls back to the deleted
  trigger's basename before `"unknown source"`.

**What approval does while stranded.** The Phase-2 strict gate skips every
`fact_add` whose evidence anchors no live chunk (`evidence_has_live_chunk`,
`commit.rs:628`, applied at `:1494`). Anchoring is by `content_hash` first.
That hash is `SHA-256(text || doc_path || position)`
(`db/chunk_hash.rs:18`), and it includes the virtual path. Evidence
therefore re-anchors only when the same bytes are ingested again **at the
same path**: a file restored in place, or a vault switched back at the same
root. A moved file never re-anchors its old proposal's evidence; D4
supersedes that proposal instead. Until evidence re-anchors:
- approving a fully stranded proposal whose items are all `fact_add`
  returns `Ok` with `skipped_unanchored` equal to the item count;
- `accepted_count` is 0, so `finalize_proposal_status` (`commit.rs:2173`)
  resolves the proposal to **`rejected`**;
- an entity shell the approval created is rolled back
  (`commit.rs:2480-2500`).

This is the correct outcome, because nothing evidenced can be committed.
The reviewer should still not be surprised by it. The "All sources deleted"
marker therefore carries the consequence in its copy: **"All sources
deleted — approving will skip facts unless a source returns at its original
path."** The
per-source marker needs no such note, because other live sources may still
anchor the facts. The stranded marker uses the error colour, not the neutral
placeholder style, because it warns about what approval will do.

This outcome is an engineering decision that protects data integrity: no
unanchored fact is ever committed. It has not been UX-tested. A reviewer who
clicks Approve and sees the proposal resolve to `rejected` may still be
surprised, whatever the copy says. Product/design should review the flow
after merge. Options include disabling Approve while the proposal is
stranded, or relabelling it.
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

The same rule heals a vault round trip. Switch away from vault A without a
backup (D7), then switch back without restoring. Re-ingesting A produces
triggers with the recorded hashes, so the stranded proposals are
superseded under the same target-match caveat.

### D5 — Rewire the five production delete functions

| Function | Change |
|---|---|
| `enqueue_vault_event` (sites 1, 2) | `let tx = conn.transaction()?; delete_document(&tx, &path_str)?; tx.commit()?;`. The function already takes `&mut Connection`, so use the checked `transaction()`, not `unchecked_transaction()`. |
| `reconcile_vault` (sites 3, 4) | call `delete_document(&tx, old_path)` on the existing `tx` (`reconcile.rs:188`) |
| `purge_brain_rows` (site 5) | call `delete_document(&tx, path)` on the existing `tx` (`reconcile.rs:256`) |
| `purge_excluded_rows` (site 6) | change the parameter to `&mut Connection` (its only caller already holds `conn_opt.as_mut()`, `lib.rs:1193-1199`), open one `conn.transaction()` around the loop, and sum the returned counts. The return type moves from `rusqlite::Result` to `anyhow::Result`; the caller only logs `Err` (`lib.rs:1202`). All-or-nothing is safe because the heal retries on the next launch. |
| `clear_vault_tables` (site 7) | see D7 |

**Grep pin.** After this change, a statement deleting from `documents`
appears only in an allowlist:
- `db/queries.rs` — `delete_document` and `clear_vault_tables`
- `db/connection.rs` — V22
- `db/okf_migration.rs` — tier purge
- `db/proposals.rs` — test module

The implementation PR's verification step checks this with a grep for the
table name, not for one statement shape:

```sh
grep -rnE "DELETE FROM documents\b" src-tauri/src tools/src
```

rev 2's pin matched only `DELETE FROM documents WHERE path` and could not
see site 7's unfiltered `DELETE FROM documents;`.

### D6 — Legacy cleanup

**L1 — Stop creating `.brain/converted`.** Remove the `create_dir_all`
calls and their error branches at:
- `lib.rs:610` (`set_vault_path`)
- `lib.rs:1544` (`switch_vault`)
- `lib.rs:3433-3436` (default vault)
- `lib.rs:3460-3464` (recovery vault)
- `vault/layout.rs:29`

Drop the matching assertions in `vault/layout.rs:49`
(`creates_all_required_directories`) and `onboard/mod.rs:306`.

Those calls were also the only guarantee that `<vault>/.brain` exists.
Production writers into it:

| Writer | Creates the directory itself? |
|---|---|
| `backup_vault_db` → `brain.db.bak` (`lib.rs:793-795`) | yes (`create_dir_all`, `:794`) |
| `write_synthesis_error` → `errors.log` (`librarian/synthesis.rs:949-951`) | yes |
| `write_error_log` → `errors.log` (`pipeline/mod.rs:444-462`) | **no**: `OpenOptions::create(true)` creates the file, not its parent, and the failure is swallowed by `if let Ok(..)` |
| `okf_migration.rs:133` → removes `.brain/proposed/*` | not applicable (one-off removal) |

**Add `std::fs::create_dir_all` of the parent to `write_error_log`**, with
its error ignored, the same pattern as `write_synthesis_error`. Without it,
L1 makes pipeline error logging a silent no-op on every vault created after
this change.

The app database is not affected: it lives at `~/.brain/brain.db` (or
`CURATED_BRAIN_DB`), resolved by `retrieval::resolve_brain_paths`, not
under the vault. Existing empty `converted/` directories in users' vaults
are left alone. They are inert and already excluded from walks, and
removing user filesystem content is not worth the risk.

**L2 — Remove `Stage::Deleting`.** No code sets it. It lived only in the
removed worker arm. Remove:
- the variant, its `as_str` arm and its `from_u8` arm in
  `pipeline/watchdog/heartbeat.rs:18,32,45`
- the `Deleting` budget arm and its stale comment
  ("Unindexed `LIKE` scan over wiki_pages (pipeline/mod.rs:229-256)") in
  `budgets.rs:34-35`
- `Deleting` from `stage_uses_shared_sqlite` in `recovery.rs:90`
- the test stage lists in `budgets.rs:88` and `recovery.rs:204`
- the doc-comment mentions in `watchdog/mod.rs:51,241`

Why removal is safe:
- **The number is never persisted.** The stage lives in an in-memory
  `AtomicU8` (`heartbeat.rs:87`), and `from_u8` is its only decoder. `8`
  then falls through to `_ => Stage::Idle` (`heartbeat.rs:46`).
- **The string is persisted but never read back.** `mirror_heartbeat`
  writes `stage.as_str()` into `pipeline_heartbeat.stage`
  (`watchdog/mod.rs:552-567`, V13). Nothing decodes that column; the only
  other references are row-count assertions in `connection.rs` tests. The
  live `wiki-status-change` event takes `ingestStage` from in-memory
  updates (`lib.rs:1090`), and the frontend compares it only against its
  own phases (`StepWatchItThink.tsx`). So a database last written with
  `'deleting'`, which no build since `2ed0acf` can do, would be inert.
- The implementation PR re-runs
  `grep -rn "pipeline_heartbeat" src-tauri/src tools/src src` to confirm
  that nothing has started reading the column since this review.

Leave the SQL comment inside `MIGRATION_V14` (`schema.rs:285`) untouched.
Migration text is historical.

**L3 — Keep the `wiki_pages` table.** Do not drop it.
`run_okf_migration` runs on every `AppDb::open_with_config`
(`connection.rs:831`) until the `okf_migrated_at` meta key is set, and it
reads `wiki_pages` (`okf_migration.rs:81,121`). An older database opened by
a new build therefore still needs the table. Dropping it safely would need
a new migration gated on the marker, which buys nothing. It stays a
documented leftover. `clear_vault_tables` keeps deleting from it.

### D7 — Vault switch without restore keeps pending proposals, stranded

> **SUPERSEDED for the vault-switch path (2026-09-15, by #213).** The
> per-vault-vs-global question this section deferred has been answered:
> per-vault. `clear_vault_tables` clears pending proposals with everything
> else. This section's reasoning is retained for the record; do not implement
> the stranding rule.

`switch_vault` with `restore_backup = false` (or with no backup present)
opens the brain database raw and calls `clear_vault_tables`
(`lib.rs:1602-1603`). That function empties `curated_relationships`,
`embeddings`, `chunks`, `documents`, `wiki_pages` and `folder_rules`.

**Decision:** pending proposals survive the switch as stranded proposals
with recorded provenance. They are not deleted.

Inside the existing transaction, before the batch, run D3 step 1 **without
the path predicate**. This records every pending proposal's sources. The
pinned statement is D3's SQL with `WHERE d.path = ?1 AND p.status =
'pending'` reduced to `WHERE p.status = 'pending'`. Share the SELECT body
between the two statements rather than building SQL with `format!`. Keep
the path-filtered form as its own statement: `(?1 IS NULL OR d.path = ?1)`
would stop SQLite using the `documents.path` index.

**Why keep them.**

1. **There is one brain for all vaults, by accident.** The database is
   global (`~/.brain/brain.db`, `lib.rs:1509`), and `clear_vault_tables`
   clears only the document layer. Everything derived survives a switch:
   `curated_entities`, `llm_wiki_entries`, `llm_wiki_edges`,
   `llm_wiki_tasks`, `llm_wiki_events` and every proposal. This was not a
   design choice. `clear_vault_tables` was written on 2026-05-11
   (`780c6a0`), when `wiki_pages` held derived knowledge, so it did clear
   knowledge. The V7 OKF migration (2026-07-05, `54c887b`) moved knowledge
   into the curated and `llm_wiki_*` tables, and the function was never
   updated. Since then a switch has carried vault A's approved knowledge
   into vault B.
2. **Consistency.** A stranded proposal targets entities that survive the
   switch. Deleting only the pending layer while approved knowledge
   persists would be a lopsided half-fix of the leak in (1). Its facts,
   though, are *not* committable while stranded (see D3, "What approval
   does while stranded"). Keeping the proposal preserves the work until its
   bytes return; it does not make it approvable today.
3. **D1's principle.** Unreviewed work is never auto-disposed. A vault
   switch is not a decision about proposals.
4. **Deletion is irreversible exactly where it matters.** The switch first
   offers "Back up and continue" (`useVaultSwitcher.ts:50-69`), which
   copies the whole database, live source links included, to
   `<old vault>/.brain/brain.db.bak`. With a backup, the proposals are safe
   whatever the live database does. Without one, deleting would destroy
   the synthesis work and its LLM spend for good. Keeping them costs a
   Dismiss click.
5. **D4 heals the round trip** (see D4).

**Cost.** Vault A's pending proposals appear in vault B's Review desk
marked "All sources deleted". The recorded `doc_path` names A's files, so
their origin is legible. A bulk-dismiss action is a small follow-up if the
noise proves real.

**Not decided here.** Whether the brain should be per-vault or global is a
product and architecture decision, tracked in **#213**. It drags in outbox
delete replication for entries (the pattern #132 established for
`wiki_forget`). If
it is decided per-vault, `clear_vault_tables` must clear entities, facts,
edges, tasks and proposals together, and this stranding rule goes with
them.

**The raw connection.** `switch_vault`'s connection skips `migrate()`.
This is safe because the running app opened the same file through
`AppDb::open_with_config` at startup, which creates the table on every open
(D2), before any switch can run. `clear_vault_tables` already assumes a
migrated file: it deletes from curated and V7 tables.

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
4. **Never stamp past a deferred migration.** `migrate()` gates on
   `MAX(version)`, so stamping any version above one that refuses to stamp
   (today: V22 on rootless opens) skips that step forever and silences its
   warning. A later migration must either make its DDL idempotent and run
   it on every open, or stamp only once `MAX(version)` has reached the
   deferred version.
5. **Inventory deletes by table, not by statement shape.** Pin every
   `DELETE FROM <table>` form, including unfiltered and `WHERE id IN`
   forms, when a table gains delete-time side effects.

## Tests

Unit tests in `db/proposals.rs` / `db/queries.rs` unless noted. Seed with
`open_in_memory` + `upsert_document` + `insert_proposal`.

**Fixtures without the curated tables.** Two suites use hand-rolled
minimal schemas, and every Remove-path test in them fails once deletes
write provenance. The table does not exist there, so D3 step 1 errors.
- `db/queue.rs` tests use `open_seeded_conn` (`enqueue_test_schema_sql`,
  `documents` only).
- `lib.rs` `excluded_row_purge_tests` build `documents` + `chunks`
  inline (`lib.rs:868-884`).

Move both to `open_in_memory()`. For `queue.rs` the helper already exists
as `open_migrated_conn` (`queue.rs:259`, currently `#[allow(dead_code)]`).
Do not add the curated DDL to the local schema strings: a partial copy of
`curated_proposals` would drift from the real one. `reconcile.rs` tests
already use `open_in_memory` and need no fixture change.

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
   without commit. The document row is still present and no deleted-source
   record exists.
5. **Re-delete upsert.** Delete a path, re-create it, re-link it to the
   same pending proposal, and delete it again. There is one row and
   `doc_hash` is refreshed.
6. **Move supersede (D4).** P1 is triggered by doc A (hash `h`). Delete A.
   Upsert doc B at a new path with hash `h`. `insert_proposal` P2 has the
   same target, triggered by B. P1 becomes `superseded`. Cover both the
   `update_entity` and `new_entity` arms.
7. **Different content does not supersede.** Same as 6, but B has hash
   `h2`, so P1 stays `pending`.
8. **Resolve on a stranded proposal (`commit.rs` tests).** On an
   all-sources-deleted proposal with one `fact_add`:
   - reject returns `Ok` and the status is `rejected`;
   - approve returns `Ok` with `skipped_unanchored == 1`, the status is
     `rejected`, and no `llm_wiki_entries` row is written;
   - **re-anchor at the same path:** re-create the document at the
     original path with a chunk whose `content_hash` is
     `compute_chunk_hash(text, original_path, 0)` (the evidence carries the
     same hash), then approve. The fact lands and the status is `approved`.
   - **no re-anchor after a move:** the same text at a different path
     produces a different hash, and approve still resolves to `rejected`.
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
13. **`clear_vault_tables` (D7, `queries.rs` `clear_vault_tables_tests`).**
    Seed two pending proposals and one approved proposal with sources.
    After the clear, both pending proposals stay `pending`, each has its
    deleted-source rows with correct paths and hashes, the approved one has
    none, and `documents` is empty. Extend the existing
    `clear_vault_tables_empties_all_vault_data` assertions rather than
    duplicating its seed.
14. **Migration placement (D2, `connection.rs` tests).**
    - A rootless open (`open_in_memory`) has the table and index, and
      `MAX(version)` stays 21.
    - The rooted `migrate(Some(VaultRoots))` test (`connection.rs:2790`)
      asserts 23.
    - **Regression for the skip-forever bug:** rewind to 21 as the existing
      rootless-V22 test does. Run `migrate(None)` and assert that 23 is
      *not* stamped. Then run `migrate(Some(roots))` and assert that the
      seeded canonical row is rewritten and the version is 23.
    - Running `migrate` twice is idempotent.
15. **Error log directory (L1, `pipeline/mod.rs` tests).** On a vault with
    no `.brain` directory, `write_error_log` creates `.brain/errors.log`
    and the line is present.
16. **Legacy.** Layout and onboard tests pass without the `converted`
    assertions; `Stage::from_u8(8) == Stage::Idle`.
17. **Shared helper parity.** For a partially stranded proposal,
    `pending_review_queue`'s `deleted_source_docs` equals
    `get_proposal_detail`'s `deleted_source_paths`.

Frontend: extend the `src/__tests__/fixtures/proposals.ts` fixtures with
`deleted_source_paths`. In `ReviewEvidencePanel.test.tsx` and
`ReviewMode.test.tsx`, assert:
- the per-source marker renders and is not a button;
- "All sources deleted" renders when `source_doc_paths` is empty, **both**
  with a non-empty `deleted_source_paths` and with an empty one (a proposal
  stranded before V23);
- the "No source documents cited." placeholder is gone.

## Non-goals

- **Approved knowledge.** Facts and entities from approved proposals
  intentionally outlive their source. A human verified them, and
  `librarian_evidence` keeps the quote and `content_hash`. Marking them
  stale (for example via the engine's `lifecycle_status`/`stale_after`)
  is a separate product decision; file it as a follow-up if wanted.
- **Per-vault vs. global brain** (D7). The cross-vault leak of approved
  knowledge through `clear_vault_tables` is recorded here and tracked in
  #213. It is not fixed.
- **Bulk dismiss** for stranded proposals (D7 cost).
- **Re-linking live sources on move.** D4 supersedes the duplicate; it does
  not re-attach the old proposal to the new document. (rev 2 claimed that
  per-chunk evidence re-resolves after a move. It does not: `content_hash`
  includes the doc path, so it re-resolves only when a file is restored at
  the same path.)
- **Rename detection in the desktop watcher / startup reconcile** (making
  them re-point rows like `reconcile_vault`). This would remove the
  Remove+Create split at its source. It is larger and orthogonal.
- **Dropping `wiki_pages`** (L3).
- **Deleting existing `.brain/converted` directories** in users' vaults
  (L1).
- **Legacy review shim** (`review_shim.rs`): `ShimReviewPage` does not gain
  the new field.
- `db/proposals.rs:818` (test-only raw delete), `db/okf_migration.rs:149`
  (migration bulk delete) and `db/connection.rs:142-163` (V22 collision
  delete).
