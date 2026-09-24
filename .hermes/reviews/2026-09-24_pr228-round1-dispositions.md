# Opus review round 1 (PR #228) — dispositions

Prior review: 1 BLOCKER, 4 MAJOR, 8 MINOR. Verdict was "Changes requested".
All findings addressed in commit e2ec63b.

## BLOCKER

- **B1 (UI never receives `watcherHealth`)** — FIXED. `src/hooks/useWikiStatus.ts`
  now carries `watcherHealth` on all three paths: initial state
  (`watcherHealth: 'working'`), snapshot path
  (`snapshot.watcherHealth ?? 'working'`), event path
  (`normalized.watcherHealth ?? prev.watcherHealth ?? 'working'`).
  Regression tests added in `src/__tests__/useWikiStatus.test.ts`
  (4 new tests: degraded via event, prior value kept when event omits the
  field, snapshot default when backend omits, degraded via snapshot).

## MAJOR

- **M1 (one notify error latches degraded forever + errors.log every 60s)** —
  FIXED. `spawn_watcher_monitor` in `src-tauri/src/lib.rs` now tracks a
  `degraded` bool and consumes the error timestamp (`last_seen_error_secs`):
  latch/append only on a clean→degraded transition; the same error can never
  re-trip on a later tick; a later clean tick clears the latch (one emit).
  Verdict logic extracted into the pure `monitor_tick_verdict` fn, unit-tested
  in `mod watcher_monitor_verdict_tests` (5 tests, incl. the M1 re-trip
  regression and the m4 same-second case).
- **M2 (every clean tick emits wiki-status-change)** — FIXED. Clean ticks
  while already working do nothing; `clear_watcher_degraded` fires only on
  the degraded→working transition.
- **M3 (spawn failure leaves no monitor running; missing-handle tick idles)** —
  FIXED. Extracted `replace_monitor()` and call it on ALL early-return paths
  of `start_file_watcher_inner` (spawn Err, canonicalize Err,
  still_canonical mismatch) so a monitor always runs and its ticks
  re-assert the latched state (spec §2 "the monitor still starts"). The
  missing-handle tick now latches degraded (transition-gated) instead of
  idling, matching the spec's "missing handle reads as armed_at == 0".
- **M4 (missing tests)** — FIXED.
  - Monitor tick test: `watcher_monitor_verdict_tests` (5 unit tests over
    the pure verdict fn — covers "force armed_at = 0 / new error → latched
    exactly once" semantics plus consumption/re-trip).
  - `ct heal --yes` nothing-to-do case:
    `heal_with_yes_on_clean_brain_exits_zero_with_zero_summary` in
    `tools/tests/ct_heal.rs` (exit 0, `{"evaluated":0,"soft_deleted":0,
    "edges_purged":0}` on stdout).
  - Frontend `watcherHealth` propagation: the 4 new hook tests above.
    (The monitor-loop wall-clock behavior itself remains untested at the
    thread level; the pure-verdict seam is the tested contract.)

## MINOR

- **m1 (refusal missing live-row count, spec §6)** — FIXED. The
  `ct heal` refusal now opens the db read-only, counts live
  `librarian_inferred` rows, and prints
  "would evaluate N live librarian_inferred row(s) …" ("?" fallback on any
  error so the refusal never masks itself). Pinned by
  `heal_refusal_reports_the_live_row_count_it_would_evaluate`.
- **m2 (hand-built JSON)** — FIXED. `heal_run` now prints
  `serde_json::to_string(&summary)`; stdout contract tied to the struct.
  Existing ct_heal JSON assertions prove the shape is unchanged.
- **m3 (stale fixture comment)** — FIXED. `seed_heal_fixture` doc now
  describes the actual edges (dead partner + self-edge, both purged).
- **m4 (same-second error missed by strict `>`)** — FIXED. The error
  disjunct now compares against the consumed `last_seen_error_secs`, not
  the last clean tick; pinned by `same_second_error_is_still_seen`.
- **m5 (is_alive process-wide fd count / GTK masking)** — FIXED (documented).
  `is_alive` doc now names the linked-library masking risk (GTK/GIO/
  WebKitGTK) and the remedy (baseline count before arming, compared per
  tick) if a future dependency starts holding inotify fds. No behavior
  change: incident data shows no masked fds today, and a baseline adds
  state for a risk with no observed instance.
- **m6 (formatting churn burying the diff)** — FIXED. `src/lib/tauri.ts`,
  `StatusBar.tsx`, `MaintenanceDashboard.tsx` restored to the pre-PR style;
  the branch diff for these files is now watcher-only (5 / 11 / 11 lines).
- **m7 (wrong stop() doc comment)** — FIXED. Comment now describes the
  real behavior: an abandoned wedged monitor finds the NEW handle and may
  log one latched line alongside the new monitor before exiting; the spec
  accepts that window.
- **m8 (heal.rs doc overclaims "Tauri maintenance commands" use this core)** —
  FIXED. Module doc now lists the real callers (scheduler +
  `ct heal`) and states explicitly that `run_wiki_heal` ("Heal Database"
  button) still calls `heal_lost_librarian_inferred` — consolidation is
  deliberate follow-up, not an oversight.
