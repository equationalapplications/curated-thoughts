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
