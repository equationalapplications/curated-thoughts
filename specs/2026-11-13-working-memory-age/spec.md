# `working-memory-age`: a chunks-only folder whose chunks drop out of search with age

**Date:** 2026-11-13
**Status:** Draft — awaits the owner's approval (Step-3 review). Rev 3: addresses the Opus spec delta (2 MAJOR / 3 MINOR, all fixed: register-before-prepare at every filtered consumer replaces the register-at-open rule — 43 open sites make named-open incomplete; `aged_out` given a precise request-scoped negated-fragment definition and named emitters; completeness test defined in §6.5; numbering and OQ1-pending fixes). Rev 2: addresses the Opus spec review round 1 (4 MAJOR / 2 MINOR, all fixed: fifth consumer `recall_chunks`, `resolve_prefix` strip-then-relative, tie rule DECIDED larger-wins, `aged_out` flag, benchmark backdating, day-boundary semantics). Backed by the converged investigation of 2026-11-13 (Opus Tier-3 rounds 1-3 + Fable Tier-4 rev 4; every finding verified against the tree at `d26654a3` and addressed).
**Issue:** #347. **Owner rulings:** 2026-11-12/13 (design); 90-day default proposed here, not ruled.

## 1. Problem

A working-memory folder holds session notes and handoffs. The owner rules it `chunks-only` with the existing `ingest.folder_tiers` key: the Librarian chunks it but never extracts facts. Today those chunks stay searchable forever. The ruling: after a set age, a note stops appearing in search. The file stays in the vault and in git. A search may ask for older records on purpose.

## 2. Settled decisions (do not re-ask)

- Extraction is settled by `ingest.folder_tiers = "chunks-only"`. This spec adds no extraction behavior.
- The age setting is a per-folder value, resolved like `folder_tiers` (deepest matching prefix wins).
- Aging hides chunks from search. It never deletes or edits vault files (INTENT rule 9).
- Existing facts extracted before a folder was tiered chunks-only are NOT re-processed. The wisdom layer is untouched.
- `ct wisdom match` is untouched: it queries `llm_wiki_entries`, which chunks-only folders never produce.
- Absent key = the folder never ages out. Empty config keeps today's behavior exactly.

## 3. The age setting: `ingest.folder_ages`

1. Add `#[serde(default)] pub folder_ages: HashMap<String, u64>` to `IngestConfig` (`config/mod.rs:400-420`). The value is days.
2. Add the key to `known_ingest_keys` (`config/mod.rs:1320`) and to the lenient-load salvage handling (`:1764-1796`), so a degraded load preserves and reports it like `folder_tiers` does.
3. Resolution reuses the `match_prefixes` core (`config/mod.rs:250-303`) on an already-relative path. **The scalar function must NOT call `resolve_prefix` with the raw absolute path** (rev 2, Opus MAJOR-2): the absolute branch pays `fs::canonicalize` calls on a strip miss (`walk_vault.rs:134/:138`). The function strips the captured root itself, then calls `resolve_prefix(&map, rel, None)`; `NoMatch` and `Unplaceable` both mean NULL ("never ages").
4. **Tie rule (DECIDED, rev 2 — Opus MAJOR-3): when two same-depth keys conflict, the LARGER age wins** — aging hides data, and hiding is the less conservative outcome; the diagnostic names the rule (`Tie` outcome, `config/mod.rs:294-298`; tiers settle the same case with most-conservative-wins, `:368`).
5. Add a resolver wrapper next to `ingest_tier_for` (for example `age_days_for(path) -> Option<u64>`) on the policy type.
6. **Default: there is NO code default (pending OQ1 — this states the recommended option).** 90 days is the value the spec's example config documents:
   ```json
   "ingest": { "folder_tiers": { "records": "chunks-only" },
               "folder_ages":  { "records": 90 } }
   ```

## 4. The age basis (DECIDED)

Age = days since the document's first successful index: `MIN(run_at)` over `ingest_runs WHERE outcome = 'indexed'` (migration V10, `schema.rs:171-185`). This needs no schema migration. The known limits, which this spec accepts:

