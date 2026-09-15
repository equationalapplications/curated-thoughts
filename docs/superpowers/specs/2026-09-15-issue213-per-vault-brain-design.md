# Per-vault brain: vault switch clears the knowledge layer atomically

**Date:** 2026-09-15
**Status:** Draft (rev 1)
**Branch:** spec/213-per-vault-brain
**Issue:** #213
**Priority:** High (approved knowledge silently leaks across vaults into
search, retrieval and MCP answers, with no source documents behind it)

## Decision

**The brain is per-vault.** A vault's brain contains only knowledge
derived from that vault's documents. Switching vaults without restoring a
backup starts from a fresh brain. The mechanism is a logical clear inside
the existing single-transaction `clear_vault_tables` — not a database
file per vault. Per-vault *persistence* already exists via the backup
mechanism: `<vault>/.brain/brain.db.bak` is a full snapshot, and the
restore path replaces the whole `brain.db` file (`lib.rs:1611-1613`).
What has been broken since 2026-07-05 is per-vault *clearing*.

## Revision history

- **rev 1** (this document). Decides per-vault over global; specifies the
  knowledge-layer clear, the outbox delete replication, the supersession
  of the #211 D7 stranding rule, and the switch-UX confirmation gate.

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
4. **Straight deletes.** `llm_wiki_events`,
   `llm_wiki_source_ref_index`, `curated_proposal_items`,
   `curated_proposal_sources`, `curated_proposal_deleted_sources`,
   `curated_proposals`, `curated_entities`. The proposal-family deletes
   are **explicit, not cascade-reliant** (D1a).
5. **Document layer (unchanged).** `curated_relationships`,
   `embeddings`, `chunks`, `documents`, `wiki_pages`, `folder_rules`.
6. **Removed.** The `record_deleted_sources_sql!` call
   (`queries.rs:185`): recording sources for proposals the same
   transaction now deletes is pointless. The macro stays — single-doc
   `delete_document` (`queries.rs:156`) still uses it.

**D1a — explicit deletes over cascades.** `switch_vault`'s raw
connection relies on the bundled SQLite build for `ON DELETE CASCADE`
behaviour. For this batch wipe nothing may depend on a pragma default:
items, sources and V23 provenance rows are deleted by name before
`curated_proposals`, so the wipe is correct even if a future environment
swaps in a system SQLite compiled without foreign keys enabled.

### Deliberately untouched

- **`llm_wiki_outbox`.** Never truncated here. Its undrained `Delete`
  rows *are* the replica cleanup. The outbox worker is stopped before the
  switch closure and restarted against the same `db_path` afterwards
  (`lib.rs:1588-1600`, `1681-1688`), so the rows drain in order once the
  worker returns; any pre-existing undrained `Insert`/`Update` rows for
  vault A's records stay ahead of the deletes, which is the correct
  replication order.
- **`schema_version`, meta, watchdog/heartbeat tables.** The migration
  and liveness state is not vault knowledge. (D7 already documents why
  skipping `migrate()` here is safe.)
- **`llm_wiki_meta`.** May hold package-level non-knowledge state;
  blanket-deleting it risks destroying that. Stale per-vault ontology
  manifests are overwritten by `initWorkspaceId`/`setupWiki` on the new
  vault.
- **`curated_agent_log`.** Append-only audit trail, never surfaced by
  search, retrieval or MCP. It survives by design so the history of what
  agents did is not rewritten by a vault switch.

## D2 — Replica semantics

Only `llm_wiki_entries` and `llm_wiki_tasks` are replicated
(`push_entries_outbox` `commit.rs:1282-1302`, `push_tasks_outbox`
`commit.rs:1304-1324`; edges are not replicated — see
`archiving_pushes_no_edge_outbox_rows`, `commit.rs:3349-3369`).
Entities, proposals, events, evidence and the source-ref index have no
replica, so clearing them needs no outbox rows. The Postgres replica
converges to "this vault's knowledge = empty" purely by draining the
outbox the switch produced — the same invariant #132 established for
`wiki_forget`, including the rollback property: the outbox pushes happen
inside the transaction, so a mid-ceremony failure aborts with zero outbox
rows (`forget_rollback_leaves_no_outbox_rows`,
`wiki_forget.rs:357-377`).

## D3 — Superseding #211 D7

The #211 spec (rev 3) kept pending proposals across a no-restore switch
as stranded proposals with recorded provenance, explicitly deferring
per-vault-vs-global to #213. That decision is now made, and it is
**per-vault**, so:

- Pending proposals are cleared together with everything else. D7's
  stranding rule for the vault-switch path is **superseded**.
- The #211 spec gets a rev-4 amendment recording the supersession and
  pointing at this spec. D3/D5 of that spec — provenance recording on
  single-document deletion, the V23 table, the grep pin — are **unaffected**
  and remain in force; they serve the document-deletion path.
