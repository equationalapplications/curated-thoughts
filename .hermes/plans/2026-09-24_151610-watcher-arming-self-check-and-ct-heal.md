# Watcher-arming self-check + headless ct heal — Implementation Plan

> **For Hermes:** Use subagent-driven-development skill to implement this plan task-by-task.

**Goal:** Implement the approved spec `docs/superpowers/specs/2026-09-24-watcher-arming-self-check-and-ct-heal-design.md` (PR #228 branch `feat/watcher-arming-self-check-and-ct-heal`) so the app detects a dead/never-armed vault watcher loudly and heal can run headlessly via `ct heal`.

**Architecture:** Six pieces, in dependency order: (1) `WatcherHandle` gains `armed_at`/`last_error_at` atomics + `is_alive()` + `record_watcher_error` seam; (2) a `WatcherMonitor` Tauri state + periodic tick thread owned by `start_file_watcher_inner` that resolves the live watcher handle per tick, latches `degraded` on `wiki-status-change`/snapshot, and appends to `.brain/errors.log`; (3) `watcherHealth` field carried on the status payload + frontend surfaces; (4) the two eprintln-only spawn call sites escalate like the command path; (5) heal core extracted to `db::heal::heal_invalid_sources_conn` with a `HealSummary` return; (6) `Cmd::Heal` in `tools/src/bin/ct.rs` with a `--yes` write gate and JSON stdout, plus per-piece tests.

**Tech Stack:** Rust (Tauri 2, notify, rusqlite, clap), TypeScript/React (Tauri IPC hooks), existing repo test harnesses (`cargo test`, `pnpm vitest`).

**Critical state note (2026-09-24):** The implementation described by this plan **already exists on the branch and is pushed** (tip `b861088`, clean tree, all CI-equivalent gates green). This plan documents the procedure end-to-end and remains the canonical reference; if re-executing, the per-task "verify" steps below confirm each piece is already in place — treat any drift between plan and branch as a finding to report, not something to silently re-implement.

**Spec:** `docs/superpowers/specs/2026-09-24-watcher-arming-self-check-and-ct-heal-design.md` (committed on this branch: 26ffb43, dd50c9c, e914df2, 4de3c7d).

---

## Current context / assumptions

- Branch: `feat/watcher-arming-self-check-and-ct-heal`, tracking `origin/` and up to date.
- PR: #228 "Spec: watcher-arming self-check + headless ct heal" (OPEN, base `main`).
- Working tree: clean.
- Gates that must be green before finishing: `cargo fmt --check`; `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --features test-utils -- -D warnings`; `cargo clippy --manifest-path tools/Cargo.toml --all-targets -- -D warnings`; `cargo test --manifest-path src-tauri/Cargo.toml --features test-utils --lib`; `cargo test --manifest-path tools/Cargo.toml`; `pnpm lint`; `pnpm vitest run`.
- Known unrelated pre-existing issue: `db::ddl_compat::rust_llm_wiki_ddl_matches_core_llm_wiki_package` can fail depending on the installed `node_modules/@equationalapplications/core-llm-wiki` version vs. the Rust `LLM_WIKI_PACKAGE_DDL` constant (7.1.1→7.7.4 bump in d261467). It is flaky/environment-dependent, unrelated to this spec. If it fails, re-run `pnpm install --frozen-lockfile` and re-test; do not "fix" it inside this PR.

---

## Files likely to change (full list, matches what landed)

| File | Change |
|---|---|
| `src-tauri/src/watcher/fs_watcher.rs` | `armed_at`, `last_error_at`, `is_alive()`, `record_watcher_error()`, 3 new tests |
| `src-tauri/src/watcher/mod.rs` | re-export `unix_secs_now` |
| `src-tauri/src/lib.rs` | `WatcherMonitor` state, monitor thread, degraded latch/clear + errors.log helpers, `WatcherHealth` + `watcher_health` on `WikiStatusFlags`/snapshot, `start_file_watcher_inner` lifecycle wiring, 2 escalation sites |
| `src-tauri/src/db/heal.rs` | NEW — `heal_invalid_sources_conn`, `HealSummary`, 4 tests |
| `src-tauri/src/db/mod.rs` | `pub mod heal;` |
| `src-tauri/src/pipeline/mod.rs` | `write_error_log` visibility widened to `pub(crate)` |
| `src-tauri/tests/edge_writer_gate.rs` | baseline entry for the heal.rs test fixture's edge INSERT |
| `src/lib/tauri.ts` | `watcherHealth?: 'working' \| 'degraded'` on `WikiStatusPayload` |
| `src/components/shell/StatusBar.tsx` | loud `⚠ Watcher degraded` pill |
| `src/components/settings/MaintenanceDashboard.tsx` | watcher health status line |
| `tools/src/cmds.rs` | `heal_run()` |
| `tools/src/bin/ct.rs` | `Cmd::Heal { yes }` + dispatch |
| `tools/tests/ct_heal.rs` | NEW — CLI write-gate tests |

---

## Step-by-step plan

### Task 1: WatcherHandle arming/liveness state

**Objective:** Make the watcher carry `armed_at`/`last_error_at` and a liveness probe, and stop swallowing notify errors.

**Files:**
- Modify: `src-tauri/src/watcher/fs_watcher.rs`
- Modify: `src-tauri/src/watcher/mod.rs`

**Steps:**
1. Add `pub fn unix_secs_now() -> u64` (SystemTime since epoch, `0` on clock failure).
2. Add `pub fn record_watcher_error(last_error_at: &AtomicU64, err: &notify::Error)` — stores `unix_secs_now()` and `eprintln!`s. Free function (not a method) so tests can drive it without channel tricks.
3. Extend `WatcherHandle` with `pub armed_at: Arc<AtomicU64>` and `pub last_error_at: Arc<AtomicU64>`.
4. Add `pub fn is_alive(&self) -> bool` — Linux: scan `/proc/self/fd` via `fs::read_dir` + `fs::read_link`, return true if any target contains `inotify`; fail **open** (true) if `/proc` unreadable; non-Linux: always `true`.
5. In `spawn_vault_watcher`, set `armed_at` after `watch()` returns Ok, wire `Ok(Err(e)) => record_watcher_error(...)` into the event loop (replacing the silent swallow).
6. Re-export `unix_secs_now` from `watcher/mod.rs`.
7. **Tests (TDD):** add `spawned_watcher_sets_armed_at`, `freshly_spawned_watcher_is_alive`, `record_watcher_error_bumps_last_error_at` in the existing `mod tests` (TempDir + mpsc pattern already used there).
8. Verify: `cargo test --manifest-path src-tauri/Cargo.toml --features test-utils --lib watcher::` → 11 passed (8 pre-existing + 3 new).
9. Commit: `feat(watcher): arm/liveness state on WatcherHandle + record_watcher_error seam`

**Verify on existing branch:** `git show bd24875 --stat` shows only `src-tauri/src/watcher/fs_watcher.rs`.

### Task 2: Heal core extraction

**Objective:** Move the heal body out of the Tauri-State-bound lib.rs fn into a conn-only core that returns a countable summary.

**Files:**
- Create: `src-tauri/src/db/heal.rs`
- Modify: `src-tauri/src/db/mod.rs` (`pub mod heal;`)
- Modify: `src-tauri/src/lib.rs` (`heal_invalid_sources` becomes a thin wrapper)
- Modify: `src-tauri/tests/edge_writer_gate.rs` (baseline entry for the fixture INSERT)

**Steps:**
1. Create `HealSummary { evaluated: usize, soft_deleted: usize, edges_purged: usize }` (`Clone, Debug, Default, PartialEq, Serialize`).
2. Implement `pub fn heal_invalid_sources_conn(conn: &mut Connection, _vault: PathBuf) -> Result<HealSummary>` — same SELECT (`deleted_at IS NULL AND source_ref IS NOT NULL AND source_type = 'librarian_inferred'`), same `source_ref_is_still_grounded` policy, per-row atomic soft-delete + `purge_edges_for_entry` (capture its return), `healed` events per affected entity. Do NOT touch `heal_lost_librarian_inferred` (add only a follow-up comment).
3. `heal_invalid_sources` in lib.rs keeps its `DbState`/`VaultConfigState` signature and delegates.
4. Note: `edge_purge`'s partner-alive retention means an edge whose partner is still live survives; fixtures must pre-soft-delete partners to see purges (existing `heal_lost_librarian_inferred_purges_edges_of_soft_deleted_entries` test shows the pattern).
5. **Tests (table-driven, conn-only, `open_in_memory()`):** `heals_ungrounded_row_purges_edges_and_counts_everything`, `grounded_row_stays_live_and_its_edges_survive`, `empty_brain_yields_zero_summary_and_writes_no_events`, `skipped_rows_are_not_double_evaluated_on_second_pass`.
6. Register the heal.rs fixture edge-insert in `tests/edge_writer_gate.rs` baseline (the gate's own remediation path; heal core is purge-only).
7. Verify: `cargo test --manifest-path src-tauri/Cargo.toml --features test-utils --lib db::heal` → 4 passed.
8. Commit: `feat(db): extract heal core to db::heal::heal_invalid_sources_conn`

**Verify:** `git show 9638677 --stat`.

### Task 3: WatcherMonitor + degraded latch + status wiring

**Objective:** Periodic self-check thread that resolves the live handle/vault per tick, latches degraded, appends errors.log, and clears on recovery.

**Files:**
- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/src/pipeline/mod.rs`

**Steps:**
1. Widen `write_error_log` to `pub(crate)` (same appender pattern, writes `<vault>/.brain/errors.log`).
2. Add `WatcherMonitorHandle { cancel: Arc<AtomicBool>, join: JoinHandle<()> }` with a `stop()` that cancels + joins (2s bound via `pipeline::watchdog::join_with_timeout` semantics; on timeout, abandon), and `WatcherMonitor(Mutex<Option<WatcherMonitorHandle>>)` state.
3. Add `enum WatcherHealth { Working, Degraded }` with `as_str()`; add `watcher_health: WatcherHealth` to `WikiStatusFlags` (default `Working`) and to `WikiStatusSnapshot` + `emit_wiki_status` payload.
4. Add `latch_watcher_degraded(app, reason)` — sets `flags.watcher_health = Degraded` + `write_error_log(vault, reason)`; `clear_watcher_degraded(app)` — sets `Working`.
5. Add `spawn_watcher_monitor(app)` — first tick 5s, then 60s. Per tick: resolve `WatcherStarted` (live) and `VaultConfigState` (live) via `try_state`; degraded iff `armed_at == 0 || !is_alive() || last_error_at bumped since last clean tick`; clean tick records `unix_secs_now()` and clears the latch. No-handle → idle (spawner latches on failure).
6. Wire lifecycle in `start_file_watcher_inner` (which needs the new `State<WatcherMonitor>` param — add `#[allow(clippy::too_many_arguments)]`):
   - early-return path (same vault): reinstate monitor + handle untouched;
   - real start: stop old monitor first (cancel + 2s bounded join), then old watcher, then spawn both;
   - spawn failure: `latch_watcher_degraded` **before** returning the Err;
   - successful registration: store new monitor + `clear_watcher_degraded`.
7. Register `.manage(WatcherMonitor(Mutex::new(None)))` in the real app builder (test mocks don't need it).
8. Verify: `cargo check --manifest-path src-tauri/Cargo.toml --features test-utils --all-targets` → clean; full lib suite → 947 passed.
9. Commit: `feat(watcher): periodic self-check monitor + degraded latch + watcherHealth on wiki status`

**Verify:** `git show fd7903c --stat`.

### Task 4: Frontend surfaces

**Objective:** Carry `watcherHealth` to the UI and make degradation loud.

**Files:**
- Modify: `src/lib/tauri.ts`
- Modify: `src/components/shell/StatusBar.tsx`
- Modify: `src/components/settings/MaintenanceDashboard.tsx`

**Steps:**
1. `tauri.ts`: add `watcherHealth?: 'working' | 'degraded'` to `WikiStatusPayload` (absent on older backends → treated as working).
2. `StatusBar.tsx`: when `wikiStatus.watcherHealth === 'degraded'`, render a loud button (`⚠ Watcher degraded`) styled like the existing diagnostics pill, with title/aria-label pointing at `.brain/errors.log`.
3. `MaintenanceDashboard.tsx`: status line — degraded: "⚠ Vault watcher degraded: the watcher is not armed or has stopped, so file changes are NOT being ingested. Details in .brain/errors.log."; working: "Vault watcher: working (armed and alive at the last self-check)." with `aria-live="polite"`.
4. Verify: `pnpm lint` clean; `pnpm vitest run` → 524 passed, 1 skipped (pre-existing). Note: repo-root `tsc --noEmit` has pre-existing upstream type-drift errors unrelated to these files; don't chase them in this PR.
5. Commit: `feat(ui): surface watcherHealth in StatusBar and MaintenanceDashboard`

**Verify:** `git show 5555e58 --stat`.

### Task 5: Spawn-call-site escalation

**Objective:** The two eprintln-only call sites must escalate like the command path.

**Files:**
- Modify: `src-tauri/src/lib.rs`

**Steps:**
1. Recovery branch eprintln (~lib.rs:1501 area, inside `recover_after_failed_switch_vault`): after the eprintln, call `latch_watcher_degraded` with the error.
2. Switch-restart eprintln (~lib.rs:1802 area, after successful switch): same escalation.
3. The command path (`start_file_watcher`) already returns the error to the frontend, and `start_file_watcher_inner` itself latches on spawn failure (Task 3 step 6) — no change needed there.
4. Verify: `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --features test-utils -- -D warnings` clean.
5. Commit: `feat(watcher): escalate watcher spawn failures to errors.log + degraded status`

**Verify:** part of `fd7903c` on the branch (the escalation logic shipped inside the monitor commit).

### Task 6: `ct heal` subcommand

**Objective:** Headless heal with the same write-gate discipline as `ct ingest`, printing a machine-readable summary.

**Files:**
- Modify: `tools/src/cmds.rs`
- Modify: `tools/src/bin/ct.rs`
- Create: `tools/tests/ct_heal.rs`

**Steps:**
1. `cmds.rs`: `pub fn heal_run() -> Result<()>` — `crate::write::resolve()` → `open_rw` (5s busy timeout, matches every other concurrent writer) → `tauri_app_lib::db::heal::heal_invalid_sources_conn(&mut conn, brain_dir)` → `println!` single JSON object `{"evaluated":N,"soft_deleted":N,"edges_purged":N}`.
2. `ct.rs`: add `Cmd::Heal { yes }` after `Cmd::Librarian` in the enum; dispatch mirrors the `Cmd::Ingest` gate (`ct.rs:303-318`): without `--yes`, resolve paths and `eprintln!("refusing: \`ct heal\` would soft-delete ungrounded wiki entries in {db_path} (a write). Pass --yes to proceed.")`, return `Ok(1)`.
3. **Tests** (`tools/tests/ct_heal.rs`, following `ct_write_cmds.rs` patterns — `run_ct` helper with `CURATED_BRAIN_DIR` tempdir, `init_brain_db`, then raw rusqlite seeding):
   - `heal_without_yes_exits_one_naming_db_and_does_not_mutate`: refusal exit 1, stderr contains `--yes` and the db path; seeded ungrounded row still live after.
   - `heal_with_yes_heals_purges_and_prints_summary_json`: exit 0; stdout parses as JSON with `evaluated:1, soft_deleted:1`; row soft-deleted; dead-partner edge purged, live edge kept; exactly one `healed` event in `llm_wiki_events`.
   - Fixture detail: seed one entry with legacy path `source_ref` pointing at a nonexistent document plus a grounded control entry; seed edges so one has a dead partner and one doesn't.
4. Verify: `cargo test --manifest-path tools/Cargo.toml --test ct_heal` → 2 passed; full tools suite → all binaries 0 failed.
5. Commit: `feat(tools): ct heal subcommand with --yes write gate + summary JSON`

**Verify:** `git show c25cd61 --stat`.

### Task 7: Final gate sweep + push

**Objective:** Everything green at the tip, then publish.

**Steps:**
1. `cargo fmt` then `cargo fmt --check` → clean.
2. Both clippy gates → clean.
3. `cargo test --manifest-path src-tauri/Cargo.toml --features test-utils --lib` → 947 passed, 0 failed.
4. `cargo test --manifest-path tools/Cargo.toml` → 0 failed everywhere (incl. `edge_writer_gate`).
5. `pnpm lint` + `pnpm vitest run` → clean.
6. Reconcile with remote if the branch diverged (this happened once: a sibling had pushed the same gate-baseline commit; `git rebase` resolved it — re-verify `git diff <remote>..HEAD` is only the intended delta).
7. `git push origin feat/watcher-arming-self-check-and-ct-heal`.

**Verify:** `git status --short --branch` shows `...origin/feat/watcher-arming-self-check-and-ct-heal` with no ahead/behind and no dirty files; PR #228 shows the commits.

---

## Tests / validation summary

- Rust unit: 3 new fs_watcher tests, 4 new db::heal tests.
- CLI integration: 2 new tests in `tools/tests/ct_heal.rs`.
- Suite totals at tip: src-tauri lib 947 passed; tools all green; vitest 524 passed / 1 skipped (pre-existing skip); eslint clean.
- All five gates re-verified after final commit.

## Risks, tradeoffs, open questions

- **`is_alive()` is process-wide, not per-watcher** (spec acknowledges): a future second inotify consumer could mask a death. Acceptable per spec; revisit if another inotify user lands.
- **Monitor tick is wall-clock based** — a system sleep can delay ticks; degradation is then detected late but never falsely (latch keys on real signals, not tick absence).
- **`write_error_log` visibility change** (`pub`→`pub(crate)`): minimal blast radius, same crate.
- **Pre-existing `ddl_compat` flake** (see context): environment-driven, do not fix here.
- **UI back-compat:** `watcherHealth` is optional on the payload; older backends simply don't send it and the UI treats absence as working.