1. Documents indexed before V10 have no `ingest_runs` row. They never age. (V10 inserts no backfill rows; the only production inserts are `mark_document_indexed`/`mark_document_error`, `db/queries.rs:106/:118`.)
2. Deleting a document row wipes its history (the `none`-tier round trip, issue #211; duplicate-row cleanup `connection.rs:143-164`; `queue.rs:130/:186`; `lib.rs:1074`). A rename keeps the doc id and the age (`reconcile.rs:297-302`).
3. The insert is best-effort; a failed insert never fails the index.
4. The only bulk wipe is `switch_vault` without backup restore (`clear_vault_tables`, one caller `lib.rs:2091`). Ages restart on a later switch back. Accepted; no cross-reset preservation.
5. The basis answers "how long in the index", never "how old is the note".

## 5. Where the filter applies

The filter runs at SQL level, as one shared WHERE fragment added to the existing `d.status = 'indexed'` arm. Applies to:

1. `search::semantic_search` (`search/mod.rs:145-201`) — MCP `vault_semantic_search` and the CLI facade.
2. `related_chunks` / `related_chunks_try_paths` (`search/mod.rs:203-308`).
3. `RECALL_CHUNKS_SQL_BASE` and its verbatim twins — both copies (`tool_dispatch.rs:371`, `tools/src/queries.rs:76`) and both SQL-shape tests change together. FIVE consumers build queries from the shared constant: the four `format!` sites (`tool_dispatch.rs:692`, `:810`, `curated_thoughts_mcp.rs:239`, `:363`) plus `recall_chunks` (`tools/src/queries.rs:316`, which uses `.to_string()` and serves the `ct` CLI). A missed registration makes SQLite fail with `no such function` — CLI recall must not break, not even with empty config.
4. The app's `search_vault` (`lib.rs:3129`) and `get_related_chunks` (`lib.rs:3146`) — INTENT requires the app and the agent tools to stay one pipeline.

Does NOT apply to:

- `get_structural_neighbors` (`lib.rs:3223`, queries `:3262`/`:3299`) — navigation, not recall. The spec keeps it working on aged chunks. (Open question OQ2 for the owner.)
- `chunk_ids_for_entry` (`lib.rs:3340`, query `:3434`) — evidence anchoring for heal/re-anchor; hiding aged chunks there could sever provenance. Exempt.
- `ct wisdom match` and all `llm_wiki_entries` paths — out of scope by §2.

## 6. The mechanism: a registered scalar function

The age depends on each row's path, so per-row resolution must happen in SQL. Three mechanisms were weighed in the investigation; the spec chooses **(a)**:

1. Add one `pub fn register_ct_functions(&Connection)` to `tauri_app_lib`. It registers one scalar function, `ct_folder_age_days(path)`, which returns the configured cutoff in days for the row's folder, or NULL when the folder is unconfigured.
2. **Register before prepare, at every filtered consumer** (rev 3, Opus delta MAJOR-1): the repo has 43 `Connection::open`/`open_with_flags` call sites across 19 production files, so a register-at-open rule with named sites cannot be complete — a missed site fails the query with `no such function` even on empty config. Instead, every consumer that runs a filtered query calls `register_ct_functions(conn)` immediately before preparing it. The function is idempotent and cheap (it guards on an already-registered check), so calling it per query is safe on any connection, including test fixtures on raw connections. `open_brain_readonly` additionally registers at open, so the parity fixture stays honest.
3. Enable rusqlite's `functions` feature (workspace level; today the workspace has `["bundled"]`, `src-tauri` has `["backup"]`, tools inherits none). This is a new pattern in this repo; the shared helper keeps it in ONE function body.
4. Extend `tools/tests/shared_queries.rs:14` to pin the fragment on both crates (its fixture runs on `open_brain_readonly`, `:25-28`). Update the SQL-shape assertions (`tools/src/queries.rs:1269-1272`).
5. **Registration completeness test (rev 3, replaces the rev-2 §11 pointer):** one test runs EVERY recall entry point — the five shared-SQL consumers, `semantic_search`, `related_chunks`, and the app commands `search_vault`/`get_related_chunks` — each on a freshly opened raw connection and with an age configured for one folder. Any missed `register_ct_functions` call fails the entry point with `no such function`; the test asserts success instead.

The function's runtime rules (from the investigation, Fable M2):

- Capture the policy AND the vault root once per connection, not per row. Never call `fs::read` of config.json per row, never `fs::canonicalize` per row.
- Relativize the row's path with a pure string `strip_prefix` against the captured root. A placement miss means "never ages".
- The sidecar learns the vault root from `IngestPolicy.vault_root` (`config/mod.rs:706`); loading it is new work for the sidecar, stated in the plan.

The full WHERE fragment (both NULL arms are mandatory — an unconfigured folder never ages, and a pre-V10 document never ages):

```sql
AND (
      ct_folder_age_days(d.path) IS NULL                       -- folder unconfigured: never ages
   OR (SELECT MIN(ir.run_at) FROM ingest_runs ir
        WHERE ir.doc_id = c.doc_id AND ir.outcome = 'indexed') IS NULL   -- pre-V10: never ages
   OR (unixepoch()
       - (SELECT MIN(ir.run_at) FROM ingest_runs ir
          WHERE ir.doc_id = c.doc_id AND ir.outcome = 'indexed')) / 86400
       <= ct_folder_age_days(d.path)
    )
```

**Semantics and cost (stated, rev 2 — Opus MINOR-2):** integer division means a note ages out the day AFTER its N-day boundary, not at exactly N×24 hours; that is the intended semantics. With empty config the OR short-circuits on the first arm — the added per-row cost is one in-memory lookup. With an age configured, the two correlated subqueries run per row; both are covered by `idx_ingest_runs_doc_time` (`schema.rs:182`). A grouped LEFT JOIN would halve that but would add a JOIN to every consumer's query even with the feature unused; the spec keeps the subqueries and records the trade-off.

Rejected: (b) generated `CASE`/`LIKE` SQL — two implementations of one rule, drift-prone; (c) Rust post-filter before truncation — correct today (no SQL LIMIT on these paths) but duplicates the rule at five-plus call sites forever and breaks when an ANN path applies LIMIT in SQL.

## 7. Filter, not delete

Aged chunks stay in the index. Aging is reversible: changing the config restores results with no re-ingest. No reaper, no compaction in this PR. Index growth is accepted; a future compaction pass is the answer if size matters.

## 8. Benchmarks ("How changes are made" step 4)

This is a recall change, so the gates apply: no superseded fact injected; LongMemEval hit rate; abstention precision; paraphrase tests (at least 200, across models); exactly-once; cached output byte-identical to fresh output. The spec records that hiding chunks by folder and age is a form of ranking by origin, which INTENT lists as out of scope for ranking — the gate run is the check that this does not degrade recall.

**Backdating requirement (rev 2, Opus MINOR-1):** scratch brains are freshly indexed, so `MIN(run_at)` is about now and the filter never fires in a plain benchmark run. The benchmark fixture must backdate `ingest_runs.run_at` on a known share of documents (or set age 0 on one folder), and the gates run BOTH with the filter active and with it off, so the delta is measurable.

## 9. Surfacing (DECIDED, rev 3 — Opus MAJOR-4 round 1 + delta MAJOR-2)

Age-filtered responses must not lie by silence. The filter runs in SQL, so hidden rows never reach Rust; counting what would have ranked needs their scores and defeats the filter. The spec therefore defines a FLAG, not a count:

1. **Meaning:** `aged_out: true` when at least one chunk WOULD have matched this request's non-age filters and fails the age arm. Not a vault-wide count — that is true on nearly every search and says nothing.
2. **Computation:** ONE COUNT query per filtered search. It keeps the request's non-age filters (path filters, document filters) and the `d.status = 'indexed'` arm as they are, and NEGATES the age arm:
   ```sql
   SELECT COUNT(*) FROM chunks c JOIN documents d ON c.doc_id = d.id
   WHERE d.status = 'indexed' /* AND the request's non-age filters */
     AND NOT ( ct_folder_age_days(d.path) IS NULL
            OR (SELECT MIN(ir.run_at) FROM ingest_runs ir
                 WHERE ir.doc_id = c.doc_id AND ir.outcome = 'indexed') IS NULL
            OR (unixepoch() - (SELECT MIN(ir.run_at) FROM ingest_runs ir
                 WHERE ir.doc_id = c.doc_id AND ir.outcome = 'indexed')) / 86400
              <= ct_folder_age_days(d.path) )
   ```
   A count above zero sets the flag. No vector work.
3. **Emitters (DECIDED):** `semantic_search` (it serves MCP `vault_semantic_search`, the CLI facade, and the app's `search_vault`) and the recall chunk leg (`RECALL_CHUNKS_SQL_BASE` consumers). NOT emitted by `related_chunks` or `get_structural_neighbors` — navigation surfaces stay clean. The flag rides on the response alongside the results; the plan fixes the exact field.

## 10. Open questions for the owner

- **OQ1 — the 90-day default (§3.6 states option 1 as the recommended pending this ruling).** The ruling left the default open. Options:
   1. Document 90 days in the example config, no code default. (Recommended: the ruling says "default 90 days" as a proposal; the example makes it visible and changeable without a release.)
   2. Hard-code 90 days as the code default for any folder that has `chunks-only` but no age entry.
- **OQ2 — structural neighbors.** Keep `get_structural_neighbors` exempt (recommended: it is navigation, and the app's search box still filters), or filter it too so no app surface shows aged working memory.

## 11. Out of scope

- Extracting facts from working-memory folders, or re-processing existing facts.
- Deleting chunks or documents by age.
- CTI-side changes (lessons/decisions deposited separately is a separate repo's work).
- Note-date ("how old is the note") aging. The investigation keeps it as the alternative if the owner rules against the index-age basis.
