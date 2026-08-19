# Spec: OKF-Native Backend Migration (Phase 3)

**Date:** 2026-07-05
**Status:** Implemented (2026-07-06, v1.10.0); Phase 5 additions: `curated_agent_log` writers + 90-day pruning, `healed` maintenance events (all implemented 2026-07-08). Deferred: maintenance `incremental_vacuum` (§4), `wiki_pages` drop in V8 (§1).
**Branch:** `main` (shipped v1.10.0)
**Related:** `2026-07-05-ux-vision-okf-native-design.md` (phases 4–5 block on this), `../../../clanker/docs/superpowers/specs/2026-07-03-okf-export-design.md`, `../../../clanker/docs/superpowers/specs/2026-07-04-okf-import-support-design.md`, `../../../expo-llm-wiki/docs/okf-profile.md` (normative llm-wiki OKF profile v1 — postdates this spec; see the addendum at the bottom)

## Problem

The librarian synthesizes whole markdown wiki pages: chunks in, one page of markdown out, stored as a `wiki_pages` metadata row plus a file in `.brain/proposed/`, promoted to `<vault>/wiki/*.md` on approval. The approved UX vision makes OKF concepts (entities, facts, tasks, edges, events) first-class UI objects with fact-level review, which the page-shaped pipeline cannot express: there is no per-fact accept/reject, no evidence linkage, no entity identity, and no event log capture.

## Verified Current State (2026-07-05)

**The OKF data model already exists in `brain.db` — this migration is not greenfield.** Two data models share the same SQLite file:

1. **Rust-owned** (`src-tauri/src/db/schema.rs`, migration runner in `db/connection.rs:31-79`, current head V6): `documents`, `chunks` (+V4 line metadata, +V5 tier `entity_id`), `embeddings`, `wiki_pages` (proposal metadata), `folder_rules`, `curated_relationships` (code call-graph).
2. **Package-owned** — `@equationalapplications/core-llm-wiki@4.9.0` (the same package Clanker uses) creates and writes `llm_wiki_entries` (facts, with confidence/source_type/soft-delete/embedding columns), `llm_wiki_tasks`, `llm_wiki_events`, `llm_wiki_edges` (with `UNIQUE(entity_id, source_id, target_id, edge_type)`), `llm_wiki_entity_manifests`, `llm_wiki_checkpoints`, `llm_wiki_meta`, and `outbox` from TypeScript via the generic `wiki_exec`/`wiki_run` passthrough commands (`lib.rs:1452-1500`). Rust only reads these tables (`wiki_graph.rs`, header comment declares read-only).

Other verified load-bearing facts:

