# `recall-eligibility-provenance`: `ct recall` stops serving superseded facts and labels what it serves (PR-1)

**Date:** 2026-11-27
**Status:** Draft
**Base:** origin/main 3.18.0 (`fc715239`). Every cite below was read on disk at that commit.

## 1. Problem

CTI (the integrations repo) builds every host's session-start block from `ct recall --json --k 3`. Today that breaks two of CTI's rules, and its own README says so ("The session-start block has no provenance labels and may include a superseded fact", `curated-thoughts-integrations/README.md:263-266`).

1. **Superseded and expired facts are served as current.** The wiki leg, `rank_wiki_entries` (`tools/src/queries.rs:176`), filters only `deleted_at IS NULL`, the forgotten arm (`:193`, `:198`) and the term LIKE. It never checks `superseded_by`/`superseded_at` or `valid_to`. The live gate has both checks: `entry_eligibility_where` (`src-tauri/src/wisdom_match.rs:610`) carries `(e.superseded_by IS NULL OR e.superseded_at > :now)` (`:626`) and `(e.valid_to IS NULL OR e.valid_to > :now)` (`:629`). The app copy (`src-tauri/src/tool_dispatch.rs:434`) has the same hole. Breaks CTI rule 3 and the Librarian's promise "Recall that skips superseded and expired facts" (INTENT.md, The Active Librarian).
2. **The CLI/sidecar copy emits no provenance.** Its items carry `id, entity_id, title, text, source_ref, confidence` (`queries.rs:229-236`). The app copy already adds `source_type` (through `provenance_for`, `wisdom_match.rs:273`) and `verified` (`tool_dispatch.rs:498-515`). Breaks CTI rule 5 and INTENT rule 3 ("labeled ... as per-hit `source_type` and `verified` labels").
3. **The CLI copy ranks `inferred` above `confirmed`.** It sorts the confidence string (`queries.rs:239`, `:248`); the app copy uses an ordinal `confirmed=3 > inferred=2 > other=0` (`tool_dispatch.rs:519-523`).
4. **`ct recall` drops the wiki leg when the chunk leg is empty.** `recall_cmd` (`queries.rs:448`) returns `EXIT_NO_RESULTS` (`write.rs:45`, value 2) at `:451-453` before `rank_wiki_entries` runs at `:454`. A brain with facts but no chunk hit for the seed query yields nothing.

## 2. Settled decisions (do not re-ask)

- This PR (PR-1) is the compliance fix plus an additive `kind` field. A later PR-2 adds `ct wisdom brief`; a later CTI PR consumes it. Neither is designed here.
- Recall stays keyword-based. It does NOT gain the embedding or scheme requirements of the live gate.
- No ranking change except item 3's bug fix. Provenance and kind never change rank (INTENT Out of scope: no quiet ranking by origin).

## 3. Design

### 3.1 One eligibility source of truth (A)

Add to `wisdom_match.rs` one `pub(crate)` fn `temporal_arms_sql(t: &Temporal) -> String`. It returns the three arms now written inline in `entry_eligibility_where` (`:626-635`), in the same order and with the same text: superseded, valid_to, forgotten (via `forgotten_entry_arm`, `:640`). `entry_eligibility_where` becomes `base + temporal_arms_sql(t)`. Its output must stay byte-identical (pinned, §5 test 1).

Because the `tools` crate cannot see `pub(crate)` items, add a public, connection-taking wrapper in the style of `entry_not_forgotten_arm` (`:667`): `pub fn entry_recall_arms(conn) -> Result<RecallArms { sql: String, needs_now: bool }>`. It probes with `temporal_columns_shape_only` (`:321`), NOT `temporal_columns` (`:342`). Reason: the `:311-320` doc comment — a corrupt `wisdom_active_scheme` must not fail reads that use no embeddings, and recall uses none. It fails closed (errors propagate), like the forgotten arm. Both `rank_wiki_entries` copies call it, replacing their forgotten-arm calls, and bind `:now` only when `needs_now` (binding an unused named param is a rusqlite error, see `eligible_entry_ids`, `:683-700`). The recall statement switches from positional `?1` to named `:term` so it can mix with `:now`.

