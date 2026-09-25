# Opus review round 5 (PR #228, 577c2a4) — dispositions

Round-5 verdict: **Approve with nits** — 0 BLOCKER, 0 MAJOR, 5 MINOR.
Per the termination rule (0 BLOCKERs, no unresolved prior MAJORs, every
finding dispositioned), the cycle closes here; these nits are addressed in
this commit as comment/doc/audit-trail fixes (no behavior changes).

## MINOR

- **m1 (superseded-exit comment overclaims the false-log prevention) —
  FIXED.** Comment rewritten to say plainly that while B is mid-reconcile
  the slot is empty anyway, so A's monitor CAN still fire "no registered
  handle" once and latch Degraded until B registers — accepted (one line
  per overlapping start, self-clears on B's success, and not spawning
  would leave zero monitors if no follow-up start comes).

- **m2 ("bounded by the next user-visible watcher action" is too strong) —
  FIXED (behavioral, the reviewer's own suggestion).** The monitor thread's
  latch now starts as `Option<bool>::None` ("unknown") and the FIRST tick
  always publishes its verdict: a healthy first tick actively clears any
  stale caller-Degraded (closing the same-vault early-return hole), a
  degraded first tick logs once (the spec §2 re-assert). The "bounded"
  caveat in the doc is replaced by this mechanism.

- **m3 (stale replace_monitor doc: "stop-old-then-spawn-new") — FIXED.**
  Doc now states the actual order: spawn-new under lock, swap, release,
  stop-old, with the harmless sub-second two-monitor overlap explained.

- **m4 (line 1275 stops the monitor while holding the MutexGuard through
  the 2s join) — FIXED.** `take()` now binds to a local first, guard drops,
  then `stop()` runs — matching the round-4 atomicity rationale.

- **m5 (disposition file nits) — FIXED.** Round-4 placeholder replaced with
  the real hash (577c2a4); the round-3 file's duplicated m3 bullets merged
  into one, with the cross-reference moved where round-4 m3 asked.
