# Per-vault brain: vault switch clears the knowledge layer atomically

**Date:** 2026-09-15
**Status:** Draft (rev 3)
**Branch:** spec/213-per-vault-brain
**Issue:** #213
**Priority:** High (approved knowledge silently leaks across vaults into
search, retrieval and MCP answers, with no source documents behind it)

## Decision

**The brain is per-vault.** A vault's brain contains only knowledge
created while that vault was active: facts and tasks the librarian
derived from its documents, **agent memories written through the MCP
server, manual entity/task edits, and bundle imports** — not just
document-derived knowledge. Switching vaults without restoring a backup
starts from a fresh brain, and re-ingesting documents does not bring
back what was manually created. The mechanism is a logical clear inside
the existing single-transaction `clear_vault_tables` — not a database
file per vault. Per-vault *persistence* already exists via the backup
mechanism: `<vault>/.brain/brain.db.bak` is a full snapshot, and the
restore path replaces the whole `brain.db` file (`lib.rs:1611-1613`).
What has been broken since 2026-07-05 is per-vault *clearing*.

`switch_vault` is the only runtime path that changes the active vault
and therefore the only clear site. (`set_vault_path`, `lib.rs:593`,
also points the config at a directory, but its only non-test caller is
the first-run setup wizard, which gates on `needsSetup` — no vault
configured. It never clears, and this spec leaves it alone.)

## Revision history

- **rev 1** proposed the knowledge-layer clear, the outbox delete
  replication, the supersession of the #211 D7 stranding rule, and the
  switch-UX confirmation gate.
- **rev 2** folds in a code-verification review:
  - **Restore-path replica gap (high).** rev 1's replica guarantee
    covered only the no-restore path. A restore replaces `brain.db`
    wholesale, so the replica kept the *outgoing* vault's knowledge and
    lost the restored vault's. D3 now specifies a pre/post-restore
    replica sync, and the replica is described accurately as an event
    log, not a converging store.
  - **Unbounded parameter count (high).** `delete_librarian_evidence`
    builds one `IN (...)` clause over all doomed ids; past SQLite's
    32,766-variable limit the whole switch fails into recovery. D1 now
    specifies chunking, with a parameter-limit test.
  - **Exhaustive table matrix.** rev 1's prose missed
    `llm_wiki_checkpoints`, `ingest_runs`, `stall_strikes` and the
    migration-staging tables, and misplaced ontology manifests (they
    live in `llm_wiki_entity_manifests`, not `llm_wiki_meta`).
    D2 replaces the prose with a row-per-table matrix.
  - **Dialog styling reality.** Tauri's native `message()` dialog cannot
    style individual buttons; the confirm gate uses `kind: "warning"`
    with an explicit label, and its exact placement is pinned (D5).
  - **Knowledge definition broadened.** Agent memories, manual edits and
    imports are in scope for both the definition and the confirm copy.
  - `set_vault_path` boundary documented; `curated_agent_log`
    non-interference claim moved from assertion to a plan verification
    task.
- **rev 3** (this document) folds in a second review wave (9 verified
  findings):
  - **`curated_agent_log` leak found (high).** The rev 2 keep-row's
    deferred reader check found a reader: the table feeds the Unified
    Timeline (`db/events.rs:98`) with no vault scoping, rendered in the
    Timeline/Activity Feed. The row flips to **Clear** and the deferred
    verification task is resolved.
  - **Restore-path sync misses undrained Deletes (high).** Records the
    outgoing vault hard-deleted (e.g. `wiki_forget`) before the switch
    leave undrained outbox Deletes that the copy destroys; capture now
    re-pushes every undrained outbox row verbatim (D3).
  - **Capture window pinned (high).** Capture must run between
    `release_global_db_lock` (`lib.rs:1602`) and `remove_sqlite_sidecars`
    (`lib.rs:1609`) — after sidecar removal a concurrent `--mcp`
    connection's un-checkpointed WAL rows are unreadable.
  - **Capture durability (high).** The captured state is persisted to a
    `brain.db.sync-capture` sidecar before the copy and consumed by a
    crash-recovery sync at startup; in-memory-only capture lost the
    outgoing vault's replica obligations on any crash or sync failure.
  - **Bulk clear (efficiency).** The rev 2 per-row `hard_delete_entries`
    loop (~3 statements per fact) is replaced by a set-based
    `INSERT … SELECT` + `DELETE` ceremony (D1.1); `hard_delete_entries`
    stays for `wiki_forget` and per-entity paths.
  - **Factual corrections.** `ingest_runs` is *not* cleared today (the
    raw switch connection never enables `PRAGMA foreign_keys`, so the
    cascade never fires); the manifests keep-row's `initWorkspaceId`
    rationale was wrong (`ifAbsent: true` seeds, never overwrites) and
    is re-argued; the backup-honesty claim is scoped against re-backup
    from a pre-fix restore.
  - **Test seeds.** The acceptance test now seeds archived (soft-deleted)
    entry and task rows to pin that the ceremony has no
    `deleted_at IS NULL` filter.

