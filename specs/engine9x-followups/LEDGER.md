# LEDGER — engine9x-followups (issue #376, PR #379)

Plan: `docs/superpowers/plans/2026-11-25-issue376-engine9x-followups.md` · Spec: `spec.md` (same dir, Q5–Q9 owner-approved 2026-11-25) · Investigation: `investigation.md` (FINAL)
Execution: SDD controller loop, implementers = Claude Code sonnet/low (default-implementer SOP), caps $0.90/task enforced via --max-budget-usd. Crash-resume scratch: gitignored `.superpowers/sdd/2026-11-25-issue376-engine9x-followups/progress.md`.

## Baselines (2026-11-25, heavy-build, HEAVY_EXIT=0)
- clippy src-tauri `--features test-utils,mcp-server --all-targets -D warnings`: EXIT 0 · clippy tools: EXIT 0
- src-tauri full suite `--test-threads=1`: **lib 1735 passed / 0 failed**; 91 result lines ALL ok · tools suite: green
- vitest: 97 files / **629 passed + 1 skipped**, exit 0 · tsc: exit 0
- engine9 filtered suite: 12 passed (1723 filtered) — Task 2 delta judged against this

## Recipe corrections (supersede plan literals)
- CARGO_TARGET_DIR = `<worktree-root>/.cargo-shared-target` (AGENTS.md one-canonical-location rule; plan Task 11's kv-work literal is STALE).
- Branch upstream is origin/main — push explicitly `git push origin HEAD:chore/engine9x-followups`, read back `git rev-parse origin/chore/engine9x-followups`.

## Task rows

| Task | Status | Commits | Gates | Adjudication |
|------|--------|---------|-------|--------------|
| 0 — plan-time checks | DONE | (ticked in plan) | 3-way inventory probe | CT_OWNED allow-list 3→8 (see plan Task 0 annotation); FTS-family + legacy_source_type_idx exclusions ruled for Task 9; Q6 join-back NO (S7 stands, no follow-up); S9 feasible in-lane |
| 1 — S1 stdout→stderr | DONE | `e2a490e9` | clippy EXIT 0; grep 22/0 verified | — |
| 2 — S2 PRAGMA grouping | DONE | `50849454` | engine9 12 passed (unchanged, controller re-run) | Deviation accepted: `*t == table` deref (mechanical type fix, behavior identical) |
| 3 — S3 CI comments | DONE | `1e28cbf5` | grep verified (4 lines = site-2 double text, intentional) | — |
| 4 — S4 .gitattributes | DONE | `0658ef0d` | check-attr verified (sql: text/eol=lf; db: binary macro → diff unset); no w/crlf | Child hit grant gaps (check-attr, ls-remote) — improvised correctly; controller re-verified |
| 5 — S5a sweep DELETEs | DONE | `d4d00fcb` | clear_vault_tables 7 passed (controller re-run); clippy ×2 exit 0 | Plan defect ruled: deposit_kick_state V25 root-gated → existence-guarded DELETE (semantic no-op when absent); other 5 plain; test seeds V25 shape. Implementer $0.22 initial (honest BLOCKED) + $0.37 resume leg. RED caveat ledgered (partial RED accepted) |
| 6 — S5b config-before-reopen + pins | DONE | `d3019ee7` | switch_pin_tests 3 passed; restore_sync 16 passed; clippy ×2 exit 0 (controller re-runs) | Restore pin = ordering essence (full install covered by restore_sync suite); Run 1 failure logged not propagated; "V22 untouched" not directly asserted — carried to stage-close review focus |
| 7 — S5c repopulation + drain hook | DONE | `bf364efd` | apply_stored 1, stated_ledger 14, pipeline 73, clippy ×2; NEGATIVE CONTROL: probe disabled → drain-hook pair FAILS (101), restored → 2 passed (controller) | RED-first skipped by implementer — ruled acceptable via controller negative control; worker tests drive a real temp brain + worker thread, fire counted via channel |
