# Plan: engine 9.x adoption — PR-B (core bump + DDL regen + ddl_compat extractor + engine mirrors)

Status: Draft (plan author, 2026-11-25). Transient scaffolding — deleted in the
same-PR cleanup commit.
Spec: `specs/engine-9x-adoption/spec.md` (Approved 2026-11-24; errata §8). Spec
wins on any conflict with this plan; INTENT wins over both.
Scope: spec §3 PR-B (steps 1–7, ROLLOUT NOTE, ACCUMULATION REALITY), §4 PR-B
file list, §5 "New tests required in PR-B" (1–6) + the TS-side `setup()` test.
PR-A is landed (V31/V32 on main); PR-B must not edit PR-A behavior.
Base: origin/main `78160f98`. Branch `chore/engine-9x-prb`. Worktree
`/mnt/kv-work/build/worktrees/curated-thoughts/engine-9x-prb`.

## 0. Ground rules for every task

### 0.1 Environment preamble (verbatim from pins; run once per shell)

```bash
export CARGO_TARGET_DIR=/mnt/kv-work/build/worktrees/curated-thoughts/engine-9x-prb/.cargo-shared-target
export CARGO_PROFILE_DEV_DEBUG=0
export CARGO_PROFILE_TEST_DEBUG=0
```

- Full-suite Rust gates through `~/.hermes/bin/heavy-build -- bash <script.sh>` (script
  file, never inline `bash -c`); single-crate targeted runs may go bare.
- `--manifest-path` AFTER the subcommand. Rust gates strictly serial, `--test-threads=1`.
- `tools/Cargo.toml` has NO `test-utils` feature — never pass it there.
- Disk <30G free / ENOSPC / linker Bus error = STOP and report; never delete caches.
- pnpm installs: `CI=true pnpm install --no-frozen-lockfile` for the pin-bump install
  (plain `pnpm install` in non-TTY fails; `CI=true` alone implies frozen, which fails a
  lockfile-CHANGING install).
- `node_modules` at the worktree root is a SYMLINK to the primary checkout — resolves to
  the SHARED primary install. Before capturing any version-labeled fixture from on-disk
  package content: `pnpm why @equationalapplications/core-llm-wiki` must show the locked
  version. Stale-resolution captures poison downstream fixtures.

Lane additions:

- Build location: AGENTS.md wins (pins ruling) — the absolute worktree-root
  `.cargo-shared-target` above. Never a relative `CARGO_TARGET_DIR`.
- Pre-flight before every cargo gate: `df -h "$(dirname "$CARGO_TARGET_DIR")"` ≥30G free
  (baseline: 75G free on `/mnt/kv-work`). Record the line in the task receipt.
- Gate scripts live at `.superpowers/sdd/engine-9x-pr-b/gates/t<N>.sh` (shape of
  `.superpowers/sdd/engine-9x-pr-b/baseline.sh`: `set -u`, `cd "$WT"`, preamble exports,
  one redirect per gate, `echo "<GATE>_EXIT=$?"`). Full-suite runs: `~/.hermes/bin/heavy-build -- bash .superpowers/sdd/engine-9x-pr-b/gates/t<N>.sh`.
- Rust gates serial machine-wide: never two cargo processes at once, never inside a
  parallel wave.
- pnpm: ONLY T2's bump install uses `--no-frozen-lockfile`. Every other install is
  `CI=true pnpm install --frozen-lockfile`.
- Before ANY read of on-disk package content (T2 diff, T3 regen, T3 ddl_compat verify,
  T8 TS test): `pnpm why @equationalapplications/core-llm-wiki` must show 9.18.0 in every
  resolution. Save it into the task receipt. T2 strictly precedes T3–T8.
- Staging: explicit paths only. `git add -A` / `git add .` are banned.
- Scratch drivers (capture tests, regen scripts) are deleted before commit. Scratch
  scripts live under `.superpowers/sdd/engine-9x-pr-b/scratch/` (gitignored, `.gitignore:34`);
  any scratch `#[test]` inside `src-tauri/` is removed before the commit that would carry it.
- `build.yml` red on push is pre-existing, not a gate (pins CI note). The gate is `ci.yml`.

### 0.2 Canonical gate commands

- G-tauri: `cargo test --manifest-path src-tauri/Cargo.toml --lib --features test-utils -j 2 -- --test-threads=1`
- G-tools: `cargo test --manifest-path tools/Cargo.toml --lib -j 2 -- --test-threads=1`
- G-heal: `cargo test --manifest-path tools/Cargo.toml --test ct_heal -j 2 -- --test-threads=1`
- G-vitest: `pnpm vitest run`
- G-tsc: `pnpm typecheck`
- G-clippy (closeout; CI-exact, `ci.yml:102`): `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --features test-utils -- -D warnings`

Filtered form appends the test-name filter before `--`, e.g.
`cargo test --manifest-path src-tauri/Cargo.toml --lib --features test-utils -j 2 ddl_compat -- --test-threads=1`.

### 0.3 Baseline (controller-measured on `78160f98`)

| Gate | Result |
|---|---|
| G-tauri | 1708 passed, 0 failed, exit 0 (264.47s, cold build) |
| G-tools | 61 passed, 0 failed, exit 0 |
| G-heal | 16 passed, 0 failed, exit 0 |
| G-vitest | 96 files, 625 passed + 1 skipped, exit 0 |
| G-tsc | exit 0 |

