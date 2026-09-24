Verification is done. The main finding is in the frontend: the hook throws away the new field, so the UI never shows it. I'm writing up the review now.

## Review: PR #228 — watcher-arming self-check + `ct heal`

### BLOCKER

**B1. The UI never receives `watcherHealth`, so the StatusBar and MaintenanceDashboard warnings can't appear.** `src/hooks/useWikiStatus.ts` builds its state from explicit field lists in two places, and the PR changes neither:
- The snapshot path (`useWikiStatus.ts:126-136`) copies nine named fields into `applyPayload` and leaves out `watcherHealth`.
- The event path (`useWikiStatus.ts:167-177`) does the same when it builds `payload`, then spreads it into state (`...payload`). `watcherHealth` is dropped from every `wiki-status-change` event too.

So `wikiStatus.watcherHealth` is always `undefined`. The `=== "degraded"` checks in `StatusBar.tsx` and `MaintenanceDashboard.tsx` are never true, and the dashboard always says "working". The backend work is correct, but the user sees nothing, which is exactly the "silent failure" this PR is meant to fix. The spec's snapshot-backfill requirement (§3) is also not met. The fix is to add `watcherHealth: snapshot.watcherHealth ?? 'working'` to the snapshot path and `watcherHealth: normalized.watcherHealth ?? prev.watcherHealth` to the event path, plus a hook test.

### MAJOR

**M1. One notify error latches "degraded" for good and appends to errors.log every 60 s.** In `lib.rs` `spawn_watcher_monitor`, the error branch runs `continue` without moving `last_clean_tick_secs` forward. After one transient `notify::Error`, `last_error_at > last_clean_tick_secs` stays true on every later tick, so:
- the watcher never recovers to `working` without a vault switch. The spec's recovery latch (§2) can't fire for this case.
- `latch_watcher_degraded` runs every tick, so each tick writes one errors.log line and one `wiki-status-change` emit. That is about 1,440 lines a day. The spec says to latch "once per transition".

The fix is to track a `degraded: bool` in the thread and only append/emit on a transition. For the error disjunct, record the error timestamp you consumed (e.g. `last_seen_error = last_error_at`), so a later clean tick can clear it.

**M2. Every clean tick emits `wiki-status-change`, even when nothing changed.** `clear_watcher_degraded` → `update_wiki_status_from_app` → `update_wiki_status` (`lib.rs:315-323`), which always emits because there is no check for a change. That means a full-payload event and a re-render of every `useWikiStatus` consumer every 60 s for the life of the app. The fix is the same transition gate as M1, or compare inside the updater and skip the emit when the value is unchanged.

**M3. After a spawn failure no monitor runs, which the spec explicitly forbids.** `start_file_watcher_inner` stops the old monitor (`lib.rs:~1136`) before it spawns. On the `Err(e)` branch (`lib.rs:~1543`) it latches and returns without starting a new one, and it does the same on every earlier `?` return between the monitor stop and registration. The spec (§2, "Spawn failure") says "The monitor still starts" and that ticks re-assert the latched state.

Combined with B1, a spawn-time degradation is emitted exactly once, often before the webview is listening, and never again. Once B1 is fixed, the snapshot covers the UI side, but the "re-assert" behaviour and the spec's promise are still missing. Also, the tick's `None =>` branch comment ("just idles") contradicts the spec, which says a missing handle should read as `armed_at == 0`.

**M4. Spec-listed tests are missing.**
- There is no self-check monitor test (spec Tests: "force `armed_at = 0`, tick once, assert … latched exactly once and the errors.log line exists"). That test would have caught M1.
- There is no `ct heal --yes` test for the nothing-to-do case (zero counts, exit 0).
- There is no frontend test for `watcherHealth` propagation, which would have caught B1.

### MINOR

- **m1.** In `tools/src/bin/ct.rs:330-343`, the refusal without `--yes` prints only the db path. The spec (§6) also asks for "the live-row count it would evaluate".
- **m2.** `tools/src/cmds.rs` `heal_run` builds the JSON by hand, but `HealSummary` already derives `Serialize`. `serde_json::to_string(&summary)` keeps the stdout contract tied to the struct.
- **m3.** The `tools/tests/ct_heal.rs:18-21` doc comment says the fixture has "one edge whose partner is alive (must survive)". The fixture actually has a self-edge that is asserted to be purged. The comment should match.
- **m4.** In `lib.rs` `spawn_watcher_monitor`, `last_clean_tick_secs` uses whole seconds and a strict `>`. An error recorded in the same second as a clean tick is missed. Comparing against the consumed error timestamp (see M1) fixes this too.
- **m5.** `is_alive` counts inotify fds for the whole process. On Linux, GTK/GIO/WebKitGTK inside a Tauri app can open their own inotify fds, which would hide a dead watcher. The spec's claim that "no other inotify user exists in the tree" only covers our code, not linked libraries. The incident data (0 fds) suggests they don't open any today. That's worth a comment, or a check that compares against a baseline count taken before arming.
- **m6.** `src/lib/tauri.ts`, `StatusBar.tsx` and `MaintenanceDashboard.tsx` pick up a lot of unrelated reformatting (single → double quotes, re-wrapping). There's no prettier config in the repo, and `useWikiStatus.ts` still uses single quotes. This buries the real changes and makes the codebase's style inconsistent. Revert the formatting-only hunks.
- **m7.** In `WatcherMonitorHandle::stop`, the doc says an abandoned monitor "finds no handle to check". In fact it finds the *new* handle and could double-log alongside the new monitor. The spec admits this, but the comment is wrong.
- **m8.** The "Heal Database" button (`run_wiki_heal`, `lib.rs:2321`) calls `heal_lost_librarian_inferred`, not the extracted core. So `ct heal` matches the scheduler but not the button. This is documented in the follow-up note, but the `heal.rs` module doc should say that "the Tauri maintenance commands" do not use this path.

**Verdict:** Changes requested. The UI never receives `watcherHealth` (B1), and one notify error permanently latches "degraded" while writing to errors.log every minute (M1). Fix both, with tests, before merging.