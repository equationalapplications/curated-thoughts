# Watcher-arming self-check + headless `ct heal` — design spec

- Date: 2026-09-24
- Status: Draft — revised post-review (GLM 5.3 spec review 2026-09-24,
  verdict "changes requested"; all 6 Important + 5 Minor findings addressed
  in this revision; delta note sent back to the reviewer for convergence
  check)
- Branch: `feat/watcher-arming-self-check-and-ct-heal`
- Related backlog item: Sep 14 re-scoped P1 "add a first-class watcher-armed
  check and a headless heal trigger" (supersedes "Do 1" loop-step retirement)
- Related ops record: `ct-heal-runbook.md` (agents vault; not a repo doc) —
  this PR replaces that runbook's workaround stack with first-class machinery.

## Problem

The vault watcher is the sole trigger for both ingest staging and the heal
scheduler, and it can fail **silently** in two ways:

1. **Never arms.** Sep 10 21:35 relaunch (v2.10.1) came up with the watcher
   never armed: 3 nightly heal triggers (Sep 10–13) were complete no-ops yet
   every night reported "clean". Nothing logged, nothing surfaced.
2. **Dies mid-life.** Sep 15: the same GUI PID observed holding an inotify fd
   dropped to 0 with no relaunch — the watcher can die while the process
   lives, and nothing notices.

The code enables both failure modes:

- `spawn_vault_watcher` (src-tauri/src/watcher/fs_watcher.rs:52-93) calls
  `watcher.watch(&vault_path, RecursiveMode::Recursive)` exactly once
  (fs_watcher.rs:58) and never verifies arming again. Runtime notify errors
  are swallowed at fs_watcher.rs:81 (`Ok(Err(_)) => {}`); a drained notify
  backend just idles the 150 ms `recv_timeout` loop forever.
- Of the three spawn call sites, only the `start_file_watcher` Tauri command
  returns the error to the frontend (lib.rs:1867-1885); `switch_vault`'s
  restart and its recovery branch only `eprintln!` (eprintlns at lib.rs:1802
  and 1501). There is no persistent, loud, or pollable signal anywhere.
- Heal is GUI-only. The heal pass lives in private, Tauri-`State`-bound
  functions in src-tauri/src/lib.rs (`heal_invalid_sources`, lib.rs:417-500;
  scheduler thread `spawn_heal_scheduler`, lib.rs:502-538, 3 s debounce), and
  the headless `ct` CLI has no heal subcommand (tools/src/bin/ct.rs Cmd enum,
  ct.rs:14). Headless heal today requires an elaborate workaround stack
  (inotify-fd gate + up-to-8-minute empirical probe file + mid-trigger check)
  documented in the ops runbook — machinery the app should own.

Because the nightly heal cron cannot trust the watcher, it currently gates on
`/proc/<pid>/fd` inotify counts of a *foreign* process. The app is the right
owner of that signal.

## Goals

1. The app detects, loudly and persistently, when its vault watcher is not
   armed or has died — at startup and while running.
2. Heal can be run headlessly (`ct heal`) against the same code path the GUI
   scheduler uses, with the same write-gate discipline as `ct ingest`.
3. No new event channels, states, or duplication: reuse the wiki-status bus,
   the errors.log appender pattern, the PipelineHealth latch semantics, and
   the existing `tauri_app_lib` dependency from the tools crate.

## Non-goals

- No change to heal's contract: it evaluates only live `librarian_inferred`
  rows (lib.rs:438-442 selection unchanged), soft-deletes ungrounded rows,
  purges edges, and writes `healed` events. No new purge behaviors.
- No replacement of the pipeline watchdog (src-tauri/src/pipeline/watchdog/)
  or the reconcile startup sweep (src-tauri/src/reconcile.rs:50).
- No dotfile-ingest guard (separate backlog P3).
- No cross-platform fd probing beyond Linux `/proc` (elsewhere the check
  degrades to spawn-success + event-loop health, see below).
- No runbook edits in this repo (the ops runbook lives in the agents vault
  and is updated separately after the release lands).

## Design