Receipts: `.superpowers/sdd/engine-9x-pr-b/receipts/baseline-*.out`. Every task's
evidence compares against these numbers. A count drop without a matching planned
deletion is a regression. New tests raise the count; record the delta per task.

### 0.4 Evidence rule

Receipts to `.superpowers/sdd/engine-9x-pr-b/receipts/t<N>-<gate>.out`; RED proofs as
`t<N>-<gate>-red.out`. The LEDGER tick links the receipt path. Every receipt carries the
`df -h` line (cargo gates) or the `pnpm why` line (package reads).

### 0.5 Commit protocol — the same-commit rule (spec §2, §3 PR-B header, §6 alt 3)

The pin bump, lockfile, DDL regen, guard update, and engine-mirror migration MUST land
in ONE commit; every split intermediate fails `verify_llm_wiki_schema` on existing DBs.
Per Q1 (recommended): T0–T1 commit standalone (green at 8.0.2); T2–T9 accumulate as
local, UNPUSHED `wip(engine9):` commits; the controller squashes them
(`git reset --soft <T1 head>` + one commit) into the atomic commit before first push.
Nothing is pushed between T2 and the squash. The ddl_compat test is RED on WIP heads
by design.

### 0.6 Open questions

Tasks name the open question that blocks them. No blocked step starts until the
controller records a ruling in LEDGER.md. Questions live in LEDGER.md (single source).

## 1. Verified anchors (worktree at `78160f98`, checked by the plan author)

