# `wisdom-brief`: `ct wisdom brief`, the standing statements for a project (PR-2)

**Date:** 2026-11-28
**Status:** Draft
**Base:** branch `feat/wisdom-brief`, stacked on PR-1 (`feat/recall-eligibility-provenance`, PR #404, tip `79fd00cd`). Every cite below was read on disk on this branch. Cites into CTI are to `curated-thoughts-integrations` as of today.

## 1. Problem

Session start today asks `ct recall` for the top 3 facts matching a fixed seed phrase plus the folder name (CTI `integrations/claude-code/scripts/ct_wisdom.py:234-285`, seed at `:58`). That is a guess: there is no conversation yet, so relevance is a coin flip, and the facts that matter most (a project's rules and traps) can lose to whatever matches the seed words.

CT already knows which facts are standing for a project: the facts attached to the project's repo hub and to the company (org) hub (areas spec, `specs/2026-11-14-areas/spec.md`; config `wisdom.areas_paths` and `wisdom.org_entity_id`, `src-tauri/src/config/mod.rs:1102-1118`). `ct wisdom brief` returns them, with no query and no embedding call. CTI rule 8 (relevance is CT's job) says this logic belongs in CT.

## 2. Settled decisions (owner rulings, 2026-11-27; do not re-ask)

- `ct wisdom brief` is CT-owned. CTI calls it at session start and falls back to `ct recall` on an older CT (probe: `ct wisdom brief --help`). That consumer work is a later CTI PR, not this one.
- It returns the project's STANDING statements (repo hub + org hub) plus a compact covered-topics index. No query, no embedding.
- It reuses PR-1's eligibility (`entry_recall_arms`, `src-tauri/src/wisdom_match.rs:466`: superseded, `valid_to`, forgotten, plus `deleted_at IS NULL`). It never carries a second copy of those arms.
- Drop order when the size cap is hit, bottom up: Tier 1 `rule`+`trap`; Tier 2 `decision`; Tier 3 `lesson`; Tier 4 no kind (oldest untyped dropped first). Tier comes from PR-1's `kind_tier` (`wisdom_match.rs:298`). Inside a tier: repo scope first, then org; newest first inside each scope.
- Area-scoped statements (`area IS NOT NULL`) are not listed; they are summarized in the index (mid-task `--path=` delivery surfaces them).
- Space is reserved for the index so statement text cannot crowd it out.
- Each statement carries a `ct-fact`-usable `id`, `source_type`, `verified`, `kind` (same names and meaning as `recall_item`, `wisdom_match.rs:340`). Output is deterministic.
- PR-2 also writes fail-open delivery rows to `curated_agent_log` (not `llm_wiki_entries.access_count`; reasons: INTENT rules 1 and 12, outbox replication, and `access_count` is a ranking tie-break input in the engine).

## 3. Design

### 3.1 Command and flags

`ct wisdom brief [--json] [--repo-path <path>] [--max-chars <n>]` in `WisdomCmd` (`tools/src/bin/ct.rs:381`, dispatch `:829`). Read-only on facts. `--help` exits 0 on a CT that has it; an older CT rejects the unknown subcommand (non-zero), which is the CTI probe.

- `--repo-path` follows the existing deposit flag of the same name (`ct.rs:399-400`: "Relative paths are resolved against the current directory"). Default `.`. It is resolved by the existing `canonicalize_lenient` (`src-tauri/src/config/mod.rs:1279`, relative resolved against cwd). The later CTI PR spawns `ct` with cwd = project root (today it spawns with cwd = `~`, `ct_wisdom.py:254`) and passes nothing or `.`, so no home path leaves the machine (CTI rule 9). CT never prints the path: not in JSON, not in stderr.
- `--max-chars` default 2200, accepted range 400..=8000 (else exit 1, usage error). See 3.4.
- No `--exclude`, no query text, no embedding profile load (so no `CURATED_EMBED_STUB`, no network).

### 3.2 Which hubs (question 1)

New `hub_resolver::resolve_brief_scope(conn, cfg: Option<&WisdomConfig>, path) -> BriefScope { repo_group: Option<Vec<String>>, org_group: Option<Vec<String>>, reason: Option<&'static str>, diagnostics: Vec<String> }`, beside `resolve_scoped` (`hub_resolver.rs:377`). It reuses the same private pieces: `configured_survivors` and `detect_collision` (`:259`, `:271`), `longest_prefix_match` (`:297`), `live_survivor` (`:230`), and `cluster_ids` (`db/entities.rs:371`, survivor first, then merged-away ids). The org block of `resolve_scoped` (`:433-452`) moves into one private `org_group_for(conn, cfg)` used by both (behavior of `resolve_scoped` unchanged, pinned by its existing tests).

Why not call `resolve_scoped`: it returns `NoPrefix` before computing the org group (`:392`), and it turns an unusable areas file into `Unresolved` (`:404-411`). The brief needs the org band in both cases and has no use for the areas file (area names come from the `area` column, 3.5).

| Situation | Statements from | `scope` | `scope_reason` |
|---|---|---|---|
| Path under a configured checkout, org resolves | repo + org | `repo+org` | `null` |
| Same, no org configured | repo | `repo` | `null` |
| Same, org configured but unresolvable | repo | `repo` | `org_unresolved` |
| No `wisdom` block, or no `areas_paths` | org if configured | `org` or `none` | `unconfigured` |
| `NoPrefix` (path under no checkout) | org if configured | `org` or `none` | `no_prefix` |
| `Unresolved` (collision, cycle, dead hub, tie) | org if configured | `org` or `none` | `unresolved` |

Recommendation: org-only, never empty and never unscoped. The org hub is by design visible to every project (owner ruling: "repo hub + company/org hub"), so serving it where the repo is unknown is within INTENT rule 8. It also means CTI's current `cwd=~` spawn still gets company rules. A brief never falls back to the whole brain; this differs from `ct wisdom match`, where `NoPrefix` means UNSCOPED (`queries.rs:1009-1018`). Long reason strings go to stderr as `wisdom_brief_diagnostic\t<text>` (the `wisdom_scoped_diagnostic` house form, `queries.rs:1220`); they hold entity ids, never paths. A config that fails to load (`BrainConfig::load_lenient`, as `queries.rs:980`) behaves as `unconfigured` plus a stderr line. Merge groups are bound exactly as `band_predicate_where` does: the repo and org id lists are survivor-plus-losers.

### 3.3 What counts as a statement (question 4)

A statement is an eligible live fact whose `entity_id` is in the repo or org merge group and whose `area IS NULL` (the band predicate's repo and org arms with no area, `wisdom_match.rs:698-760`, reused as is by passing `BandBindings { area: None, .. }`; org-only mode passes the org group as `repo_group` and labels every row `org` in Rust). Typed and untyped facts both count. Recommendation: no draft filter. The live gate and PR-1 recall have no lifecycle arm (PR-1 spec 7.1), INTENT rule 10 says every live fact is recallable, and only the confirmation queue requires `lifecycle_status = 'stable'` (`db/queue.rs:315`). A fresh brain (all wiki entries were wiped) has almost only untyped facts, and new facts get no `okf_type` until the later deposit `kind:` lane, so for now most statements land in Tier 4 and are ordered by scope then newest; the tier machinery is already in place for typed facts. Rows with an id that fails `valid_fact_id` (`wisdom_match.rs:269`) or with empty title and body are skipped (CTI drops them anyway, `ct_wisdom.py:273`).

On a pre-V30 brain with no `area` column everything is area-less (same degrade as `band_predicate_where`'s `has_area = false`). PR-1's `RecallArms` has no `has_area`; add the field (its only constructor is `wisdom_match.rs:474`).

### 3.4 Order, budget, drop rule (question 2)

Order: `kind_tier` asc, then scope (repo 0, org 1), then `created_at` desc, then `id` asc (the id tie-break, PR-1 OQ-C, makes bytes a pure function of the rows). `created_at` rather than `updated_at`: it does not move when a fact is touched. Output order is this order. Provenance and confidence are labels only and never enter the order (INTENT Out of scope: no quiet ranking by origin).

Budget is defined over a canonical rendering CT documents, because CT cannot see each host's markup:

- Statement cost = `chars(title) + chars(text) + chars(id) + ITEM_OVERHEAD`, `ITEM_OVERHEAD = 60`. That covers the CTI claude-code/hermes render: `**title** <!-- ct-fact:ID -->\ntext` plus the `\n\n` separator is 25 chars of markup (`ct_wisdom.py:343-352`; marker format `ct_ledger.py:32-33`), and 35 more chars for CTI's provenance label (CTI rule 5).
- Index cost = `chars(index.text) + 2`.
- Default 2200: Hermes allows 2500 chars at session start (`ct_wisdom.py:62`, CTI rule 6); CTI's heading is 37 chars plus 2 (`:316`), so a full brief renders to about 2240, leaving about 260 for sanitizing growth and label variance. CTI keeps its own hard cap as a backstop; its truncate-the-first-overflow branch (`:353-362`) should never fire on a brief.
- Index reserve: the index is built first with its own cap `min(400, max_chars / 4)`; statements get `max_chars - index_cost`. Statements can never push the index out.
- Whole statements only. Walk the ordered list and keep the longest prefix that fits (that is "drop from the bottom up": a lower tier is never kept while a higher one is dropped). Never cut text mid-fact. One exception: a statement whose cost alone exceeds the statement budget can never fit, so it is skipped (counted as `oversize`) instead of blocking everything below it.
- Drops are not quiet: `omitted` reports counts per tier and the index says how many statements are not shown.

### 3.5 Covered-topics index (question 3)

Built from the same eligible rows; no graph traversal (INTENT Out of scope; merge-group expansion via `entity_redirects` is not link-following, same as the areas spec). Contents:

- `areas`: area name and eligible count for facts in the repo merge group with `area IS NOT NULL`. Sorted count desc, name asc; at most 8; names cut to 40 chars. The org band has no areas (the org arm requires `area IS NULL`).
- Counts: eligible statements per band, and how many are shown.
- Hub display names from `live_entities.name` of the survivors, cut to 40 chars.
- A fixed hint telling the agent how to pull more: `ct wisdom match --path=<file>` for area facts, and the sidecar `wiki_context` (`mcp_server.rs:73`) for graph neighbours.

Not included: "top entities by fact count". Facts attach to the hub (merge group), so that list would be one or two names; listing related entities needs edges, which is out of scope.

`index.text` is the pre-rendered line CTI appends verbatim: `Covered: repo <name> (<n> statements, <k> shown); org <name> (<n>, <k> shown). Areas not listed, surfaced when you work on their files: <a> (<c>), ... +<m> more. More on demand: ct wisdom match --path=<file>, or wiki_context.` Over the cap it sheds areas from the tail, then hub names, then the area clause; at `max_chars >= 400` the shortest form always fits. Empty (`""`) when there are no statements and no areas.

### 3.6 Output, exit codes, diagnostics, performance (question 5)

```
{"schema":1,"scope":"repo+org","scope_reason":null,
 "budget":{"max_chars":2200,"used_chars":1834},
 "statements":[{"id","entity_id","title","text","source_ref","confidence",
                "source_type"?,"verified"?,"kind"?,"band":"repo"|"org"}],
 "index":{"text":"...","areas":[{"name":"auth","count":3}],
          "counts":{"repo":12,"org":5}},
 "omitted":{"statements":4,"oversize":0,"by_tier":{"rule_trap":0,"decision":0,"lesson":1,"untyped":3}}}
```

Items are `recall_item` output plus `band` (reuse, not a new builder). `schema` is a `u32` like `WisdomMatch.schema` (`wisdom_match.rs:31,158`), constant `BRIEF_SCHEMA_VERSION = 1`. Printed with `print_json` (`tools/src/paths.rs:27`). No timestamps, no paths, nothing from `curated_agent_log` or `access_count`, so the same brain and args give identical bytes; a fact crossing its `valid_to` between two calls is the only legitimate change. `--json` omitted prints a short human listing.

Exit codes: 0 on success including "nothing to say" (empty `statements`, `index.text` `""`); 1 on any error (no brain, bad flag, fail-closed shape probe from `entry_recall_arms`, like PR-1 spec 3.5). Exit 2 is not used. Reason: every CTI host treats any non-zero exit as a failed, non-memoized call (PR-1 spec 3.5; `ct_wisdom.py:260-262`, `wisdom.ts:560`, `bootstrap.ts:189`), and on openclaw it ticks the circuit breaker. "Nothing to say" is a normal answer (CTI rule 4), so it must be exit 0. This departs from the owner's "0 / 2 / error" sketch (decision OD-3).

Queries (all by `entity_id IN (...)`, served by `llm_wiki_entries_entity_idx`, `db/okf_ddl.rs:54`; the forgotten arm uses the #378 anti-join index mirror):
1. Statements: `SELECT <recall columns>, e.created_at FROM llm_wiki_entries e WHERE e.deleted_at IS NULL{arms.sql}{band predicate, area None}`.
2. Area counts: same eligibility, `e.entity_id IN (repo group) AND e.area IS NOT NULL GROUP BY e.area`.
3. Two `live_entities.name` point lookups; two PRAGMA probes (inside `entry_recall_arms`).
Sorting and fitting are in Rust, so tier order has one definition (`kind_tier`). Rows are not LIMITed: a LIMIT before the tier sort could hide a rule behind newer untyped facts. Cost is bounded by the hubs' live facts (design target about 10,000 per brain, INTENT). The read connection gets `busy_timeout` 1 s; no network and no model, so a call is local I/O only. Measured in the scratch-brain run (section 7).

### 3.7 Delivery log (telemetry)

Table: `curated_agent_log(id, client, tool, operation CHECK IN ('read','write'), entity_id, summary, created_at)` (`db/okf_ddl.rs:272-280`). Not replicated: no outbox file mentions it (grep over `src-tauri/src`). Pruned at 90 days when the app migrates (`db/connection.rs:966-969`), cleared by `clear_vault_tables` (`db/queries.rs:264`). Existing writers: the sidecar's generic `log_agent_access` (`tool_dispatch.rs:1224`, skipped for `curated_*` and `wisdom_*` tool names, `:1500-1507`, so those names are free), the fail-closed `log_agent_access_checked_with_summary` (`db/commit.rs:87`), and the areas rename audit row.

Writing:
- New `tools/src/delivery_log.rs`, `record_delivery(brain, tool, entity_id, summary)`. It opens its own connection: `OpenFlags::READ_WRITE | NO_MUTEX`, no `CREATE` (the sidecar precedent, `curated_thoughts_mcp.rs:613-626`). `write::open_rw` (`write.rs:63`) is not used: it passes `SQLITE_OPEN_CREATE` and a 5 s `busy_timeout`, which would create an empty `brain.db` and stall a per-turn call.
- `busy_timeout` 100 ms (the sidecar's 5 s fits a long-lived server, not a CTI call with a 3-5 s timeout). On timeout the row is lost. Contention with the app writer is the only risk; WAL readers never block it.
- Inserts through `db::commit::log_agent_access_checked_with_summary` (reuse), error swallowed. Fail-open: a failed open, a missing table or column, a locked DB or a read-only filesystem changes nothing: stdout bytes and exit code are identical, and at most one short stderr line `wisdom_delivery_log_skipped\t<error class>` is printed. The write happens after stdout is printed. `CURATED_DELIVERY_LOG=0` turns it off (benchmarks, tests).
- No row when nothing was delivered, so volume tracks deliveries, not calls.
- Row: `client = "ct-cli"`, `operation = "read"`, `tool = "wisdom_brief"` or `"wisdom_match"`. `entity_id` = repo hub survivor (org if no repo) for the brief; NULL for match. `summary` is JSON: `{"v":1,"surface":"brief","n":5,"ids":["..."]}`; match adds `"target":"message|path|command|pattern"` and `"corrections":[ids]`. Ids only, capped at 64. Never query text, paths, commands or patterns (CTI rule 9 in spirit). One row per call, not per fact: a per-fact row would multiply volume 5-30x for no extra information.
- Surfaces in PR-2: `ct wisdom brief` and `ct wisdom match` (both inject without the agent asking; mid-task delivery is the half the index points at). `ct recall` is deferred: it is also a hand-typed command, and it is the fallback CTI is moving off. Adding it later is one call (decision OD-1).
- Side effect to know: the app's activity timeline reads every row as an `agent_access` event, "`<client> called <tool>`" (`db/events.rs:98-112`). Match rows can add up to a few hundred timeline lines a day.
- `ct wisdom match` is documented "read-only" (`ct.rs:435-436`, `queries.rs:913`). It stays read-only for facts; the help text is reworded to say it appends a best-effort audit row.

## 4. INTENT check

- Rule 1: no fact is written; the log row is an audit row, not a fact. Rule 12: `curated_agent_log` is CT's own table, not the engine's.
- Rule 3: reads only ingested facts in the brain. Rule 4/Supersession: `entry_recall_arms` (superseded, expired, forgotten) plus `deleted_at`.
- Rule 5: one brain; no machine-specific state in the output.
- Rule 8: scoped to the matched repo hub and the org hub; no unscoped fallback.
- Rule 10: unconfirmed facts are included and labeled (`source_type`, `verified`).
- Rule 7: no hubs, or nothing eligible, yields an empty brief rather than filler.
- Out of scope: no graph traversal; no ranking by origin (order is kind, scope, recency; drops are disclosed in `omitted`).

## 5. Out of scope

The CTI consumer PR; follow-up CT lanes (deposit `kind:`); logging `ct recall` or the sidecar `wiki_*` tools; changing `ct wisdom match` selection (its output bytes must not change); draft filtering; a per-host render budget; timeline filtering of delivery rows.

## 6. Tests (names and locations in plan.md)

1. Eligibility via the shared arms: superseded, expired `valid_to`, forgotten, deleted are never in `statements` (mandatory).
2. Scope table (3.2): each row, incl. org-only on `NoPrefix`/`Unresolved`/no config, merge-group loser ids included, foreign hub excluded.
3. Area facts absent from `statements`, counted in `index.areas`.
4. Order and drop: tier, then scope, then newest, then id; bottom-up drop; oversize skip; index never crowded out.
5. Fields: `id`, `source_type`, `verified`, `kind`, `band`; ids valid for `ct-fact`.
6. Determinism: two runs byte-identical, also with delivery logging on (the log table changes between runs).
7. `ct wisdom brief --help` exits 0; `--max-chars` range.
8. Delivery log: row written with ids only; no row when empty; fail-open (missing table, locked DB, read-only file); stdout and exit identical with `CURATED_DELIVERY_LOG=0`; `ct wisdom match` output bytes pinned unchanged.
9. `resolve_scoped` behavior unchanged after the `org_group_for` extraction (existing tests).

## 7. Benchmark gates (INTENT step 4; question 6)

- No superseded fact ever injected: MANDATORY; tests 1 and 6 plus an end-to-end CLI case.
- Cached output byte-identical: test 6.
- Exactly-once: the ledger is CTI's (CTI rule 1). CT's part is that every id appears once in a brief (test 5).
- LongMemEval hit rate, abstention precision, 200-prompt paraphrase: they score query relevance and the embedding gate. The brief has no query, no embedding and no floor, so they do not apply. The 2026-11-27 owner waiver (INTENT step 4) does not apply either: it covers compliance/eligibility fixes with byte-identical matcher SQL, and this is a new injection surface.
- Proposed proportionate measurement (OD-2): (a) the fixture tests above; (b) a scratch-brain run (never the live brain; synthetic hubs, 50 / 1,000 / 10,000 facts, mixed kinds and areas, some superseded/expired/forgotten): output size never over the cap, determinism over 20 runs, p95 wall time, zero superseded ids; (c) one human read of the rendered 2500-char block. Results go in the PR body.

Review rungs: per the 2026-11-27 owner ruling (INTENT step 3), Fable design + code review and /code-review, recorded here and in the PR body.

## 8. Owner decisions (pending)

- **OD-1 Which surfaces log.** Recommend `brief` + `match`; defer `recall`. Alternative: `brief` only (no timeline noise, but mid-task deliveries stay unmeasured).
- **OD-2 Gates.** Recommend ratifying section 7: LongMemEval/abstention/paraphrase not applicable; replaced by fixture tests + scratch-brain run. Not a waiver under the 2026-11-27 line.
- **OD-3 Exit code for "nothing to say".** Recommend exit 0 with empty arrays (so CTI memoizes it and openclaw's breaker is not ticked), not exit 2.
- **OD-4 No repo hub resolved.** Recommend org-only brief with `scope_reason`, not an empty brief. Alternative: empty on any `NoPrefix`/`Unresolved`.

## 9. Open questions for review

- `ITEM_OVERHEAD = 60` and default 2200 come from the claude-code/hermes `ct_wisdom.py` render. The TypeScript hosts (`wisdom.ts`, `bootstrap.ts`) were not read for markup size; the later CTI PR should confirm.
- Drafts: confirm no `lifecycle_status` filter (3.3).
- `created_at` vs `updated_at` as "newest" (3.4).
- Should delivery rows carry a host label? Today `client` is always `ct-cli`; a `--client` flag would let the timeline say which host.
- `statements[]` carries `source_ref` and `entity_id` because `recall_item` does; CTI ignores them, but they add bytes to the JSON (not to the budget).
- `wal_autocheckpoint=0` on the log connection (plan task 7) so a CLI call never pays for a checkpoint.

## 10. Rejected alternatives

- List area-scoped statements ranked behind repo/org within their tier (the owner's allowed alternative): rejected for the index-only form; mid-task `--path=` delivery already surfaces them, and they would eat the budget with facts that are irrelevant to the session's first files.
- Bump `access_count` for delivery telemetry: rules 1/12, outbox replication, engine tie-break (2.).
- Call `resolve_scoped` directly: loses the org band on `NoPrefix` and fails on a bad areas file (3.2).
- Skip-and-continue fitting (keep any statement that fits): keeps a lesson while dropping a rule, breaking the drop order.
- Truncate statement text to fit: cuts a fact mid-sentence.
- Row per delivered fact; `open_rw` for logging; SQL `ORDER BY CASE okf_type`: volume; creates a db file and 5 s stall; second definition of tiers.

## 11. Files touched

- `src-tauri/src/wisdom_brief.rs` (new), `src-tauri/src/lib.rs` (`pub mod wisdom_brief`), `src-tauri/src/wisdom_match.rs` (`RecallArms.has_area`), `src-tauri/src/config/hub_resolver.rs` (`org_group_for`, `resolve_brief_scope`).
- `tools/src/delivery_log.rs` (new), `tools/src/lib.rs`, `tools/src/queries.rs` (`wisdom_brief_cmd`, log call in the `print_wisdom_match` path), `tools/src/bin/ct.rs` (subcommand, help text).
- Tests: `tools/tests/ct_wisdom_brief.rs` (new), `tools/tests/ct_wisdom_match.rs` (log + byte pin), unit tests in the new modules.
- `specs/2026-11-28-wisdom-brief/`. NOT touched: the live brain, INTENT.md.