## Problem

### Where the leak comes from

The brain database is global (`~/.brain/brain.db`, resolved by
`retrieval::resolve_brain_paths`, `retrieval/mod.rs:33-75`; the path is
never vault-dependent). When `switch_vault` runs with
`restore_backup = false` — or with no backup present — it opens that
database raw and calls `clear_vault_tables` (`lib.rs:1614-1618`,
`queries.rs:183-196`). That function empties only the document layer:
`curated_relationships`, `embeddings`, `chunks`, `documents`,
`wiki_pages`, `folder_rules`.

Everything derived since the V7 OKF migration survives the switch:
`curated_entities`, `llm_wiki_entries`, `llm_wiki_edges`,
`llm_wiki_tasks`, `llm_wiki_events`, `llm_wiki_source_ref_index`,
every `curated_proposals` row (+ items and sources),
`librarian_evidence`, and the V23
`curated_proposal_deleted_sources` rows. Vault A's approved entities,
facts, edges and tasks then show up in vault B's search, retrieval and
MCP answers with no source documents behind them.

### Why this is drift, not design

`clear_vault_tables` was written 2026-05-11 (`780c6a0`), when derived
knowledge lived in `wiki_pages` — clearing it meant a fresh brain. The
V7 OKF migration (2026-07-05, `54c887b`) moved knowledge into
`curated_entities` and the `llm_wiki_*` tables, and
`clear_vault_tables` was never updated. The "fresh brain" behaviour was
lost without anyone noticing. The #211 spec's D7 documented the leak and
deferred exactly this decision to #213.

### Approaches considered

- **A. Logical per-vault (chosen).** Extend `clear_vault_tables` to clear
  the knowledge layer atomically, replicating deletes for the replicated
  tables. DML only, no migration, reuses the pattern #132 established for
  `wiki_forget`.
- **B. Physical per-vault — a DB file per vault.** True isolation, but
  invasive: `resolve_brain_paths`' env contract (`CURATED_BRAIN_DB`) is
  shared with the tools crate and headless `ct watch`; the existing
  global file needs relocating; `VaultLock` and the outbox worker
  re-pointing all key on the global path. Most of its benefit already
  exists via backup-restore. Rejected.
- **C. Global with per-row vault scoping.** Keeps all vaults' knowledge
  simultaneously; that is "global, on purpose," the opposite of the
  product intent. Rejected.

## D1 — The clear ceremony (`clear_vault_tables` v2)