Degraded shapes follow `temporal_columns`: a missing `superseded_by`, `valid_to`, or events `forgotten_at`/`related_entry_id` column drops only that arm. Pre-migration brains stay readable. The CLI opens brains read-only without migrating (`retrieval/mod.rs:156-165`), so such shapes are reachable there.

Recall's eligibility therefore = `deleted_at IS NULL` + the three arms. It deliberately omits the title/body/embedding NOT NULL and scheme arms of the live gate (those are embedding concerns). `now` is epoch milliseconds from `tools/src/cmds.rs:1191` (`now_ms`) on the CLI and the same clock in the app copy; the app copy needs a `now_ms` source (a `SystemTime` call in the dispatch fn, passed in so tests control it).

Rejected: copying the three arm strings into both recall fns (drift; the repo has already had this bug class, see the `:311` comment and the issue #378 "every bypass site interpolates THIS" rule on `forgotten_entry_arm`).

### 3.2 Provenance and verified on the CLI/sidecar copy (B)

Add `e.source_type` and `e.okf_verified` to the SELECT. Emit `source_type` via `provenance_for` only when it is in the vocabulary, and `verified` via `wiki_graph::verified_label` (`wiki_graph.rs:55`, confirmed facts only) — the exact lines of `tool_dispatch.rs:498-515`. `verified_label` is `pub(crate)`; export a thin `pub` wrapper, or move the item-building into one shared `pub fn recall_item(...)` in the library that BOTH copies call (preferred: it also carries `kind` and the ordinal, and removes the copy drift for the item shape; the SQL stays per-copy, see OQ2). `okf_verified` can be absent on old shapes (it is in the migration-added OKF set, `schema_guard.rs:37-47`); the CLI probes it the way `has_area` is probed and selects `NULL` when missing.

### 3.3 `kind` field (C)

`kind` is the statement kind, stored in the `okf_type` column (`specs/engine-9x-adoption/spec.md:40-77`: the four kinds are manifest node types in `okf_type`, no separate column). Add `pub const KIND_VOCAB: &[&str] = &["rule","decision","lesson","trap"]` and `pub fn kind_for(okf_type: Option<&str>) -> Option<String>` beside `PROVENANCE_VOCAB`/`provenance_for` (`wisdom_match.rs:47`, `:273`), same shape. Anything else (NULL, the legacy `fact`, `handoff`, `session_recap`, unknown slugs, wrong case) maps to `None`.

Where it appears, always `#[serde(skip_serializing_if = "Option::is_none")]` / absent key:
- recall wiki items, both copies;
- `WisdomItem` (`wisdom_match.rs:135-150`), next to `band`/`area`. `gated_entries` adds `e.okf_type` to its SELECT only when the column exists (`has_okf_type` on `Temporal`, probed with the others at `:324` and `:348`, like `has_area`, `:296`), and `CandidateRow` (`:501`) gains one member;
- corrections: `load_item` (`:1740`) adds `okf_type` to its SELECT, behind the same probe (it runs without a `Temporal`, so it probes directly with `existing_columns`).

Every `WisdomItem { .. }` literal gets `kind` (`wisdom_match.rs:896`, `:1750`, `tools/src/bin/calibrate_two_stage.rs:1246`).

Byte pin: because absent means skipped, `message_mode_output_byte_identical` (`tool_dispatch.rs:2212`) must pass UNCHANGED for a fact with NULL `okf_type`. (It asserts the exact string at `:2258-2260`.) A second pin proves a typed fact (`rule`) DOES add `"kind":"rule"` — and only that — to the `ct wisdom match` JSON. That is an additive field on a cached surface; CTI ignores unknown keys (it reads named fields only, e.g. `ct_wisdom.py:270-279`), so no `SCHEMA_VERSION` bump, same reasoning as `band`/`area` (the pin's own comment, `:2200-2210`).

### 3.4 Confidence ordinal (D)

The CLI copy takes the app copy's ordinal (`confirmed=3, inferred=2, other=0`). Put it in the shared item helper so there is one definition.

### 3.5 Wiki leg when the chunk leg is empty (E)

`recall_cmd` restructure: open the brain and run the chunk leg; on zero chunks keep going with an empty chunk list; run the wiki leg; exit 2 only when BOTH are empty. JSON shape stays `{"results": [...], "wiki": [...]}`; `results` is `[]` when only wiki hits exist. Blank query still exits 2 before anything runs (`queries.rs:416-418`, pinned by `empty_query_exits_two`). `run_query` returns `Option<(Connection, Vec<ScoredChunk>)>` (`:411`); change it (or add a sibling used by `recall_cmd`) to always hand back the connection.

**CLI contract change.** Before: exit 2 whenever chunks are empty. After: exit 0 when any leg has hits; exit 2 only when both are empty. Text mode prints the wiki section when chunks are empty.

**Who parses the exit code** (all verified in the CTI tree): every host treats any non-zero exit as failure class `exit`, not memoized — `claude-code`, `hermes`, `docker-agent` `scripts/ct_wisdom.py:260-262`; `deepseek` and `opencode` `src/wisdom.ts:560`; `openclaw` `src/wisdom/bootstrap.ts:189` (`r.code !== 0` → `fail`, which feeds its circuit breaker). None distinguishes 2 from 1. Effects:
- Brains with wiki hits but zero chunk hits flip from a failed, un-memoized call (and, on openclaw, a breaker tick) to a successful block. This is the intended fix.
- Both-empty brains are unchanged: still exit 2, still a quiet `exit` failure. That is wrong in spirit (CTI rule 4: "nothing matches" should do nothing, not count as a failure) but it is CTI's parser to fix, not CT's contract. Recorded for the CTI PR; not changed here, because exit 2 is also the documented empty-query/no-result code for `search`/`code` (`ct_search_recall_code.rs:62`).
- No CTI change is required for PR-1 to be safe. `source_type`, `verified` and `kind` are additive keys on items; the hosts read `id`, `title`, `text` only (`ct_wisdom.py:273-279`). The superseded-fact fix needs no CTI code either: removed rows simply stop arriving. CTI should still add the labels to its block (a CTI PR, out of scope here).

## 4. Out of scope

- `ct wisdom brief` (PR-2) and the CTI consumer PR.
- Excluding drafts from recall (OQ1), embedding/scheme requirements on recall, any ranking change beyond the ordinal fix, corrections semantics on recall.
- Merging the two `rank_wiki_entries` SQL bodies (OQ2).
- Changing the exit code of both-empty recall.
- Re-processing stored facts; any migration.

## 5. Tests (names/locations in plan.md)

1. `entry_eligibility_where` byte pin: golden SQL strings for the four shapes (both temporal cols x forgotten present/absent, plus neither) captured BEFORE the refactor. `tools/tests/calibrate_two_stage_harness.rs:358` already mirrors the fragment; it must keep passing.
2. Recall eligibility: superseded with `superseded_at <= now` excluded; future `superseded_at` included; `superseded_by` set with NULL `superseded_at` excluded (matches the gate's `NULL > :now` = NULL = false); expired `valid_to` excluded; `valid_to` in the future included; forgotten excluded; deleted excluded.
3. Degraded shapes: no `superseded_by`/`superseded_at`, no `valid_to`, no events `forgotten_at`, no `okf_type`, no `okf_verified` — each still returns rows and no SQL error.
4. Provenance + verified on CLI/sidecar output (confirmed with `okf_verified` list, inferred without, out-of-vocabulary `source_type` omitted).
5. `kind`: `rule|decision|lesson|trap` emitted; NULL, `fact`, `handoff`, junk, `Rule` omitted — on recall items (both copies), `WisdomItem`, and a correction.
6. Byte pin `message_mode_output_byte_identical` unchanged and green; new typed-fact pin.
7. NEW pin of the `ct recall --json` shape: exact key set of `results[]` and `wiki[]` items.
8. Wiki leg runs with zero chunk hits (exit 0, `results: []`); both empty exits 2; blank query exits 2.
9. Confidence ordinal ordering on the CLI copy.

## 6. Benchmark gates (INTENT "How changes are made" step 4)

- **No superseded fact is ever injected: MANDATORY**, satisfied by test group 2 plus an end-to-end `ct recall` case (superseded row never in output).
- **Exactly-once / cached output byte-identical:** satisfied by the byte pin (test 6) and test 1.
- **LongMemEval hit rate, abstention precision, 200-prompt paraphrase:** see OQ3. Position: not required for this change. It only REMOVES rows that the live gate already forbids and ADDS labels; it adds no query path, floor, embedding or ranking input, so there is no hit-rate or abstention surface to measure. No in-repo LongMemEval runner exists (the 2026-11-13 benchmark records `docs/benchmarks/2026-11-13-working-memory-age-hit-rate.md:57` and the `rule10` spec both call it controller-side/pending infra). The in-repo gate harnesses (`scripts/calibrate-two-stage.sh`, `calibration-run/`, `tools/src/bin/calibrate_two_stage.rs`) calibrate `ct wisdom match` floors, which this PR does not touch.

## 7. Open questions for review

1. **Drafts.** Should `lifecycle_status = 'draft'` facts also be excluded from recall? Not required by INTENT rules 3/5; the live gate does not filter them either (`entry_eligibility_where` has no lifecycle arm). Recommendation: no, keep recall and the gate consistent; revisit with PR-2 if `brief` wants it.
2. **Unify the two `rank_wiki_entries` copies now?** Recommendation: share the predicate (`entry_recall_arms`) and the item builder now; leave the two SQL bodies and loops apart. A full merge changes the app copy's dependency surface (the app copy is `pub(crate)` in `src-tauri`, the CLI copy lives in `tools`) and is a separate refactor.
3. **Step-4 gates.** Confirm §6: mandatory no-superseded test only; waive LongMemEval/abstention/paraphrase for a removal-plus-label change. If the reviewer disagrees, no runnable harness exists in-repo and the run would be controller-side (a scratch brain with real embeddings, never the live brain).
4. **Both-empty exit code.** Keep 2 (documented family contract) or move CTI toward treating 2 as "nothing matched"? Recommendation: keep 2; fix in CTI.
5. **Supersession with NULL `superseded_at`.** The shared arm hides such rows (`NULL > :now` is not true). That matches the live gate, so recall now agrees with it. A reviewer should confirm the engine's `supersede` always stamps `superseded_at`.
6. **`kind` on `ct wisdom match`** touches a cached, replayed surface (additive, absent for untyped facts). Confirm the "no `SCHEMA_VERSION` bump" ruling extends from `band`/`area` to `kind`.
7. **Performance.** The wiki leg now runs on brains where it previously did not (zero chunk hits). It is one LIKE scan per term, as before; the 5 s CTI recall timeout is unchanged. Worth a reviewer glance on large brains.
8. **Sidecar JSON.** `curated_recall_context` (sidecar, `curated_thoughts_mcp.rs:235`, and app, `tool_dispatch.rs:679`) gains `kind` and drops superseded rows. Both are consumed by agents, not parsers; confirm no MCP client pins the old shape.

## 8. Files touched

- `src-tauri/src/wisdom_match.rs`: `temporal_arms_sql`, `entry_recall_arms`/`RecallArms`, `KIND_VOCAB`/`kind_for`, `Temporal.has_okf_type`, `WisdomItem.kind`, `CandidateRow`, `gated_entries`, `load_item`, tests.
- `src-tauri/src/tool_dispatch.rs`: app `rank_wiki_entries` (arms, `kind`, `now`), tests.
- `src-tauri/src/wiki_graph.rs`: expose `verified_label` publicly if the shared item builder lives in the library.
- `tools/src/queries.rs`: CLI `rank_wiki_entries`, `run_query`, `recall_cmd`.
- `tools/src/bin/curated_thoughts_mcp.rs`: wrapper unchanged; test at `:720`.
- `tools/src/bin/calibrate_two_stage.rs`: `WisdomItem` literal (`:1246`).
- Tests: `tools/tests/ct_search_recall_code.rs`, `tools/tests/forgotten_read_gate.rs`, `tools/tests/ct_wisdom_match.rs`, `tools/tests/calibrate_two_stage_harness.rs` (must stay green).
- `specs/2026-11-27-recall-eligibility-provenance/` (this folder).
- NOT touched: INTENT.md (no rule changes; rules 3/5/10 already state the target), the live brain.

## 9. Rejected alternatives

- **Reuse `entry_eligibility_where` directly for recall.** It carries embedding, scheme and NOT NULL arms; recall is keyword-based and must still surface facts without embeddings.
- **Copy the arms.** Drift; see 3.1.
- **Emit `kind` from a new column.** None exists; `okf_type` is the engine's store.
- **Fail recall (exit 1) on an unreadable shape.** Fail closed on probe errors, yes (matches the forgotten arm); degrade only on genuinely missing columns.
