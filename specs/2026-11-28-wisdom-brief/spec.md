# `wisdom-brief`: `ct wisdom brief`, the standing statements for a project (PR-2)

**Date:** 2026-11-28
**Status:** Implemented (2026-11-28). Fable design review + delta re-review + code review (APPROVE WITH MINORS, folded) + /code-review; owner rulings §8. Perf: p95 137 ms at 10,000 fixture facts.
**Rev 3 (2026-11-27):** folds Fable delta re-review (APPROVE WITH NITS): `ITEM_OVERHEAD` 80, Tier 4 wording, nits.
**Rev 2 (2026-11-27):** folds Fable design review (REQUEST CHANGES: M1-M6, minors, nits) and owner rulings OD-1..OD-4, OR-1..OR-5.
**Base:** branch `feat/wisdom-brief`, stacked on PR-1 (`feat/recall-eligibility-provenance`, PR #404, tip `79fd00cd`). Every cite below was read on disk on this branch. Cites into CTI are to `curated-thoughts-integrations` as of today.

## 1. Problem

Session start today asks `ct recall` for the top 3 facts matching a fixed seed phrase plus the folder name (CTI `integrations/claude-code/scripts/ct_wisdom.py:234-285`, seed at `:58`). That is a guess: there is no conversation yet, so relevance is a coin flip, and the facts that matter most (a project's rules and traps) can lose to whatever matches the seed words.

CT already knows which facts are standing for a project: the facts attached to the project's repo hub and to the company (org) hub (areas spec, `specs/2026-11-14-areas/spec.md`; config `wisdom.areas_paths` and `wisdom.org_entity_id`, `src-tauri/src/config/mod.rs:1102-1118`). `ct wisdom brief` returns them, with no query and no embedding call. CTI rule 8 (relevance is CT's job) says this logic belongs in CT.

## 2. Settled decisions (owner rulings, 2026-11-27; do not re-ask)

- `ct wisdom brief` is CT-owned. CTI calls it at session start and falls back to `ct recall` on an older CT (probe: `ct wisdom brief --help`). That consumer work is a later CTI PR, not this one.
- It returns the project's STANDING statements (repo hub + org hub) plus a compact covered-topics index. No query, no embedding.
- It reuses PR-1's eligibility (`entry_recall_arms`, `src-tauri/src/wisdom_match.rs:466`: superseded, `valid_to`, forgotten, plus `deleted_at IS NULL`). It never carries a second copy of those arms.
- Drop order when the size cap is hit, bottom up: Tier 1 `rule`+`trap`; Tier 2 `decision`; Tier 3 `lesson`; Tier 4 no kind (oldest untyped dropped first). Tier comes from PR-1's `kind_tier` (`wisdom_match.rs:298`). Inside Tiers 1-3: repo scope first, then org; newest first inside each scope. Tier 4 (untyped) is ordered newest-first across repo and company together, so the oldest untyped statement is dropped first, globally (OR-1).
- Area-scoped statements (`area IS NOT NULL`) are not listed; they are summarized in the index (mid-task `--path=` delivery surfaces them).
- Space is reserved for the index so statement text cannot crowd it out.
- Each statement carries a `ct-fact`-usable `id`, `source_type`, `verified`, `kind` (same names and meaning as `recall_item`, `wisdom_match.rs:340`). Output is deterministic.
- PR-2 also writes fail-open delivery rows to `curated_agent_log` (not `llm_wiki_entries.access_count`; reasons: INTENT rules 1 and 12, outbox replication, and `access_count` is a ranking tie-break input in the engine).

## 3. Design

### 3.1 Command and flags

`ct wisdom brief [--json] [--repo-path <path>] [--max-chars <n>]` in `WisdomCmd` (`tools/src/bin/ct.rs:381`, dispatch `:829`). Read-only on facts. `--help` exits 0 on a CT that has it; an older CT rejects the unknown subcommand (non-zero), which is the CTI probe.

- `--repo-path` follows the existing deposit flag of the same name (`ct.rs:399-400`: "Relative paths are resolved against the current directory"). Default `.`. It is resolved by the existing `canonicalize_lenient` (`src-tauri/src/config/mod.rs:1279`, relative resolved against cwd). The later CTI PR spawns `ct` with cwd = project root (today it spawns with cwd = `~`, `ct_wisdom.py:254`) and passes nothing or `.`, so no home path leaves the machine (CTI rule 9). CT never prints the `--repo-path` value: not in JSON, not in stderr, not in `load_lenient` diagnostics (a test runs a brain whose config fails to load and greps stderr for the path). Config-derived strings CT does not control (a hub path inside a TOML parse error) are not claimed covered; the test pins the `--repo-path` value only.
- `--max-chars` default 2200, accepted range 600..=8000 (else exit 1, usage error). The lower bound is 600, not 400, because at 400 the index cap is 100 chars while the fixed hint alone is about 63 and the shortest repo line with its hint about 102 (worst case about 137, still within the 150 cap) (Fable M2); at 600 the cap is 150 and the shortest form fits. The shed ladder in 3.5 still ends in an empty index (cost 0) as a defensive floor. See 3.4.
- No `--exclude`, no query text, no embedding profile load (so no `CURATED_EMBED_STUB`, no network).

### 3.2 Which hubs (question 1)

New `hub_resolver::resolve_brief_scope(conn, cfg: Option<&WisdomConfig>, path) -> BriefScope { repo_group: Option<Vec<String>>, org_group: Option<Vec<String>>, reason: Option<&'static str>, diagnostics: Vec<String> }`, beside `resolve_scoped` (`hub_resolver.rs:377`). It reuses the same private pieces: `configured_survivors` and `detect_collision` (`:259`, `:271`), `longest_prefix_match` (`:297`), `live_survivor` (`:230`), and `cluster_ids` (`db/entities.rs:371`, survivor first, then merged-away ids). The org block of `resolve_scoped` (`:433-452`) moves into one private `org_group_for(conn, cfg)`, and the repo-resolution steps (configured survivors, collision, longest prefix, live survivor, `cluster_ids`) move into one shared private helper, so there is no third copy. Both are used by `resolve_scoped` and `resolve_brief_scope`; `resolve_scoped` behavior and every reason string stay byte-identical, pinned by the existing ~28 tests in `src-tauri/tests/hub_resolver.rs`, which must stay green unmodified.

Why not call `resolve_scoped`: it returns `NoPrefix` before computing the org group (`:392`), and it turns an unusable areas file into `Unresolved` (`:404-411`). The brief needs the org band in both cases and has no use for the areas file (area names come from the `area` column, 3.5).

| Situation | Statements from | `scope` | `scope_reason` |
|---|---|---|---|
| Path under a configured checkout, org resolves | repo + org | `repo+org` | `null` |
| Same, no org configured | repo | `repo` | `null` |
| Same, org configured but unresolvable | repo | `repo` | `org_unresolved` |
| No `wisdom` block, or no `areas_paths` | org if configured | `org` or `none` | `unconfigured` |
| `NoPrefix` (path under no checkout) | org if configured | `org` or `none` | `no_prefix` |
| `Unresolved` (collision, cycle, dead hub, tie) | org if configured | `org` or `none` | `unresolved` |

Ruled (OD-4): org-only, never empty and never unscoped. When no repo hub is configured or resolved, the brief is company-only with `scope_reason`, never the whole brain. Band precedence: a fact whose `entity_id` is in both groups is labelled `repo` (repo wins). `scope_reason` precedence when several apply: `unconfigured`, then `no_prefix`, then `unresolved`, then `org_unresolved` (first match wins). The org hub is by design visible to every project (owner ruling: "repo hub + company/org hub"), so serving it where the repo is unknown is within INTENT rule 8. It also means CTI's current `cwd=~` spawn still gets company rules. A brief never falls back to the whole brain; this differs from `ct wisdom match`, where `NoPrefix` means UNSCOPED (`queries.rs:1009-1018`). Long reason strings go to stderr as `wisdom_brief_diagnostic\t<text>` (the `wisdom_scoped_diagnostic` house form, `queries.rs:1220`); they hold entity ids, never paths. A config that fails to load (`BrainConfig::load_lenient`, as `queries.rs:980`) behaves as `unconfigured` plus a stderr line. Merge groups are bound exactly as `band_predicate_where` does: the repo and org id lists are survivor-plus-losers.

### 3.3 What counts as a statement (question 4)

A statement is an eligible live fact whose `entity_id` is in the repo or org merge group and whose `area IS NULL` (the band predicate's repo and org arms with no area, `wisdom_match.rs:698-760`, reused as is by passing `BandBindings { area: None, .. }`; org-only mode passes the org group as `repo_group` and labels every row `org` in Rust). Typed and untyped facts both count. Ruled: no draft filter. The live gate and PR-1 recall have no lifecycle arm (PR-1 spec 7.1), INTENT rule 10 says every live fact is recallable, and only the confirmation queue requires `lifecycle_status = 'stable'` (`db/queue.rs:315`). A fresh brain (all wiki entries were wiped) has almost only untyped facts, and new facts get no `okf_type` until the later deposit `kind:` lane, so for now most statements land in Tier 4 and are ordered newest-first across repo and company (OR-1); the tier machinery is already in place for typed facts. Rows with an id that fails `valid_fact_id` (`wisdom_match.rs:269`) or with empty title and body are skipped (CTI drops them anyway, `ct_wisdom.py:273`).

On a pre-V30 brain with no `area` column everything is area-less (same degrade as `band_predicate_where`'s `has_area = false`). PR-1's `RecallArms` has no `has_area`; add the field (its only constructor is `wisdom_match.rs:474`).

### 3.4 Order, budget, drop rule (question 2)

Order: `kind_tier` asc; then for Tiers 1-3 scope (repo 0, org 1); then `created_at` desc; then `id` asc. For Tier 4 (untyped) scope is NOT a key: repo and org interleave newest-first (OR-1). `created_at` is coerced like `coerce_updated_at` (`tool_dispatch.rs:419`): a TEXT or malformed cell never drops the row; it sorts as 0, oldest. The `id` tie-break (PR-1 OQ-C) makes the bytes a pure function of the rows. `created_at` is used rather than `updated_at` because it does not move when a fact is touched. Output order is this order. Provenance and confidence are labels only and never enter the order (INTENT Out of scope: no quiet ranking by origin).

Budget is defined over a canonical rendering CT documents, because CT cannot see each host's markup:

- Statement cost = `utf16(title) + utf16(text) + utf16(id) + ITEM_OVERHEAD`, where `utf16(s) = s.encode_utf16().count()`, to match TypeScript `.length` on the TS hosts (Python `len` differs only for astral chars; UTF-16 is the conservative of the two). `ITEM_OVERHEAD = 80` is a documented contract with CTI: "CTI per-item markup plus labels <= 80 UTF-16 units + id". Derivation (CTI `ct_wisdom_live.py:265-272`, `**%s** %s (provenance: %s)`, plus the `ct_wisdom.py:343-352` render and `ct_ledger.py:32-33` marker): markup 25 + ` (provenance: X)` with the longest X at 33 + the kind label in its worst form ` (kind: decision)` at 17 = 75, leaving 5 spare. If CTI also renders `verified`, raise the constant to 90 in a CT release CTI pins. The 2200 default still leaves slack for sanitizing growth and label variance.
- Index cost = `utf16(index.text) + 2`, and exactly 0 when `index.text` is empty (no separator is rendered).
- Default 2200: Hermes allows 2500 chars at session start (`ct_wisdom.py:62`, CTI rule 6); CTI's heading is 37 chars plus 2 (`:316`), so a full brief renders to about 2240, leaving about 260 for sanitizing growth and label variance. CTI keeps its own hard cap as a backstop; its truncate-the-first-overflow branch (`:353-362`) should never fire on a brief.
- Index reserve (no circularity, Fable M1): the index text contains `<k> shown`, which depends on the fit. Algorithm: (1) build the index once with `k := n` (all statements shown) under cap `min(400, max_chars / 4)`; this fixes the shed level and the reserve; (2) fit statements into `max_chars - index_cost`; (3) re-render the index at that SAME shed level with the real `k` and never re-shed. A real `k` is never wider than `n`, so the final text is never longer than the reserve (a test crosses a digit boundary: n=10, k=9). Statements can never push the index out.
- Whole statements only. Walk the ordered list and keep the longest prefix that fits (that is "drop from the bottom up": a lower tier is never kept while a higher one is dropped). Never cut text mid-fact. One exception: a statement whose cost alone exceeds the statement budget can never fit, so it is skipped (counted as `oversize`) instead of blocking everything below it.
- Drops are not quiet: `omitted` reports counts per tier and the index says how many statements are not shown. `omitted.statements` counts every eligible statement not returned INCLUDING `oversize`; `by_tier` sums to `omitted.statements` (oversize items are counted in their tier too); `oversize` is a subset figure, not additive.

### 3.5 Covered-topics index (question 3)

Built from the same eligible rows; no graph traversal (INTENT Out of scope; merge-group expansion via `entity_redirects` is not link-following, same as the areas spec). Contents:

- `areas`: area name and eligible count for facts in the repo merge group with `area IS NOT NULL`. Sorted count desc, name asc; at most 8; names cut to 40 chars. The org band has no areas (the org arm requires `area IS NULL`).
- Counts: eligible statements per band, and how many are shown.
- Hub display names from `live_entities.name` of the survivors, cut to 40 chars.
- A fixed hint telling the agent how to pull more: `ct wisdom match --path=<file>` for area facts, and the sidecar `wiki_context` (`mcp_server.rs:73`) for graph neighbours.

Not included: "top entities by fact count". Facts attach to the hub (merge group), so that list would be one or two names; listing related entities needs edges, which is out of scope.

`index.text` is the pre-rendered line CTI appends verbatim: `Covered: repo <name> (<n> statements, <k> shown); org <name> (<n>, <k> shown). Areas not listed, surfaced when you work on their files: <a> (<c>), ... +<m> more. More on demand: ct wisdom match --path=<file>, or wiki_context.` Over the cap it sheds areas from the tail, then hub names, then the area clause, then the hint, then the whole index (empty text, cost 0). At `max_chars >= 600` the cap is at least 150 and the shortest form (the repo line with its hint, about 102 chars; worst case about 137) always fits, pinned by a test. Empty (`""`) when there are no statements and no areas.

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

Exit codes (OD-3, ruled): 0 on success including "nothing to say" (empty `statements`, `index.text` `""`); 1 on any error (no brain, bad flag, fail-closed shape probe from `entry_recall_arms`, like PR-1 spec 3.5). Exit 2 is not used. Reason: every CTI host treats any non-zero exit as a failed, non-memoized call (PR-1 spec 3.5; `ct_wisdom.py:260-262`, `wisdom.ts:560`, `bootstrap.ts:189`), and on openclaw it ticks the circuit breaker. "Nothing to say" is a normal answer (CTI rule 4), so it must be exit 0. This departs from the owner's "0 / 2 / error" sketch; the owner ruled exit 0 with empty arrays (OD-3).

Queries (all by `entity_id IN (...)`, served by `llm_wiki_entries_entity_idx`, `db/okf_ddl.rs:54`; the forgotten arm uses the #378 anti-join index mirror):
1. Statements: `SELECT <recall columns>, e.created_at FROM llm_wiki_entries e WHERE e.deleted_at IS NULL{arms.sql}{band predicate, area None}`.
2. Area counts: same eligibility, with named binds (`:repo0`, ...), `e.entity_id IN (repo group) AND e.area IS NOT NULL GROUP BY e.area`.
3. Two `live_entities.name` point lookups; two PRAGMA probes (inside `entry_recall_arms`).
Sorting and fitting are in Rust, so tier order has one definition (`kind_tier`). Rows are not LIMITed: a LIMIT before the tier sort could hide a rule behind newer untyped facts. Cost is bounded by the hubs' live facts (design target about 10,000 per brain, INTENT). The read connection gets `busy_timeout` 1 s; no network and no model, so a call is local I/O only. Measured in the scratch-brain run (section 7).

### 3.7 Delivery log (telemetry)

Table: `curated_agent_log(id, client, tool, operation CHECK IN ('read','write'), entity_id, summary, created_at)` (`db/okf_ddl.rs:272-280`). Not replicated: no outbox file mentions it (grep over `src-tauri/src`). Pruned at 90 days when the app migrates (`db/connection.rs:966-969`), cleared by `clear_vault_tables` (`db/queries.rs:264`). Existing writers: the sidecar's generic `log_agent_access` (`tool_dispatch.rs:1224`, skipped for `curated_*` and `wisdom_*` tool names, `:1500-1507`, so those names are free), the fail-closed `log_agent_access_checked_with_summary` (`db/commit.rs:87`), and the areas rename audit row.

Writing:
- New `tools/src/delivery_log.rs`, `record_delivery(db_path: &Path, tool, entity_id, summary)` (a `&Path`, not a `Brain`). It opens its own connection: `OpenFlags::READ_WRITE | NO_MUTEX`, no `CREATE` (the sidecar precedent, `curated_thoughts_mcp.rs:613-626`). `write::open_rw` (`write.rs:63`) is not used: it passes `SQLITE_OPEN_CREATE` and a 5 s `busy_timeout`, which would create an empty `brain.db` and stall a per-turn call.
- Log connection settings: no `CREATE` flag, `busy_timeout` 100 ms (the sidecar's 5 s fits a long-lived server, not a CTI call with a 3-5 s timeout), `wal_autocheckpoint=0`, `synchronous=NORMAL`. On timeout the row is lost. Contention with the app writer is the only risk; WAL readers never block it.
- Latency ordering: CTI reads stdout only at process exit, so the log write is on the call's critical path. Therefore (a) flush stdout explicitly BEFORE the log write; (b) drop the read connection first (a rollback-journal brain would self-block a writer behind our own reader); (c) skip logging if the call has already used about 1.5 s; (d) no `unwrap`/`expect` anywhere in the module. Log failures never change output or exit code.
- Inserts through `db::commit::log_agent_access_checked_with_summary` (reuse), error swallowed. Fail-open: a failed open, a missing table or column, a locked DB or a read-only filesystem changes nothing: stdout bytes and exit code are identical, and at most one short stderr line `wisdom_delivery_log_skipped\t<error class>` is printed. The error-class vocabulary is closed: `open`, `no_table`, `busy`, `readonly`, `other`. The SQLite error message is never printed (it contains the db path). The write happens after stdout is printed. `CURATED_DELIVERY_LOG=0` turns it off (benchmarks, tests).
- No row when nothing was delivered, so volume tracks deliveries, not calls.
- Row: `client = "ct-cli"`, `operation = "read"`, `tool = "wisdom_brief"` or `"wisdom_match"`. `entity_id` = repo hub survivor (org if no repo) for the brief; NULL for match. `summary` is JSON: `{"v":1,"surface":"brief","n":5,"ids":["..."]}`; match adds `"target":"message|path|command|pattern"` and `"corrections":[ids]`. Ids only, capped at 64 (OR-5). Never query text, paths, commands or patterns (CTI rule 9 in spirit). Ids of facts later forgotten remain in the log until its 90-day retention prunes them; this is accepted. One row per call, not per fact: a per-fact row would multiply volume 5-30x for no extra information.
- Surfaces in PR-2: `ct wisdom brief` and `ct wisdom match` (both inject without the agent asking; mid-task delivery is the half the index points at). `ct recall` is deferred: it is also a hand-typed command, and it is the fallback CTI is moving off. Adding it later is one call (OD-1, ruled).
- Timeline (OD-1 + OR-3, ruled): the app's activity timeline reads every `curated_agent_log` row as an `agent_access` event (`db/events.rs:98-112`). In THIS PR the timeline query excludes `tool IN ('wisdom_brief','wisdom_match')` rows (a `WHERE` on that arm of the UNION), with a test in `src-tauri` seeding both tool names plus one other and asserting only the other appears.
- `ct wisdom match` is documented "read-only" (`ct.rs:435-436`, `queries.rs:913`). `wisdom_match_cmd` (`queries.rs:916`) collapses `--command` and `--pattern` into one target (`ct.rs:899-904`), so dispatch passes a new target-kind parameter (`message|path|command|pattern`). The logger is NOT threaded through pub `run_message_path` (`queries.rs:1254`, used by `tools/tests/activity_helper_parity.rs`); `wisdom_match_cmd` logs before each `return Ok(0)`. It stays read-only for facts; the help text is reworded to say it appends a best-effort audit row.

## 4. INTENT check

- Rule 1: no fact is written; the log row is an audit row, not a fact. Rule 12: `curated_agent_log` is CT's own table, not the engine's.
- Rule 3: reads only ingested facts in the brain. Rule 4/Supersession: `entry_recall_arms` (superseded, expired, forgotten) plus `deleted_at`.
- Rule 5: one brain; no machine-specific state in the output.
- Rule 8: scoped to the matched repo hub and the org hub; no unscoped fallback.
- Rule 10: unconfirmed facts are included and labeled (`source_type`, `verified`).
- Rule 7: no hubs, or nothing eligible, yields an empty brief rather than filler.
- Out of scope: no graph traversal; no ranking by origin (order is kind, then scope for Tiers 1-3, then recency; drops are disclosed in `omitted`).

## 5. Out of scope

The CTI consumer PR; follow-up CT lanes (deposit `kind:`); logging `ct recall` or the sidecar `wiki_*` tools; changing `ct wisdom match` selection (its output bytes must not change); draft filtering; a per-host render budget. Consumer notes for the later CTI PR: amend CTI's glossary and rule 7, which call recall/match read-only (OR-4: both now append a best-effort audit row); sanitize `index.text` (it carries hub and area names) before rendering; a host label in delivery rows, if wanted, is a validated slug carried in `summary`, not the `client` column.

## 6. Tests (names and locations in plan.md)

1. Eligibility via the shared arms: superseded, expired `valid_to`, forgotten, deleted are never in `statements` (mandatory).
2. Scope table (3.2): each row, incl. org-only on `NoPrefix`/`Unresolved`/no config, merge-group loser ids included, foreign hub excluded.
3. Area facts absent from `statements`, counted in `index.areas`.
4. Order and drop: tier, then scope (Tiers 1-3), then newest, then id; Tier 4 interleaves repo and org newest-first and a TEXT `created_at` keeps the row; bottom-up drop; oversize skip; index never crowded out; `k` crossing a digit boundary (n=10, k=9); `--max-chars 600` renders a non-empty index within its cap; `by_tier` sums to `omitted.statements`.
5. Fields: `id`, `source_type`, `verified`, `kind`, `band`; ids valid for `ct-fact`.
6. Determinism: two runs byte-identical, also with delivery logging on (the log table changes between runs).
7. `ct wisdom brief --help` exits 0; `--max-chars` range.
8. Delivery log: row written with ids only; no row when empty; fail-open (missing table, locked DB, read-only file); stdout and exit identical with `CURATED_DELIVERY_LOG=0`; `ct wisdom match` output bytes pinned unchanged.
9. `resolve_scoped` behavior unchanged after the `org_group_for` and shared repo-step extraction (existing tests in `src-tauri/tests/hub_resolver.rs`, unmodified).
10. Timeline excludes `wisdom_brief`/`wisdom_match` rows.

## 7. Benchmark gates (INTENT step 4; question 6)

- No superseded fact ever injected: MANDATORY; tests 1 and 6 plus an end-to-end CLI case.
- Cached output byte-identical: test 6.
- Exactly-once: the ledger is CTI's (CTI rule 1). CT's part is that every id appears once in a brief (test 5).
- LongMemEval hit rate, abstention precision, 200-prompt paraphrase: they score query relevance and the embedding gate. The brief has no query, no embedding and no floor, so they do not apply. The 2026-11-27 owner waiver (INTENT step 4) does not apply either: it covers compliance/eligibility fixes with byte-identical matcher SQL, and this is a new injection surface.
- Ruled gates (OD-2): (a) the fixture tests above (drop order incl. OR-1, budget, determinism and byte-identical re-run, no superseded fact); (b) a scratch-brain run (never the live brain; synthetic hubs, 50 / 1,000 / 10,000 facts, mixed kinds and areas, some superseded/expired/forgotten) with a perf gate p95 wall time <= 250 ms at 10,000 facts, plus output never over the cap, determinism over 20 runs and zero superseded ids. Results go in the PR body. The ruling is recorded in INTENT.md "How changes are made" step 4 as one dated owner-ruling line (plan task 11).
- Measured (2026-11-28; scratch brains with fixture facts and no embeddings; debug `ct` build, delivery log off, 20 runs each): 50 facts p95 27 ms; 1,000 facts p95 34 ms; 10,000 facts p50 130 ms, p95 137 ms (gate <= 250 ms). Output byte-identical across the 20 runs at every size, `used_chars` <= 2200, and zero superseded, expired or forgotten ids.

Review rungs: per the 2026-11-27 owner ruling (INTENT step 3), Fable design + code review and /code-review, recorded here and in the PR body.

## 8. Owner decisions (ruled)

- **OR-1 Untyped tier.** Tier 4 orders newest-first across repo and company together (oldest untyped dropped first, globally). Tiers 1-3 keep repo-before-company, newest first within each scope. "Newest" is `created_at`, coerced like `coerce_updated_at`, then `id` asc.
- **OD-1 + OR-3 Logging.** Deliveries of both `ct wisdom brief` and `ct wisdom match` are logged; in this PR the app timeline excludes `tool IN ('wisdom_brief','wisdom_match')` rows (`src-tauri/src/db/events.rs` ~:98-112), with a test.
- **OD-2 Gates.** Fixture tests + a scratch-brain run with p95 <= 250 ms at 10,000 facts, in place of LongMemEval, abstention and paraphrase runs (they measure query matching). Recorded in INTENT.md step 4.
- **OD-3 Nothing to say.** Exit 0 with empty arrays.
- **OD-4 No repo hub.** Company-only brief with `scope_reason`, never the whole brain.
- **OR-4 CTI docs.** The later CTI PR amends CTI's glossary and rule 7 (they call recall/match read-only).
- **OR-5 Delivery privacy.** Delivery rows carry fact ids only (never query text or paths); ids of facts later forgotten remain for the log's retention (about 90 days); accepted.

## 9. Resolved review questions

- Overhead and the 2200 default fit all four CTI hosts once cost is counted in UTF-16 units. `ITEM_OVERHEAD = 80` is a documented contract (3.4).
- No draft filter (3.3). "Newest" is `created_at` (3.4).
- No host label in delivery rows now; later, a validated slug in `summary`, not the client column.
- `source_ref` and `entity_id` stay in `statements[]` (reuse of `recall_item`).
- `wal_autocheckpoint=0` on the log connection: yes; checkpoint-on-close is kept.

## 10. Rejected alternatives

- List area-scoped statements ranked behind repo/org within their tier (the owner's allowed alternative): rejected for the index-only form; mid-task `--path=` delivery already surfaces them, and they would eat the budget with facts that are irrelevant to the session's first files.
- Bump `access_count` for delivery telemetry: rules 1/12, outbox replication, engine tie-break (2.).
- Call `resolve_scoped` directly: loses the org band on `NoPrefix` and fails on a bad areas file (3.2).
- Skip-and-continue fitting (keep any statement that fits): keeps a lesson while dropping a rule, breaking the drop order.
- Truncate statement text to fit: cuts a fact mid-sentence.
- Row per delivered fact; `open_rw` for logging; SQL `ORDER BY CASE okf_type`: volume; creates a db file and 5 s stall; second definition of tiers.

## 11. Files touched

- `src-tauri/src/wisdom_brief.rs` (new), `src-tauri/src/lib.rs` (`pub mod wisdom_brief`), `src-tauri/src/wisdom_match.rs` (`RecallArms.has_area`), `src-tauri/src/config/hub_resolver.rs` (`org_group_for`, shared repo-step helper, `resolve_brief_scope`), `src-tauri/src/db/events.rs` (timeline excludes delivery tools).
- `tools/src/delivery_log.rs` (new), `tools/src/lib.rs`, `tools/src/queries.rs` (`wisdom_brief_cmd`, log calls at the `wisdom_match_cmd` exits), `tools/src/bin/ct.rs` (subcommand, help text).
- Tests: `src-tauri/tests/hub_resolver.rs` (`brief_scope_*`), `tools/tests/ct_wisdom_brief.rs` (new), `tools/tests/ct_wisdom_match.rs` (log + byte pin), `tools/tests/common/mod.rs` (`with_scoped_brain` and seed helpers moved from `ct_wisdom_match.rs`), a timeline test in `src-tauri`, unit tests in the new modules.
- `INTENT.md`: one appended owner-ruling line in "How changes are made" step 4 (OD-2).
- `specs/2026-11-28-wisdom-brief/`. NOT touched: the live brain.
