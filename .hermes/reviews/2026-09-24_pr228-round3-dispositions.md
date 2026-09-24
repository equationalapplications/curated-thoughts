# Opus review round 3 (PR #228, 0c1a223) — dispositions

Round-3 verdict was "Changes requested": 1 MAJOR (M1), 5 MINOR (m1-m5).
Addressed in commit b268826.

## MAJOR

- **M1 (vault-changed `Ok` exit left no monitor running) — FIXED.**
  `src-tauri/src/lib.rs` `start_file_watcher_inner`: the
  `still_canonical != target_canonical` branch now calls
  `replace_monitor(&monitor, app)` before returning `Ok`. Comment documents
  why: this function stopped the newer generation's monitor at the top, so
  it must hand one back even on the superseded path. This restores the
  round-1 behavior the closure refactor dropped.

## MINOR

- **m3-note (cross-ref, moved here per round-4 m3):** the round-2 m6 fix
  (StatusBar quote flip) was an unrelated-formatting change inside the
  round-2 fix commit — it is the direct remediation of round-1's m6 and is
  itemized here to keep the audit trail tidy.

- **m1 (same failure logged up to 3x: wrapper + callers + fresh-monitor
  first tick) — FIXED (callers).** Both caller-side
  `latch_watcher_degraded` calls (`recover_after_failed_switch_vault`,
  `switch_vault` post-switch restart) removed — the inner wrapper owns
  latching on every Err; callers keep only the eprintln. Comments updated
  (the old "inner spawn-failure latch cannot fire" comment was false, as
  the reviewer said). The fresh-monitor first-tick re-assert remains by
  design: it is the spec §2 re-assert, transition-gated to one line, and
  the m5-style shared-latch alternative was already rejected in round-2.

- **m2 (440-line re-indent buries the diff; broken string-literal indents)
  — PARTIALLY FIXED / accepted.** The three stale multi-line `eprintln!`
  string indents are fixed (lib.rs reconcile/watch open-failure arms).
  The closure wrapper itself is KEPT: restructuring into a
  `start_file_watcher_body` helper would churn the diff a second time in
  the same PR and the wrapper is functionally the M2 fix round 2 asked
  for. The re-indentation is mechanical, one-time, and every future diff
  against this function is clean. ACCEPTED with rationale rather than
  reverted — reverting would reintroduce the round-2 M2 bug.

- **m3 (StatusBar quote flip not itemized in dispositions) — FIXED.**
  Round-2 dispositions file now carries an explicit line item for it
  (round-3 m3 cross-reference added).

- **m4 (WAL/-shm read-only-open caveat undocumented) — FIXED.**
  `tools/src/bin/ct.rs` refusal comment now states that on a WAL-mode
  brain.db with a missing/unwritable `-shm`, the read-only open can fail
  and the count shows "?" — documented as the designed fallback.

- **m5 (divergence caveat enforced only by prose) — PARTIALLY FIXED /
  deferred.** Kept as documented invariant for now; a `debug_assert` would
  need the monitor thread to read `WikiStatusFlags` on every tick, which is
  exactly the coupling the invariant exists to avoid. DEFERRED: add a
  debug_assert if a second Degraded-writing caller ever lands (tracked in
  this file; nothing to do today).
