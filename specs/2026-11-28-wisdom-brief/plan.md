# Plan: `ct wisdom brief` + delivery log (PR-2)

**Rev 3 (2026-11-27):** folds Fable delta re-review (APPROVE WITH NITS).
**Rev 2 (2026-11-27):** folds Fable design review (REQUEST CHANGES: M1-M6, minors, nits) and owner rulings OD-1..OD-4, OR-1..OR-5.

Spec: `spec.md` (read it first; section numbers below point into it). TDD: write the failing test, run it and SEE it fail, make the change, run it green. One commit per task. Never touch a live brain: tests use `with_seeded_brain` (scratch brain dir). Rust runs SERIAL, one cargo at a time. If anything in the spec is unclear, STOP and ask.

## Setup (every shell)

```
export CARGO_TARGET_DIR=/Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/.cargo-shared-target
df -h "$(dirname "$CARGO_TARGET_DIR")"   # need >= 30G free
```
Every cargo command below is ONE line, exactly as written (no placeholders): it exports `CARGO_TARGET_DIR`, `cd`s into the crate and runs. Shape: `export CARGO_TARGET_DIR=<repo>/.cargo-shared-target && cd <worktree>/<crate> && cargo test <args> -- --test-threads=1`. Unit-test tasks use `--lib` with a filter. Before the final task check `df -h` shows >= 30G free (about 67G was free on 2026-11-27).
Every test run must show `>= 1 passed` in its output for the new tests (a filter that matches nothing prints `0 passed`: that is a FAILURE of the step). On linker "Bus error" or ENOSPC: stop and report; never delete caches.

## Task 1: `RecallArms.has_area`

- Files: `src-tauri/src/wisdom_match.rs` (struct `RecallArms` ~:450, `entry_recall_arms` ~:466, tests module).
- Failing test first: `recall_arms_reports_has_area` in the `wisdom_match.rs` tests: in-memory DB with a full `llm_wiki_entries`+`llm_wiki_events` shape (copy the helper an existing `entry_recall_arms` test uses) with and without an `area` column; assert `has_area` true/false.
- Change: add `pub has_area: bool` (value `t.has_area`, already computed by `temporal_from_snapshot`).
- Run: `export CARGO_TARGET_DIR=/Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/.cargo-shared-target && cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/.worktrees/wisdom-brief/src-tauri && cargo test --lib --features test-utils,mcp-server recall_arms_reports_has_area -- --test-threads=1` (>=1 passed), then `export CARGO_TARGET_DIR=/Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/.cargo-shared-target && cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/.worktrees/wisdom-brief/src-tauri && cargo test --lib --features test-utils,mcp-server wisdom_match -- --test-threads=1` to be sure no literal elsewhere broke.

## Task 2: hub resolver, shared helpers + `resolve_brief_scope` (spec 3.2)

- Files: `src-tauri/src/config/hub_resolver.rs`; tests in `src-tauri/tests/hub_resolver.rs` (the module has no tests module; the ~28 `resolve_scoped` tests live in that integration file).
- Failing tests first (add to `src-tauri/tests/hub_resolver.rs`, reusing its fixture helpers): `brief_scope_bound_repo_and_org`, `brief_scope_no_prefix_gives_org_only`, `brief_scope_no_prefix_no_org_is_none`, `brief_scope_unconfigured`, `brief_scope_unresolved_collision_gives_org_only`, `brief_scope_org_unresolvable_sets_reason`, `brief_scope_ignores_unusable_areas_file`, `brief_scope_group_includes_merged_losers`, `brief_scope_repo_wins_when_ids_in_both_groups`, `brief_scope_reason_precedence`.
- Change:
  1. Move the org block of `resolve_scoped` (`:433-452`) into private `fn org_group_for(conn, cfg) -> (Option<Vec<String>>, Vec<String>)`.
  2. Extract the repo-resolution steps (configured survivors, collision, longest prefix, live survivor, `cluster_ids`) into one shared private helper used by `resolve_scoped` and the new function (no third copy). Reason strings byte-identical. The existing tests must stay green UNMODIFIED.
  3. Add `pub struct BriefScope { pub repo_group: Option<Vec<String>>, pub org_group: Option<Vec<String>>, pub reason: Option<&'static str>, pub diagnostics: Vec<String> }` and `pub fn resolve_brief_scope(conn, cfg: Option<&WisdomConfig>, path: &str) -> BriefScope`. Reasons: `unconfigured`, `no_prefix`, `unresolved`, `org_unresolved`, else `None` (precedence in that order). NEVER call `match_area`. Org group always computed when `cfg.org_entity_id` is set (also for NoPrefix/Unresolved). `cfg` None or empty `areas_paths`: `unconfigured` (org still computed if `cfg` has an org id).
