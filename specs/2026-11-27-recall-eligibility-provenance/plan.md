# Plan: recall-eligibility-provenance (PR-1)

Transient scaffolding; deleted in the same-PR cleanup commit. Spec: `spec.md`. Rules: TDD (write the failing test, watch it fail, then change), one commit per task, no INTENT edits, never the live brain (tests use scratch profiles / in-memory DBs). Run cargo SERIAL, from the repo root, with:

```
export CARGO_TARGET_DIR=/Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/.cargo-shared-target
df -h "$(dirname "$CARGO_TARGET_DIR")"   # >= 30G free first (AGENTS.md)
```

Command shorthand below: `CT-TOOLS` = `cargo test -p curated-thoughts-tools` (crate name verified, `tools/Cargo.toml:2`), `CT-APP` = `cargo test -p curated-thoughts --lib` (package name `curated-thoughts`, lib `tauri_app_lib`, `src-tauri/Cargo.toml:2,16`). Always add a test-name filter after `--`.

If a step does not match the spec, STOP and ask; do not invent.

## Task 1: pin `entry_eligibility_where` BEFORE touching it
- Files: `src-tauri/src/wisdom_match.rs` (tests module, starts ~`:1764`).
- Failing test first: `entry_eligibility_where_sql_is_byte_pinned`. Build `Temporal` values by hand (fields: `has_superseded_by`, `has_valid_to`, `has_area`, `has_forgotten_at`, `scheme`; `scheme` is private but the test is in-module) for: all columns; no `superseded_by`; no `valid_to`; neither; no `forgotten_at`. Assert `entry_eligibility_where(&t)` equals a literal string. Generate the literals by RUNNING the current function once (print them), paste them in. This test must PASS on unmodified code; it is a golden, not a red test.
- Run: `CT-APP -- entry_eligibility_where_sql_is_byte_pinned`.
- Also run `cargo test -p curated-thoughts-tools --test calibrate_two_stage_harness` and note it is green (it mirrors the fragment at `:358`).
- Commit: `test(wisdom): pin entry_eligibility_where SQL`.

## Task 2: factor `temporal_arms_sql`
- Files: `src-tauri/src/wisdom_match.rs`.
- Change: move the three arms (superseded, valid_to, forgotten, now `:626-635`) into `pub(crate) fn temporal_arms_sql(t: &Temporal) -> String`; `entry_eligibility_where` = existing base string + `temporal_arms_sql(t)`. No text change.
- Test: Task 1's pin plus `temporal_arms_sql_is_the_suffix_of_the_eligibility_where` (assert `entry_eligibility_where(&t).ends_with(&temporal_arms_sql(&t))` for each shape).
- Run: `CT-APP -- wisdom_match`. All existing tests green.
- Commit: `refactor(wisdom): factor temporal_arms_sql`.

## Task 3: public `entry_recall_arms` for the tools crate
- Files: `src-tauri/src/wisdom_match.rs`.
- Failing test first: `entry_recall_arms_probe_shapes` in the same tests module, using `open_in_memory()` (full shape: `needs_now` true, sql contains `e.superseded_by`, `e.valid_to`, the forgotten arm) and a hand-built degraded table (create `llm_wiki_entries` WITHOUT `superseded_by`/`superseded_at`/`valid_to`, no events table columns: `needs_now` false, sql empty or forgotten-only per shape).
- Change: `pub struct RecallArms { pub sql: String, pub needs_now: bool }` and `pub fn entry_recall_arms(conn: &Connection) -> Result<RecallArms>`: `temporal_columns_shape_only(conn)?` then `temporal_arms_sql(&t)`; `needs_now = t.has_superseded_by || t.has_valid_to`. Errors propagate (fail closed). Do NOT call `temporal_columns` (no scheme resolution).
- Run: `CT-APP -- entry_recall_arms`.
- Commit: `feat(wisdom): entry_recall_arms shared recall eligibility`.

## Task 4: `KIND_VOCAB` / `kind_for` and the shared recall item builder
- Files: `src-tauri/src/wisdom_match.rs` (vocab next to `PROVENANCE_VOCAB` `:47`, fn next to `provenance_for` `:273`); `src-tauri/src/wiki_graph.rs` (`verified_label` `:55`: make it `pub`).
- Failing tests first (in `wisdom_match.rs` tests): `kind_for_normalizes` — `rule|decision|lesson|trap` map to themselves; `None`, `"fact"`, `"handoff"`, `"session_recap"`, `"Rule"`, `""`, `"zebra"` map to `None`. And `recall_item_fields` for the builder below: provenance, verified, kind, absent keys, ordinal.
- Change: add `pub fn recall_item(id, entity_id, title, text, source_ref, confidence, source_type, okf_verified, okf_type) -> serde_json::Value` plus `pub fn confidence_ordinal(Option<&str>) -> i64` (`confirmed=3, inferred=2, else 0`, copied from `tool_dispatch.rs:519-523`). The item always has `id, entity_id, title, text, source_ref, confidence`; adds `source_type` via `provenance_for`, `verified` via `verified_label` (same code as `tool_dispatch.rs:498-515`), and `kind` via `kind_for`; absent keys when `None`.
- Run: `CT-APP -- kind_for_normalizes recall_item`.
- Commit: `feat(wisdom): kind vocabulary and shared recall item builder`.

