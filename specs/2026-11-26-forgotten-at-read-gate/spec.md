# Spec: read-time gate for forgotten facts (`forgotten_at`) — #378

**Status:** Implemented (owner-approved 2026-11-26 21:32 ET — OQ1 survival guard + OQ4 OR predicate folded; SDD lane complete 2026-11-27: 6 tasks + fix cycle, controller-verified gates, independent review + delta ALL FINDINGS RESOLVED)
**Date:** 2026-11-26
**Ruling issue:** equationalapplications/curated-thoughts#378
**All file:line evidence against `origin/main` @ `bbe15dc2` (3.16.4).**

## Owner ruling (verbatim, 2026-11-26)

> Owner ruling (2026-11-26, <@821113777088626690>): **honor `forgotten_at` at read time on the CT side, spec-first, no engine-bump wait.** Requirements the spec must satisfy:
>
> 1. **Single shared predicate, not scattered clauses** — route the check through the shared entry-eligibility helper (e.g. `entry_eligibility_where` / shared visibility helper) rather than duplicating filters per tool, and pin completeness across recall, `wisdom_match`, two-stage, FTS/vector search, and the wiki tools.
> 2. **Table location, indexing, top-k starvation** — confirm whether `forgotten_at` lives on `llm_wiki_entries` or needs a `derived_events` join; ensure it's indexed; post-filtering of forgotten rows must not starve vector/FTS top-k retrieval of live results.
> 3. **Audit the UI read path** — check whether the Tauri frontend reads wiki entries via Rust commands or directly via TS/core-llm-wiki SQL; if TS queries entries directly, disclose or cover that surface so UI and CLI/MCP never disagree on whether a fact is forgotten.
>
> If core-llm-wiki has the same blind spot for direct TS consumers, file a separate non-blocking upstream note (expo-llm-wiki) rather than gating CT's fix on an engine bump. Issue stays open pending the spec-first lane.

## Owner ruling #2 — survival guard (verbatim, 2026-11-26, resolves OQ1)

> 3 (Option 1 with a multi-event survival guard): Hide entries at read time, but do not hide an entry just because one linked event was forgotten if other live events or stated provenance still back it:
> 1. All-backing-events rule: An event-backed entry is hidden iff it has linked derived_events and zero linked events remain with forgotten_at IS NULL (plus never hide structural entity/hub containers or stated-ledger rows via event-forget). If the link is strictly 1:1 in the schema, prove and cite that in the spec; if many-to-one is possible, enforce the NOT EXISTS (live event) guard so forgetting one session event never collateral-deletes a shared rule or entity.
> 2. Verify UI parity: Since engine forget() leaves the entry visible in core-llm-wiki, explicitly verify in the spec whether the Tauri UI reads entries via Rust commands (inheriting this gate) or via TS core-llm-wiki queries directly—so UI and CLI/MCP genuinely cannot agree to disagree.

## Owner ruling #3 — structural-container predicate (verbatim, 2026-11-26, resolves OQ4)

> OQ4 — 3 (the OR predicate of both candidates as a future-proof guard). Live 0/1,458 verification clears the over-protection concern today, preserves the D5 commitment, and keeps the guard in place if container rows ever land in llm_wiki_entries. Proceed with that in the #378 (PR #395) spec.

**Verification basis (2026-11-26, live `brain.db` census + `origin/main` code audit, read-only):** there is no `llm_wiki_entities` table (only `llm_wiki_entity_manifests` ontology config); all 1458 live entries are leaf statements (id prefix `fact…`, `entity_id` = shared `ent_<hash>`, `tier` = `wisdom`, `area` NULL); `id = entity_id` matches **0/1458** rows and no ingest path mints such rows; `id`/`entity_id` in the reserved tier namespace matches **0/1458** rows (the namespace exists only in the manifests table). Neither signal over-protects a leaf row today; the OR guard is deliberately vacuous now and fails safe if container rows ever land in `llm_wiki_entries`.

## Problem

