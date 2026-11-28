# Plan: `ct wisdom brief` + delivery log (PR-2)

Spec: `spec.md` (read it first; section numbers below point into it). TDD: write the failing test, run it and SEE it fail, make the change, run it green. One commit per task. Never touch a live brain: tests use `with_seeded_brain` (scratch brain dir). Rust runs SERIAL, one cargo at a time. If anything in the spec is unclear, STOP and ask.

## Setup (every shell)

```
export CARGO_TARGET_DIR=/Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/.cargo-shared-target
df -h "$(dirname "$CARGO_TARGET_DIR")"   # need >= 30G free
```
Library (src-tauri) command form, run from the crate dir:
`cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/.worktrees/wisdom-brief/src-tauri && cargo test --features test-utils,mcp-server <filter> -- --test-threads=1`
Tools crate form: `cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/.worktrees/wisdom-brief/tools && cargo test --test <file> <filter> -- --test-threads=1`
Every test run must show `>= 1 passed` for the new tests (a filter that matches nothing prints `0 passed`: that is a FAILURE of the step). On linker "Bus error" or ENOSPC: stop and report; never delete caches.

## Task 1: `RecallArms.has_area`

- Files: `src-tauri/src/wisdom_match.rs` (struct `RecallArms` ~:450, `entry_recall_arms` ~:466, tests module).
- Failing test first: `recall_arms_reports_has_area` in the `wisdom_match.rs` tests: in-memory DB with a full `llm_wiki_entries`+`llm_wiki_events` shape (copy the helper an existing `entry_recall_arms` test uses) with and without an `area` column; assert `has_area` true/false.
- Change: add `pub has_area: bool` (value `t.has_area`, already computed by `temporal_from_snapshot`).
- Run: `... cargo test --features test-utils,mcp-server recall_arms_reports_has_area -- --test-threads=1`, then the whole `wisdom_match` filter to be sure no literal elsewhere broke.

## Task 2: hub resolver, `org_group_for` + `resolve_brief_scope` (spec 3.2)

- Files: `src-tauri/src/config/hub_resolver.rs` (+ its tests module).
- Failing tests first (names): `brief_scope_bound_repo_and_org`, `brief_scope_no_prefix_gives_org_only`, `brief_scope_no_prefix_no_org_is_none`, `brief_scope_unconfigured`, `brief_scope_unresolved_collision_gives_org_only`, `brief_scope_org_unresolvable_sets_reason`, `brief_scope_ignores_unusable_areas_file`, `brief_scope_group_includes_merged_losers`. Reuse the fixture style of the existing `resolve_scoped` tests in the same file.
- Change:
  1. Move the org block of `resolve_scoped` (`:433-452`) into private `fn org_group_for(conn, cfg) -> (Option<Vec<String>>, Vec<String>)`; call it from `resolve_scoped`. Do not change any output.
  2. Add `pub struct BriefScope { pub repo_group: Option<Vec<String>>, pub org_group: Option<Vec<String>>, pub reason: Option<&'static str>, pub diagnostics: Vec<String> }` and `pub fn resolve_brief_scope(conn, cfg: Option<&WisdomConfig>, path: &str) -> BriefScope`. Reasons: `unconfigured`, `no_prefix`, `unresolved`, `org_unresolved`, else `None`. Same steps as `resolve_scoped` up to `cluster_ids`; NEVER call `match_area`. Org group always computed when `cfg.org_entity_id` is set (also for NoPrefix/Unresolved). `cfg` None or empty `areas_paths`: `unconfigured` (org still computed if `cfg` has an org id).
- Run: `... cargo test --features test-utils,mcp-server hub_resolver -- --test-threads=1` (new AND the existing `resolve_scoped` tests must pass).

## Task 3: pure ordering and fitting (spec 3.4)