## Task 5: app `rank_wiki_entries` uses the shared pieces
- Files: `src-tauri/src/tool_dispatch.rs` (`:434-540`; tests ~`:2158`, `:2212`, helper `seed_file_db` `:2010`).
- Failing tests first (tool_dispatch tests): `recall_excludes_superseded`, `recall_future_superseded_at_still_included`, `recall_excludes_expired_valid_to`, `recall_future_valid_to_included`, `recall_excludes_forgotten_and_deleted`, `recall_item_carries_kind`. Seed through a full-shape DB (`open_in_memory()`), setting `superseded_by`/`superseded_at`/`valid_to` by UPDATE. `rank_wiki_entries` needs a `now`; add a `now_ms: i64` parameter (test passes a fixed value like `1_000_000`; the dispatch caller `:679` passes the clock). Update the three existing callers in tests.
- Change: replace the forgotten-arm lines (`:445-448`) with `entry_recall_arms(conn)?`; named params `:term`/`:now` (bind `:now` only if `needs_now`); build items with `recall_item`; use `confidence_ordinal`; add `e.okf_type` to the SELECT (the app always opens a migrated DB, so no probe here, but verify `seed_file_db` at `:2010` already has `okf_type`; it does, `:2031`).
- Existing `message_mode_output_byte_identical` (`:2212`) MUST pass unchanged: run it explicitly.
- Run: `CT-APP -- tool_dispatch::tests` then `CT-APP -- message_mode_output_byte_identical`.
- Commit: `fix(recall): app recall skips superseded/expired facts, adds kind`.

## Task 6: `kind` on `WisdomItem`, `gated_entries`, `load_item` (corrections)
- Files: `src-tauri/src/wisdom_match.rs` (`Temporal` `:290`, both probes `:324`/`:348`, `WisdomItem` `:135`, `CandidateRow` `:501`, `gated_entries` `:725`, `load_item` `:1740`); `tools/src/bin/calibrate_two_stage.rs:1246`.
- Failing tests first (wisdom_match tests): `wisdom_item_kind_rule_serializes`, `wisdom_item_kind_absent_for_null_fact_unknown`, `correction_item_carries_kind`, `degraded_shape_without_okf_type_still_matches` (hand-built table lacking `okf_type`), and a second byte pin in `tool_dispatch.rs` tests, `message_mode_typed_fact_adds_only_kind`: same fixture as `:2212` but `okf_type = 'rule'`; expected string is the existing one with `,"kind":"rule"` appended after `"provenance":"librarian_inferred"` (field order = struct order: put `kind` AFTER `area`, so for an unscoped item it follows `provenance`; confirm by running).
- Change: `Temporal.has_okf_type` (probe in BOTH constructors; add the field to any other `Temporal {` literal, grep shows only the two); `WisdomItem.kind: Option<String>` with `#[serde(skip_serializing_if = "Option::is_none")]`; `gated_entries` appends `e.okf_type` to the SELECT only when `has_okf_type` (follow the `has_area` pattern at `:765-771`; mind the positional column index in the mapper); `CandidateRow` gains one `Option<String>`; the loop (`:836`) destructures it and sets `kind: kind_for(..)`; `load_item` probes the column with `crate::db::ddl_compat::existing_columns` and selects `okf_type` or `NULL`. Add `kind: None`/value to the three `WisdomItem {` literals (`:896`, `:1750`, `calibrate_two_stage.rs:1246`).
- Run: `CT-APP -- wisdom_match`, `CT-APP -- message_mode`, and `cargo test -p curated-thoughts-tools --test calibrate_two_stage_harness`.
- Commit: `feat(wisdom): additive kind field on items and corrections`.

