# Per-vault brain: vault switch clears the knowledge layer atomically

**Date:** 2026-09-15
**Status:** Draft (rev 2)
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
- **rev 2** (this document) folds in a code-verification review:
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

1. **Entries via the #132 ceremony.** `SELECT (id, entity_id) FROM
   llm_wiki_entries`, then `hard_delete_entries` (`commit.rs:551-574`)
   with every row. The ceremony pushes one `OutboxOperation::Delete` row
   per entry (`table_name = "entries"`, payload `{"id"}`), deletes the
   entry rows, deletes their `librarian_evidence`
   (`delete_librarian_evidence`, `commit.rs:516-526`), and purges
   endpoint-dead edges. This is exactly what `wiki_forget` does
   (`wiki_forget.rs:26-59`); the clear is `forget` applied to the whole
   vault.
2. **Tasks, mirrored.** `SELECT (id, entity_id) FROM llm_wiki_tasks`,
   push `push_tasks_outbox` (`commit.rs:1304-1324`) `Delete` rows, then
   `DELETE FROM llm_wiki_tasks`. Tasks have no evidence table.
3. **Edge sweep.** `DELETE FROM llm_wiki_edges` unconditionally. Every
   endpoint the edges could reference is doomed, so the ceremony's
   per-entry purge plus this sweep empties the table.
4. **Straight deletes.** Per the D2 matrix: the remaining knowledge
   tables, the document layer, and the vault-scoped operational tables.
5. **Removed.** The `record_deleted_sources_sql!` call
   (`queries.rs:185`): recording sources for proposals the same
   transaction now deletes is pointless. The macro stays — single-doc
   `delete_document` (`queries.rs:156`) still uses it.

**D1a — explicit deletes over cascades, and bounded batches.**
`switch_vault`'s raw connection relies on the bundled SQLite build for
`ON DELETE CASCADE` behaviour; per the `delete_librarian_evidence`
docstring, pragma state is not guaranteed across all connections, so
nothing in this wipe may depend on it. Separately, SQLite rejects
statements beyond its variable-number limit (32,766 by default), and
`delete_librarian_evidence` currently interpolates every doomed id into
one `IN (...)` clause — a large brain would fail the whole switch into
recovery. **`delete_librarian_evidence` gains chunked deletes** (same
shape as `purge_edges_for_hard_deleted`'s `BATCH_PURGE_CHUNK` loop,
`edge_purge.rs:163-185`). This is a shared-helper change: `wiki_forget`,
`evidence_regrade`, `evidence_repair` and the lib.rs prune all get the
bounding for free. A test pins behaviour with the limit deliberately
lowered on the test connection.

## D2 — Table disposition matrix

The authoritative list. The acceptance test asserts this table row by
row. ("Clear" = explicit `DELETE FROM` inside the ceremony transaction.)

| Table | Holds | Fate | Rationale |
|---|---|---|---|
| `llm_wiki_entries` | Facts (document-derived, agent memories, manual, imports) | Clear via ceremony (D1.1) | Replicated; Delete rows required |
| `librarian_evidence` | Fact provenance | Clear via ceremony | Dies with its entries |
| `llm_wiki_tasks` | Tasks | Clear via ceremony (D1.2) | Replicated; Delete rows required |
| `llm_wiki_edges` | Edges | Clear (D1.3 sweep) | Endpoints all doomed; not replicated |
| `llm_wiki_events` | Event log | Clear | Vault knowledge |
| `llm_wiki_source_ref_index` | Source-ref TOCTOU ledger | Clear | References vault A paths/hashes |
| `llm_wiki_checkpoints` | Per-entity heal/memory checkpoints | Clear | Keys are doomed entities; stale checkpoints could skip heal work for a reused entity id |
| `curated_entities` | Entities | Clear | The leak's subject |
| `curated_proposals` (+ `curated_proposal_items`, `curated_proposal_sources`) | Proposals | Clear, items and sources deleted by name before proposals | Supersedes #211 D7 (D4) |
| `curated_proposal_deleted_sources` | V23 provenance rows | Clear | Provenance for proposals that no longer exist |
| `curated_relationships`, `embeddings`, `chunks`, `documents`, `wiki_pages`, `folder_rules` | Document layer | Clear (pre-existing behaviour, now explicit rows in the matrix) | Unchanged from today |
| `ingest_runs` | Ingest history | Clear **explicitly** | Cleared today only by cascade from `documents`; D1a forbids cascade reliance |
| `stall_strikes` | Quarantine strike ledger keyed by document path | Clear | Keys are post-V22 vault-relative paths; a same-relative-path file in the new vault must not inherit vault A's blame |
| `llm_wiki_outbox` | Replica event log | **Never truncated** | Its undrained rows are the replica cleanup (D3) |
| `llm_wiki_meta` | Package meta incl. the `okf_migrated_at` marker (`okf_migration.rs:155`) | Keep | Holds the OKF migration marker; deleting it would re-run the V7 migration over an empty-but-stamped schema |
| `llm_wiki_entity_manifests` | Ontology manifests | Keep | `tier_fact`/`tier_wisdom` manifests are (re)created at app startup; clearing them would degrade the new vault until restart. Stale working-tier manifests are overwritten by `initWorkspaceId` |
| `curated_agent_log` | Agent audit trail | Keep (pending verification) | Append-only audit; plan must verify no search/retrieval/MCP reader before merge |
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

1. **Capture (before the copy).** From the live database, read
   `(id, entity_id)` pairs for every `llm_wiki_entries` and
   `llm_wiki_tasks` row. This must happen before `std::fs::copy`
   replaces the file (any time after `release_global_db_lock` swapped in
   the stub is fine, via a short-lived read connection).
2. **Sync (after `AppDb::open_with_config` reopens the restored file,
   before the worker restart).** In one transaction on the reopened
   database: push `Delete` events for every captured pair, then push
   `Insert` events for every entry and task row now present in the
   restored file, using the existing full-payload builders
   (`wiki_fact_outbox_payload` / `wiki_task_outbox_payload`,
   `commit.rs:1189-1280`). Timestamp the sync rows with the current
   `now_ms` so the drain orders them after the restored file's stale
   pre-backup events.

Ordering argument: the restored file's own undrained events (vault A's
state at backup time) carry older `created_at` and drain first; then the
captured vault-B Deletes; then vault A's re-asserted Inserts. B's
record ids and A's are disjoint (LLM-generated ids), so the interleaving
is conflict-free and the replica's end state matches the restored file.

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
snapshot. (A backup taken *before* this fix may still contain foreign
rows; restoring it restores what it contains — that is what backups
mean.)

## Non-goals

- **A database file per vault** (approach B). Backup-restore is the
  per-vault persistence mechanism.
- **Row counts in the confirm dialog.** Static enumeration only.
- **Bulk dismiss for stranded proposals.** Still a possible #211
  follow-up for the document-deletion path; vault switches no longer
  produce stranded proposals.
- **A custom modal component for the confirm gate.** Native `message()`
  with `kind: "warning"` (D5).
- **Changing `set_vault_path`.** First-run-only; see Decision.

## Testing

- **Acceptance test (`queries.rs`, `clear_vault_tables_tests`).** Seed
  every table in the D2 matrix's "clear" rows (documents, chunks,
  embeddings, curated_relationships, wiki_pages, folder_rules,
  ingest_runs, entities, entries + evidence + edges, tasks, events,
  source_ref_index, checkpoints, proposals + items + sources, V23
  provenance rows, stall_strikes) plus pre-existing undrained `Insert`
  rows in `llm_wiki_outbox` and a `llm_wiki_meta` marker. Run
  `clear_vault_tables`, then assert **row by row against D2**: every
  clear-row table empty; every keep-row table untouched (meta marker
  intact, manifests intact, agent log intact, heartbeat present, outbox
  = one `Delete` per seeded entry and task preceded by the untouched
  pre-existing `Insert` rows, payload `{"id"}`). This is the issue's
  acceptance test.
- **Parameter-limit test.** Lower SQLite's variable-number limit on the
  test connection (`sqlite3_limit(conn, SQLITE_LIMIT_VARIABLE_NUMBER,
  …)`), seed more entries than the lowered limit, run the clear, assert
  success and complete deletion. Pins D1a's chunking.
- **Rollback test.** Mirror `forget_rollback_leaves_no_outbox_rows`:
  induce a failure mid-ceremony (edge purge is the seam), assert the
  transaction aborts and zero new outbox rows were written.
- **Restore-path sync test.** Unit-test the new sync helper directly:
  live DB with entries/tasks (some with unsent outbox events), capture,
  swap in a "restored" file containing different rows plus stale
  pre-backup outbox events, run the sync, assert outbox order = stale
  events → captured Deletes → restored Inserts, with full payloads.
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
- **Plan verification task.** Before merge, confirm by reading
  `tool_dispatch.rs` and the retrieval paths that `curated_agent_log`
  has no search/retrieval/MCP reader; if one exists, the D2 keep-row
  flips to clear and this spec is amended.

## Interaction with #211

This spec supersedes exactly one rule of #211: D7's vault-switch
stranding. Everything else #211 shipped — V23 provenance recording on
document deletion, the unanchored-approval gate, the legacy cleanups —
stands. The #211 spec file is amended (rev 4) in the same PR as the
implementation, so no reader is left following a superseded rule.
