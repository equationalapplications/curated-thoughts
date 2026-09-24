# Opus review round 4 (PR #228, b268826) — dispositions

Round-4 verdict was "Changes requested": 1 MAJOR (M1), 5 MINOR (m1-m5).
Addressed in commit <this commit>.

## MAJOR

- **M1 (`replace_monitor` not atomic → orphaned monitor on overlapping
  starts) — FIXED.** `replace_monitor` now installs the new handle with
  `.replace(...)` while holding the lock in ONE critical section, and stops
  the old handle only after releasing the lock. The take-then-install
  window (B installs M2 into the empty slot; A's install silently drops M2,
  which has no Drop and would run forever) is gone.

## MINOR

- **m1 (superseded-exit monitor can false-log "no registered handle"
  during B's slow reconcile) — FIXED.** The vault-changed `Ok` exit now
  spawns ONLY if the monitor slot is still empty: a newer start B is
  guaranteed to install its own monitor at registration, so A
  unconditionally replacing would both race B's install and start a
  monitor whose first 5s tick fires during B's reconcile.

- **m2 (round-2 m5 invariant false for the wrapper latch) — FIXED
  (reworded).** The doc on `spawn_watcher_monitor` no longer claims the
  stale-Degraded case cannot happen; it now names the exact
  counter-example (second `canonical_vault_from_config` failing after a
  concurrent start registered a live handle) and documents the
  bounded-stale-window rationale for not coupling the thread to the status
  lock. Shared-latch state remains rejected.

- **m3 (round-3 cross-reference filed in the round-2 dispositions file) —
  FIXED.** Moved to `2026-09-24_pr228-round3-dispositions.md`; the round-2
  file now just points there.

- **m4 (formatting-only hunks mixed into the behavior fix) — ACCEPTED for
  this commit / noted.** Those hunks are rustfmt output over code the
  closure wrapper moved; splitting them into a separate `style:` commit
  now would rewrite the branch history mid-review for zero behavioral
  value. Future behavior commits keep formatting out.

- **m5 (WAL "?" fallback not test-pinned) — NO CHANGE, by design.** The
  reviewer marked it "no change needed"; the fallback stays documented in
  the refusal comment.