## Task 7: CLI/sidecar `rank_wiki_entries`
- Files: `tools/src/queries.rs` (`:176-254`), tests `tools/tests/forgotten_read_gate.rs`, `tools/tests/ct_search_recall_code.rs`, `tools/tests/common/mod.rs` (seeding helpers, `seed_fact` `:158`), sidecar test `tools/src/bin/curated_thoughts_mcp.rs:720`.
- Failing tests first (new file `tools/tests/ct_recall_eligibility.rs`, `mod common;`): superseded excluded, future `superseded_at` included, `superseded_by` with NULL `superseded_at` excluded, expired and future `valid_to`, forgotten (reuse `event()` pattern from `forgotten_read_gate.rs`), deleted, `source_type` + `verified` on items (confirmed row with `okf_verified = '[{"by":"human:t","at":5}]'`; check the exact stored list shape in `wiki_graph.rs:55-90` first), out-of-vocab `source_type` omitted, `kind` normalization (`rule` kept; `fact`, `bogus` omitted), confidence ordinal (`confirmed` before `inferred`, equal overlap), degraded shapes (open a hand-built DB missing: superseded cols; `valid_to`; events `forgotten_at`; `okf_type`; `okf_verified` — each query must succeed). Call `curated_thoughts_tools::queries::rank_wiki_entries(&conn, q, n)` directly with a `brain_db()` connection, setting columns by UPDATE.
- Change: SELECT adds `e.source_type, e.okf_verified, e.okf_type` (the last two replaced by `NULL AS ...` when a probe via `existing_columns` says the column is absent); `entry_recall_arms(conn)?` replaces the forgotten-arm call (`:193`), `:term`/`:now` named params (`now` from `crate::cmds::now_ms()` or equivalent path; check the module path), `recall_item(...)` builds the JSON, `confidence_ordinal` for the sort key (change the tuple's third element from `String` to `i64`). `rank_wiki_entries` keeps its public signature `(conn, query, limit)` (callers: `recall_cmd`, sidecar wrapper `curated_thoughts_mcp.rs:114`, `forgotten_read_gate.rs`).
- Run: `cargo test -p curated-thoughts-tools --test ct_recall_eligibility --test forgotten_read_gate --test shared_queries`, then `cargo test -p curated-thoughts-tools --bin curated-thoughts-mcp` (bin name verified, `tools/Cargo.toml:65`).
- Commit: `fix(recall): ct/sidecar recall skips superseded/expired facts, adds provenance, kind, ordinal`.

## Task 8: `ct recall` runs the wiki leg with zero chunk hits
- Files: `tools/src/queries.rs` (`run_query` `:411`, `recall_cmd` `:448`); tests `tools/tests/ct_search_recall_code.rs`.
- Failing tests first: `recall_runs_wiki_leg_with_zero_chunk_hits` (seeded brain with a wiki fact but a query that matches no chunk: expect exit 0, `results == []`, `wiki` has the fact); `recall_both_empty_exits_two`; keep `empty_query_exits_two` (`:62`) green; `recall_json_shape_is_pinned`: assert exact key sets of the top-level object (`results`, `wiki`), a `results[]` item (copy keys from `ScoredChunk` serialization, run once to read them) and a `wiki[]` item (`id, entity_id, title, text, source_ref, confidence, source_type` plus absent `verified`/`kind` for an inferred untyped fact). Check how `with_seeded_brain` seeds chunks so the no-chunk-hit query is reliable under `CURATED_EMBED_STUB=constant8` (the stub gives every text the same direction, so a chunk may always match; if so, seed a brain WITHOUT chunks, using the wiki-only fixture, and say so in the test comment).
- Change: add a `run_query_conn` (or make `run_query` return `(Connection, Vec<ScoredChunk>)` for `recall_cmd`'s use only; `search_cmd`/`code_cmd` keep returning exit 2 on empty). `recall_cmd`: blank query -> exit 2 (unchanged); otherwise run chunks, run wiki (limit 5, unchanged), `if chunks.is_empty() && wiki.is_empty() { return Ok(EXIT_NO_RESULTS) }`, else print as today. Update the doc comment to state the new contract.
- Run: `cargo test -p curated-thoughts-tools --test ct_search_recall_code`.
- Commit: `fix(recall): wiki leg runs when the chunk leg is empty`.

## Task 9: sweep and docs
- Run the full affected suites serially: `CT-APP`, `cargo test -p curated-thoughts-tools`, then `cargo clippy` the touched crates if the repo's CI does. Record counts in LEDGER.
- Grep `docs/` and `README.md` for statements describing `ct recall` exit codes or wiki fields; update them (CLI contract change, spec 3.5). Do not edit INTENT.md.
- Verify the end-to-end no-superseded case by hand on a SCRATCH brain only (never the live brain): supersede a fact, `ct recall`, confirm it is absent.
- Benchmark gates: mandatory no-superseded test is Task 5/7; record the OQ3 disposition in the PR body.
- Flip spec Status to `Reviewed` only after review; to `Implemented` at the end; then delete `plan.md` and `LEDGER.md` in the cleanup commit (AGENTS.md).