### 1. WatcherHandle gains arming/liveness state (src-tauri)

`WatcherHandle` (fs_watcher.rs:26-50) gains:

- `armed_at: Arc<AtomicU64>` — unix-secs timestamp set once `watch()` returns
  Ok; 0 means "spawn failed before arming" (the current `?` on
  `RecommendedWatcher::new` / `watch` already prevents the handle from
  existing in that case, so this primarily distinguishes never-armed handles
  in tests and in `switch_vault` bookkeeping).
- `last_error_at: Arc<AtomicU64>` — bumped whenever the event loop consumes a
  `notify::Error` (the fs_watcher.rs:81 swallow becomes
  `Ok(Err(e)) => { last_error_at.store(now); eprintln!(...); }` — logged, not
  silently dropped).
- `pub fn is_alive(&self) -> bool` — Linux: at least one entry under
  `/proc/self/fd` whose target contains `inotify` (cheap `readlink` scan,
  matching the incident evidence source). Scope note: this counts the whole
  process's inotify fds, so it is a lower bound — it can never
  false-negative the incident signature (0 fds = backend closed = dead),
  but in principle another inotify user could mask a death with a
  false-alive; no other inotify user exists in the tree today; other
  platforms: `true` (no OS signal available; the event-loop error latch
  above is the signal).

No synthetic probe file at startup: ops data shows staging latency spans 1 s
to ~7 min under librarian scan-debounce, so a bounded probe is either slow or
false-negative-prone. The `/proc` fd count is instantaneous and matched the
incident signature exactly (0 fds = dead watcher, twice confirmed).

### 2. Periodic watcher self-check (src-tauri)

A lightweight monitor thread whose lifecycle is owned by
`start_file_watcher_inner` — the same function that owns the watcher and the
heal scheduler (spawned at lib.rs:1325) — NOT by `WatcherHandle` (which would
couple fs_watcher types to lib.rs state). A new
`WatcherMonitor(Mutex<Option<MonitorHandle>>)` Tauri state, managed alongside
`WatcherStarted`, holds `{ cancel: Arc<AtomicBool>, join: JoinHandle<()> }`.

Lifecycle rules on every path of `start_file_watcher_inner`:

- **Early return (same vault, lib.rs:1013-1019):** the existing handle and
  monitor are reinstated untouched — nothing is stopped, nothing is
  restarted, no tick can fire against a stale handle.
- **Real start/switch:** stop the old monitor first (cancel flag +
  `join_with_timeout` 2 s; on timeout, abandon — safe by the probe rule
  below), then the old watcher, then spawn both anew.
- **Spawn failure:** latch degraded immediately (errors.log append +
  `update_wiki_status_from_app`) before returning — the incident class is a
  startup failure, so the signal must not wait for the first tick. The
  monitor still starts; `armed_at = 0` makes its first tick re-assert the
  latched state.

Per-tick behavior (first tick at +5 s, then every 60 s):

- The monitor holds an `AppHandle` clone (like the heal scheduler thread)
  and resolves the CURRENT watcher handle (via `WatcherStarted`) and CURRENT
  vault path (via `VaultConfigState`) at each tick — never a captured pair.
  This makes an abandoned monitor harmless: it probes the truth about the
  current watcher and appends to the current vault's errors.log. The only
  residual is rare double-logging while an abandoned monitor drains, accepted
  here because monitor stops are 2 s-bounded joins that essentially always
  finish.
- If the current handle reports `armed_at == 0` or `!is_alive()`, latch
  degraded (once per transition) and:
  - append a line to `<vault>/.brain/errors.log` via a new appender following
    the `write_error_log` pattern (src-tauri/src/pipeline/mod.rs:448-470,
    same `[<unix-secs>] <msg>` format, same IO-error tolerance; on an
    unconfigured vault the append no-ops exactly like `write_error_log`);
  - bump the wiki-status flags via `update_wiki_status_from_app`
    (lib.rs:300-302) so the existing `wiki-status-change` event
    (lib.rs:141-154) carries it to the UI.
