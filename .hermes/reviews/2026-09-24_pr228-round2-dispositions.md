# Opus review round 2 (PR #228, b861088..e2ec63b) — dispositions

Round-2 verdict was "Changes requested": 2 MAJOR (M1, M2), 6 MINOR (m1-m6).
All addressed in commit 0c1a223.

## MAJOR

- **M1 (`ct heal` refusal could CREATE brain.db) — FIXED.**
  `tools/src/bin/ct.rs`: the refusal count now opens via
  `tauri_app_lib::retrieval::open_brain_readonly` (read-only flags; fails on
  a missing file instead of creating it) and falls back to "?".
  New regression test `heal_refusal_does_not_create_brain_db_on_a_fresh_brain`
  (tools/tests/ct_heal.rs) runs the refusal against an empty tempdir and
  asserts brain.db still does not exist and the stderr shows the "?" count.

- **M2 (early returns after the monitor stop left no monitor running) — FIXED.**
  `src-tauri/src/lib.rs` `start_file_watcher_inner`: the entire post-stop body
  is now an inner closure; the wrapper latches degraded AND calls
  `replace_monitor` on ANY Err — covering "pipeline not running" (x2),
  `BrainConfig::load_lenient`, and the vault-lock failure. One exit path, no
  scattered `?`. The success path is unchanged (registers handle, restarts
  monitor, clears latch).

## MINOR

- **m1 (same failure written to errors.log up to 3x) — FIXED.** With the M2
  wrapper owning the latch, the inner spawn-failure arm no longer latches
  (it just propagates); the mid-path arms no longer call `replace_monitor`.
  One failure = exactly one errors.log line, written by the wrapper; the
  restarted monitor's first tick re-asserts at most once via its own
  transition gate (and the comment documents this).

- **m2 (comment overstated "later re-assert") — FIXED.** Reworded: with the
  change-only gate the monitor re-asserts exactly once at its first tick;
  the durable channel for a late-subscribing webview is the
  `get_wiki_status` snapshot.

- **m3 (MonitorTick doc lied; `degraded` param dead; unwrappable Option) —
  FIXED.** `MonitorTick.degraded_now` is now a plain `bool`; the doc matches;
  the unused `degraded` parameter and the dead `let Some(..) else` branch are
  gone; thread body and all five tests updated to the new signature.

- **m4 (change-only logic untested) — PARTIALLY FIXED / documented limitation.**
  The transition gate still lives in the thread body (needs an AppHandle +
  real state store to test). The pure verdict covering every degradation
  disjunct and timestamp-consumption rule is unit-tested (5 tests). Extracting
  the thread's latch update into a testable step fn is deferred — the
  behavior is 6 lines of bool-flip gated on the already-tested verdict.
  DEFERRED: extract a `MonitorLatch::step` helper as follow-up if the thread
  body grows beyond the gate.

- **m5 (thread latch can diverge from global status) — FIXED (documented
  invariant).** Added a doc paragraph on `spawn_watcher_monitor` explaining
  the divergence scenario and why today's call order makes it unreachable
  (callers latch only on failures that leave the handle dead/absent, so the
  thread's next tick agrees). Shared state rejected: it would couple the
  thread to WikiStatusState locking per tick for a case that cannot occur.

- **m6 (quote style nit in StatusBar.tsx) — FIXED.** Single → double quotes
  to match the file. (Round-3 m3: yes, this is an unrelated-formatting
  change inside the round-2 fix commit — it is the direct remediation of
  round-1's m6 and is called out here as its own line item.)