- Files: `src-tauri/src/wisdom_brief.rs` (new), `src-tauri/src/lib.rs` (`pub mod wisdom_brief;` beside `pub mod wisdom_match;`).
- Failing tests first (in the new file's tests module, no DB): `order_is_tier_then_scope_then_newest_then_id`, `drop_is_bottom_up_prefix`, `lower_tier_never_kept_when_higher_dropped`, `oversize_statement_is_skipped_not_blocking`, `omitted_counts_by_tier`, `cost_formula_uses_chars_not_bytes` (use a title with multi-byte chars), `index_reserve_is_subtracted_first`.
- Change: define
  ```rust
  pub const BRIEF_SCHEMA_VERSION: u32 = 1;
  pub const ITEM_OVERHEAD: usize = 60;
  pub const INDEX_CAP: usize = 400;
  pub enum Band { Repo, Org }
  pub struct Candidate { pub id: String, pub band: Band, pub tier: u8, pub created_at: i64, pub item: serde_json::Value, pub cost: usize }
  pub struct Omitted { pub statements: usize, pub oversize: usize, pub rule_trap: usize, pub decision: usize, pub lesson: usize, pub untyped: usize }
  pub fn order(c: &mut [Candidate])        // tier asc, band asc (Repo first), created_at desc, id asc
  pub fn fit(c: Vec<Candidate>, budget: usize) -> (Vec<Candidate>, Omitted)  // longest fitting prefix; oversize skipped
  pub fn item_cost(title: &str, text: &str, id: &str) -> usize  // chars, + ITEM_OVERHEAD
  ```
  `tier` is `kind_tier(okf_type)` from `wisdom_match.rs` (never re-implement it).
- Run: `... cargo test --features test-utils,mcp-server wisdom_brief -- --test-threads=1`.

## Task 4: the covered-topics index (spec 3.5)

- Files: `src-tauri/src/wisdom_brief.rs`.
- Failing tests first: `index_lists_areas_count_desc_name_asc`, `index_caps_areas_at_eight_with_more`, `index_sheds_areas_then_names_to_fit_cap`, `index_empty_when_nothing_to_say`, `index_text_never_exceeds_cap`, `index_cap_is_min_400_quarter_of_max`.
- Change: `pub struct Index { pub text: String, pub areas: Vec<AreaCount>, pub counts: Counts }` and `pub fn build_index(repo_name: Option<&str>, org_name: Option<&str>, counts, shown, areas: Vec<(String,usize)>, cap: usize) -> Index`. Names and area names cut to 40 chars (by chars). Pure function; the sentence template is in spec 3.5.
- Run: same filter as Task 3.

## Task 5: `build_brief_at` against a database (spec 3.2-3.6)

- Files: `src-tauri/src/wisdom_brief.rs`.
- Failing tests first (in-memory DB through the real DDL, as the `wisdom_match.rs` tests do; names): `superseded_expired_forgotten_deleted_never_listed` (MANDATORY), `area_facts_not_listed_but_counted`, `foreign_hub_excluded`, `merged_loser_entity_included`, `org_only_when_no_repo`, `typed_facts_outrank_untyped_when_budget_tight`, `statement_fields_present` (id, entity_id, title, text, source_ref, confidence, source_type, verified, kind, band; no key for absent labels), `ids_unique`, `two_runs_byte_identical`, `pre_area_column_shape_degrades`, `invalid_id_skipped`.
- Change:
  ```rust
  pub struct BriefOpts { pub max_chars: usize }
  pub struct Brief { /* schema, scope, scope_reason, budget, statements, index, omitted */ }  // Serialize, field order = spec 3.6
  pub fn build_brief_at(conn: &Connection, scope: &BriefScope, opts: &BriefOpts, now_ms: i64) -> anyhow::Result<Brief>
  ```
  Steps: `entry_recall_arms(conn)?` (error propagates: fail closed); `BandBindings { repo_group, org_group, area: None }` through `band_predicate_where(&b, arms.has_area)`; org-only passes the org group as `repo_group` and labels `Org`; bind `:now` only when `arms.needs_now`; the band predicate's named params bound with the statement (see how `gated_entries` binds them, `wisdom_match.rs:~967`); SELECT as spec 3.6 query 1 (select `NULL AS col` for absent optional columns, as `rank_wiki_entries_at` does); build items with `recall_item` + `"band"`; area counts query 2 (skip when `!has_area`); names from `live_entities`; then Tasks 3 and 4 helpers. `used_chars` = sum of costs + index cost.
- Run: `... cargo test --features test-utils,mcp-server wisdom_brief -- --test-threads=1`.

## Task 6: the CLI command (spec 3.1, 3.6)

- Files: `tools/src/bin/ct.rs` (`WisdomCmd::Brief { json, repo_path: Option<String>, max_chars: usize }`, dispatch beside `Match`), `tools/src/queries.rs` (`pub fn wisdom_brief_cmd(json_mode, repo_path, max_chars) -> Result<i32>`), `tools/tests/ct_wisdom_brief.rs` (new).
- Failing tests first (`ct_wisdom_brief.rs`, using `run_ct`, `with_seeded_brain`, `brain_db` from `tests/common`; model the scoped fixture on `with_scoped_brain` in `ct_wisdom_match.rs:597`): `help_exits_zero`, `max_chars_out_of_range_exits_1`, `brief_json_shape_pinned` (exact top-level key set and statement key set), `repo_and_org_statements_listed`, `no_prefix_returns_org_only_with_reason`, `no_config_returns_empty_exit_0`, `unreadable_brain_exits_1`, `two_runs_byte_identical`, `relative_repo_path_resolves_against_cwd` (run `ct` with `current_dir` = the checkout; use `run_ct_with_env` or a `Command` with `.current_dir`), `output_never_contains_the_repo_path`, `superseded_fact_absent_end_to_end`, `stderr_diagnostic_has_no_path`.
- Change: the command opens `open_ro`, `busy_timeout(1s)`, loads config with `BrainConfig::load_lenient` (print its diagnostics with the `wisdom_scoped_diagnostic` channel), calls `resolve_brief_scope` and `build_brief_at(.., now_ms())`, prints with `print_json` (or a short text listing), exit 0. Errors propagate (exit 1). Never call `load_embed_profile`. Print `scope.diagnostics` as `wisdom_brief_diagnostic\t<text>` on stderr. No delivery log yet.
- Run: `cd .../tools && cargo test --test ct_wisdom_brief -- --test-threads=1`.

## Task 7: delivery log module (spec 3.7)

- Files: `tools/src/delivery_log.rs` (new), `tools/src/lib.rs` (`pub mod delivery_log;`), `tools/tests/ct_wisdom_brief.rs` (log tests).
- Failing tests first (unit tests in the module, scratch brains): `writes_one_row_with_ids_only`, `no_row_when_no_ids`, `ids_capped_at_64`, `missing_table_is_fail_open`, `missing_db_does_not_create_file`, `locked_db_fails_open_within_300ms` (hold `BEGIN IMMEDIATE` on another connection), `env_off_writes_nothing`, `summary_contains_no_query_or_path`.
- Change: `pub fn record_delivery(brain: &Brain, tool: &str, entity_id: Option<&str>, summary: serde_json::Value)` returning `()`: check `CURATED_DELIVERY_LOG=0`; open `Connection::open_with_flags(db_path, READ_WRITE | NO_MUTEX)` (NOT `write::open_rw`: it has `SQLITE_OPEN_CREATE`, `write.rs:63`); `busy_timeout(100ms)`; `pragma_update(None, "wal_autocheckpoint", 0)`; insert with `tauri_app_lib::db::commit::log_agent_access_checked_with_summary(&conn, "ct-cli", tool, entity_id, "read", Some(&summary.to_string()))`; any error becomes one stderr line `wisdom_delivery_log_skipped\t<short class>` and returns. A helper `fn summary(surface, n, ids, extra)` builds `{"v":1,"surface":..,"n":..,"ids":[..]}`.
- Run: `cd .../tools && cargo test --lib delivery_log -- --test-threads=1`.

## Task 8: log the brief (spec 3.7)

- Files: `tools/src/queries.rs` (`wisdom_brief_cmd`), `tools/tests/ct_wisdom_brief.rs`.
- Failing tests first: `brief_writes_one_delivery_row_with_ids` (read `curated_agent_log`: tool `wisdom_brief`, client `ct-cli`, `entity_id` = repo hub, summary ids equal the output ids), `brief_empty_writes_no_row`, `brief_output_identical_with_log_on_and_off` (stdout bytes and exit code, `CURATED_DELIVERY_LOG=0` vs default), `brief_two_runs_identical_after_log_rows_exist`, `brief_fail_open_when_log_table_dropped` (output unchanged, exit 0).
- Change: after printing, call `record_delivery`. The write happens AFTER stdout is printed and flushed.
- Run: `cd .../tools && cargo test --test ct_wisdom_brief -- --test-threads=1`.

## Task 9: log `ct wisdom match` (spec 3.7; skip if the owner rules OD-1 = brief only)

- Files: `tools/src/queries.rs` (`print_wisdom_match` ~:1304 and its three call sites ~:1073, :1279, :1298; `wisdom_match_cmd` ~:916), `tools/src/bin/ct.rs` (help text of `Match`: drop "read-only" wording for "no fact writes; appends a best-effort audit row"), `tools/tests/ct_wisdom_match.rs`.
- Failing tests first: `match_delivery_writes_row_with_ids_and_target_kind_only` (assert the summary has no query text and no path), `match_zero_hits_writes_no_row`, `match_stdout_and_exit_identical_with_log_off`, `match_fail_open_when_log_table_dropped`, and run the WHOLE existing `ct_wisdom_match.rs` plus the `message_mode_output_byte_identical` pin in `src-tauri` (`tool_dispatch.rs:2212`) UNCHANGED.
- Change: build one small `DeliveryLog { brain: &Brain, target: &'static str }` at the top of `wisdom_match_cmd` and pass it to `print_wisdom_match` (and to the helper at `:1262` that calls it); `print_wisdom_match` calls `record_delivery` AFTER printing when `entries` or `corrections` is non-empty. If clippy `too_many_arguments` fires, bundle `json_mode` and the log into one struct. Do not touch any selection code.
- Run: `cd .../tools && cargo test --test ct_wisdom_match -- --test-threads=1` and the src-tauri filter `message_mode_output_byte_identical`.

## Task 10: scratch-brain measurement and wrap-up (spec 7)

- Files: `specs/2026-11-28-wisdom-brief/` only (results table in the PR body, not a new file); a throwaway script in the scratchpad, NOT committed.
- Do: build a scratch brain dir (`CURATED_BRAIN_DIR=<tmp>`; never the live brain) with synthetic hubs at 50 / 1,000 / 10,000 facts (mixed kinds, areas, some superseded/expired/forgotten); run `ct wisdom brief --json` 20 times each: byte-identical, `used_chars <= max_chars`, zero superseded ids, p95 wall time. Record numbers for the PR body.
- Run the full CI forms once: `cd .../src-tauri && cargo test --features test-utils,mcp-server -- --test-threads=1` and `cd .../tools && cargo test -- --test-threads=1`; `cargo clippy` for both crates with the repo's flags (`.github/workflows/ci.yml`).
- Flip the spec status Draft -> Implemented only after review; delete `plan.md` and `LEDGER.md` in the same-PR cleanup commit (AGENTS.md).
