# Final fresh-eyes pass (PR #228, 9275a09..34ab002) — dispositions

Full-range BLOCKER-only pass per the termination rule step 5. Verdict:
**approve after fixing M1** (one-line `is_finished()` check); m1 "worth
including in the same pass". All findings addressed in this commit.

## MAJOR

- **M1 (non-Linux liveness can't see a dead watcher thread; Linux relies on
  the fd-scan guess) — FIXED.** `WatcherHandle::is_alive` now starts with
  `self.join.is_finished()` (stable since 1.61) — the exact cross-platform
  signal for event-loop-thread death (callback panic, `Disconnected`
  break), which `last_error_at` never sees. The Linux fd scan remains as
  the backend-death check on top.

## MINOR

- **m1 (switch_vault teardown leaves the monitor running → false "no
  registered handle" alarm during the ≤10s pipeline join / restore /
  reopen window) — FIXED.** The teardown block now takes and stops the
  monitor alongside the watcher; restart and recovery respawn it via
  `start_file_watcher_inner`.

- **m2 (`ct heal` passes brain_dir as the `vault` argument) — FIXED.**
  `heal_run` now resolves and passes the CONFIGURED VAULT ROOT (falling
  back to the brain dir only when unset), matching the GUI scheduler's
  call-site semantics if the grounding check ever reads the parameter.

- **m3 (comment misstates what `run_wiki_heal` runs) — FIXED.** Reworded:
  `run_wiki_heal` calls the sibling pass only; `heal_invalid_sources` runs
  from the scheduler thread and `ct heal`.

- **m4 (stale `lib.rs:417-500` line reference in db/heal.rs) — FIXED.**
  Reference is now by function name.

- **m5 (scheduler discards HealSummary/Err) — FIXED.** The scheduler now
  eprintlns the summary when it soft-deleted anything and the error on
  failure — silent no-op heals were precisely the Sep 10-13 incident class.