- Recovery latch: if a later tick finds the watcher alive again (after a
  `switch_vault` restart), clear the degraded flag through the same path.
- The monitor never restarts the watcher itself — restart policy stays with
  `switch_vault` and the user; the check's job is to make the failure loud,
  not to heal it silently.

### 3. Watcher health surfaces in the UI (src-tauri + frontend)

Extend `WikiStatusFlags` (lib.rs:133-140) with a watcher-health field.
Deliberately NOT the full `PipelineHealth` vocabulary
(src-tauri/src/pipeline/watchdog/mod.rs:150-166): its `Stalled` means a
supervised pipeline stage trip with recovery machinery behind it; a
transient notify error carries no such machinery, and reusing the string
would make StatusBar imply recovery that never comes. So:

- `watcherHealth: "working" | "degraded"` — `working` = armed and alive;
  `degraded` = not armed, fd probe dead, OR a notify error was consumed
  since the last clean tick (the transient/steady distinction lives in the
  errors.log lines and the degraded latch, not in a third UI state).
- Frontend: `useWikiStatus` (src/hooks/useWikiStatus.ts) already receives the
  snapshot + change events; StatusBar
  (src/components/shell/StatusBar.tsx:22-41,
  95-107) gains a loud watcher label mirroring the existing
  `degraded`/`stalled` ingest strings; MaintenanceDashboard
  (src/components/settings/MaintenanceDashboard.tsx:13) surfaces the same
  field. The
  snapshot backfill command (lib.rs:~2253) carries the new field so a
  mid-degradation mount shows it immediately — same shape as the PR #219
  `record_wiki_diagnostic` counts.

### 4. Loud spawn failures at all three call sites (src-tauri)

The `switch_vault` restart path (lib.rs:1793-1797) and recovery branch
(lib.rs:1492-1496) upgrade from bare `eprintln!` to the same errors.log
append + `update_wiki_status_from_app` degradation as §2, then continue (the
vault switch itself still succeeds; the watcher failure is reported, not
fatal to the switch).

### 5. Heal core extraction (src-tauri)

New module `src-tauri/src/db/heal.rs` (sibling to the existing
`db::commit::source_ref_is_still_grounded` it calls):

- `pub fn heal_invalid_sources_conn(conn: &mut Connection, vault: PathBuf) ->
  Result<HealSummary>` — the body of `heal_invalid_sources`
  (lib.rs:417-500) with the `State` extraction hoisted to callers. `HealSummary`
  = `{ evaluated: usize, soft_deleted: usize, edges_purged: usize }`.
  NOTE: `edges_purged` requires capturing `purge_edges_for_entry`'s `usize`
  return (edge_purge.rs:94), currently discarded at lib.rs:474 — capture it
  into the summary; behavior-neutral, otherwise the CLI's stdout contract
  undercounts.
- `lib.rs::heal_invalid_sources` becomes a thin wrapper: resolve
  `DbState`/`VaultConfigState`, open the connection, delegate, keep the
  existing `update_wiki_status_from_app` healing-flag choreography
  (lib.rs:527-533) in the scheduler thread. Behavior identical; the
  HealScheduler debounce is untouched.
- `heal_lost_librarian_inferred` (lib.rs:1987-2036) is left alone in this PR
  except for a follow-up comment: it overlaps but has a different
  selection/reachability contract; unifying it is a future cleanup, not a
  behavior change smuggled into this one.

### 6. `ct heal` subcommand (tools)

- New `Cmd::Heal { yes: bool }` variant in tools/src/bin/ct.rs (enum at
  ct.rs:14), placed after `Librarian`, mirroring the `Ingest` write-gate
  (ct.rs:303-318): without `--yes`, print the planned db path via
  `resolve_brain_paths()` (tools/src/paths.rs:23) and the live-row count it
  would evaluate, exit 1; with `--yes`, open via `write::open_rw`
  (tools/src/write.rs:63), run `heal_invalid_sources_conn`, print the
  `HealSummary`, exit 0.
- No debounce, no watcher, no GUI dependency: `ct heal` is a direct pass —
  this is what removes the nightly cron's dependence on a live GUI.
