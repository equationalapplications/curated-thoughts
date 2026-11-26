# Spec: read-time gate for forgotten facts (`forgotten_at`) — #378

**Status:** Spec for approval (owner ruling recorded on #378)
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

## Problem

`llm_wiki_events.forgotten_at` (engine `forget({ eventId })`) is mirrored in the Rust replica — column declared (`src-tauri/src/db/okf_ddl.rs:143`), migration shim (`src-tauri/src/db/connection.rs:1464`), partial index mirror (`src-tauri/src/db/okf_ddl.rs:385-387`) — but **zero Rust readers filter it**. The only other Rust references are schema-guard enumerations and tests (`src-tauri/src/db/schema_guard.rs:111`, `src-tauri/src/db/connection.rs:8062`). Once any host calls the engine's `forget({ eventId })`, forgotten events (and, if entry-level gating is adopted, their linked facts) resurface in every Rust read path: recall, wisdom match, two-stage retrieval, wiki search/context, entity manifests, and the unified timeline. INTENT §12 ("The Rust side follows the engine's words", `INTENT.md:226`) requires the replica to honor the engine's forget verb.

## Evidence

### E1. Where `forgotten_at` lives — events table, NOT entries

- `llm_wiki_events.forgotten_at INTEGER` is the only column: `src-tauri/src/db/okf_ddl.rs:135-145` (declared at `:143`).
- `llm_wiki_entries` has **no** `forgotten_at` column: full DDL at `src-tauri/src/db/okf_ddl.rs:14-52`.
- `entries.derived_events TEXT` (JSON, `okf_ddl.rs:51`) and `llm_wiki_events.related_entry_id TEXT` (`okf_ddl.rs:141`) are the two available join keys. `related_entry_id` is the engine-populated, indexed-by-insert path; `derived_events` is an opaque JSON blob written by CT's entry pipeline — the join should use `related_entry_id`.
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

### E4. TS / frontend read-path audit — verdict: **no direct TS SQL; surface is engine-mediated**

- `src/` contains **no** direct `SELECT`/`llm_wiki_*` SQL (grep over `src/**/*.ts{,x}`: only table-prefix constant and a DOM `SELECT` tag check; `src/lib/wiki.ts:74-83`).
- The frontend reads the wiki DB exclusively through the engine: `createWiki` from `@equationalapplications/react-llm-wiki` (`src/lib/wiki.ts:1`) over `tauriWikiAdapter`, a passthrough of Rust commands `wiki_exec` / `wiki_get_all` / `wiki_get_first` (`src/lib/wikiAdapter.ts:21-33`). All entry/event SQL in TS originates **inside the engine package**.
- Engine coverage: on a fresh install the lockfile resolves `@equationalapplications/core-llm-wiki@9.18.0` (`package.json:8`, `pnpm-lock.yaml`), whose dist carries exactly 11 `forgotten_at IS NULL` filters on its event readers. **The TS surface therefore already honors event-level forget once the engine is current.**
- Caveat found during audit: the *shared checkout's* stale `node_modules` still symlinked engine 7.13.0, which contains zero `forgotten_at` code (a plain `pnpm install` against the lockfile fixes it; done in this worktree). Worth a one-line note to Kurt; not a code change.
- **Upstream note: NOT filed.** The investigation condition ("engine itself serves forgotten facts to direct TS consumers") is not met: engine 9.18.0 filters forgotten events on all of its own read APIs; entries are consistently untouched by `forget({ eventId })` in the engine too. The only genuine open item (event-forget vs. entry-visibility semantics) is CT's owner question below, not an engine defect.

## Proposed design

### D1. Extend the shared predicate with a forgotten arm (events readers + entries readers)

1. **Event-table readers — direct column predicate.** `load_events` (`db/entities.rs:655-661`) and the `llm_wiki_events` arm of `list_events` (`db/events.rs:64-82`) add `AND forgotten_at IS NULL` to their WHERE. This mirrors engine semantics exactly (INTENT §12) and is served by the existing entity indexes — no post-fetch filtering, no starvation risk. `queue.rs:2179` (internal replay scan) is audited but unchanged: it replays write history, not user-visible reads.

2. **Entries-table readers — extend `entry_eligibility_where` with a NOT-EXISTS arm.** When the shape has the `forgotten_at` column, the fragment gains:

   ```sql
   AND NOT EXISTS (
     SELECT 1 FROM llm_wiki_events fe
     WHERE fe.related_entry_id = e.id
       AND fe.forgotten_at IS NOT NULL
   )
   ```

   This automatically covers `gated_entries`, `eligible_entry_ids`, `candidate_set`, and the two-stage legs (all four interpolate the shared fragment — E3 map). The fragment body stays shape-conditional via the existing `Temporal`-style probe (new field on the probe struct, mirroring `has_superseded_by` / `has_valid_to` at `wisdom_match.rs:596-605`), so pre-v19 shapes compile the fragment without the arm.

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

The predicate is pushed **into the candidate-selection WHERE** (D1.2/D1.3) — the anti-join removes only dead candidates *before* scoring, so cosine/LIKE top-k is computed over the live candidate set and live results are never crowded out by post-fetch filtering. No LIMIT-then-filter pattern is introduced anywhere. The NOT-EXISTS is per-row indexed (D2); on databases with no forgotten events (the common case) the partial index is empty and the arm is a single index probe per candidate — negligible.

### D4. TS surface disposition

No TS change required: the frontend never queries the wiki DB directly (E4); event-level forget is already honored by engine 9.18.0 through the adapter passthrough. CT's Rust gate is the choke point for the CLI/MCP/recall side. The spec does **not** propose TS-side entry gating, because the engine itself does not suppress entries on event forget — see OQ1 before extending entry suppression beyond engine semantics.

## Test plan

1. **Shared-fragment parity** (extends `wisdom_match` tests): seed an entry + event with `related_entry_id` and `forgotten_at` set; assert `gated_entries`, `eligible_entry_ids`, `candidate_set`, `rank_wiki_entries`, `wiki_search_in_scheme`, and `curated_get_wiki_entry` all exclude it; assert clearing `forgotten_at` restores eligibility everywhere (single-source-of-truth regression trap).
2. **Degraded shapes:** same fixtures on a pre-v19 shape (no `forgotten_at` column) — queries compile, arm omitted, behavior identical to today (mirrors `connection.rs:8062` column probing).
3. **Event readers:** `load_events` and `list_events` hide forgotten events; `queue.rs` replay still sees them.
4. **Starvation guard:** seed 1 forgotten + 5 live entries where the forgotten one scores highest; assert the returned top-k contains 5 live entries (no hole), not 4 + a gap.
5. **Graph traversal:** forgotten entry never appears as a fact/node seed via `wiki_context` / `wiki_traverse_graph`.
6. **Index mirror:** schema-guard test asserts `llm_wiki_events_related_forgotten_idx` present when columns exist, absent otherwise (pattern: `connection.rs:8192`).
7. **Cross-surface agreement:** engine (TS, via `core-llm-wiki` test harness) forgets an event → Rust recall and UI both stop returning the event (UI assertion optional/manual; the Rust assertion is the gate).

## Open questions for the owner

1. **OQ1 (the real one): does forgetting an EVENT suppress its ENTRY?** The engine's `forget({ eventId })` tombstones only the event; the linked entry stays live and remains engine-visible on the TS side (E1/E4). This spec's D1.2 entry-level gate goes **beyond** engine semantics — it makes CT recall/search hide the fact while the UI (engine-mediated) would still show the entry, violating the ruling's own "UI and CLI/MCP never disagree" requirement in the opposite direction. Owner must pick: (a) **events-only gate** (D1.1 alone; strict INTENT §12 parity; recall still surfaces the entry text), or (b) **entry gate as specced** (D1.1 + D1.2; the entry disappears everywhere in CT, and the engine/UI side needs a matching ruling upstream). This spec is written for (b) per the ruling's wording ("a forgotten fact never surfaces in recall/search/wiki results"), but (a) is the semantically conservative read.
2. **OQ2: does CT ever call `forget({ eventId })` today?** Nothing in `src-tauri/src` writes `forgotten_at`; if no CT surface invokes event-forget yet, this issue is purely defensive and the priority/urgency is the owner's call.
3. **OQ3: stale engine install on the shared checkout** (7.13.0 symlink vs lockfile 9.18.0) — flagging only; a `pnpm install` resolves it. No action requested in this PR.