Same function, same single transaction, same sole caller
(`switch_vault`'s no-restore path). The raw connection skips `migrate()`;
this is safe for the same reason D7 documented — the running app opened
the same file through `AppDb::open_with_config` at startup, so the tables
exist. No new tables, no watermark bump (stays 23): this is DML only.
Every delete is explicit; nothing relies on `ON DELETE CASCADE` (D1a).

New body, in order:

1. **Entries, set-based.** rev 2 routed the clear through the #132
   per-row ceremony (`hard_delete_entries`, `commit.rs:551-574`); that
   is the wrong shape at vault scale — roughly three `execute()` round
   trips per fact (outbox insert, entry delete, then chunked sweeps)
   inside the single transaction the user is staring at a "switching"
   screen during. A 20k-fact brain would cost 40k+ round trips. The
   whole vault is doomed, so the clear uses the set-based shape
   `clear_entity_content` already uses per entity
   (`bundle_apply.rs:760-814`), applied vault-wide:

   ```sql
   INSERT INTO llm_wiki_outbox
       (id, entity_id, table_name, record_id, operation, payload, created_at)
   SELECT 'out_' || lower(hex(randomblob(12))), entity_id, 'entries', id,
          'DELETE', json_object('id', id), ?1
   FROM llm_wiki_entries;

   DELETE FROM librarian_evidence
    WHERE entry_id IN (SELECT id FROM llm_wiki_entries);

   DELETE FROM llm_wiki_entries;
   ```

   This emits byte-identical events to `push_entries_outbox` — same
   unprefixed `table_name = "entries"`, same `{"id"}` payload — with the
   id shaped like the helper's (`out_` + 24 hex chars); a test pins the
   format match. A constant number of statements per table, zero
   per-row round trips, and no variable count to chunk (D1a's chunking
   now serves only the helper's other callers). The delete reads every
   row — **no `deleted_at IS NULL` filter**: an archived fact's Insert
   may have drained long ago, so its replica needs a Delete regardless.
2. **Tasks, mirrored.** The same shape against `llm_wiki_tasks` with
   `table_name = "tasks"` — outbox `INSERT … SELECT` then
   `DELETE FROM llm_wiki_tasks`, minus the evidence delete. Tasks have
   no evidence table. Archived tasks are included, same rule as
   entries.
3. **Edge sweep.** `DELETE FROM llm_wiki_edges` unconditionally. Every
   endpoint the edges could reference is doomed, so this sweep alone
   empties the table; with entries deleted set-based, the ceremony's
   per-entry edge purge (`purge_edges_for_hard_deleted`) is not invoked
   on this path at all.
4. **Straight deletes.** Per the D2 matrix: the remaining knowledge
   tables, the document layer, and the vault-scoped operational tables.
5. **Removed.** The `record_deleted_sources_sql!` call
   (`queries.rs:185`): recording sources for proposals the same
   transaction now deletes is pointless. The macro stays — single-doc
   `delete_document` (`queries.rs:156`) still uses it.

`hard_delete_entries` itself is untouched and keeps every caller it has
today — `wiki_forget` and the per-entity paths operate at a scale where
the per-row ceremony's consistency value exceeds its round-trip cost.
Only the vault-wide clear goes set-based.

**D1a — explicit deletes over cascades, and bounded batches.**
`switch_vault`'s raw connection relies on the bundled SQLite build for
`ON DELETE CASCADE` behaviour; per the `delete_librarian_evidence`
docstring, pragma state is not guaranteed across all connections, so
nothing in this wipe may depend on it. (The raw switch connection
in particular never sets `PRAGMA foreign_keys` — see the `ingest_runs`
row in D2.) Separately, SQLite rejects statements beyond its
variable-number limit (32,766 by default), and
`delete_librarian_evidence` currently interpolates every doomed id into
one `IN (...)` clause — a large single-entity forget would fail. **The
clear no longer routes through it** (D1.1 is set-based and has no
variable count), but **`delete_librarian_evidence` gains chunked
deletes** anyway (same shape as `purge_edges_for_hard_deleted`'s
`BATCH_PURGE_CHUNK` loop, `edge_purge.rs:163-185`): `wiki_forget`,
`evidence_regrade`, `evidence_repair` and the lib.rs prune all get the
bounding for free. A test pins behaviour with the limit deliberately
lowered on the test connection.

## D2 — Table disposition matrix

The authoritative list. The acceptance test asserts this table row by
row. ("Clear" = explicit `DELETE FROM` inside the ceremony transaction.)

| Table | Holds | Fate | Rationale |
|---|---|---|---|
| `llm_wiki_entries` | Facts (document-derived, agent memories, manual, imports) | Clear via ceremony (D1.1) | Replicated; Delete rows required |
| `librarian_evidence` | Fact provenance | Clear (D1.1 bulk subquery) | Dies with its entries |
| `llm_wiki_tasks` | Tasks | Clear via ceremony (D1.2) | Replicated; Delete rows required |
| `llm_wiki_edges` | Edges | Clear (D1.3 sweep) | Endpoints all doomed; not replicated |
| `llm_wiki_events` | Event log | Clear | Vault knowledge |
| `llm_wiki_source_ref_index` | Source-ref TOCTOU ledger | Clear | References vault A paths/hashes |
| `llm_wiki_checkpoints` | Per-entity heal/memory checkpoints | Clear | Keys are doomed entities; stale checkpoints could skip heal work for a reused entity id |
| `curated_entities` | Entities | Clear | The leak's subject |
| `curated_proposals` (+ `curated_proposal_items`, `curated_proposal_sources`) | Proposals | Clear, items and sources deleted by name before proposals | Supersedes #211 D7 (D4) |
| `curated_proposal_deleted_sources` | V23 provenance rows | Clear | Provenance for proposals that no longer exist |
| `curated_relationships`, `embeddings`, `chunks`, `documents`, `wiki_pages`, `folder_rules` | Document layer | Clear (pre-existing behaviour, now explicit rows in the matrix) | Unchanged from today |
| `ingest_runs` | Ingest history | Clear **explicitly** | **Not cleared today.** rev 2 claimed a cascade from `documents`; in fact the raw switch connection (`lib.rs:1615`) never sets `PRAGMA foreign_keys` (SQLite ships it off; this codebase enables it explicitly per connection elsewhere), so the `ON DELETE CASCADE` never fires and every no-restore switch already orphans these rows with dangling doc_ids. The explicit clear fixes that defect |
| `stall_strikes` | Quarantine strike ledger keyed by document path | Clear | Keys are post-V22 vault-relative paths; a same-relative-path file in the new vault must not inherit vault A's blame |
| `llm_wiki_outbox` | Replica event log | **Never truncated** | Its undrained rows are the replica cleanup (D3) |
| `llm_wiki_meta` | Package meta incl. the `okf_migrated_at` marker (`okf_migration.rs:155`) | Keep | Holds the OKF migration marker; deleting it would re-run the V7 migration over an empty-but-stamped schema |
| `llm_wiki_entity_manifests` | Ontology manifests | Keep | `initWorkspaceId` seeds via `setOntologyManifests(…, { ifAbsent: true })` (`ontologySeed.ts:48-51`) — a seed-if-absent that by definition never overwrites, so rev 2's "overwritten by `initWorkspaceId`" rationale was wrong. Keep stands on the real argument: `tier_fact`/`tier_wisdom` are fixed global ids whose manifests encode the global `config.json` ontology selection, not vault state, and working-tier ids are per-vault-path hashes — no row can be vault-stale in the first place |
| `curated_agent_log` | Agent audit trail | **Clear** | Rev 2 kept it pending a reader check; the check found one: `db/events.rs:98` unions this table into the Unified Timeline with only an `entity_id` filter — no vault scoping — surfaced by `timeline_api::list_events_cmd` (`lib.rs:3328`) and rendered by TimelineMode/ActivityFeedPanel. After a switch, the new vault's Activity Feed would show the old vault's agent operations (client, tool, summary). Not replicated (entries/tasks only, D3), so no outbox rows needed |
| `schema_version`, `pipeline_heartbeat`, `pipeline_stalls`, `system_strikes` | Migration watermark, watchdog state | Keep | Not vault knowledge; heartbeat/stalls are process-liveness post-mortem state; `system_strikes` is single-row system-wide |
| `documents_v15`, `wiki_pages_new`, `entries_backup`, `llm_wiki_entries_rebuild` | Migration staging | N/A | Transient rewrite tables; absent in steady state. The acceptance test seeds none of them |

## D3 — Replica semantics (both switch paths)

Only `llm_wiki_entries` and `llm_wiki_tasks` are replicated
(`push_entries_outbox` `commit.rs:1282-1302`, `push_tasks_outbox`
`commit.rs:1304-1324`; edges are not replicated — see
`archiving_pushes_no_edge_outbox_rows`, `commit.rs:3349-3369`). The
Postgres sink is an **append-only event log** (`wiki_outbox_events`,
`outbox/postgres.rs`), so the guarantee is per-record — every replicated
record's final state is represented by a correctly ordered event — not
whole-store convergence. Events for unknown record ids are no-ops on the
replica; duplicate Inserts are idempotent re-assertions.

**No-restore path (unchanged by this spec's mechanics, made correct by
D1).** The outbox lives in the same file being cleared and is never
truncated: one `Delete` event per doomed entry and task, pushed inside
the ceremony transaction. The worker is stopped before the switch
closure and restarted against the same `db_path` afterwards
(`lib.rs:1588-1600`, `1681-1688`), so the events drain in
`created_at ASC, rowid ASC` order (`outbox/mod.rs:89`) once the worker
returns; any pre-existing undrained vault-A events stay ahead of the
deletes. The rollback property carries over from #132: pushes happen
inside the transaction, so a mid-ceremony failure aborts with zero new
outbox rows (`forget_rollback_leaves_no_outbox_rows`,
`wiki_forget.rs:357-377`).

**Restore path (new: the replica sync).** A restore is
`std::fs::copy(backup, brain.db)` (`lib.rs:1611-1612`). The outbox lives
inside that file, so without extra work the replica sees *nothing*: the
outgoing vault's records are never deleted from the replica, the
restored vault's records are never re-inserted, and any of the outgoing
vault's events still undrained at copy time are destroyed with the old
file. `switch_vault`'s restore branch therefore gains:

1. **Capture (before the copy).** From the live database, via a
   short-lived read connection, read:
   - `(id, entity_id)` for every `llm_wiki_entries` and
     `llm_wiki_tasks` row — all of them, archived rows included; and
   - every row currently in `llm_wiki_outbox`. The worker is stopped
     before the switch closure (`lib.rs:1588-1600`), so anything still
     in the table is undrained by construction. This matters beyond
     re-asserting inserts: an undrained `Delete` for a record the
     outgoing vault already hard-deleted (e.g. a `wiki_forget` run
     while the replica was unreachable) has no surviving table row to
     capture from — if the copy destroyed that outbox row, the replica
     would serve the "forgotten" record forever. That is the #132
     privacy defect, reintroduced through the restore path; re-pushing
     the undrained rows verbatim closes it.

   **Timing is pinned:** the capture connection opens after
   `release_global_db_lock` has swapped in the stub (`lib.rs:1602`) and
   **before** `remove_sqlite_sidecars` (`lib.rs:1609`) deletes `-wal`.
   "Any time after the stub swap" is not good enough: a concurrent
   process — the headless `--mcp` server holds its own brain.db
   connection (the V21 concurrent-open case) — prevents the implicit
   WAL checkpoint on close, so rows written since the last checkpoint
   exist only in the `-wal` that sidecar removal is about to delete. A
   capture after that point reads a stale main file and silently misses
   records.

   The capture is **persisted to a sidecar file next to the database**
   (`brain.db.sync-capture`, JSON) and fsynced **before**
   `std::fs::copy` runs. Holding it only in memory means a crash or a
   failed sync after the copy permanently destroys the outgoing vault's
   replica obligations — its rows no longer exist anywhere, and the
   divergence is silent.

2. **Sync (after `AppDb::open_with_config` reopens the restored file,
   before the worker restart).** In one transaction on the reopened
   database, in this order:
   - re-push every captured undrained outbox row verbatim (original
     `created_at` included, preserving intra-vault event order);
   - push `Delete` events for every captured `(id, entity_id)` pair;
   - push `Insert` events for every entry and task row now present in
     the restored file, using the existing full-payload builders
     (`wiki_fact_outbox_payload` / `wiki_task_outbox_payload`,
     `commit.rs:1189-1280`), timestamped with the current `now_ms`.

   On commit, delete the sidecar. If the transaction fails (disk full,
   busy lock from a lingering connection), the error propagates into
   the existing switch recovery **with the sidecar left in place**, and
   startup recovery finishes the job:

**Crash recovery.** If the process dies between the copy and the sync
commit — or the sync fails — the sidecar survives with the capture in
it. Wherever the app reopens the brain (a check beside the switch path
itself), a present `brain.db.sync-capture` triggers the same sync
transaction, then deletes the sidecar. Re-running is safe: duplicated
re-pushes are harmless because the replica already treats duplicate
Inserts as idempotent re-assertions and Deletes of unknown ids as
no-ops, and the drain order still converges to the same end state. The
sidecar is bounded — at most one captured row per replicated record —
not a growth path.

Ordering argument: the restored file's own undrained events (vault A's
state at backup time) carry the oldest `created_at` and drain first;
then B's re-pushed undrained events in their original relative order —
all newer than the backup, since B was active only after the restore
that created them; then, at the switch's `now_ms`, B's captured
Deletes and A's re-asserted Inserts. B's record ids and A's are
disjoint (LLM-generated ids), so the interleaving is conflict-free and
the replica's end state matches the restored file, with B's pending
deletes — including forgets of already-deleted records — preserved.

This sync runs regardless of whether a replica is configured — the rows
sit in the outbox and drain only if the worker is running.

## D4 — Superseding #211 D7

The #211 spec (rev 3) kept pending proposals across a no-restore switch
as stranded proposals with recorded provenance, explicitly deferring
per-vault-vs-global to #213. That decision is now made, and it is
**per-vault**, so:

- Pending proposals are cleared together with everything else. D7's
  stranding rule for the vault-switch path is **superseded**.
- The #211 spec gets a rev-4 amendment recording the supersession and
  pointing at this spec. D3/D5 of that spec — provenance recording on
  single-document deletion, the V23 table, the grep pin — are
  **unaffected** and remain in force; they serve the document-deletion
  path.
- `curated_proposal_deleted_sources` keeps its writer on the
  document-deletion paths and gains a clear on the switch path (D2):
  provenance rows whose proposals are gone are dead weight.
- Code: the D7 record call is removed from `clear_vault_tables`, its
  docstring is rewritten (it currently documents the stranding rule and
  names #213 as the tracker), and the
  `clear_vault_tables_empties_all_vault_data` test flips from "pending
  proposals survive" to "nothing survives".

**On #211's D1 principle** ("unreviewed work is never auto-disposed"):
clearing pending proposals does not violate it, because the destruction
is no longer a silent side effect. The switch UI makes it an explicit,
confirmed decision (D5), and the backup path preserves the work. A vault
switch *with* a confirmed no-restore choice *is* a decision about
proposals.

## D5 — Switch UX (`useVaultSwitcher.ts`)

Once per-vault is real, "Continue without backup" is genuinely
destructive: it permanently destroys approved knowledge — including
agent memories and manual edits that re-ingest cannot rebuild — and the
LLM spend that produced it. The hook uses Tauri's native `message()`
dialogs, which cannot style individual buttons, so severity is conveyed
by `kind` and explicit button labels rather than per-button styling.

- **Backup dialog copy** stops underclaiming. The body no longer says
  "your indexed data"; it says the backup saves this vault's index **and
  knowledge base** (entities, facts, tasks, agent memories) to `<path>`
  so it can be restored after switching back. Backup-first remains the
  highlighted path.
- **Destructive-confirm gate.** When the user picks "Continue without
  backup", a second `message()` dialog opens **immediately — before the
  restore dialog, and regardless of whether the target vault has a
  backup** (a restore also overwrites the current brain with no fresh
  backup of it). Title: **"Destroy this vault's knowledge?"**; body
  stating plainly that continuing permanently deletes all approved
  entities, facts, edges, tasks, pending proposals, **agent memories and
  manual edits** for this vault, and cannot be undone. `kind: "warning"`;
  buttons "Go back" (returns to the switch flow, nothing switched) and
  "Switch without backup" — the explicit label carries the destructive
  weight a red fill would in an in-app modal. A custom in-app modal with
  true per-button styling is deliberately not introduced here; the native
  dialog matches every other gate in this flow.
- **Restore dialog copy** gains one clause: restoring brings back the
  knowledge base, not just the document index. The existing
  "documents changed since the backup will be re-indexed" caveat stays.

**Backup honesty.** Today `backup_vault_db` (`lib.rs:755-804`) snapshots
the whole global file, silently including other vaults' leaked knowledge.
After this change the live database only ever holds the current vault's
knowledge, so every backup taken from then on is a pure single-vault
snapshot — **with one caveat**: a vault whose live brain was seeded by
restoring a pre-fix backup carries the old backup's foreign rows, and a
fresh backup taken from that state inherits them, re-propagating the
contamination to every future restore of the new `.bak`. Those rows are
cleared only when that vault next switches without restore. (A backup
taken *before* this fix may still contain foreign rows; restoring it
restores what it contains — that is what backups mean.)

## Non-goals

- **A database file per vault** (approach B). Backup-restore is the
  per-vault persistence mechanism.
- **Row counts in the confirm dialog.** Static enumeration only.
- **Bulk dismiss for stranded proposals.** Still a possible #211
  follow-up for the document-deletion path; vault switches no longer
  produce stranded proposals.
- **A custom modal component for the confirm gate.** Native `message()`
  with `kind: "warning"` (D5).
- **Routing `wiki_forget` and per-entity deletes through the bulk
  shape.** They stay on the per-row `hard_delete_entries` ceremony;
  only the vault-wide clear goes set-based (D1).
- **Replicating tables beyond `llm_wiki_entries`/`llm_wiki_tasks`.**
  The cleared `curated_agent_log`, edges, events and the rest have no
  replica and need no outbox rows (D2/D3).
- **Changing `set_vault_path`.** First-run-only; see Decision.

## Testing

- **Acceptance test (`queries.rs`, `clear_vault_tables_tests`).** Seed
  every table in the D2 matrix's "clear" rows (documents, chunks,
  embeddings, curated_relationships, wiki_pages, folder_rules,
  ingest_runs, entities, entries + evidence + edges, tasks, events,
  source_ref_index, checkpoints, proposals + items + sources, V23
  provenance rows, stall_strikes, agent log) — **including one archived
  (soft-deleted) entry and one archived task** — plus pre-existing
  undrained `Insert` rows in `llm_wiki_outbox` and a `llm_wiki_meta`
  marker. Run `clear_vault_tables`, then assert **row by row against
  D2**: every clear-row table empty; every keep-row table untouched
  (meta marker intact, manifests intact, heartbeat present, outbox =
  one `Delete` per seeded entry and task **including the archived
  ones**, preceded by the untouched pre-existing `Insert` rows, payload
  `{"id"}`, id shaped `out_` + 24 hex chars). The archived seeds pin
  that the bulk ceremony has no `deleted_at IS NULL` filter — a
  refactor adding the codebase's common live-rows-only filter must fail
  here, or archived facts would keep flowing from the replica after a
  no-restore switch. This is the issue's acceptance test.
- **Parameter-limit test.** Lower SQLite's variable-number limit on the
  test connection (`sqlite3_limit(conn, SQLITE_LIMIT_VARIABLE_NUMBER,
  …)`), seed more entries than the lowered limit, run `wiki_forget` —
  the path that still routes through `delete_librarian_evidence` — and
  assert success and complete deletion. The clear itself is set-based
  (D1.1) and has no variable count; this pins D1a's chunking for the
  helper's remaining callers.
- **Rollback test.** Mirror `forget_rollback_leaves_no_outbox_rows`:
  induce a failure mid-ceremony (a deliberately failing statement after
  the outbox inserts, e.g. against the documents delete), assert the
  transaction aborts and zero new outbox rows were written.
- **Restore-path sync test.** Unit-test the new sync helper directly:
  live DB with entries/tasks **and** undrained outbox rows — including
  a `Delete` for a record already hard-deleted (the
  forget-then-switch case) — capture, swap in a "restored" file
  containing different rows plus stale pre-backup outbox events, run
  the sync, assert outbox order = stale events → re-pushed undrained
  events (original `created_at`) → captured Deletes → restored Inserts,
  with full payloads.
- **Sidecar crash-recovery test.** Simulate the crash: capture written,
  copy done, sync never run. Reopen with the sidecar present, run the
  recovery path, assert the sync events land, the sidecar is deleted,
  and a second recovery run is a no-op (idempotent).
- **Idempotency.** `clear_vault_tables` twice in a row succeeds with
  identical end state.
- **Empty brain.** Clear on a fresh, migrated database succeeds and
  writes no outbox rows.
- **Frontend (vitest, `useVaultSwitcher` tests).** New backup copy; the
  confirm gate fires on "Continue without backup" (with and without a
  target backup present), before the restore dialog; "Go back" returns
  without switching; "Switch without backup" proceeds with
  `kind: "warning"`; restore copy; and the gate does **not** fire when
  the user took a backup.

(The rev 2 "plan verification task" — confirming `curated_agent_log`
has no search/retrieval/MCP reader before merge — is resolved by rev 3:
the reader exists (`db/events.rs:98`, Unified Timeline), so the D2 row
is Clear and no verification remains.)

## Interaction with #211

This spec supersedes exactly one rule of #211: D7's vault-switch
stranding. Everything else #211 shipped — V23 provenance recording on
document deletion, the unanchored-approval gate, the legacy cleanups —
stands. The #211 spec file is amended (rev 4) in the same PR as the
implementation, so no reader is left following a superseded rule.
