# Investigation — wisdom_deposit tool surface + agent write-path removal (2026-10-01)

Constitution: `INTENT.md` v2 (business rules 1–9; this work implements rule 1's
sanctioned write path and the removals it mandates). Handoff/decision context:
equational-wiki `records/sessions/2026-10-01-handoff-wisdom-architecture-implementation.md`
and `immutable-source-files/agents/memories/wisdom-deposit-file-first-intent-2026-10-01.md`.

All claims below were read from source at `main` (`34f39de`) on 2026-10-01, marked `[V]`.

## 1. Current agent-reachable write surface (what rule 1 removes)

### 1.1 Main sidecar (the `curated-thoughts` MCP server Kurt's agents attach to)

[V] `src-tauri/src/mcp_server.rs` registers 16 tools (lines 19–292). The four
to remove are:

- `curated_add_wisdom` (line 220) → `tool_dispatch::dispatch_tool_call(ctx, "curated_add_wisdom", …)` (line 230)
- `curated_update_wisdom` (line 238)
- `curated_archive_wisdom` (line 256)
- `curated_proposal_decide` (line 292)

[V] `src-tauri/src/tool_dispatch.rs` dispatch arms at lines 1587–1604 route the
four names to handler fns (`dispatch_curated_add_wisdom` at 967, update/archive
siblings at ~1001–1254 documented as sharing the "atomic mutation+audit
contract").

[V] Integration test pins the surface: `src-tauri/tests/mcp_integration.rs:537-560`
asserts `tools/list` returns **exactly** the 16 names; it also exercises
`curated_add_wisdom` round-trips (line ~563+). Both must change with the surface.

### 1.2 Second MCP binary (`tools` crate)

[V] `tools/src/bin/curated_thoughts_mcp.rs` exposes only 6 tools
(`vault_semantic_search`, `vault_related_chunks`, `curated_recall_context`,
`curated_get_wiki_entry`, `curated_search_code`, `graph_neighbors`, plus
`curated_superpowers_setup`) — lines 151–507. It never exposed the four write
tools, so there is nothing to remove here — **except** its setup-instructions
text advertises `curated_add_wisdom` to clients (line 584: "6. `curated_add_wisdom`:
Add new entries to the wisdom layer."). That instruction line must be replaced
with the new deposit tool or the setup flow teaches a removed tool.

### 1.3 `ct` CLI

[V] `tools/src/bin/ct.rs` (`Cmd` enum, lines 14–176): the CLI-side embodiment of
`curated_proposal_decide` is `ct approve` (line ~85, `approve_cmd` at 548, which
writes via `cli_common::approve_one`/`approve_all`). There is **no** CLI
add/update/archive wisdom command today (`ct wiki forget` is incident-cleanup
hard-delete; `ct heal` is reconcile soft-delete; `ct proposals review` is the
interactive Human Verification Gate).

Open design point (carried to the spec): INTENT says "MCP and CLI alike" for the
removals. `ct approve` is a human-typed interactive command today, but it is the
same approve-wisdom path agents can shell out to. The HVG `ct proposals review`
is the interactive human gate and must survive.

## 2. The file-first deposit path (substrate that already exists)

### 2.1 Note/file writing

[V] `dispatch_vault_write_note` (`tool_dispatch.rs:287`) is a thin adapter over
`crate::okf::write::write_note(vault_dir, path, frontmatter, body, if_match, allow_shrink)`
— the supplied frontmatter's `updated_at` IS the If-Match token (issue #231).
`vault_write_note` remains on the agent surface and is the generic note writer;
`wisdom_deposit` is a specialized, opinionated wrapper (fact-format pinning +
provenance stamping + ingest kick), not a replacement for it.

### 2.2 Fact file format

[V] `src-tauri/src/okf/fact_file.rs`: `build_fact_file(fact, related, profile)` /
`parse_fact_file(content)` — the canonical fact-document round-trip
(`type`, `title`, `tags`, `timestamp`, `resource`, `id`, `entity_id`,
`confidence`, …; profile `"llm-wiki/1"` vs `"llm-wiki/2"` field sets, including
`status` + provenance keys in v2). Deposited agent facts should reuse this exact
format so ingest parses them with zero new format code.

### 2.3 Where deposits land

[V] Vault layout: `immutable-source-files/agents/` is the agent-append-only area
(INTENT rule 9; the app enforces append-only semantics for agent writes via the
existing writable-subdir checks in `okf/write.rs` — `NOTE_WRITABLE_SUBDIRS`
cross-check cited in the issue-245 session record). Deposits must target
subpaths under `immutable-source-files/agents/` only; the tool must refuse other
paths.

## 3. The deposit→ingest kick (new work; handoff item C6)

[V] The sidecar has **no watcher and no ingest sweep access today**:
- `src-tauri/src/watcher/fs_watcher.rs` provides `spawn_vault_watcher(vault_path, callback)`
  (line 133) + `VaultLock` (line 222) — used by the `ct watch` daemon
  (`tools/src/bin/ct.rs` `Watch` command), not by the MCP server process.
- Ingest entry points: `pipeline::ingest_document` (`pipeline/mod.rs:479`),
  `ingest_document_with_vault_root` (:493); one-shot CLI `ct ingest --yes`.
- There is no channel from the MCP server process to a running watcher/ingest
  worker (no shared queue, no socket, no DB-backed job table that the watcher
  polls for out-of-band kicks).

Consequence (matches INTENT rule 3): v1 of `wisdom_deposit` reports
`pending: true` and `kick: "not_requested"` or `kick: "requested"` depending on
whether an ingest host could be reached; the honest no-ingest-host case is
explicit, not silently swallowed. Design options for the kick channel (decision
in the design doc): (a) spawn `ct ingest` best-effort from the deposit path when
a brain/vault lock can be acquired; (b) DB-backed pending-kick row polled by the
watcher; (c) defer kick entirely to the watcher's natural file-event (deposit is
just a file write; a running `ct watch` ingests it anyway). Option (c) is free
today but only works when a watcher happens to run; the status tool must make
the difference visible.

## 4. Supersession substrate (for `wisdom_propose_supersession`)

[V] Engine migration 13 columns exist in CT mirrors: `llm_wiki_entries` gains
`valid_from`, `valid_to`, `superseded_by`, `superseded_at` + partial indexes
(`docs/superpowers/specs/2026-09-30-llm-wiki-7-9-adoption-design.md` §2; mirrored
in `okf_ddl.rs` + the V24 mirror migration in `connection.rs`). The **write**
path that sets them (supersession application by the Active Librarian) does not
exist yet — this spec only defines the deposit file that *requests* it, per
INTENT rule 4. `db/wisdom.rs` `archive_wisdom`/`archive_wisdom_in_tx` (lines
371–380) is the existing soft-delete the Librarian will supersede.

[V] RR-4 stands: Rust-side readability of the migration-13 columns is unverified
(no non-test reader of `superseded_by` in `src-tauri/src/` today — grep clean).

## 5. Provenance / attestation (bounded here; B1 is a separate spec)

[V] No `human_attestations` table exists anywhere in the repo (grep clean) — B1
is greenfield and is NOT built here. [V] `user_stated` provenance is stamped by
the current direct-insert path (`db/wisdom.rs`, `db/commit.rs`, `entities.rs`,
`connection.rs`, `tool_dispatch.rs`). Removing the four tools stops *new*
laundered rows; the ~21-row re-ingestion cleanup and the bulk-attestation
migration are B1 scope.

## 6. What must change together (surface-coherence inventory)

1. `mcp_server.rs` — remove 4 tool registrations; add the 4 `wisdom_*` tools.
2. `tool_dispatch.rs` — remove 4 arms + handlers; add deposit/status/supersede/pending handlers.
3. `mcp_integration.rs` — the 16-name assertion and add-wisdom round-trip tests → new-surface tests.
4. `tools/src/bin/curated_thoughts_mcp.rs:584` — setup-instructions text.
5. `ct` CLI — `approve` removal/re-scope decision; new `wisdom` subcommands for CLI parity (INTENT: "agent-facing means BOTH the MCP sidecar AND the ct CLI").
6. Docs — README/tool-list references to the removed tools (sweep required; RR-3).

## 7. Open questions for the design doc

- Q1: kick channel for deposit→ingest (§3 options a/b/c, or a combination).
- Q2: exact `wisdom_deposit` parameter shape (free-form body vs pre-parsed fact fields; entity/tag handling without entity-select surfaces).
- Q3: `wisdom_deposit_status` truth source — brain lookup by doc path (`source_ref`) vs chunks-table probe vs watcher ledger.
- Q4: `ct approve` fate under "MCP and CLI alike" (remove vs gate to interactive-only).
- Q5: does `wisdom_pending` read the filesystem (list uningested deposit files) — and how it stays out of recall/injection (rule 3).