- Config/vault resolution identical to the other write commands
  (`CURATED_BRAIN_DB` / `CURATED_BRAIN_CONFIG` / `CURATED_BRAIN_DIR`,
  tools/src/paths.rs).
- Concurrent-writer safety: `write::open_rw` already sets
  `PRAGMA busy_timeout = 5000` (tools/src/write.rs:57-71), so a `ct heal`
  racing the desktop app's WAL writer retries transient locks rather than
  failing; the heal pass itself is already transactional per entry
  (unchecked_transaction, lib.rs:468-476). A simultaneous GUI heal run is
  idempotent-safe (both passes select the same live rows; a row
  already soft-deleted by the other pass drops out of the second pass's
  selection) — documented here, no new locking machinery. Expected cosmetic
  artifact: two concurrent passes can each write a `healed` event for the
  same entity (each snapshots its selection before the other's soft-delete
  commits); duplicate events are accepted and noted for the cron's reader.
- Stdout contract: one line of summary JSON
  (`{"evaluated":N,"soft_deleted":N,"edges_purged":N}`) so the nightly cron
  can log machine-readable verdicts instead of deriving them from SQL.

## Tests

src-tauri (must compile under `--features test-utils` for clippy and
`test-utils,mcp-server` for tests, per .github/workflows/ci.yml:102-112;
single-threaded execution):

- fs_watcher.rs tests (existing pattern: TempDir + mpsc sink,
  fs_watcher.rs:215-290): arming sets `armed_at`; `is_alive()` true after a
  healthy spawn on Linux. For the notify-error path, do NOT inject via
  channel tricks (dropping the notify sender yields `Disconnected`, not an
  `Err` item — the loop would break, not record): extract the error-handling
  into a small `record_watcher_error(&last_error_at, &err)` free function
  called from the `Ok(Err(e))` arm and unit-test that function directly; the
  loop wiring stays a one-liner.
- Heal core tests (new `db/heal.rs` test mod, connection-only like the
  reconcile suite's table-driven style, reconcile.rs:329-586): grounded row
  survives; ungrounded row soft-deleted with edges purged and a `healed`
  event written; summary counts correct.
- Self-check monitor test with the `test-utils` mock app pattern
  (cf. lib.rs:5715-5735): force `armed_at = 0`, tick once, assert the
  degraded flag latched exactly once and the errors.log line exists.

tools (no extra features):

- tools/tests/ct_heal.rs following ct_write_cmds.rs (tempdir +
  `temp_env::with_vars([("CURATED_BRAIN_DIR", …)])` + `run_ct()` via
  `env!("CARGO_BIN_EXE_ct")`): refusal without `--yes` (exit 1, refusal text
  on stderr, db path printed); `--yes` on a seeded ungrounded
  `librarian_inferred` row (seed via `tools/tests/common/mod.rs`'s
  `tauri_app_lib` API) exits 0, row soft-deleted, edges purged, summary JSON
  on stdout; `--yes` with nothing to do exits 0 with zero counts.

CI impact: none new — both crates' existing feature sets cover the above.

## Risks / open questions

- `/proc` probing is Linux-only; on macOS/Windows the self-check is
  blind to fd-level death (mitigated by the notify-error latch, which is
  portable). Acceptable: the failure incidents are all on the Linux
  workstation, and the alternative (synthetic probe) has a measured worst
  case of ~7 minutes.
- `WikiStatusFlags` grows a field — a serialization addition to the
  `wiki-status-change` payload; frontend consumers are additive readers, so
  no versioning concern.
- Monitor lifecycle is fully specified in §2: owned by
  `start_file_watcher_inner` (not `WatcherHandle`), reinstated untouched on
  the same-vault early return (lib.rs:1013-1019), stopped with a 2 s bounded
  join on real switches, and harmless even if abandoned (it probes the
  current handle and vault per tick). Shutdown: the monitor holds an
  `AppHandle` clone and never exits on app teardown — process exit reaps it;
  the final tick cannot misfire because it reads live state, and no
  degradation it latches after teardown is observable. Accepted.