- `ddl_compat.rs:23-39` `extract_setup_database_ddl` — marker `async function setupDatabase(db, prefix)` (`:24`), first `` await db.execAsync(` `` only (`:29`), close `"  `);"` (`:33`). `package_ddl_matches_rust` `:42-82` reads `../node_modules/@equationalapplications/core-llm-wiki/dist/index.js` (`:43-44`). `split_ddl_statements` `:13-20` splits on bare `;`. Test `rust_llm_wiki_ddl_matches_core_llm_wiki_package` `:166`.
- Installed 8.0.2 dist: `setupDatabase` at `dist/index.js:760`, ONE block at `:761`; helper calls `createOkfV02IndexesIfColumnsExist` `:905`, `createTemporalIndexesIfColumnsExist` `:906`; helper bodies `:908`, `:925` carry their own `execAsync` calls (must stay excluded).
- `okf_ddl.rs:11-154` `LLM_WIKI_PACKAGE_DDL` (9 CREATE TABLEs; comment at `:54-59` carries the dist's literal `\xA7` escape — verbatim means dist text). `migration_v7_sql` `:221-227` is the ONLY path that runs the const (fresh brains, `version < 7`, `connection.rs:275-277`). Apply helpers: `apply_llm_wiki_v12_edge_index` `:238`, `apply_llm_wiki_v13_temporal_indexes` `:256-274` (column-gated; the v13 pattern).
- CT does NOT mirror `createOkfV02IndexesIfColumnsExist` today (zero `lifecycle_status_idx` hits under `src-tauri/src`) — a pre-existing gap PR-B closes (spec step 5).
- `schema_guard.rs:8` pin `"8.0.2"`; `TableExpectation` `:10-13` (`&'static`); `LLM_WIKI_TABLES` `:15-148` (9 entries); CT-local entries columns `tier` `:53`, `embed_scheme` `:56`, `area` `:58` (spec step 3 omits `area` — carry it); `verify_table` `:179-215`; `verify_llm_wiki_schema` `:218-226` (`< 7` no-op gate `:219`); tests `:233-303`.
- `connection.rs` `migrate()` `:220`; V17 guarded-ALTER precedent `:318-336`; V24 `apply_v24_temporal_columns` `:987` (snapshot → `BEGIN IMMEDIATE` → re-inspect under lock; stamp gated on V22), called `:750`; V30 `apply_v30_area_column` `:1356-1413` (stamp every open, gated `stamped_now >= 22`); `apply_sso9_migrations(conn)?` `:868`; PR-A consts `SSO9_REFRESH_VERSION = 31` `:1419`, `SSO9_RETYPE_VERSION` `:1421`; every-open engine mirrors `:968` (v12), `:972` (v13); guard `:974`; `migrate_open_db` `:2889`; `open_in_memory` `:2893`; tests `mod tests` `:2946`.
- PR-A tripwire: `sso9_retype_has_no_triggers_on_retyped_columns` `connection.rs:6563-6587` asserts every trigger on `llm_wiki_entries` is NOT an unscoped `AFTER/BEFORE UPDATE ON` trigger. The new ledger/resurrect triggers sit on `llm_wiki_entries` (see Q16).
- `okf_migration.rs`: zero `ALTER`/`CREATE`/`add_column_if_missing` hits; one engine-table write `INSERT OR IGNORE INTO llm_wiki_events (id, entity_id, event_type, summary, related_entry_id, created_at)` `:269` (explicit column list).
- Raw entry deletes (spec cites drifted): `commit.rs:634` (spec `:616`), `queries.rs:227` in `clear_vault_tables` `:207` (matches), `bundle_apply.rs:1017` (spec `:982`). Test-only hit `edge_purge.rs:801`.
- Zero hits for `source_pointer|prior_valid_to|derived_events|forgotten_at|source_documents|source_passages|vector_removals|external_index_in_use` outside `node_modules/`, `specs/`, `.superpowers/`.
- `package.json` overrides `:8-9`, deps `:49-51` = `8.0.2`; `:52` sso = `9.18.0`.
- TS: `SQLiteAdapter` imported from core at `src/lib/wikiAdapter.ts:2` (shape `:19-47`). No node SQLite dependency in `package.json`; Node is v24.22.1 (`node:sqlite` built in).
- Integration tests live in `src-tauri/tests/*.rs` (e.g. `db_migration_v30_area.rs`); G-tauri is `--lib` and does NOT run them. CI does (`ci.yml:112`).

## 2. Tasks

Order: T0 → T1 → T2 → T3 → T4 → T5 → T6 → T7 → T8 → T9 → T10. Spec step order is
preserved inside the atomic set (step 1 regen T3, steps 2–3 T4, step 4 T5, step 5
T6–T7, step 6 T9, step 7 T1 + T3). The extractor (spec step 7) moves ahead of the
install so it lands green against the single-block 8.0.2 dist (Q2).

### Task 0 — base captures (8.0.2 fixtures), before ANY edit

Scope: spec §5 test 3 fixture ("8.0.2 expectation fixture"); §5 test 2 fixture ("a DB
fixture created under the 8.0.2 DDL").
Blocked by: Q4, Q5.
Blocked-by tasks: none. MUST run before T2 (install) and before any Rust edit.

Steps:

1. `git rev-parse HEAD` = `78160f98…`; `pnpm why @equationalapplications/core-llm-wiki` shows 8.0.2. Receipt `t0-capture.out`.
2. DDL snapshot: write the body of `LLM_WIKI_PACKAGE_DDL` (text between `r#"` and `"#`, byte-exact) from `git show 78160f98:src-tauri/src/db/okf_ddl.rs` to `src-tauri/src/db/testdata/core-llm-wiki-8.0.2.ddl.sql`. Record `sha256sum`.
3. Expectations snapshot: write `src-tauri/src/db/testdata/core-llm-wiki-8.0.2.expectations.json` — `[{ "name": …, "columns": [ … ] }]` in `LLM_WIKI_TABLES` order, generated from `git show 78160f98:src-tauri/src/db/schema_guard.rs` by a scratch script (never hand-typed). Includes the CT-local `tier`, `embed_scheme`, `area`.
4. Fixture brain (Q5 recommendation): scratch `#[test] #[ignore]` in `connection.rs` tests opens a fresh ROOTED tempfile brain (the `db_migration_v30_area.rs:14-26` `config.json` pattern, so V22 stamps), asserts `MAX(version) = 32`, then `VACUUM INTO 'src-tauri/src/db/testdata/ct-brain-v32-core-8.0.2.db'`. Run it once filtered; delete the scratch test. Record file size and `SELECT version FROM schema_version` rows.
5. Commit the three testdata files only (no consumer yet; tree stays green).

Gates: filtered scratch run (bare, single test); `git status` shows only the three testdata files.
RED proof: n/a (fixtures).
Evidence: `t0-capture.out` (HEAD, pnpm why, sha256s, fixture size, stamped versions).
Ledger text: `T0 — 8.0.2 DDL text, 8.0.2 TableExpectation JSON, and a V32-stamped 8.0.2-shape fixture brain captured from base 78160f98; committed as testdata`

### Task 1 — `ddl_compat` extractor: every execAsync block inside `setupDatabase`

Scope: spec §3 step 7 (extractor + unit test + trigger split-shape); §5 test 4 (unit half).
Blocked by: Q2.
Blocked-by tasks: T0.

Steps:

1. RED first: add unit tests in `ddl_compat.rs` `mod tests` against inline JS fixtures:
   - two-block fixture: `async function setupDatabase(db, prefix) {` + block 1 (two `CREATE TABLE IF NOT EXISTS ${prefix}…`) + a non-exec line (`await createXIfColumnsExist(db, prefix);`) + block 2 (a table, a partial index, and a `CREATE TRIGGER IF NOT EXISTS ${prefix}… BEGIN INSERT …; END;` trigger) + `}` + a following helper `async function createXIfColumnsExist(db, prefix) { await db.execAsync(\`CREATE INDEX … ${prefix}helper_idx …\`); }`. Assert the extracted set = block 1 + block 2 statements, prefix substituted, and NO `helper_idx`.
   - one-block fixture: result equals what the current extractor returns (regression pin).
   - split-shape: the trigger in block 2 splits into the SAME fragments whether it comes from `extract_setup_database_ddl` (package side) or `split_ddl_statements` over the equivalent Rust text (the `…; END` → two fragments shape), asserted explicitly.
   Run filtered → RED (current extractor drops block 2). Receipt `t1-g-tauri-red.out`.
2. Extend `extract_setup_database_ddl` (`ddl_compat.rs:23-39`):
   - bound the function body: from the marker to the end of `setupDatabase` (the first line that is exactly `}` at column 0 after the marker — the dist's function-close shape; verify against `dist/index.js:760-907` at 8.0.2 and again at 9.18.0 in T3);
   - collect EVERY `` await db.execAsync(` `` inside that bound, in order; each block closes at its own closing backtick + `);` (not the hard-coded two-space `"  `);"`, which a one-line block would not match);
   - substitute `${prefix}`, concatenate blocks, then `split_ddl_statements` as today;
   - `expect` messages name which invariant failed (marker missing, zero blocks, unterminated block).
3. Keep the signature (`(&str, &str) -> Vec<String>`); no caller changes.
4. GREEN: filtered unit tests + `rust_llm_wiki_ddl_matches_core_llm_wiki_package` against the still-installed 8.0.2 dist (single block — proves no regression on the real dist).
5. Commit (standalone; green at 8.0.2).

Gates: `cargo test --manifest-path src-tauri/Cargo.toml --lib --features test-utils -j 2 ddl_compat -- --test-threads=1` (RED then GREEN); `pnpm why` line (8.0.2).
Evidence: RED receipt; GREEN receipt incl. the package-match test at 8.0.2.
Ledger text: `T1 — extract_setup_database_ddl concatenates every execAsync block inside setupDatabase (helpers excluded); two-block, one-block, and trigger split-shape unit tests RED→GREEN; package match green on 8.0.2 dist`

### Task 2 — pin bump + install + lockfile (opens the atomic set)

Scope: spec §3 PR-B header (five pin lines); §4 `package.json`, `pnpm-lock.yaml`.
Blocked by: Q1, Q3.
Blocked-by tasks: T1.

Steps:

1. Edit `package.json`: overrides `:8` core-llm-wiki, `:9` core-okf; deps `:49` core-llm-wiki, `:50` react-llm-wiki, `:51` schema-org-llm-wiki → `"9.18.0"`. Five lines, nothing else.
2. `CI=true pnpm install --no-frozen-lockfile`. Full output → `t2-install.out`.
3. `pnpm why` for `@equationalapplications/core-llm-wiki`, `core-okf`, `react-llm-wiki`, `schema-org-llm-wiki`, `schema-software-org` → `t2-why.out`. Every resolution 9.18.0; the PR-A-era unmet `core-llm-wiki` 9.18.0 range warning from sso is GONE (overrides' forcing effect ended). Any remaining warning reported verbatim.
4. `git diff --stat pnpm-lock.yaml` + a grep of changed package keys → `t2-lockfile.out`. Allowed churn: the four bumped packages, their direct dependency edges, and the sso edge to core. Anything else: STOP and report before continuing.
5. Re-grep the TS import surface (spec §2.5): the 13 imported symbols still resolve (G-tsc), and `librarianSystemPrompt` has zero hits.
6. RED evidence for the atomic set: run `ddl_compat` filtered → RED with package-only drift naming `llm_wiki_source_documents`, `llm_wiki_source_passages`, `llm_wiki_vector_removals` and the two ledger triggers. This is spec 7(a)'s live proof that the T1 extractor reaches block 2 of the real dist. Receipt `t2-ddl-compat-red.out`.
7. `wip(engine9):` commit, unpushed (§0.5).

Gates: `pnpm why` (above); G-tsc; G-vitest (expect 625 passed + 1 skipped unchanged — the TS surface is additive); filtered `ddl_compat` (RED, expected).
Evidence: install log, why log, lockfile scope, tsc/vitest receipts, ddl_compat RED.
Ledger text: `T2 — five pin lines → 9.18.0; install + lockfile scope verified; every resolution 9.18.0; tsc/vitest green; ddl_compat RED with block-2 drift (expected, atomic set open)`

### Task 3 — regenerate `LLM_WIKI_PACKAGE_DDL` from the installed 9.18.0 dist

Scope: spec §3 step 1; step 7(a)(b); §5 test 4 (integration half).
Blocked-by tasks: T2.

Steps:

1. `pnpm why` → 9.18.0 (receipt). Confirm in `dist/index.js` (cite line numbers in LEDGER): the `async function setupDatabase(db, prefix)` signature is unchanged; the body holds exactly TWO `` await db.execAsync(` `` template blocks; list every helper call inside the body (expected: OKF v0.2 indexes, temporal indexes, source document/passage indexes, resurrect trigger — record the exact helper names and the SQL each emits for T7). Any other shape: STOP.
2. Mechanical regen, no hand edits: scratch `node` script `.superpowers/sdd/engine-9x-pr-b/scratch/regen-ddl.mjs` locates the same body bound and blocks as the T1 extractor, takes each block's RAW template text, replaces `${prefix}` with `llm_wiki_`, joins block 1 + `\n` + block 2, and prints it. The output replaces the body between `r#"` and `"#` at `okf_ddl.rs:11-154`. Keep dist escapes byte-for-byte (e.g. `\xA7`). If the text contains `"#`, widen the raw-string delimiter (`r##"…"##`) — never edit the SQL.
3. Spec 7(b): confirm the two ledger trigger bodies (`CREATE TRIGGER … BEGIN …; END`) fragment identically on both sides — record the fragment list for each trigger from the package side and the Rust side in LEDGER (they must be equal lists).
4. Count `CREATE TABLE` statements in the regenerated const: expected 12 (11 + 1); `CREATE TRIGGER`: expected 2. Record the actual numbers — they pin T4.
5. Confirm every statement in the regen is `IF NOT EXISTS` (precondition of Q8's recommended mechanism). Record exceptions.
6. Update the module doc / const doc only if they state a version or a "single block" fact.
7. `wip(engine9):` commit.

Gates: filtered `ddl_compat` → GREEN (`package_ddl_matches_rust` at the regen head, spec §5 test 4).
Expected collateral: G-tauri is RED at this head (fresh-brain guard tests see the new columns) until T4 — do not run the full suite here.
Evidence: dist line cites, regen script output hash, fragment table, statement counts, GREEN receipt.
Ledger text: `T3 — LLM_WIKI_PACKAGE_DDL regenerated mechanically from 9.18.0 dist (both blocks, prefix applied, no hand edits); two-block shape + trigger fragmentation confirmed; package_ddl_matches_rust GREEN`

### Task 4 — `schema_guard.rs`: pin, column sets, 3 new tables, count test, parameterized verify

Scope: spec §3 steps 2–3; §5 test 1; §5 test 3 entry point (`verify_schema_with`).
Blocked by: Q4 (fixture format, for the `verify_schema_with` signature).
Blocked-by tasks: T3.

Steps:

1. RED first: add `llm_wiki_tables_cover_every_package_create_table` — `LLM_WIKI_TABLES.len()` == count of statements starting `CREATE TABLE` in `split_ddl_statements(LLM_WIKI_PACKAGE_DDL)` (and the name sets match). Filtered run → RED (9 vs 12). Also record `verify_passes_on_fresh_v7_database` (`schema_guard.rs:234`, spec §5 test 1) RED at this head ("unexpected columns" on the new entries/events columns). Receipt `t4-g-tauri-red.out`.
2. `schema_guard.rs:8` → `"9.18.0"`.
3. Column sets from the regenerated DDL (never from the spec's list): `llm_wiki_entries` gains `source_pointer`, `prior_valid_to`, `derived_events`; `llm_wiki_events` gains `forgotten_at`. Keep CT-local `tier`, `embed_scheme`, `area`. Any other column delta T3 surfaced: STOP and report.
4. Add three `TableExpectation` entries: `llm_wiki_source_documents` (8 columns), `llm_wiki_source_passages` (6 columns), `llm_wiki_vector_removals` — columns copied from the regenerated DDL. The 8/6 counts are v17 invariants (spec step 5); a mismatch is a STOP.
5. Parameterize: `TableExpectation<'a> { name: &'a str, columns: &'a [&'a str] }`; `pub(crate) fn verify_schema_with(conn, expects: &[TableExpectation<'_>]) -> Result<()>` (same `< 7` gate); `verify_llm_wiki_schema` becomes a thin wrapper over `LLM_WIKI_TABLES`. Expose a `#[cfg(test)]` loader that builds `Vec<TableExpectation>` from `testdata/core-llm-wiki-8.0.2.expectations.json` (T0).
6. Fixture fidelity test: copy `ct-brain-v32-core-8.0.2.db` (T0) to a tempdir; open it RAW (`rusqlite::Connection::open`, no migrate); `verify_schema_with(old_expects)` PASSES. Pins that the JSON and the DB agree on the 8.0.2 shape.
7. Spec §5 test 1 (fresh install): a fresh brain through CT's real open path (rooted tempfile brain, and `open_in_memory`) passes `verify_llm_wiki_schema`. NOTE: the fresh path goes `migration_v7_sql` → ladder; the CT-local columns arrive by ALTER, so a raw-DDL-only DB cannot pass the guard by design.
8. `wip(engine9):` commit.

Gates: filtered `schema_guard` and `ddl_compat` → GREEN. (`db::connection` tests that open file brains are expected RED until T6 — record, do not chase.)
Evidence: RED receipt; GREEN receipts; table/column diff (8.0.2 → 9.18.0) recorded in LEDGER.
Ledger text: `T4 — guard pinned 9.18.0; entries/events column sets + 3 new TableExpectations from the regen; len == CREATE TABLE count test; verify_schema_with + 8.0.2 JSON loader; fixture fidelity + fresh-install tests green`

### Task 5 — `okf_migration.rs` non-conflict audit (spec step 4)

Scope: spec §3 step 4; §4 "`okf_migration.rs` (only if step 4 applies)".
Blocked-by tasks: T4.

Steps:

1. Audit method (each line is evidence, receipt `t5-audit.out`):
   - grep `okf_migration.rs` for `ALTER TABLE|ADD COLUMN|add_column_if_missing|CREATE (TABLE|INDEX|TRIGGER)` → expected zero;
   - grep the whole `src-tauri/src` and `tools/src` for the 4 new column names and 3 new table names outside `okf_ddl.rs`/`schema_guard.rs` → expected zero (pre-T3 base: zero repo-wide);
   - list every engine-table write in `okf_migration.rs` (expected: `llm_wiki_events` `:269` with an explicit column list — `forgotten_at` is nullable, so no conflict; `llm_wiki_meta` `:324`, `:341` key/value — no conflict) and every other CT write path that inserts into `llm_wiki_entries`/`llm_wiki_events` WITHOUT an explicit column list (`INSERT INTO llm_wiki_(entries|events) VALUES` / `SELECT *`) — any hit breaks positionally under the new columns: STOP and report.
2. Expected outcome: no change to `okf_migration.rs`. If the audit finds a CT-side pre-add of an engine-owned column: remove it in this task with a RED/GREEN test, and report first.
3. Run the existing `okf_migration` lib tests (filtered) at this head.

Gates: filtered `okf_migration` (lib); the audit receipt.
Evidence: audit receipt with the three grep results; GREEN receipt.
Ledger text: `T5 — okf_migration audit: no CT pre-add of engine-owned columns/tables; explicit-column engine writes only; no change (or: change + test, if found)`

### Task 6 — engine-mirror migration step (V33): columns, tables, ledger triggers

Scope: spec §3 step 5 (ALTERs, CREATE TABLEs, two ledger triggers; v17 invariants; re-cite engine guards); §5 tests 2 (Rust half), 3, 5.
Blocked by: Q7, Q8, Q10, Q14, Q16.
Blocked-by tasks: T4, T5.

#### 6a. RED first (all on tempfile copies of the T0 fixture brain; never migrate the checked-in file)

- Test 2 (existing-DB path): copy fixture → `migrate_open_db` (`connection.rs:2889`) → `verify_llm_wiki_schema` passes. RED now (missing columns / missing tables).
- Test 3 (round trip, forward-incompat): copy fixture → migrate → `verify_schema_with(old 8.0.2 expectations)` bails with `"unexpected columns"` naming `source_pointer`, `prior_valid_to`, `derived_events` (entries) — assert the substring and the names. Twin assertion (T4 step 6): old expectations PASS on an unmigrated copy. RED now (no migration → old expectations pass → expected bail absent).
- Test 5 (trigger behavior): on the migrated copy, seed one live entry, soft delete (`UPDATE llm_wiki_entries SET deleted_at = … WHERE id = …`) → exactly 1 `llm_wiki_vector_removals` row for it; hard `DELETE` another entry with no `external_index_in_use` marker → 0 new rows. Positive control (Q14): set `llm_wiki_meta.external_index_in_use = '1'`, hard delete a third entry → exactly 1 row. RED now (no table).
- Idempotency: migrate the copy twice; schema, stamps, and row counts identical.
- Stamp gating: a rootless open (V22 deferred) gets the columns/tables/triggers but no V33 stamp.
Receipt `t6-g-tauri-red.out`.

#### 6b. Slot

1. Sync: `git fetch origin`; if origin/main moved, `git merge origin/main` (never rebase). Recompute N = highest stamped literal/const in `connection.rs` + 1 (expected 33 = `SSO9_RETYPE_VERSION` + 1).
2. `pub(crate) const ENGINE9_MIRROR_VERSION: i64 = 33;` — literal, doc comment states the derivation and the V27 renumber precedent. Every stamp/test/log reads the const. Record N in LEDGER.

#### 6c. Step body — `apply_engine9_mirror(conn)` (V24/V30 pattern)

1. Re-cite the shipped 9.18.0 engine guards in the doc comment (spec step 5 demands it): `migrations.ts` v16/v18/v19 existence guards and the v17 closure's column-set + zero-row pre-conditions — cite `dist/index.js` line numbers from the installed 9.18.0 dist.
2. Every open, ungated (the guard at `:974` demands the shape even on rootless opens): lock-free snapshot of missing columns via `existing_columns`; if anything is missing (column, table, or trigger), `BEGIN IMMEDIATE`, re-inspect under the lock, apply, `COMMIT`; any error → `ROLLBACK` and propagate.
3. Columns: `llm_wiki_entries.source_pointer`, `.prior_valid_to`, `.derived_events`; `llm_wiki_events.forgotten_at` — declared types copied verbatim from the engine's v16/v18/v19 `ALTER` text in the 9.18.0 dist (they must equal the template's declarations; T7's parity test enforces it).
4. Tables + ledger triggers (Q8 recommendation): after the ALTERs, execute `LLM_WIKI_PACKAGE_DDL` itself (every statement `IF NOT EXISTS`, confirmed in T3 step 5) — creates `llm_wiki_source_documents`, `llm_wiki_source_passages`, `llm_wiki_vector_removals`, the ledger's partial index, and both ledger triggers with verbatim text by construction. No duplicated SQL literals.
5. v17 invariants: no CT-local column on the two source tables; no row writes to them anywhere in CT (T5 grep + a test asserting zero rows after migrate).
6. Stamp `ENGINE9_MIRROR_VERSION` on every open when `MAX(version) >= 22` (`apply_v30_area_column` `:1401-1411` shape), outside the column-add branch.
7. Placement: in `migrate()` right after `apply_sso9_migrations(conn)?` (`connection.rs:868`) — slot order, and the sso9 re-type then always runs before the ledger triggers exist on an upgrading brain (Q16).
8. Log one `[ct::migrate] engine9 mirror (slot N): …` line listing what was added (silent on steady state).

Gates: filtered `db::connection` and `schema_guard` → GREEN; then G-tauri full (heavy-build) — expect 1708 + new tests, 0 failed, except any T7-owned parity test not yet written.
Evidence: RED receipt; GREEN receipts; LEDGER: slot N, engine dist cites, declared types used.
Ledger text: `T6 — V33 (ENGINE9_MIRROR_VERSION) mirrors v16/v18/v19 columns + source/ledger tables + ledger triggers on existing brains (guarded ALTERs, IF NOT EXISTS DDL re-run, BEGIN IMMEDIATE, V22-gated stamp); existing-DB, round-trip, trigger, idempotency tests RED→GREEN`

### Task 7 — helper-created objects + parity test

Scope: spec §3 step 5 (migration-only partial indexes `entries_deleted_idx`, `events_forgotten_idx`; resurrect trigger; `idx_source_passages_doc`/`_entity`; `idx_source_documents_ref`/`_hash`; OKF v0.2 indexes); §5 test 6.
Blocked by: Q6, Q13, Q16.
Blocked-by tasks: T6.

Steps:

1. RED first:
   - Test 6 (parity, definition per Q6): fresh rooted brain vs migrated fixture copy — `sqlite_master` table/index/trigger NAME sets equal; index and trigger SQL equal after `normalize_ddl_statement`; per table, `PRAGMA table_info` compared as a name-keyed map of `(type, notnull, dflt_value, pk)`; table SQL compared normalized for every table PR-B does not ALTER.
   - Helper presence (parity alone is vacuous for objects NEITHER path creates): both brains contain every helper object T3 step 1 listed, by exact prefixed name.
   Receipt `t7-g-tauri-red.out`.
2. Add apply helpers in `okf_ddl.rs` beside `apply_llm_wiki_v13_temporal_indexes` (`:256`), one per engine origin, SQL verbatim from the 9.18.0 dist helper bodies with the prefix applied, each gated like the engine's helper (column / table existence via `existing_columns` / `sqlite_master`):
   - `apply_llm_wiki_okf_v02_indexes` (`createOkfV02IndexesIfColumnsExist`);
   - `apply_llm_wiki_v18_v19_partial_indexes` (`entries_deleted_idx`, `events_forgotten_idx` — migration-only in the engine, never in the const);
   - `apply_llm_wiki_source_store_indexes` (the four source indexes; UNIQUE where the engine has UNIQUE);
   - `apply_llm_wiki_resurrect_trigger` (`createResurrectTrigger`).
   Doc comments state "created OUTSIDE the setupDatabase template; must stay out of `LLM_WIKI_PACKAGE_DDL`" (v13 wording).
3. Call them on every open in `migrate()` beside `:968`/`:972`, after the V33 step and before the guard (`:974`), so fresh AND migrated brains converge (the fresh path never runs engine helpers otherwise).
4. Q16 check: record each new trigger's event clause. If any is an unscoped `AFTER/BEFORE UPDATE ON llm_wiki_entries`, `sso9_retype_has_no_triggers_on_retyped_columns` (`connection.rs:6563`) goes RED — STOP and report; do not edit the PR-A test.
5. Re-run T6's trigger test: the resurrect trigger must not change its asserted counts (if it does, report the engine behavior; do not adjust expectations silently).
6. `wip(engine9):` commit.

Gates: filtered `db::connection`, `okf_ddl`, `schema_guard`, `ddl_compat` → GREEN (ddl_compat proves no helper SQL leaked into the const).
Evidence: RED/GREEN receipts; LEDGER: helper names + dist line cites + trigger event clauses.
Ledger text: `T7 — engine helper objects (OKF v0.2 idx, v18/v19 partial idx, source idx, resurrect trigger) mirrored as every-open apply helpers; parity + helper-presence tests RED→GREEN; PR-A trigger tripwire green`

### Task 8 — engine-side compatibility proof (TS `setup()` over a Rust-chain DB)

Scope: spec §5 test 2 (TS half: "run the real 9.18.0 `setup()` over a DB produced by the Rust chain").
Blocked by: Q9.
Blocked-by tasks: T7.

Steps:

1. Rust side: `engine9_migrated_schema_fixture_is_current` dumps the migrated fixture copy's `llm_wiki_*` objects (`SELECT type, name, sql FROM sqlite_master WHERE name LIKE 'llm\_wiki\_%' ESCAPE '\' AND sql IS NOT NULL ORDER BY type, name`) into `src-tauri/src/db/testdata/ct-engine9-migrated.llm_wiki.schema.sql` and asserts byte equality; `CT_REGEN_FIXTURES=1` rewrites it. Generate once, commit.
2. TS side: `src/__tests__/engine9-setup-compat.test.ts` (`// @vitest-environment node`): a minimal `SQLiteAdapter` (`src/lib/wikiAdapter.ts:19-47` shape) over `node:sqlite` `DatabaseSync` (no new dependency); build an in-memory DB from the schema file; seed `llm_wiki_meta` with the 8.0.2-era engine schema version (key and value read from the 9.18.0 dist, expected 15) so v16–v19 actually execute their guards; run the real exported 9.18.0 `setup()` with prefix `llm_wiki_`; assert it resolves and the engine version reads 19. Second case: meta key absent (CLI-only brain). Negative control: add an extra column to `llm_wiki_source_documents` → `setup()` throws (proves the v17 invariant check is exercised, not bypassed).
3. `pnpm why` → 9.18.0 in the receipt.
4. `wip(engine9):` commit.

Gates: `pnpm vitest run src/__tests__/engine9-setup-compat.test.ts`; G-vitest (97 files, 625 + new passed + 1 skipped); G-tsc; filtered Rust schema-fixture test.
RED proof: the negative-control case (no natural pre-change RED: an 8.0.2 DB upgrades cleanly under the engine by design).
Evidence: receipts; LEDGER: the meta key/value used and the exported entry point name.
Ledger text: `T8 — real 9.18.0 setup() completes over the Rust-chain DB (stamped-15 and unstamped cases); v17 negative control throws; schema dump fixture drift-tested`

### Task 9 — delete-path disposition (spec step 6): verification, no code change

Scope: spec §3 step 6; INTENT §12; §4 "vector-ledger consumption surface … identified at implementation time".
Blocked by: Q11.
Blocked-by tasks: T7 (triggers present).

Steps:

1. Re-grep raw entry deletes (`DELETE FROM llm_wiki_entries`) and any write to `llm_wiki_vector_removals` across `src-tauri/src` and `tools/src`. Expected: `commit.rs:634`, `queries.rs:227` (`clear_vault_tables`), `bundle_apply.rs:1017`; zero ledger writes/deletes. Record cites (they drifted from the spec's `:616`/`:982`). Receipt `t9-audit.out`.
2. Test: on a migrated fixture copy, drive each of the three call sites' functions (or their smallest public wrapper) over seeded entries with no marker → the ledger row count is unchanged by the hard delete, and pre-existing ledger rows (from a prior soft delete) SURVIVE each delete path (spec: "NOT deleting or bypassing ledger rows").
3. Draft the PR-body §12 divergence note in LEDGER: three raw delete sites stay raw; `forget()`/`erase` are TS-surface operations with no Rust binding (cite `packages/core/src/types.ts:1566-1570` at v9.18.0 per spec, re-verified in the installed dist's `.d.ts`); consumption obligation limited to (i) triggers survive migration (T6/T7 tests) and (ii) no ledger bypass (step 2 test).

Gates: filtered `db::connection` (or the module the step-2 test lives in) → GREEN.
RED proof: n/a (verification of unchanged code); the step-2 test fails if any path bypasses the ledger.
Ledger text: `T9 — delete paths dispositioned per spec step 6: 3 raw sites stay raw (no Rust forget/erase binding), triggers survive migration, ledger rows survive every delete path; §12 divergence note drafted`

### Task 10 — atomic commit, full gates, PR body

Scope: spec §5 Rust/Frontend gate list; ROLLOUT NOTE; ACCUMULATION REALITY; INTENT §12 record.
Blocked by: Q1, Q10, Q17.
Blocked-by tasks: T0–T9.

Steps:

1. Sync with origin/main (merge, never rebase). Re-check slot N (T6b); re-run `CI=true pnpm install --frozen-lockfile` + `pnpm why`.
2. Confirm no scratch test/driver remains (`git grep -n "scratch"` in changed files; `git status`). Stray `target/` dirs: none.
3. Controller squashes T2–T9 WIP commits into ONE atomic commit (§0.5). `git show --stat` must list `package.json`, `pnpm-lock.yaml`, `okf_ddl.rs`, `schema_guard.rs`, `connection.rs`, `ddl_compat.rs` (if T1 was folded), testdata files, the TS test. Receipt `t10-atomic.out`.
4. Full gates at the atomic head, each separately, disk pre-flight before each cargo gate, Rust via heavy-build:
   G-tauri, G-tools, G-heal, G-vitest, G-tsc, G-clippy (CI-exact line, §0.2). Q10: the CI integration line (`ci.yml:112`) if ruled in.
5. Expected: G-tauri ≥ 1708 + new tests, 0 failed; G-tools 61; G-heal 16; G-vitest 97 files, 0 failed (625 + new + 1 skipped); tsc exit 0; clippy exit 0. Any count drop without a planned deletion = regression.
6. PR body draft in LEDGER (sections): summary; ROLLOUT NOTE verbatim intent (forward-incompatible; update app, CLI, MCP sidecar in the same rollout; failure rests on column additions); ACCUMULATION REALITY (soft deletes enqueue from merge day; nothing drains until a consumer sets `external_index_in_use`; follow-up expo-llm-wiki#487); INTENT §12 divergence (T9 text); spec-cite drift (`commit.rs:634`, `bundle_apply.rs:1017`); slot N; rulings table; gate counts vs baseline.
7. Spec status line → "PR-B (…): Implemented <date>" (Q17); cleanup commit deletes `plan.md` + `LEDGER.md` in the same PR.
8. Post-session sweep (AGENTS.md): `du -sh "$CARGO_TARGET_DIR"`, stray `target/` check, `df -h /` and `/mnt/kv-work`.
9. Controller-owned, post-merge: `pnpm install` in the primary checkout reconciles shared `node_modules` (PR-A Q9 precedent) — unless Q3 ruled worktree-local.

Gates: full set above. Ledger text: `T10 — atomic commit assembled; full gates green vs baseline (counts recorded); clippy clean; PR body (rollout, accumulation, §12) drafted; spec status updated; sweep recorded`

## 3. Risks

1. **Same-commit rule vs reviewable tasks.** WIP commits are RED on ddl_compat by design; pushing one exposes a broken intermediate. §0.5 + Q1.
2. **Shared `node_modules`.** The 9.18.0 install lands in the primary checkout. A `pnpm tauri dev` there runs engine v16–v19 on the live brain; installed 8.0.2-guard binaries (app, CLI, MCP sidecar) then fail "unexpected columns". Q3.
3. **Column order.** ALTER-added columns append after CT-local `tier`/`embed_scheme`/`area`; fresh brains carry them inline. Literal sqlite_master equality is impossible; Q6 defines parity.
4. **Helper SQL has no drift gate.** `ddl_compat` covers template blocks only; helper mirrors (T7) can drift silently on a future bump (v13 precedent shares the gap). Q13.
5. **Trigger tripwire.** An unscoped UPDATE trigger on `llm_wiki_entries` breaks PR-A's `connection.rs:6563` test and would fire on the sso9 re-type. Q16.
6. **Positional inserts.** Any `INSERT INTO llm_wiki_entries VALUES (…)` without a column list breaks under new columns. T5 audit.
7. **Fixture binary in repo.** `ct-brain-v32-core-8.0.2.db` is a binary blob. Q5.
8. **Ledger growth.** Unbounded soft-delete accumulation is accepted, engine-owned behavior (spec); PR body records it.
9. **Integration tests.** G-tauri is `--lib`; `src-tauri/tests/*` (CI `ci.yml:112`) is unmeasured at baseline. Q10.
10. **Disk.** Cold build cost (~15–25 min, pins); 75G free at baseline. Pre-flight every cargo gate.