- Run: `export CARGO_TARGET_DIR=/Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/.cargo-shared-target && cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/.worktrees/wisdom-brief/src-tauri && cargo test --features test-utils,mcp-server --test hub_resolver -- --test-threads=1` (new AND existing tests pass; >=1 `brief_scope` passed).

## Task 3: pure ordering and fitting (spec 3.4)

- Files: `src-tauri/src/wisdom_brief.rs` (new), `src-tauri/src/lib.rs` (`pub mod wisdom_brief;` beside `pub mod wisdom_match;`).
- Failing tests first (in the new file's tests module, no DB): `order_is_tier_then_scope_then_newest_then_id`, `drop_is_bottom_up_prefix`, `lower_tier_never_kept_when_higher_dropped`, `oversize_statement_is_skipped_not_blocking`, `omitted_counts_by_tier`, `cost_formula_uses_utf16_units` (a title with multi-byte BMP chars and one astral char: cost counts `encode_utf16().count()`), `tier4_interleaves_repo_and_org_newest_first`, `tier4_oldest_dropped_first_globally`, `text_created_at_keeps_row`, `index_reserve_is_subtracted_first`.
- Change: define
  ```rust
  pub const BRIEF_SCHEMA_VERSION: u32 = 1;
  pub const ITEM_OVERHEAD: usize = 80;
  pub const INDEX_CAP: usize = 400;
  pub enum Band { Repo, Org }
  pub struct Candidate { pub id: String, pub band: Band, pub tier: u8, pub created_at: i64, pub item: serde_json::Value, pub cost: usize }
  pub struct Omitted { pub statements: usize, pub oversize: usize, pub rule_trap: usize, pub decision: usize, pub lesson: usize, pub untyped: usize }
  pub fn order(c: &mut [Candidate])        // tier asc; band asc (Repo first) for tiers 1-3 only; created_at desc; id asc (OR-1: tier 4 ignores band)
  pub fn fit(c: Vec<Candidate>, budget: usize) -> (Vec<Candidate>, Omitted)  // longest fitting prefix; oversize skipped
  pub fn item_cost(title: &str, text: &str, id: &str) -> usize  // UTF-16 code units, + ITEM_OVERHEAD (80, documented CTI contract: "CTI per-item markup plus labels <= 80 UTF-16 units + id"; 90 if CTI also renders `verified`)
  ```
  `tier` is `kind_tier(okf_type)` from `wisdom_match.rs` (never re-implement it). `created_at` is coerced like `coerce_updated_at` (`tool_dispatch.rs:419`). `Omitted.statements` includes `oversize`; the four tier counts sum to it.
- Run: `export CARGO_TARGET_DIR=/Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/.cargo-shared-target && cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/.worktrees/wisdom-brief/src-tauri && cargo test --lib --features test-utils,mcp-server wisdom_brief -- --test-threads=1` (>=1 passed).

## Task 4: the covered-topics index (spec 3.5)

- Files: `src-tauri/src/wisdom_brief.rs`.
- Failing tests first: `index_lists_areas_count_desc_name_asc`, `index_caps_areas_at_eight_with_more`, `index_sheds_areas_then_names_to_fit_cap`, `index_empty_when_nothing_to_say`, `index_text_never_exceeds_cap`, `index_cap_is_min_400_quarter_of_max`, `index_cost_zero_when_text_empty`, `index_k_crossing_digit_boundary_n10_k9` (the reserve is built with k=n and the final text, re-rendered with k=9 at the same shed level, is never longer), `index_at_max_chars_600_fits_non_empty`, `index_shed_ladder_ends_hint_then_empty`.
- Change: `pub struct Index { pub text: String, pub areas: Vec<AreaCount>, pub counts: Counts }` and `pub fn build_index(repo_name: Option<&str>, org_name: Option<&str>, counts, shown, areas: Vec<(String,usize)>, cap: usize) -> (Index, ShedLevel)` plus `pub fn render_index_at(level: ShedLevel, .., shown) -> Index`, so the caller builds once with `shown = n`, fits, then re-renders at the SAME level with the real `shown` (never re-sheds; spec 3.4). Names and area names cut to 40 chars. Shed ladder: areas from the tail, hub names, area clause, hint, whole index (empty, cost 0). Pure function; the sentence template is in spec 3.5.
- Run: same command as Task 3 (>=1 passed).

## Task 5: `build_brief_at` against a database (spec 3.2-3.6)

- Files: `src-tauri/src/wisdom_brief.rs`.
- Failing tests first (in-memory DB through the real DDL, as the `wisdom_match.rs` tests do; names): `superseded_expired_forgotten_deleted_never_listed` (MANDATORY), `area_facts_not_listed_but_counted`, `foreign_hub_excluded`, `merged_loser_entity_included`, `org_only_when_no_repo`, `typed_facts_outrank_untyped_when_budget_tight`, `statement_fields_present` (id, entity_id, title, text, source_ref, confidence, source_type, verified, kind, band; no key for absent labels), `ids_unique`, `two_runs_byte_identical`, `pre_area_column_shape_degrades`, `invalid_id_skipped`, `omitted_by_tier_sums_to_statements_incl_oversize`, `untyped_repo_and_org_interleave_by_created_at`.
- Change:
  ```rust
  pub struct BriefOpts { pub max_chars: usize }
  pub struct Brief { /* schema, scope, scope_reason, budget, statements, index, omitted */ }  // Serialize, field order = spec 3.6
  pub fn build_brief_at(conn: &Connection, scope: &BriefScope, opts: &BriefOpts, now_ms: i64) -> anyhow::Result<Brief>
  ```
  Steps: `entry_recall_arms(conn)?` (error propagates: fail closed); `BandBindings { repo_group, org_group, area: None }` through `band_predicate_where(&b, arms.has_area)`; org-only passes the org group as `repo_group` and labels `Org`; bind `:now` only when `arms.needs_now`; the band predicate's named params bound with the statement (see how `gated_entries` binds them, `wisdom_match.rs:~967`); SELECT as spec 3.6 query 1 (select `NULL AS col` for absent optional columns, as `rank_wiki_entries_at` does); build items with `recall_item` + `"band"`; area counts query 2 (named binds; skip when `!has_area`); names from `live_entities`; then Tasks 3 and 4 helpers in the spec 3.4 order: build the index with k=n to fix reserve and shed level, fit, re-render with real k. `used_chars` = sum of costs + index cost (0 for an empty index). Repo wins when an id is in both groups.
- Run: `export CARGO_TARGET_DIR=/Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/.cargo-shared-target && cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/.worktrees/wisdom-brief/src-tauri && cargo test --lib --features test-utils,mcp-server wisdom_brief -- --test-threads=1` (>=1 passed).

## Task 6: the CLI command (spec 3.1, 3.6)

- Files: `tools/src/bin/ct.rs` (`WisdomCmd::Brief { json, repo_path: Option<String>, max_chars: usize }`, dispatch beside `Match`), `tools/src/queries.rs` (`pub fn wisdom_brief_cmd(json_mode, repo_path, max_chars) -> Result<i32>`), `tools/tests/ct_wisdom_brief.rs` (new).
- Failing tests first (`ct_wisdom_brief.rs`, using `run_ct`, `with_seeded_brain`, `brain_db` from `tests/common`; first MOVE only `seeded_fact_with_entity`, `seed_entity` and `with_scoped_brain` (`ct_wisdom_match.rs` ~:557, :581, :597) into `tools/tests/common/mod.rs`; `seed_fact` ALREADY exists there (~:158), so delete the local copy in `ct_wisdom_match.rs` (~:17) and import `common::seed_fact`, and make `ct_wisdom_match.rs` use the moved helpers, keeping that file's tests green): `help_exits_zero`, `max_chars_out_of_range_exits_1`, `brief_json_shape_pinned` (exact top-level key set and statement key set), `repo_and_org_statements_listed`, `no_prefix_returns_org_only_with_reason`, `no_config_returns_empty_exit_0`, `unreadable_brain_exits_1`, `two_runs_byte_identical`, `relative_repo_path_resolves_against_cwd` (run `ct` with `current_dir` = the checkout; use `run_ct_with_env` or a `Command` with `.current_dir`), `output_never_contains_the_repo_path`, `superseded_fact_absent_end_to_end`, `stderr_diagnostic_has_no_path`, `load_lenient_diagnostics_never_contain_repo_path` (a brain whose config fails to load; grep stderr and stdout for the `--repo-path` value), `max_chars_600_accepted_599_rejected`.
- Change: the command opens `open_ro`, `busy_timeout(1s)`, loads config with `BrainConfig::load_lenient` (print its diagnostics with the `wisdom_scoped_diagnostic` channel), calls `resolve_brief_scope` and `build_brief_at(.., now_ms())`, prints with `print_json` (or a short text listing), exit 0. Errors propagate (exit 1). Never call `load_embed_profile`. Print `scope.diagnostics` as `wisdom_brief_diagnostic\t<text>` on stderr. No delivery log yet.
- Run: `export CARGO_TARGET_DIR=/Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/.cargo-shared-target && cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/.worktrees/wisdom-brief/tools && cargo test --test ct_wisdom_brief -- --test-threads=1` (>=1 passed), and `export CARGO_TARGET_DIR=/Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/.cargo-shared-target && cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/.worktrees/wisdom-brief/tools && cargo test --test ct_wisdom_match -- --test-threads=1` (helper move did not break it).

## Task 7: delivery log module (spec 3.7)

- Files: `tools/src/delivery_log.rs` (new), `tools/src/lib.rs` (`pub mod delivery_log;`), `tools/tests/ct_wisdom_brief.rs` (log tests).
- Failing tests first (unit tests in the module, scratch brains): `writes_one_row_with_ids_only`, `no_row_when_no_ids`, `ids_capped_at_64`, `missing_table_is_fail_open`, `missing_db_does_not_create_file`, `locked_db_fails_open_within_1s` (hold `BEGIN IMMEDIATE` on another connection; bound loosened to 1 s for loaded CI), `error_class_vocabulary_closed` (only `open|no_table|busy|readonly|other`; stderr never contains the db path), `no_unwrap_in_module` (grep check in the task, not a test; the grep excludes the module's own `#[cfg(test)]` block), `env_off_writes_nothing`, `summary_contains_no_query_or_path`.
- Change: `pub fn record_delivery(db_path: &Path, tool: &str, entity_id: Option<&str>, summary: serde_json::Value)` returning `()` (no `unwrap`/`expect` in the module): check `CURATED_DELIVERY_LOG=0`; open `Connection::open_with_flags(db_path, READ_WRITE | NO_MUTEX)` (NOT `write::open_rw`: it has `SQLITE_OPEN_CREATE`, `write.rs:63`); `busy_timeout(100ms)`; `pragma_update(None, "wal_autocheckpoint", 0)`; `synchronous=NORMAL`; insert with `tauri_app_lib::db::commit::log_agent_access_checked_with_summary(&conn, "ct-cli", tool, entity_id, "read", Some(&summary.to_string()))`; any error becomes one stderr line `wisdom_delivery_log_skipped\t<class>` (class from the closed set `open|no_table|busy|readonly|other`; never the SQLite message) and returns. The 1.5 s skip lives in one small wrapper taking the command's start `Instant` (skips the log call when elapsed >= 1.5 s), with one unit test. A helper `fn summary(surface, n, ids, extra)` builds `{"v":1,"surface":..,"n":..,"ids":[..]}`.
- Run: `export CARGO_TARGET_DIR=/Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/.cargo-shared-target && cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/.worktrees/wisdom-brief/tools && cargo test --lib delivery_log -- --test-threads=1` (>=1 passed).

## Task 8: log the brief (spec 3.7)

- Files: `tools/src/queries.rs` (`wisdom_brief_cmd`), `tools/tests/ct_wisdom_brief.rs`.
- Failing tests first: `brief_writes_one_delivery_row_with_ids` (read `curated_agent_log`: tool `wisdom_brief`, client `ct-cli`, `entity_id` = repo hub, summary ids equal the output ids), `brief_empty_writes_no_row`, `brief_output_identical_with_log_on_and_off` (stdout bytes and exit code, `CURATED_DELIVERY_LOG=0` vs default), `brief_two_runs_identical_after_log_rows_exist`, `brief_fail_open_when_log_table_dropped` (output unchanged, exit 0).
- Change: after printing, call `record_delivery`. Order: print stdout, flush stdout EXPLICITLY (CTI reads stdout only at exit), drop the read connection, then `record_delivery`. Never changes output or exit code.
- Run: `export CARGO_TARGET_DIR=/Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/.cargo-shared-target && cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/.worktrees/wisdom-brief/tools && cargo test --test ct_wisdom_brief -- --test-threads=1` (>=1 passed).

## Task 9: log `ct wisdom match` (spec 3.7; OD-1 ruled: both surfaces)

- Files: `tools/src/queries.rs` (`wisdom_match_cmd` ~:916), `tools/src/bin/ct.rs` (dispatch ~:899-904 and help text of `Match`: drop "read-only" wording for "no fact writes; appends a best-effort audit row"), `tools/tests/ct_wisdom_match.rs`.
- Failing tests first: `match_delivery_writes_row_with_ids_and_target_kind_only` (summary has no query text and no path; one case per target kind `message|path|command|pattern`), `match_zero_hits_writes_no_row`, `match_stdout_and_exit_identical_with_log_off`, `match_fail_open_when_log_table_dropped`; run the WHOLE existing `ct_wisdom_match.rs` and the `message_mode_output_byte_identical` pin in `src-tauri` (`tool_dispatch.rs:2401`) UNCHANGED.
- Change: `wisdom_match_cmd` gets a target-kind parameter from dispatch (today `ct.rs:899-904` collapses `command` and `pattern` into one target, so the kind cannot be derived inside). Do NOT thread a logger through pub `run_message_path` (`queries.rs:1254`; used by `tools/tests/activity_helper_parity.rs`, whose signature must not change). Log inside `wisdom_match_cmd` at all three exits (~queries.rs:1075, ~:1095, and the bare `Ok(0)` ~:1177) when `entries` or `corrections` is non-empty. Order at each: `emit_wisdom_unverified` (needs the connection), flush stdout, drop the connection, then log. Do not touch any selection code.
- Run: `export CARGO_TARGET_DIR=/Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/.cargo-shared-target && cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/.worktrees/wisdom-brief/tools && cargo test --test ct_wisdom_match -- --test-threads=1` and `export CARGO_TARGET_DIR=/Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/.cargo-shared-target && cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/.worktrees/wisdom-brief/tools && cargo test --test activity_helper_parity -- --test-threads=1` and `export CARGO_TARGET_DIR=/Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/.cargo-shared-target && cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/.worktrees/wisdom-brief/src-tauri && cargo test --lib --features test-utils,mcp-server message_mode_output_byte_identical -- --test-threads=1` (each >=1 passed).

## Task 10: exclude delivery rows from the app timeline (OD-1 + OR-3)

- Files: `src-tauri/src/db/events.rs` (the `curated_agent_log` arm of the UNION, ~:98-112), a test in the same file's tests module (or the existing events tests).
- Failing test first: `timeline_excludes_wisdom_delivery_rows`: seed `curated_agent_log` rows with tool `wisdom_brief`, `wisdom_match` and one other tool; assert only the other appears as `agent_access`.
- Change: add `WHERE a.tool NOT IN ('wisdom_brief','wisdom_match')` to that arm. No other change.
- Run: `export CARGO_TARGET_DIR=/Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/.cargo-shared-target && cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/.worktrees/wisdom-brief/src-tauri && cargo test --lib --features test-utils,mcp-server timeline_excludes_wisdom_delivery_rows -- --test-threads=1` (>=1 passed), then `export CARGO_TARGET_DIR=/Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/.cargo-shared-target && cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/.worktrees/wisdom-brief/src-tauri && cargo test --lib --features test-utils,mcp-server events -- --test-threads=1`.

## Task 11: INTENT.md owner-ruling line (OD-2)

- Files: `INTENT.md` only. Append to "How changes are made" step 4, after the existing 2026-11-27 line (blank line before it, italic, same indent as that line):
  `   *Owner ruling 2026-11-27: \`ct wisdom brief\`, a query-less session-start surface, is gated by fixture tests (drop order, budget, determinism, no superseded fact) and a scratch-brain p95 <= 250 ms at 10,000 facts instead of LongMemEval, abstention and paraphrase runs, which measure query matching.*`
- No test; verify with `git diff INTENT.md` showing exactly one added line plus its blank separator.

## Task 12: scratch-brain measurement and wrap-up (spec 7)

- Files: `specs/2026-11-28-wisdom-brief/` only (results table in the PR body, not a new file); a throwaway script in the scratchpad, NOT committed.
- Do: build a scratch brain dir (`CURATED_BRAIN_DIR=<tmp>`; never the live brain) with synthetic hubs at 50 / 1,000 / 10,000 facts (mixed kinds, areas, some superseded/expired/forgotten); run `ct wisdom brief --json` 20 times each: byte-identical, `used_chars <= max_chars`, zero superseded ids, p95 wall time. Perf gate: p95 <= 250 ms at 10,000 facts (OD-2). Record numbers for the PR body.
- Check disk first: `df -h /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts` shows >= 30G free.
- Run the full suites once (each >=1 passed, zero failed): `export CARGO_TARGET_DIR=/Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/.cargo-shared-target && cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/.worktrees/wisdom-brief/src-tauri && cargo test --features test-utils,mcp-server -- --test-threads=1` and `export CARGO_TARGET_DIR=/Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/.cargo-shared-target && cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/.worktrees/wisdom-brief/tools && cargo test -- --test-threads=1`
- Clippy, exactly CI's lines (`.github/workflows/ci.yml:102,109`): `export CARGO_TARGET_DIR=/Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/.cargo-shared-target && cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/.worktrees/wisdom-brief/src-tauri && cargo clippy --all-targets --features test-utils -- -D warnings` and `export CARGO_TARGET_DIR=/Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/.cargo-shared-target && cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/.worktrees/wisdom-brief/tools && cargo clippy --all-targets -- -D warnings`. Zero warnings.
- Flip the spec status to Implemented only after review; delete `plan.md` and `LEDGER.md` in the same-PR cleanup commit (AGENTS.md).