`llm_wiki_events.forgotten_at` (engine `forget({ eventId })`) is mirrored in the Rust replica — column declared (`src-tauri/src/db/okf_ddl.rs:143`), migration shim (`src-tauri/src/db/connection.rs:1464`), partial index mirror (`src-tauri/src/db/okf_ddl.rs:385-387`) — but **zero Rust readers filter it**. The only other Rust references are schema-guard enumerations and tests (`src-tauri/src/db/schema_guard.rs:111`, `src-tauri/src/db/connection.rs:8062`). Once any host calls the engine's `forget({ eventId })`, forgotten events (and, if entry-level gating is adopted, their linked facts) resurface in every Rust read path: recall, wisdom match, two-stage retrieval, wiki search/context, entity manifests, and the unified timeline. INTENT §12 ("The Rust side follows the engine's words", `INTENT.md:226`) requires the replica to honor the engine's forget verb.

## Evidence

### E1. Where `forgotten_at` lives — events table, NOT entries

- `llm_wiki_events.forgotten_at INTEGER` is the only column: `src-tauri/src/db/okf_ddl.rs:135-145` (declared at `:143`).
- `llm_wiki_entries` has **no** `forgotten_at` column: full DDL at `src-tauri/src/db/okf_ddl.rs:14-52`.
- `entries.derived_events TEXT` (JSON, `okf_ddl.rs:51`) and `llm_wiki_events.related_entry_id TEXT` (`okf_ddl.rs:140`) are the two available join keys. `related_entry_id` is the engine-populated, indexed-by-insert path; `derived_events` is an opaque JSON blob written by CT's entry pipeline — the join should use `related_entry_id`.
- **Who writes it:** nothing in `src-tauri/src` sets `forgotten_at` (grep over `src-tauri/src` for `SET forgotten_at` / `forgotten_at =` returns only DDL/migration/test lines). The engine writes it: core-llm-wiki 9.18.0 `dist/index.js:5547` (`UPDATE … SET forgotten_at = ? WHERE … AND forgotten_at IS NULL`), invoked by `forget({ eventId })` at `:12582`. CT is therefore a pure **read-side** consumer of the flag.
- **Engine semantics:** `forget({ eventId })` tombstones an **event**, not its entry. The engine filters `forgotten_at IS NULL` on all its own event readers (exactly 11 occurrences in the 9.18.0 dist `index.js:5343-5533`) and never hides the linked entry. Engine `forget({ sourceRef })` / `clearAll` hard-delete entries through a different path (CT's mirror: `src-tauri/src/db/wiki_forget.rs:25`).

### E2. Indexing status

- Existing: `llm_wiki_events_forgotten_idx ON llm_wiki_events(entity_id, forgotten_at) WHERE forgotten_at IS NOT NULL` (`okf_ddl.rs:387`, schema-guarded at `connection.rs:8192`). Serves forget-maintenance/count queries per entity.
- Existing: `llm_wiki_events_entity_idx ON llm_wiki_events(entity_id, created_at DESC)` (`okf_ddl.rs:146`). Serves the event-reader queries (all event reads are entity-scoped), so a `AND forgotten_at IS NULL` predicate on those queries needs **no new index**.
- **Missing:** no index on `related_entry_id` at all. The proposed entries-side anti-join (E4 design) needs a partial index `llm_wiki_events(related_entry_id) WHERE forgotten_at IS NOT NULL AND related_entry_id IS NOT NULL` to stay cheap. Mirrors the existing forgotten_idx pattern in `okf_ddl.rs:377-387`.
- **Degradation:** the column only exists on engine ≥ v19 (`connection.rs:1456`, `:1464`). Any new predicate arm must degrade on shapes without the column, following the `temporal_columns` / `ddl_compat` conditional-fragment pattern (`wisdom_match.rs:596-605`).

### E3. Read-path call-site completeness map (current state)

Shared predicate: `entry_eligibility_where` (`src-tauri/src/wisdom_match.rs:577-598`) gates `deleted_at`, NULL title/body, `embedding_blob`, read-scheme, and temporal validity — **no forgotten gate**. Its consumers:

| Read path | SQL builder | Uses shared predicate? | Forgotten gate today |
|---|---|---|---|
| `wisdom_match` vector leg (`gated_entries`) | `wisdom_match.rs:638-662` | ✅ (`:662`) | ❌ |
| Shared id-set (`eligible_entry_ids`) | `wisdom_match.rs:601-609` | ✅ (`:609`) | ❌ |
| Two-stage candidate set (`candidate_set`) | `two_stage.rs:87-119` | ✅ (`:96`) | ❌ |
| Two-stage reachable/complement | `two_stage.rs:121-208` | ✅ (`:132`) | ❌ |
| Recall lexical wiki leg (`rank_wiki_entries`) | `tool_dispatch.rs:450-456` | ❌ builds own SQL (`deleted_at IS NULL` + LIKE only) | ❌ |
| Recall dispatcher (`curated_recall_context` → `rank_wiki_entries`) | `tool_dispatch.rs:676` | via lexical leg above | ❌ |
| `curated_get_wiki_entry` | `tool_dispatch.rs:722-750` | ❌ own SQL (`deleted_at IS NULL` only) | ❌ |
| `wiki_search` / `wiki_search_in_scheme` (vector search) | `wiki_graph.rs:350-397` | ❌ own SQL (`:391-397`) | ❌ |
| `wiki_context` / `wiki_traverse_graph` fact leg | `wiki_graph.rs:2266-2303` (delegates to `wiki_search_in_scheme`) | ❌ | ❌ |
| Entry existence/lookups in graph walk | `wiki_graph.rs:469`, `:1145`, `:2227` | ❌ | ❌ |
| Entity manifest events (`load_events`) | `db/entities.rs:652-667` (event table read) | n/a (events) | ❌ |
| Unified Timeline (`list_events`) | `db/events.rs:39`, events UNION arm `:64-82` | n/a (events) | ❌ |
| Vault code-chunk search (`search/mod.rs`) | `search/mod.rs:153-277` | n/a — reads `chunks`/`embeddings`, never wiki tables | n/a |

Notes: there is **no FTS table over wiki entries** — the "FTS leg" of recall is the LIKE-based `rank_wiki_entries` (`tool_dispatch.rs:450-456`); grep for `fts5`/`MATCH` over wiki tables returns nothing. Every gap above is a WHERE-clause gap, not a post-fetch filter — which is what makes the pushed-into-SQL design (below) a drop-in.

### E4. Cardinality: `related_entry_id` → entry is **many-to-one** (survival guard is mandatory)

- **Schema:** `llm_wiki_events.related_entry_id TEXT` is a plain column with **no UNIQUE constraint and no index** (`okf_ddl.rs:135-145`; the only unique indexes in the DDL target other keys — `okf_ddl.rs:73`, `:130`, `:186`, `:414-415`). Nothing in the schema ties events 1:1 to entries.
- **Writers prove many-to-one:** CT's own pipeline inserts multiple events per entry as lifecycle history, all sharing one `related_entry_id`: `commit.rs:2840` and `:2873` (two inserts in one flow), `queue.rs:618`, `librarian/synthesis.rs:1258`, `heal.rs:180`, `okf_api.rs:34`, `queries.rs:862`, `events.rs:204/:273/:314/:378/:387` (synthesized / approved / healed / imported event types). The engine likewise inserts events with `related_entry_id ?? null` freely (engine 9.18.0 dist `index.js:5307-5314`, `:5324-5334`).
- **Consequence:** "any forgotten event ⇒ hide entry" would collateral-delete a shared rule/entity after a single `forget({ eventId })`. The hiding predicate must be the **NOT EXISTS (live event) survival guard** (D1.2), and entries with **zero** linked events (e.g. `user_stated` rows whose `source_ref` is the `MANUAL_SOURCE_REF` JSON sentinel, `lib.rs:5922-5926`) are structurally unreachable by event-forget.
- `entries.derived_events TEXT` (`okf_ddl.rs:51`) is a second, unindexed JSON link channel; the guard uses `related_entry_id` only (indexed via D2) and treats `derived_events` as documentation.

### E5. TS / frontend read-path audit — verdict: **no direct TS SQL; surface is engine-mediated**

- `src/` contains **no** direct `SELECT`/`llm_wiki_*` SQL (grep over `src/**/*.ts{,x}`: only table-prefix constant and a DOM `SELECT` tag check; `src/lib/wiki.ts:74-83`).
- The frontend reads the wiki DB exclusively through the engine: `createWiki` from `@equationalapplications/react-llm-wiki` (`src/lib/wiki.ts:1`) over `tauriWikiAdapter`, a passthrough of Rust commands `wiki_exec` / `wiki_get_all` / `wiki_get_first` (`src/lib/wikiAdapter.ts:21-33`). All entry/event SQL in TS originates **inside the engine package**.
- Engine coverage: on a fresh install the lockfile resolves `@equationalapplications/core-llm-wiki@9.18.0` (`package.json:8`, `pnpm-lock.yaml`), whose dist carries exactly 11 `forgotten_at IS NULL` filters on its event readers. **The TS surface therefore already honors event-level forget once the engine is current.**
- Caveat found during audit: the *shared checkout's* stale `node_modules` still symlinked engine 7.13.0, which contains zero `forgotten_at` code (a plain `pnpm install` against the lockfile fixes it; done in this worktree). Worth a one-line note to Kurt; not a code change.
- **Upstream note: NOT filed.** The investigation condition ("engine itself serves forgotten facts to direct TS consumers") is not met: engine 9.18.0 filters forgotten events on all of its own read APIs; entries are consistently untouched by `forget({ eventId })` in the engine too. The event-vs-entry visibility divergence is resolved by owner ruling #2 above (CT hides at read time with the survival guard; the engine side remains as-is, accepted).

## Proposed design

### D1. Extend the shared predicate with a forgotten arm (events readers + entries readers)

1. **Event-table readers — direct column predicate.** `load_events` (`db/entities.rs:655-661`) and the `llm_wiki_events` arm of `list_events` (`db/events.rs:64-82`) add `AND forgotten_at IS NULL` to their WHERE. This mirrors engine semantics exactly (INTENT §12) and is served by the existing entity indexes — no post-fetch filtering, no starvation risk. `queue.rs:2179` (internal replay scan) is audited but unchanged: it replays write history, not user-visible reads.

2. **Entries-table readers — extend `entry_eligibility_where` with the survival-guard arm (ruling #2).** When the shape has the `forgotten_at` column, the fragment gains a hidden-iff-all-backing-events-forgotten anti-join plus the never-hide carve-outs:

   ```sql
   AND (
     e.source_type = 'user_stated'        -- carve-out: never hide stated rows
     OR e.id = e.entity_id                -- carve-out: never hide structural containers
     OR e.id GLOB 'tier_*'                --   (OQ4 ruling #3: OR of both signals —
     OR e.entity_id GLOB 'tier_*'         --    tier_fact / tier_wisdom constants,
                                          --    tier_working::<hash> runtime-derived; wiki.ts:52-60;
                                          --    GLOB: case-sensitive, matches exactly the tier_ namespace)
     OR NOT EXISTS (SELECT 1 FROM llm_wiki_events fe  -- survival guard: hidden iff
                    WHERE fe.related_entry_id = e.id  -- it has a forgotten event
                    AND fe.forgotten_at IS NOT NULL)  -- and none live. A forgotten
     OR EXISTS     (SELECT 1 FROM llm_wiki_events le  -- event is itself a backing
                    WHERE le.related_entry_id = e.id  -- event, so this equals
                    AND le.forgotten_at IS NULL)      -- "all backing events forgotten".
   )
   ```

   The whole block is ONE `OR` group appended to the AND chain: each arm is an
   escape hatch that keeps the row. Chaining the carve-outs as plain `AND`
   clauses would instead DROP every stated and container row unconditionally
   (T3 plan review B1, 2026-11-26) — never do that.

   Rationale: `related_entry_id` → entry is many-to-one (E4), so a naive
   any-forgotten-event hide would collateral-delete shared rules after one
   session event; the guard hides an event-backed entry only when **zero**
   live events remain, per ruling #2.1. The two subquery arms are logically
   equivalent to the ruling's "has backing events and none live" wording
   (a forgotten event is itself a backing event), and the rewritten form lets
   the `NOT EXISTS (forgotten)` arm use the D2 partial index (the subquery's
   WHERE implies the index's `forgotten_at IS NOT NULL`) and short-circuit
   there; the `EXISTS (live)` arm only runs for entries that actually have a
   forgotten event (T3 plan review M1, 2026-11-26). Entries with no linked
   events are never hidden by event-forget. The `user_stated` carve-out uses
   `source_type` (`commit.rs:3111`, `lib.rs:5922-5926`); the belt-and-braces
   attestation variant additionally spares any entry whose source document
   carries a `stated_attestations` row (join pattern: `commit.rs:3009-3013`,
   `documents.path`/`hash` ⇄ `stated_attestations.document_path`/`content_hash`).
   This automatically covers `gated_entries`, `eligible_entry_ids`,
   `candidate_set`, and the two-stage legs (all four interpolate the shared
   fragment — E3 map). The fragment body stays shape-conditional via the
   existing `Temporal`-style probe (new field on the probe struct, mirroring
   `has_superseded_by` / `has_valid_to` at `wisdom_match.rs:596-605`), so
   pre-v19 shapes compile the fragment without the arm.

3. **Close the bypasses.** `rank_wiki_entries` (`tool_dispatch.rs:450`), `curated_get_wiki_entry` (`tool_dispatch.rs:722`), and `wiki_search_in_scheme` (`wiki_graph.rs:391`) build their own SQL. Each interpolates the same shared fragment (or, for the two entity-id/topic lookups, a small `entry_not_forgotten_where(conn)` helper that returns the identical arm text) so all read paths share ONE definition. The `wiki_graph` entry-existence probes (`:1145`, `:2227`, `:469`) get the same arm so a forgotten fact cannot re-enter via graph traversal.

### D2. Indexing

Add to the `okf_ddl.rs` mirror (next to `okf_ddl.rs:385-387`), guarded on both columns existing:

```sql
CREATE INDEX IF NOT EXISTS llm_wiki_events_related_forgotten_idx
  ON llm_wiki_events(related_entry_id)
  WHERE forgotten_at IS NOT NULL AND related_entry_id IS NOT NULL
```

Partial and forgotten-only, so it stays tiny and serves exactly the anti-join.

### D3. Top-k starvation strategy

The predicate is pushed **into the candidate-selection WHERE** (D1.2/D1.3) — the anti-join removes only dead candidates *before* scoring, so cosine/LIKE top-k is computed over the live candidate set and live results are never crowded out by post-fetch filtering. No LIMIT-then-filter pattern is introduced anywhere. The keep-condition's first survival arm (`NOT EXISTS (forgotten)`) is served by the D2 partial index and short-circuits: on databases with no forgotten events (the common case) the partial index is empty, that arm is true on a single probe per candidate, and the `EXISTS (live)` arm never runs — negligible. (T3 plan review M1, 2026-11-26: the earlier two-subquery form implied neither the index's WHERE and would have full-scanned `llm_wiki_events` per candidate; the equivalence rewrite fixes both.)

### D4. UI parity (ruling #2.2)

The Tauri UI reads the wiki DB **exclusively via Rust commands** — no direct TS SQL exists (`E5`: `src/lib/wikiAdapter.ts:21-33` passthrough of `wiki_exec`/`wiki_get_all`/`wiki_get_first`; `src/lib/wiki.ts` uses only `invoke` and the engine API). Because the shared-predicate gate lives inside the Rust SQL builders those commands execute, UI, CLI, and MCP inherit the identical gate — they cannot agree to disagree. Engine-mediated TS reads continue to see forgotten-event-linked entries (engine `forget({ eventId })` does not hide entries anywhere), which is accepted per ruling #2 ("Since engine forget() leaves the entry visible in core-llm-wiki"): CT's gate is the read-time choke point on the CT side.

## Test plan

1. **Shared-fragment parity** (extends `wisdom_match` tests): seed an entry + event with `related_entry_id` and `forgotten_at` set; assert `gated_entries`, `eligible_entry_ids`, `candidate_set`, `rank_wiki_entries`, `wiki_search_in_scheme`, and `curated_get_wiki_entry` all exclude it; assert clearing `forgotten_at` restores eligibility everywhere (single-source-of-truth regression trap).
2. **Survival guard (ruling #2.1):** seed one entry with two linked events; forget exactly one (`forgotten_at` set, other NULL) — the entry MUST remain visible in every gated path (collateral-delete regression). Then forget the second — the entry disappears. Re-add an event with `forgotten_at IS NULL` — the entry resurfaces. Same for an entry with two events forgotten in either order.
3. **Carve-outs:** a `user_stated` entry (`source_type = 'user_stated'`, `MANUAL_SOURCE_REF` sentinel) and an entry whose source document has a `stated_attestations` row are NEVER hidden even with all linked events forgotten. Structural containers (OQ4 ruling #3: `e.id = e.entity_id` OR `e.id`/`e.entity_id` in the reserved `tier_` namespace, matched with `GLOB 'tier_*'`) are never hidden — seed a fixture row hitting each of the three arms and assert it stays visible; on the production schema both signals match zero leaf rows (2026-11-26 live census: 0/1458), so the guard is expected to be a no-op on real data.
4. **Degraded shapes:** same fixtures on a pre-v19 shape (no `forgotten_at` column) — queries compile, arm omitted, behavior identical to today (mirrors `connection.rs:8062` column probing).
5. **Event readers:** `load_events` and `list_events` hide forgotten events; `queue.rs` replay still sees them.
6. **Starvation guard:** seed 1 fully-forgotten + 5 live entries where the forgotten one scores highest; assert the returned top-k contains 5 live entries (no hole), not 4 + a gap.
7. **Graph traversal:** a fully-forgotten entry never appears as a fact/node seed via `wiki_context` / `wiki_traverse_graph`; a survival-guard entry (mixed live/forgotten events) still does.
8. **Index mirror:** schema-guard test asserts `llm_wiki_events_related_forgotten_idx` present when columns exist, absent otherwise (pattern: `connection.rs:8192`).
9. **Cross-surface agreement:** engine (TS, via `core-llm-wiki` test harness) forgets an event → Rust recall and UI both stop returning the event; entry visibility follows the survival guard identically in CLI/MCP and UI (UI assertion optional/manual; the Rust assertion is the gate).

## Open questions

1. ~~**OQ1: does forgetting an EVENT suppress its ENTRY?**~~ **RESOLVED by owner ruling #2 (2026-11-26, quoted verbatim above): hide entries at read time, gated by the all-backing-events survival guard, with structural-container and stated-ledger carve-outs.**
2. **OQ2 (unchanged): does CT ever call `forget({ eventId })` today?** Nothing in `src-tauri/src` writes `forgotten_at`; if no CT surface invokes event-forget yet, the gate is purely defensive and priority is the owner's call.
3. **OQ3 (unchanged, informational): stale engine install on the shared checkout** (7.13.0 symlink vs lockfile 9.18.0) — a `pnpm install` resolves it. No action requested in this PR.
4. ~~**OQ4 (NEW — needs a ruling before implementation): what signal identifies "structural entity/hub containers"?**~~ **RESOLVED by owner ruling #3 (2026-11-26, quoted verbatim above): the carve-out predicate is the OR of both candidate signals** (`id = entity_id` OR `id`/`entity_id` in the reserved tier-entity namespace), as a future-proof guard. Verified before ruling: no `llm_wiki_entities` table exists, and both signals match **0/1458** live `brain.db` rows (containers do not exist as entry rows today; entity identity lives in the shared `entity_id` column + `llm_wiki_edges` + manifests), so the guard cannot over-protect a leaf fact and honors the ruling #2.1 "never hide structural hubs" commitment vacuously-but-fails-safe.
5. **OQ5 (NEW, minor): does the attestation carve-out apply belt-and-braces or `source_type` alone?** `source_type = 'user_stated'` is cheap and definitive for stated rows; the `documents` ⇄ `stated_attestations` join (pattern `commit.rs:3009-3013`) also spares librarian facts derived from an attested stated document — wider than the ruling's literal "stated-ledger rows". Default in this spec: `source_type` carve-out in SQL, attestation join documented as the owner's optional widening.
