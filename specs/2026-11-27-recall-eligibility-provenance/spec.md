# `recall-eligibility-provenance`: `ct recall` stops serving superseded facts and labels what it serves (PR-1)

**Date:** 2026-11-27
**Status:** Draft
**Review record:** Rev 2 (2026-11-27): folds Fable design review (APPROVE WITH NITS; M1-M4, m1-m8, nits). The owner waived the GLM review rung (2026-11-27); the two reviews are Fable (design + code) and /code-review.
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

### 3.1 Shared eligibility predicate (A)

Add to `wisdom_match.rs` one `pub(crate)` fn `temporal_arms_sql(t: &Temporal) -> String`. It returns the three arms now written inline in `entry_eligibility_where` (`:626-635`), in the same order and with the same text: superseded, valid_to, forgotten (via `forgotten_entry_arm`, `:640`). `entry_eligibility_where` becomes `base + temporal_arms_sql(t)`. Its output must stay byte-identical (pinned, §5 test 1).

Because the `tools` crate cannot see `pub(crate)` items, add a public, connection-taking wrapper in the style of `entry_not_forgotten_arm` (`:660`): `pub fn entry_recall_arms(conn) -> Result<RecallArms>` where `RecallArms { sql: String, needs_now: bool, has_source_type: bool, has_okf_type: bool, has_okf_verified: bool }`. The three `has_*` flags come from one `existing_columns(conn, "llm_wiki_entries")` snapshot, so the CLI and PR-2 share one probe; the CLI must NOT select `e.source_type` unprobed (`tools/src/queries.rs:194-196`; CTI `ct_preflight.py:350` treats brains without `source_type` as reachable). `existing_columns` swallows row errors (`src-tauri/src/db/ddl_compat.rs:182-191`), so `entry_recall_arms` returns `Err` if the probed column list lacks `id` (a failed or empty probe must fail closed, not degrade to "no columns"). It probes with `temporal_columns_shape_only` (`:321`), NOT `temporal_columns` (`:342`). Reason: the `:311-320` doc comment — a corrupt `wisdom_active_scheme` must not fail reads that use no embeddings, and recall uses none. The app copy already probes shape-only (`src-tauri/src/tool_dispatch.rs:449-452`), so this keeps both copies on the same probe. It fails closed (errors propagate), like the forgotten arm. Both `rank_wiki_entries` copies call it, replacing their forgotten-arm calls, and bind `:now` only when `needs_now` (binding an unused named param is a rusqlite error, see `eligible_entry_ids`, `:703-722`). The recall statement switches from positional `?1` to named `:term` so it can mix with `:now`.

Degraded shapes follow `temporal_columns`: a missing `superseded_by`, `valid_to`, or events `forgotten_at`/`related_entry_id` column drops only that arm. Pre-migration brains stay readable. The CLI opens brains read-only without migrating (`src-tauri/src/retrieval/mod.rs:156-165`), so such shapes are reachable there.

Recall's eligibility therefore = `deleted_at IS NULL` + the three arms. It deliberately omits the title/body/embedding NOT NULL and scheme arms of the live gate (those are embedding concerns). `now` is epoch milliseconds from `tools/src/cmds.rs:1191` (`now_ms`) on the CLI and the same clock in the app copy; the app copy needs a `now_ms` source (a `SystemTime` call in the dispatch fn, passed in so tests control it). Both copies get a clock-injectable core: `rank_wiki_entries_at(conn, q, n, now_ms)`, with the public 3-arg `rank_wiki_entries(conn, q, n)` a thin wrapper that supplies the real clock (tests call the `_at` form).

**Exact claim (replaces "one source of truth").** The two recall copies and the live gate share one temporal-arm text. Two non-adopting copies of the superseded/valid_to arms exist and are out of scope (no arm is forgotten there; they are listed so the claim is not overstated): `src-tauri/src/db/queue.rs:319-323` and `tools/src/bin/calibrate_two_stage.rs:1163-1167`.