- `curated_proposal_deleted_sources` keeps its writer on the
  document-deletion paths and gains a clear on the switch path (D1 step
  4): provenance rows whose proposals are gone are dead weight.
- Code: the D7 record call is removed from `clear_vault_tables`, its
  docstring is rewritten (it currently documents the stranding rule and
  names #213 as the tracker), and the
  `clear_vault_tables_empties_all_vault_data` test flips from "pending
  proposals survive" to "nothing survives".

**On #211's D1 principle** ("unreviewed work is never auto-disposed"):
clearing pending proposals does not violate it, because the destruction
is no longer a silent side effect. The switch UI makes it an explicit,
confirmed decision (D4), and the backup path preserves the work. A vault
switch *with* a confirmed no-restore choice *is* a decision about
proposals.

## D4 — Switch UX (`useVaultSwitcher.ts`)

Once per-vault is real, "Continue without backup" is genuinely
destructive: it permanently destroys approved knowledge and the LLM spend
that produced it.

- **Backup dialog copy** stops underclaiming. The body no longer says
  "your indexed data"; it says the backup saves this vault's index **and
  knowledge base** (entities, facts, tasks) to `<path>` so it can be
  restored after switching back. Backup-first remains the highlighted
  path.
- **Destructive-confirm gate.** Choosing "Continue without backup" opens
  a second dialog before the switch proceeds: title **"Destroy this
  vault's knowledge?"**; body stating plainly that continuing permanently
  deletes all approved entities, facts, edges, tasks, and pending
  proposals for this vault, and cannot be undone. Buttons: "Go back"
  (returns to the switch flow) and "Switch without backup" — styled with
  the app's destructive/danger button variant to match the severity of
  the copy. Static copy, no row counts: the enumeration is the honesty,
  and counts would add an IPC command for decoration.
- **Restore dialog copy** gains one clause: restoring brings back the
  knowledge base, not just the document index. The existing
  "documents changed since the backup will be re-indexed" caveat stays.

**Backup honesty.** Today `backup_vault_db` (`lib.rs:755-804`) snapshots
the whole global file, silently including other vaults' leaked knowledge.
After this change the live database only ever holds the current vault's
knowledge, so every backup taken from then on is a pure single-vault
snapshot. (A backup taken *before* this fix may still contain foreign
rows; restoring it restores what it contains — that is what backups mean.)

## Non-goals

- **A database file per vault** (approach B). Backup-restore is the
  per-vault persistence mechanism.
- **Row counts in the confirm dialog.** Static enumeration only.
- **Bulk dismiss for stranded proposals.** Still a possible #211
  follow-up for the document-deletion path; vault switches no longer
  produce stranded proposals.
- **Clearing `curated_agent_log` or `llm_wiki_meta`** (D1: deliberately
  untouched).

## Testing

- **Acceptance test (`queries.rs`, `clear_vault_tables_tests`).** Seed
  every table: documents, chunks, embeddings, curated_relationships,
  wiki_pages, folder_rules, curated_entities, llm_wiki_entries (with
  evidence rows and edges), llm_wiki_tasks, llm_wiki_events,
  llm_wiki_source_ref_index, curated_proposals (+ items and sources,
  mixed pending/approved), curated_proposal_deleted_sources, and
  pre-existing undrained `Insert` rows in `llm_wiki_outbox`. Run
  `clear_vault_tables`, then assert the exact population of every table:
  knowledge layer empty; document layer empty; outbox contains exactly
  one `Delete` row per seeded entry and task, each preceded by the
  untouched pre-existing `Insert` rows (drain order preserved), with
  payload `{"id"}`; `schema_version` and meta untouched. This is the
  issue's acceptance test — it pins exactly which tables are empty or
  populated, plus the outbox rows.
- **Rollback test.** Mirror `forget_rollback_leaves_no_outbox_rows`:
  induce a failure mid-ceremony (edge purge is the seam), assert the
  transaction aborts and zero outbox rows were written.
- **Idempotency.** `clear_vault_tables` twice in a row succeeds with
  identical end state.
- **Empty brain.** Clear on a fresh, migrated database succeeds and
  writes no outbox rows.
- **Frontend (vitest, `useVaultSwitcher` tests).** New backup copy; the
  confirm gate appears only on the no-backup path; "Go back" returns to
  the switch flow without switching; "Switch without backup" proceeds
  with the destructive variant styling; restore copy.
- **Supersession flip.** The renamed
  `clear_vault_tables_empties_all_vault_data` assertions become
  nothing-survives (covered by the acceptance test) and the #211 spec's
  D7 section carries the rev-4 supersession note.

## Interaction with #211

This spec supersedes exactly one rule of #211: D7's vault-switch
stranding. Everything else #211 shipped — V23 provenance recording on
document deletion, the unanchored-approval gate, the legacy cleanups —
stands. The #211 spec file is amended (rev 4) in the same PR as the
implementation, so no reader is left following a superseded rule.