- Librarian prompt (`librarian/mod.rs:303`) asks for "a concise wiki page in markdown"; output is one whole page. Synthesis input is chunks for one source path plus up to 5 cross-document structural neighbors from `curated_relationships` (`build_structural_context`, `librarian/mod.rs:69-163`) — evidence already spans documents today.
- Approval handler (`approve_wiki_page`, `lib.rs:1970-2018`) writes the markdown file and flips the `wiki_pages` status. The current `ReviewModal` is approve/reject only — the `content` parameter is round-tripped unedited from `get_proposed_content` (confirmed by the UX vision spec's own problem statement).
- Chunk retrieval routes by `chunks.entity_id` tier (`tier_fact` / `tier_wisdom` / `tier_working::<hash>`, derived in `pipeline/mod.rs:440-476`), not by `documents.status`.
- The outbox worker (`src-tauri/src/outbox/`) drains the package-written `outbox` table to Postgres. Any Rust-authored `llm_wiki_*` write must produce matching outbox rows or replication and cloud sync silently miss them.
- Clanker's OKF bundle format is `formatOkfBundle`/`parseOkfBundle` in core-llm-wiki. Known gaps documented in the Clanker specs and inherited here: edges not natively serialized (call-site augmentation), events not idempotent (deduped by `(event_type, summary, UTC-day)`), cross-entity id collision guard forces id-remap on clone. *(Superseded later the same day — profile v1 fixed the first two at the format level; see the addendum.)*

## Decisions Made During Brainstorming

- **Ownership:** Rust writes the `llm_wiki_*` tables directly. Rust migrations adopt the package DDL verbatim; compat is guarded by a pinned package version, a startup `PRAGMA table_info` check, and a build-time DDL-diff test that fails CI on drift. The TS package keeps working against the same tables.
- **Proposal storage:** new Rust-owned staging tables (`curated_proposals` / `curated_proposal_items` / `curated_proposal_sources`), replacing `wiki_pages`. Live knowledge and pending proposals never share a table.
- **Migration path:** existing approved wiki pages become entities with the full page body as the entity summary. No retroactive LLM fact extraction. Pending page proposals are dropped and their source documents re-queued for fact-level synthesis.
- **Entity resolution:** candidate entities (embedding similarity + name match) are injected into the synthesis prompt with raw ids; the LLM picks an existing id or proposes a new entity. Review catches mismatches; no silent auto-merge.
- **Event log:** domain mutations go to `llm_wiki_events` (OKF-portable). MCP/agent access goes to a new local-only `curated_agent_log` table — never exported or synced, prunable.
- **Cutover:** hard cutover behind schema V7. No feature flag, no dual-write. Legacy Tauri review commands become shims over the new tables so the existing `ReviewModal` keeps working until the phase-2 editorial desk ships.

## 1. Architecture & Schema

`brain.db` stays a single file and gains schema version **V7**. Three table families afterward:

1. **Ingest layer (unchanged):** `documents`, `chunks`, `embeddings`, `curated_relationships`, `folder_rules`.
2. **OKF knowledge layer (package DDL, now Rust-written):** all `llm_wiki_*` tables plus `outbox`. V7 executes the same `CREATE TABLE IF NOT EXISTS` statements the package runs — idempotent whether or not the TS layer already created them.
3. **New Rust-owned tables** (the existing `curated_` prefix avoids collision with future package tables):

```sql
CREATE TABLE IF NOT EXISTS curated_entities (
    id TEXT PRIMARY KEY,                 -- referenced by llm_wiki_*.entity_id (FK by convention)
    name TEXT NOT NULL,
    entity_type TEXT NOT NULL DEFAULT 'concept',   -- person | project | concept | ...
    summary TEXT NOT NULL DEFAULT '',    -- editable prose; maps to entities/{id}/index.md in an OKF bundle
    summary_embedding BLOB,              -- nullable; used for candidate retrieval; lazily backfilled
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    deleted_at INTEGER                   -- soft delete, matching package convention
);

CREATE TABLE IF NOT EXISTS curated_proposals (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK(kind IN ('new_entity','update_entity')),
    entity_id TEXT,                      -- set for update_entity; NULL until approval for new_entity
    proposed_name TEXT,                  -- new_entity only
    proposed_type TEXT,                  -- new_entity only
    reasoning TEXT,                      -- librarian's "why" for the Review evidence panel
    model TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK(status IN ('pending','approved','rejected','partial','superseded')),
    reject_reason TEXT,                  -- optional, stored only (vision: not wired to tuning)
    created_at INTEGER NOT NULL,
    resolved_at INTEGER
);

CREATE TABLE IF NOT EXISTS curated_proposal_items (
    id TEXT PRIMARY KEY,
    proposal_id TEXT NOT NULL REFERENCES curated_proposals(id) ON DELETE CASCADE,
    item_type TEXT NOT NULL CHECK(item_type IN
        ('fact_add','fact_update','fact_archive','edge_add','task_add','summary_update')),
    target_id TEXT,                      -- existing llm_wiki row id for update/archive
    payload TEXT NOT NULL,               -- JSON: {body, tags, confidence} | edge triple | task description
    evidence TEXT NOT NULL DEFAULT '[]', -- JSON: [{chunk_id, quote, start_line, end_line}]
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK(status IN ('pending','accepted','rejected')),
    edited_payload TEXT                  -- reviewer's inline edit; wins over payload at commit
);

CREATE TABLE IF NOT EXISTS curated_proposal_sources (
    proposal_id TEXT NOT NULL REFERENCES curated_proposals(id) ON DELETE CASCADE,
    doc_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    role TEXT NOT NULL DEFAULT 'evidence' CHECK(role IN ('trigger','evidence')),
    PRIMARY KEY (proposal_id, doc_id)
);

CREATE TABLE IF NOT EXISTS curated_agent_log (   -- local-only: never exported, never synced
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    client TEXT NOT NULL,                -- MCP client name / "clanker-bridge"
    tool TEXT NOT NULL,
    operation TEXT NOT NULL CHECK(operation IN ('read','write')),
    entity_id TEXT,
    summary TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);
```

Indexes: `curated_proposals(status)`, `curated_proposal_items(proposal_id)`, `curated_proposal_sources(doc_id)`, `curated_agent_log(created_at)`.

Design notes:

- **`curated_proposal_sources` exists because synthesis evidence already spans documents** (structural neighbors), and the UX vision's queue cards show "source document names" (plural) while the Library panel needs the reverse lookup "pending proposals citing this document." A junction table gives both an indexed path; the per-item `evidence` JSON keeps chunk-level quote/line detail. `role='trigger'` marks the document whose ingest fired the synthesis (a trigger row wins over an evidence row for the same document).
- **FK by convention:** `llm_wiki_*.entity_id` now means `curated_entities.id`. No SQL foreign key is declared — the package owns those tables' DDL. The chunk-tier ids in `chunks.entity_id` (`tier_fact` etc.) are a different namespace in a different column and are untouched.
- **Schema-compat guard:** core-llm-wiki version pinned in `package.json`; startup verifies `llm_wiki_*` columns via `PRAGMA table_info` against expected shapes; a build-time test diffs Rust DDL constants against the DDL extracted from `node_modules` (see Testing). If the package migrates the schema ahead of Rust, startup fails loudly with both versions named and the DB untouched — never limp along against an unknown schema.
- **`wiki_pages`** is retired from active use at V7, kept physically one version as rollback aid, dropped in V8.

## 2. Librarian Synthesis Rework

Trigger unchanged: synchronous per-ingest run (`pipeline/mod.rs:181`). Folder modes remap: `index` = skip (unchanged); `summarize` = `summary_update` proposals only; `synthesize` = full facts/edges/tasks.

Per run:

1. **Load context** — chunks for the source document plus structural neighbors (existing logic survives; tier labeling stays).
2. **Candidate entity retrieval (new):** mean of the document's chunk embeddings compared against `curated_entities.summary_embedding` (cosine, existing little-endian f32 blob format), unioned with case-insensitive name substring matches. Top ~8 candidates injected into the prompt as: raw entity id, name, type, summary snippet, and up to 5 existing facts **with their raw `llm_wiki_entries.id` values**. The prompt instructs the model to copy those ids verbatim into `target_id` for update/archive operations. Entities whose `summary_embedding` is NULL (pre-backfill) participate via name match only.
3. **Prompt** replaces "write a wiki page" with structured JSON output. Chunks are numbered `[C1]..[Cn]` in the context. Output schema:

```json
{
  "proposals": [{
    "target": {"existing_id": "..."} | {"new": {"name": "...", "type": "person|project|concept|..."}},
    "reasoning": "one-paragraph why",
    "summary_update": "full replacement prose, or null",
    "facts": [{"op": "add|update|archive", "target_id": null, "body": "...",
               "tags": [], "confidence": "certain|inferred", "evidence": ["C3", "C7"]}],
    "edges": [{"source": REF, "target": REF, "edge_type": "..."}],
    "tasks": [{"description": "...", "evidence": ["C2"]}]
  }]
}
```

   Edge endpoint `REF` is a tagged reference: `"self"` (the proposal's own target — the common case), `{"existing_id": "..."}`, or `{"new_name": "..."}` for an entity proposed in a sibling proposal of the same run. `new_name` refs are resolved at **commit** time by exact-name lookup in `curated_entities`; an unresolved ref auto-rejects that item with a recorded reason — a dangling id is never written. V1 accepts that this makes cross-proposal edges sensitive to approval order.

   The old prompt's conflict-resolution directive translates: a contradiction with an existing fact becomes a proposed `fact_update` (or `fact_archive` + `fact_add`) with reasoning, replacing the "Architectural Inconsistency wisdom entry" hack.
4. **Rust validation (strict serde):** the raw LLM response is first stripped of markdown code-fence wrappers (```` ```json … ``` ````) — models frequently fence JSON output even when told not to, and `serde_json` fails on the wrapper, not the payload. Then: unknown chunk refs dropped; `target_id` must be in the set of fact ids actually injected into this prompt (rejects hallucinated ids even when they happen to exist in the DB); `existing_id` must be an injected candidate. Malformed JSON gets one retry with the parser error appended to the prompt; a second failure writes an error event and `errors.log` entry, and no proposal. Valid evidence refs are resolved to `{chunk_id, quote, start_line, end_line}` from chunk rows (V4 line metadata).
5. **Write transaction:** proposal + items + sources atomically. Supersede rule: a new pending proposal for the same `(entity_id, trigger doc)` marks the older pending one `superseded` — the queue never shows two stale generations of the same thing.
6. **`auto_approve` folder rule:** runs the same commit path as manual approval immediately (one commit implementation) and logs the `approved` event marked as auto.

## 3. Approval / Commit Path & Tauri Surface

Partial approval is the native shape — one command, per-item decisions, one transaction:

```rust
resolve_proposal(proposal_id, decisions: Vec<ItemDecision>, reject_reason: Option<String>)
    -> CommitResult
// ItemDecision { item_id, decision: Accept | Reject, edited_payload: Option<Json> }
// CommitResult { committed: Vec<CommittedRef>, conflicts: Vec<ItemId>, dropped_edges: Vec<ItemId> }
```

The whole commit runs inside a single `BEGIN IMMEDIATE` transaction (rusqlite `TransactionBehavior::Immediate`) — the write lock is acquired upfront, and a mid-commit failure rolls back everything: no orphaned outbox rows, no partial graph edges.

Order of operations:

1. `kind = new_entity` with at least one accepted item → create the `curated_entities` row now (id generated at commit, not synthesis — no wasted ids or embeddings on rejected proposals), backfill `proposal.entity_id`.
2. Per accepted item (`edited_payload` wins over `payload`):
   - `summary_update` → conflict check: if `entity.updated_at > proposal.created_at` (a human edited the summary while the proposal was pending), the manual path skips the item and reports it in `conflicts` for the UI to re-confirm; the auto-approve path skips it silently. Otherwise update summary + `updated_at` and re-embed `summary_embedding`.
   - `fact_add` → INSERT into `llm_wiki_entries`; embedding computed at commit; `source_ref` = proposal id + evidence JSON (chunk-level provenance for the power layer); `source_type` = `user_confirmed` on manual approval, `librarian_inferred` on auto-approve (matches the package enum's semantics).
   - `fact_update` → UPDATE body/tags/`updated_at`; re-embed.
   - `fact_archive` → set `deleted_at` (soft delete, package convention).
   - `edge_add` → resolve endpoint refs (see §2), then `INSERT OR IGNORE` — the tuple UNIQUE constraint makes it idempotent. Unresolved `new_name` → item auto-rejected, listed in `dropped_edges`.
   - `task_add` → INSERT into `llm_wiki_tasks`.
3. **An outbox row is written for every `llm_wiki_*` mutation**, mirroring the package's CDC format exactly (table_name, record_id, operation, payload JSON). Without this, Postgres replication and cloud sync silently miss Rust-authored writes. Format is compat-tested like the DDL.
4. One aggregate `llm_wiki_events` row per proposal resolution ("Approved: 3 facts added to *Project X* from *notes.pdf*") — matches OKF date-stamp granularity and the Clanker import dedup tuple. Per-fact provenance lives in `source_ref`, not event spam.
5. Item statuses updated; proposal status → `approved` / `rejected` / `partial`; `resolved_at` set; `reject_reason` stored.

**New read surface:** `list_proposals(filter)` → queue cards (target name, kind, item counts, source document names via the junction table, age); `get_proposal_detail(id)` → items + hydrated evidence quotes + reasoning.

**Deleted-source degradation:** deleting a source document while a proposal citing it is pending cascades away the `curated_proposal_sources` row and the underlying chunks, but the proposal survives and its evidence JSON keeps the quote text inline. `get_proposal_detail` marks evidence entries whose `chunk_id` no longer resolves as source-deleted (quote still shown, deep-link disabled). Proposals are deliberately **not** auto-rejected on source deletion — the proposed facts may still be valid; the reviewer decides.

**New entity surface (phase 4 consumes):** `list_entities(sort, filter)`, `get_entity(id)` (entity + facts + open tasks + recent events), `create_entity`, `update_entity_summary` (re-embeds), `archive_entity`.

**Legacy shims (until the phase-2 editorial desk ships, then deleted):** `get_review_queue`, `approve_wiki_page`, `reject_wiki_page`, `get_proposed_content` are reimplemented over the proposals tables. Approve maps to all-accept `resolve_proposal`; the preview renders proposal items as text. The shim **ignores `approve_wiki_page`'s `content` parameter** — the current `ReviewModal` has no edit affordance (it round-trips `get_proposed_content` unedited), and Rust cannot deterministically parse edited markdown back into discrete items, so the parameter is dropped rather than half-honored. Documented so no stale caller expects otherwise.

## 4. V7 Migration Mechanics

Two parts: DDL in the existing migration runner; data conversion as a guarded startup step (file I/O does not belong in `migrate()`).

**V7 DDL** (runner in `connection.rs`, gated on `MAX(version) < 7`): package DDL verbatim for `llm_wiki_*` + `outbox` (idempotent), plus the five `curated_*` tables and indexes from §1.

**Data conversion** (runs after DB open when the vault path is known; idempotency guard: `llm_wiki_meta` key `okf_migrated_at`):

1. **Auto-backup first.** The existing `backup_vault_db` machinery runs before conversion. `wiki/*.md` files are never deleted — double rollback safety.
2. **Approved pages → entities.** Per `wiki_pages` row with `status='approved'`: read `<vault>/wiki/<path>`; entity name = first H1 if present, else filename stem; type = `concept`; summary = full markdown body. Entity id is **deterministic** (derived from the wiki page path hash) so a re-run after mid-conversion failure cannot duplicate. `summary_embedding` = NULL at migration (the embedder may not be up at startup); backfilled lazily by the existing re-embed machinery; candidate retrieval falls back to name match until then. File missing → entity with empty summary + warning event.
3. **Migration events.** One `llm_wiki_events` row per migrated entity ("Migrated from wiki page *X*") — the Timeline shows provenance honestly.
4. **Pending proposals dropped.** `wiki_pages` rows in `pending_review` → `orphaned`; `.brain/proposed/*.md` deleted (derived artifacts); their `source_doc_ids` get `documents.status='pending'` so the pipeline re-synthesizes them fact-level. Old page-shaped pending work becomes new fact-shaped pending work automatically.
5. **`wiki/` tier retired and purged.** The watcher drops `wiki/` from ingest roots. The conversion loop runs `DELETE FROM documents WHERE tier='wiki'` — the existing `ON DELETE CASCADE` chain (documents → chunks → embeddings, and `curated_relationships` endpoints) purges the old `tier_wisdom` chunks and their vectors atomically. This is required, not optional: chunk retrieval routes by `chunks.entity_id` tier, not `documents.status`, so merely orphaning the rows would leave stale page-chunks competing against fresh fact embeddings in semantic search. Disk files stay as archive. Post-migration, wisdom retrieval comes only from `llm_wiki_entries` embeddings via the existing `wiki_search` path; document retrieval is unchanged.
6. **`wiki_pages`** stays read-only through V7; dropped in V8.
7. **Space reclamation.** The chunk purge in step 5 can free a large fraction of `brain.db` (whole wiki tier's chunks + vectors); SQLite returns freed pages to its freelist but never shrinks the file on its own. Conversion ends with a one-time `VACUUM` (outside any transaction). The ongoing `curated_agent_log` pruning trickle just reuses freelist pages and needs no vacuuming; the existing heal/prune maintenance run gains a `PRAGMA incremental_vacuum`-or-`VACUUM` step as belt-and-braces.

Failure mid-conversion: guard key unwritten → next startup re-runs; step 2's deterministic ids and steps 4–5's idempotent statements make the re-run safe.

## 5. Event Capture Points

`llm_wiki_events` writers (Rust, each inside its owning transaction):

| Point | event_type | Where |
|---|---|---|
| Document ingested + synthesized | `synthesized` | pipeline, after proposal write |
| Proposal resolved (aggregate sentence) | `approved` / `rejected` | `resolve_proposal` commit |
| Auto-approve | `approved` (marked auto) | same commit path |
| Heal / prune / re-embed runs | `healed` | existing maintenance commands |
| V7 migration | `imported` | conversion step |
| OKF bundle import/export (phase 6) | `imported` / `exported` | reserved, not built here |

`curated_agent_log` writers: `tool_dispatch.rs` (cloud-bridge tool calls, client = `clanker-bridge`) and `mcp_server.rs` (local MCP, client name from the MCP handshake). One row per call: tool, operation (`read` for all current tools; `write` reserved), entity_id when derivable from arguments, summary. Pruning: startup deletes rows older than 90 days (constant, not configurable in v1). The Timeline UI (phase 5) unions this table with `llm_wiki_events`.

## 6. Testing & Error Handling

**Testing:**

- **DDL compat test** (the loud-drift guard): extract the `CREATE TABLE` statements from `node_modules/@equationalapplications/core-llm-wiki/dist/index.js` by regex, normalize whitespace, diff against the Rust DDL constants. CI fails on any package bump that changes the schema.
- **Outbox compat test:** a Rust-written outbox row for each mutation type asserted field-by-field against package-format fixtures captured from real TS writes.
- **Migration tests:** fixture vault (approved pages including missing-file and no-H1 cases, pending proposals, wiki-tier chunks) → run conversion → assert entities, events, re-queues, and chunk purge; run twice → assert idempotence (deterministic entity ids, no duplicates).
- **Commit path:** partial-approval matrix (accept/reject/edited mixes), the summary conflict-timestamp path, `new_name` edge resolution including the unresolved-drop case, `INSERT OR IGNORE` edge dedupe, and transaction rollback on an induced mid-commit failure asserting no orphaned outbox rows (the `BEGIN IMMEDIATE` guarantee).
- **Synthesis validation:** malformed-JSON retry, hallucinated `target_id` rejection, unknown chunk-ref dropping. LLM calls mocked per the existing inference test pattern.
- **Shim tests:** old command signatures against the new store; the `content` parameter ignored on approve.

**Error handling:**

- Synthesis failures → `errors.log` + error event (existing pattern); a proposal is never half-written (single transaction).
- Conversion failure → guard key unwritten, retried next startup; the pre-conversion backup exists.
- Startup compat-check failure (package schema ahead of Rust) → hard error surfaced to the UI naming both versions, DB untouched.

## Non-Goals (this spec)

- OKF bundle export/import for Curated Thoughts (phase 6; this spec only guarantees the data model those flows need, including `curated_entities.summary` → `entities/{id}/index.md`).
- Ontology/`entity_manifests` editing (table adopted as-is; nothing writes it yet).
- Reject-reason feedback into librarian tuning (stored only, per the vision).
- Auto-approve rule UI changes (existing `folder_rules.auto_approve` semantics carried over).
- Retrieval-ranking changes beyond removing the `tier_wisdom` chunk source.
- Summary-edit merge UX (v1 ships the timestamp conflict check; a real merge flow is v2).

### Implemented deferrals (v1.10.0)

| Item | Spec ref | Status |
|---|---|---|
| `curated_agent_log` writers (`tool_dispatch`, `mcp_server`) | §5 | Implemented (phase 5) |
| Agent log 90-day startup pruning | §5 | Implemented (phase 5) |
| `healed` events on heal/prune/re-embed maintenance | §5 | Implemented (phase 5) |
| Proposal resolution events (taxonomy: `approved`/`rejected` per v1.10 spec) | §5 | Implemented; v1.10–v1.15 wrote `action`/`observation` for resolutions; fixed in phase 5 with startup data migration keyed on summary prefix |
| Maintenance `incremental_vacuum` | §4 | One-time `VACUUM` at migration only |
| `wiki_pages` table drop | §1 | Deferred to V8 per design |
| OKF v0.2 first-class adoption (wire format flip, `okf_sources`/`okf_verified`/`lifecycle_status` columns populated on import) | §1–§3 | Adopted (Phase 7, v1.17.0) |

## Known Limitations (documented, accepted for v1)

- Cross-proposal `new_name` edges are approval-order-sensitive; unresolved refs are dropped with a recorded reason rather than deferred.
- A human summary edit racing a pending `summary_update` surfaces as a re-confirm (manual) or silent skip (auto-approve) — not a merge.
- Candidate retrieval quality degrades to name-match-only until `summary_embedding` backfill completes after migration.

## Open Questions Deferred

<!-- resolved in the addendum below -->

## Addendum: llm-wiki OKF Profile v1 (added 2026-07-05, after approval)

Later the same day, the format this spec interoperates with was standardized as the **llm-wiki OKF profile v1** (`expo-llm-wiki/docs/okf-profile.md`, normative, RFC-2119; design record `expo-llm-wiki/docs/superpowers/specs/2026-07-05-okf-profile-design.md`), implemented in core-llm-wiki **4.18.x** and extended by the summary-persistence spec targeting **4.19.0** (`expo-llm-wiki/docs/superpowers/specs/2026-07-05-okf-summary-persistence-design.md`). Nothing here changes PR-24 scope; these notes bind the **phase 6** (bundle import/export) plan:

1. **The canonical format doc now exists.** Phase 6 implements against the profile doc, not against Clanker's specs or package internals. Profile §9 requires non-TypeScript implementations to vendor **checksummed copies** of the conformance fixtures (`expo-llm-wiki/packages/okf/fixtures/golden-v1/` and `legacy-profile-0/`); the Rust export/import ships with those vendored and drift-checked, same pattern as this spec's DDL guard.
2. **Two "Verified Current State" gaps are fixed at the format level as of 4.18.x:** `formatOkfBundle` natively emits the `## Related` edge section, `profile: llm-wiki/1` root marker, and per-event `<!-- id: evt_x -->` comments; `parseOkfBundle` strips `## Related` from stored bodies and preserves event ids. The `(event_type, summary, UTC-day)` tuple survives only as the profile-0 **fallback** (profile §7) — phase 6's consumer must be id-first with tuple fallback, mirroring Clanker's adoption spec (`clanker/docs/superpowers/specs/2026-07-05-okf-profile-v1-adoption-design.md`).
3. **Id-remap-on-clone: yes, needed.** Profile §10 settles the deferred question: remapping is application behavior, and the format guarantees raw ids in frontmatter precisely so importers can remap. Phase 6's "import as new entity" path remaps fact/task ids (and, per the Clanker adoption spec's finding, **event ids too** — profile-1 bundles carry stable `evt_*` ids that would otherwise collide).
4. **Summary write-path decision (new, must be made in the phase 6 plan).** core-llm-wiki ≥ 4.19.0's `importDump` persists entity summaries into `llm_wiki_meta` under `entity_summary:{entity_id}` — a table this migration adopts, in a database where the TS `wiki_exec` passthrough still operates. Curated Thoughts' summary home is `curated_entities.summary`. Two homes = silent divergence if any TS-package import path ever runs here. Phase 6 must either (a) implement import natively in Rust writing `curated_entities.summary` and never route bundle import through the TS package (recommended — consistent with this spec's Rust-writes-directly ownership decision), or (b) define an explicit sync between the meta key and `curated_entities.summary`. Splitting reads/writes across both without a rule is the one prohibited outcome.
5. **Version pin distance.** This spec verified against core-llm-wiki **4.9.0**; profile v1 ships in **4.18.x**/**4.19.0**. The startup compat check and DDL-diff CI guard make the bump deliberate work, as designed — the 4.9→4.19 DDL delta must be reviewed when phase 6 bumps the pin. (The 4.19.0 summary change adds **no DDL** — it reuses the existing `{prefix}meta` table — so it does not by itself trip the guard.)

## References

- `src-tauri/src/db/schema.rs`, `db/connection.rs:31-79` — current schema + migration runner
- `src-tauri/src/librarian/mod.rs:185-370` — current synthesis (prompt at `:303`)
- `src-tauri/src/lib.rs:1970-2018` — current approval handler; `:1452-1500` — `wiki_exec` passthrough family
- `src-tauri/src/pipeline/mod.rs:181-189, 440-476` — synthesis trigger, tier routing
- `src-tauri/src/wiki_graph.rs`, `src-tauri/src/outbox/` — package-table readers, outbox drain
- `node_modules/@equationalapplications/core-llm-wiki/dist/index.js` — package DDL (source of truth for the compat test)
- Clanker specs `2026-07-03-okf-export-design.md`, `2026-07-04-okf-import-support-design.md` — bundle format, event dedup tuple, collision guard