Rejected: copying the three arm strings into both recall fns (drift; the repo has already had this bug class, see the `:311` comment and the issue #378 "every bypass site interpolates THIS" rule on `forgotten_entry_arm`).

### 3.2 Provenance and verified on the CLI/sidecar copy (B)

Add `e.source_type` and `e.okf_verified` to the SELECT. Emit `source_type` via `provenance_for` only when it is in the vocabulary, and `verified` via `wiki_graph::verified_label` (`wiki_graph.rs:55`, confirmed facts only) — the exact lines of `tool_dispatch.rs:498-515`. `verified_label` is `pub(crate)`. Move the item-building into one shared `pub fn recall_item(input: RecallItemInput) -> serde_json::Value` in the library that BOTH copies call, taking a struct (not nine positional args: clippy `too_many_arguments`, and several adjacent `Option<&str>` are swappable). It also carries `kind` and the ordinal; the SQL stays per-copy (see §7). `verified_label` stays private (`pub(crate)`) when `recall_item` lives in the lib; no new public wrapper. `okf_verified` can be absent on old shapes (it is in the migration-added OKF set, `schema_guard.rs:37-47`); the CLI uses `RecallArms.has_okf_verified` (and `has_source_type`, `has_okf_type`) and selects `NULL` when missing.

### 3.3 `kind` field (C)

`kind` is the statement kind, stored in the `okf_type` column (`specs/engine-9x-adoption/spec.md:40-77`: the four kinds are manifest node types in `okf_type`, no separate column). Add `pub const KIND_VOCAB: &[&str] = &["rule","decision","lesson","trap"]` and `pub fn kind_for(okf_type: Option<&str>) -> Option<String>` beside `PROVENANCE_VOCAB`/`provenance_for` (`wisdom_match.rs:47`, `:273`), same shape. Also add `pub fn kind_tier(Option<&str>) -> u8` beside `kind_for` for PR-2 (rule/trap 0, decision 1, lesson 2, none 3), with a unit test. Anything else (NULL, the legacy `fact`, `handoff`, `session_recap`, unknown slugs, wrong case) maps to `None`.

Where it appears, always `#[serde(skip_serializing_if = "Option::is_none")]` / absent key:
- recall wiki items, both copies;
- `WisdomItem` (`wisdom_match.rs:135-150`), next to `band`/`area`. `gated_entries` adds `e.okf_type` to its SELECT only when the column exists (`has_okf_type` on `Temporal`, probed with the others at `:324` and `:348`, like `has_area`, `:296`), and `CandidateRow` (`:501`) gains one member. Index: append `e.okf_type` LAST in the SELECT and read it at index `6 + has_area as usize` (mapper at `wisdom_match.rs:787-801`);
- corrections: `load_item` (`:1740`) adds `okf_type` to its SELECT, behind the same probe. `load_item` DOES run under a `Temporal` (`find_corrections` holds one, `wisdom_match.rs:1651-1675`): pass `has_okf_type` through; no per-correction PRAGMA.

Every `WisdomItem { .. }` literal gets `kind` (`wisdom_match.rs:896`, `:1750`, `tools/src/bin/calibrate_two_stage.rs:1246`).

Byte pin: because absent means skipped, `message_mode_output_byte_identical` (`tool_dispatch.rs:2212`) must pass UNCHANGED for a fact with NULL `okf_type`. (It asserts the exact string at `:2258-2260`.) A second pin proves a typed fact (`rule`) DOES add `"kind":"rule"` — and only that — to the `ct wisdom match` JSON. That is an additive field on a cached surface; CTI ignores unknown keys (it reads named fields only, e.g. `ct_wisdom.py:270-279`), so no `SCHEMA_VERSION` bump, same reasoning as `band`/`area` (the pin's own comment, `:2200-2210`).

### 3.4 Confidence ordinal (D)

Both copies use ONE ordinal, defined once in the shared item helper (`confidence_ordinal`): `confirmed`, `certain`, `verified` = 3; `inferred` = 2; anything else (incl. `tentative`, NULL) = 0. **Owner ruling OQ-B (2026-11-27):** engine-native `certain` and legacy `verified` take the top rank. This changes the app copy's order too (it previously mapped them to 0). The ordinal table here is the single reference the tests derive from.

**Deterministic tie-break (owner ruling OQ-C, 2026-11-27).** Both copies order by (term overlap desc, ordinal desc, `updated_at` desc, `id` asc). The final `id` key makes output order a pure function of the rows, so the same brain always yields the same bytes (CTI rule 2; PR-2's brief relies on it). Pin tests assert order on rows that tie on every key but `id`.

D changes CLI/sidecar ordering. The existing sidecar test `recall_wiki_tolerates_mixed_updated_at_types` (`tools/src/bin/curated_thoughts_mcp.rs:713-730`) asserts `["middle","newest","unknown"]`; `middle` has confidence `verified`, which the current ordinal maps to 0, so the implementer re-derives the expectation from the final table above (with `verified` = 3 the old order is expected to stand) and records the derivation in the test comment. Its stale comment ("ties broken by confidence ('certain' first)") is fixed. Its fixture (`open_fixture_db`, `:672-688`, lacking `okf_type`/`okf_verified`/temporal columns) is the named degraded-shape canary.

### 3.5 Wiki leg when the chunk leg is empty (E)

`recall_cmd` restructure: open the brain and run the chunk leg; on zero chunks keep going with an empty chunk list; run the wiki leg; exit 2 only when BOTH are empty. JSON shape stays `{"results": [...], "wiki": [...]}`; `results` is `[]` when only wiki hits exist. Blank query still exits 2 before anything runs (`queries.rs:416-418`, pinned by `empty_query_exits_two`). `run_query` returns `Option<(Connection, Vec<ScoredChunk>)>` (`:411`); E adds results on zero-chunk brains; change it (or add a sibling used by `recall_cmd`) to always hand back the connection.

**CLI contract change.** Before: exit 2 whenever chunks are empty. After: exit 0 when any leg has hits; exit 2 only when both are empty. Text mode prints the wiki section when chunks are empty.

**Who parses the exit code** (all verified in the CTI tree): every host treats any non-zero exit as failure class `exit`, not memoized — `claude-code`, `hermes`, `docker-agent` `scripts/ct_wisdom.py:260-262`; `deepseek` and `opencode` `src/wisdom.ts:560`; `openclaw` `src/wisdom/bootstrap.ts:189` (`r.code !== 0` → `fail`, which feeds its circuit breaker). None distinguishes 2 from 1. Effects:
- Brains with wiki hits but zero chunk hits flip from a failed, un-memoized call (and, on openclaw, a breaker tick) to a successful block. This is the intended fix.
- Both-empty brains are unchanged: still exit 2, still a quiet `exit` failure. That is wrong in spirit (CTI rule 4: "nothing matches" should do nothing, not count as a failure) but it is CTI's parser to fix, not CT's contract. Recorded for the CTI PR; not changed here, because exit 2 is also the documented empty-query/no-result code for `search`/`code` (`ct_search_recall_code.rs:62`).
- No CTI change is required for PR-1 to be safe. `source_type`, `verified` and `kind` are additive keys on items; the hosts read `id`, `title`, `text` only (`ct_wisdom.py:273-279`). The superseded-fact fix needs no CTI code either: removed rows simply stop arriving. CTI should still add the labels to its block (a CTI PR, out of scope here).

## 4. Out of scope

- `ct wisdom brief` (PR-2) and the CTI consumer PR.
- Excluding drafts from recall (§7), embedding/scheme requirements on recall, any ranking change beyond the ordinal fix, corrections semantics on recall.
- Merging the two `rank_wiki_entries` SQL bodies (§7).
- Non-adopting temporal-arm copies (`queue.rs:319-323`, `calibrate_two_stage.rs:1163-1167`) and sibling read surfaces (OQ-D).
- Changing the exit code of both-empty recall.
- Re-processing stored facts; any migration.

## 5. Tests (names/locations in plan.md)

1. `entry_eligibility_where` byte pin: golden SQL strings for the shapes (both temporal cols x forgotten present/absent, plus neither, plus one with `scheme: Some(..)`) captured BEFORE the refactor, built through one helper. `tools/tests/calibrate_two_stage_harness.rs:358` already mirrors the fragment; it must keep passing.
2. Recall eligibility: superseded with `superseded_at <= now` excluded; future `superseded_at` included; `superseded_by` set with NULL `superseded_at` excluded (matches the gate's `NULL > :now` = NULL = false); expired `valid_to` excluded; `valid_to` in the future included; forgotten excluded; deleted excluded.
3. Degraded shapes: no `superseded_by`/`superseded_at`, no `valid_to`, no events `forgotten_at`, no `okf_type`, no `okf_verified` — each still returns rows and no SQL error.
4. Provenance + verified on CLI/sidecar output (confirmed with `okf_verified` list, inferred without, out-of-vocabulary `source_type` omitted).
5. `kind`: `rule|decision|lesson|trap` emitted; NULL, `fact`, `handoff`, junk, `Rule` omitted — on recall items (both copies), `WisdomItem`, and a correction.
6. Byte pin `message_mode_output_byte_identical` unchanged and green; new typed-fact pin.
7. NEW pin of the `ct recall --json` shape: exact key set of `results[]` and `wiki[]` items.
8. Wiki leg runs with zero chunk hits (exit 0, `results: []`); both empty exits 2; blank query exits 2.
9. Confidence ordinal ordering on the CLI copy, plus the updated sidecar test (§3.4, re-derive after OQ-B). Pin tests use DISTINCT `updated_at` values so order is not a tie.
10. `kind_tier` unit test.

## 6. Benchmark gates (INTENT "How changes are made" step 4)

- **No superseded fact is ever injected: MANDATORY**, satisfied by test group 2 plus an end-to-end `ct recall` case.
- **Exactly-once / cached output byte-identical:** satisfied by the byte pin (test 6) and test 1.
- **LongMemEval hit rate, abstention precision, 200-prompt paraphrase: WAIVED by owner ruling (OQ-A, 2026-11-27).** INTENT step 4 had no waiver clause, so this PR carries a one-line dated INTENT amendment (§8) allowing owner-ruled waivers for compliance/eligibility fixes whose matcher SQL stays byte-identical; the ruling is also recorded in the PR body. This PR does two things that change retrieval output: D changes CLI/sidecar ordering (confirmed above inferred), and E adds results on zero-chunk brains. Rationale for waiving anyway: (1) the gate SQL is byte-pinned (test 1), so `ct wisdom match` selection is untouched; (2) LongMemEval, abstention and paraphrase measure the untouched `wisdom match` gate; (3) D aligns the CLI to the shipped app ordinal rather than introducing a new ranking; (4) the wiki leg uses no embeddings, so there is no embedding/floor surface to measure. No in-repo LongMemEval runner exists (`docs/benchmarks/2026-11-13-working-memory-age-hit-rate.md:57`); the in-repo harnesses calibrate `ct wisdom match` floors, which this PR does not touch. If the owner declines, the run is controller-side on a scratch brain with real embeddings, never the live brain.
- **Review rungs:** the owner waived the GLM review rung (2026-11-27). The two reviews are Fable (design + code) and /code-review. The same INTENT amendment records this in step 3.

## 7. Resolved questions (reviewer answers) and owner decisions

Resolved (Fable design review):
1. **Drafts:** no. Recall stays consistent with the gate (no lifecycle arm); revisit with PR-2.
2. **Unify the two copies:** share the predicate (`entry_recall_arms`), the item builder (`recall_item`) and the column probes (`RecallArms.has_*`). Keep the two SQL bodies and loops apart.
3. **Both-empty exit:** keep 2.
4. **NULL `superseded_at`:** hidden. The engine's `supersede` always stamps `superseded_at`; only an import may carry NULL, and hiding it matches the gate.
5. **No `SCHEMA_VERSION` bump** for `kind`. Note `kind` changes default-mode bytes for typed facts. CTI already uses a dict key named `kind` (= `entry|correction`) in `ct_wisdom_verbose.py:149`, so the CTI consumer PR must not merge the two dicts.
6. **Performance:** fine (one LIKE scan per term, as before).
7. **Sidecar:** CTI parses `wiki_entries[].id` and passes the rest through (`ct_wisdom_cc.py:536-553`); the additive keys are safe.

## 7a. Owner decisions (ruled 2026-11-27)

- **OQ-A:** step-4 benchmark waiver RATIFIED for this PR (§6), with a one-line dated INTENT amendment (§8).
- **OQ-B:** `certain` and `verified` map to the top rank (§3.4).
- **OQ-C:** deterministic `id` asc tie-break after `updated_at` in both copies (§3.4).
- **OQ-D:** sibling surfaces that still serve superseded/expired facts (`curated_get_wiki_entry`, `ct wiki list|get` at `tools/src/queries.rs:608-640`, `wiki_graph.rs`) stay out of scope; tracked in a follow-up issue (linked in the PR body).

## 8. Files touched

- `src-tauri/src/wisdom_match.rs`: `temporal_arms_sql`, `entry_recall_arms`/`RecallArms`, `KIND_VOCAB`/`kind_for`, `Temporal.has_okf_type`, `WisdomItem.kind`, `CandidateRow`, `gated_entries`, `load_item`, tests.
- `src-tauri/src/tool_dispatch.rs`: app `rank_wiki_entries` (arms, `kind`, `now`), tests.
- `src-tauri/src/wiki_graph.rs`: expose `verified_label` publicly if the shared item builder lives in the library.
- `tools/src/queries.rs`: CLI `rank_wiki_entries`, `run_query`, `recall_cmd`.
- `tools/src/bin/curated_thoughts_mcp.rs`: wrapper unchanged; test at `:720`.
- `tools/src/bin/calibrate_two_stage.rs`: `WisdomItem` literal (`:1246`).
- Tests: `tools/tests/ct_search_recall_code.rs`, `tools/tests/forgotten_read_gate.rs`, `tools/tests/ct_wisdom_match.rs`, `tools/tests/calibrate_two_stage_harness.rs` (must stay green).
- `specs/2026-11-27-recall-eligibility-provenance/` (this folder).
- `INTENT.md` "How changes are made": one dated amendment line each in steps 3 and 4 recording the 2026-11-27 owner rulings (GLM rung waived; step-4 waiver for compliance/eligibility fixes with byte-identical matcher SQL). No rule text changes; rules 3/5/10 already state the target.
- NOT touched: the live brain.

## 9. Rejected alternatives

- **Reuse `entry_eligibility_where` directly for recall.** It carries embedding, scheme and NOT NULL arms; recall is keyword-based and must still surface facts without embeddings.
- **Copy the arms.** Drift; see 3.1.
- **Emit `kind` from a new column.** None exists; `okf_type` is the engine's store.
- **Fail recall (exit 1) on an unreadable shape.** Fail closed on probe errors, yes (matches the forgotten arm); degrade only on genuinely missing columns.
