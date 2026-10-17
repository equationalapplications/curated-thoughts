# Handoff — issue271 two-stage retrieval implementation (Task 1 done, Task 2 next)

> **SUPERSEDED (pr283-review finding 3):** this is a Task-1-era snapshot kept
> for history — it is NOT current state. The AUTHORITATIVE status surfaces are
> the plan's adjudication notes
> (`docs/superpowers/plans/2026-10-15-issue271-two-stage-retrieval.md`,
> per-task checkbox + ledger annotations) and the session ledger
> (`.superpowers/sdd/2026-10-15-issue271-two-stage-retrieval/progress.md`).
> All absolute local-home paths below are historical scratch paths, not repo
> references.

**Date:** 2026-10-15 · **Branch:** `spec/issue271-two-stage-retrieval-impl` (repo: `equationalapplications/curated-thoughts`)
**Plan:** `docs/superpowers/plans/2026-10-15-issue271-two-stage-retrieval.md` (17 tasks)
**Spec:** `docs/superpowers/specs/2026-10-09-issue271-two-stage-retrieval-design.md` (rev 34, Approved)

## State

- **Task 1 (V28 migration) is DONE and REVIEW-APPROVED** (sor gate, 2026-10-15).
  Commit `9210dce` — `apply_v28_two_stage_columns` in `src-tauri/src/db/connection.rs`.
  - Adds `documents.embed_key TEXT` (NO backfill) + `documents.last_indexed_hash TEXT`
    (one-shot backfill `WHERE status IN ('indexed','pending_reindex')`, inside the ALTER
    branch only). V24 pattern: unlocked pre-check → BEGIN IMMEDIATE → under-lock
    re-inspect → ROLLBACK on error → V22-gated stamp (`INSERT OR IGNORE … 28`).
  - Call site: connection.rs:834, immediately after `apply_v27_embed_scheme`.
  - 3 new tests + 3 guard-pin bumps (MAX 27→28). Verified: `cargo test --manifest-path
    src-tauri/Cargo.toml --features test-utils,mcp-server -- --test-threads=1 v28`
    → 3 passed. Clippy --all-targets clean.
- Approved deviations (all commented in code):
  1. Pre-V15 fixture test builds the REAL V1–V14 chain (mirrors `build_pre_v18_brain`)
     instead of the plan's 5-col hand-built table (V15's explicit-column SELECT would
     crash on it).
  2. One-shot test rewinds stamps (`DELETE FROM schema_version WHERE version >= 22`)
     and re-migrates with a resolvable `VaultRoots` — rootless opens cap at 21, so the
     plan's literal test cannot see stamp 28.
  3. Two extra guard-pin bumps (27→28) beyond the one named in the plan — forced.
- Deferred (minor, from review): no test asserts `embed_key` stays NULL through an
  *upgrade* (rewind+re-migrate) path — only fresh-open. Optional to add later.
- Working tree clean except untracked `.cargo-shared-target/` (clippy cache, ignore).
- Plan checkboxes in the plan doc are NOT ticked yet (docs commit not made).

## Next steps

1. **Task 2** — shared eligibility predicate in `wisdom_match.rs`
   (`entry_eligibility_where` / `eligible_entry_ids`, refactors `gated_entries` :317
   onto it; MUST include V27 `scheme_filter_sql(temporal.scheme)` — a stage-1 query
   without it is a §3.2 violation). >1024-excludes parity guard test.
2. Then Tasks 3–17 in plan order (Task 3 stage-1 SQL consumes Task 2's predicate;
   Task 14/15 consume Task 5's entry point `wisdom_match_two_stage`).
3. Per task: TDD (Step 1 failing tests → Step 2 verify FAIL → implement → verify PASS
   → targeted + full-suite run → commit). Global Constraints command for full suite:
   see plan §Global Constraints; tests use `--features test-utils,mcp-server
   -- --test-threads=1`.
4. Optionally tick Task 1's plan checkboxes in a docs commit.

## Gotchas

- The repo has its own skills: `superpowers:executing-plans` /
  `subagent-orchestration` flow was used; sor-gate review between tasks.
- `mark_document_indexed` hash param, swap-tx `embed_key` writes, verify-scratch
  bootstrap are **Task 2+ / step-2** scope — do NOT touch in Task 2.
- Spec rev 34 r28-m3: pre-V28/RO brains need a `documents` column probe ⇒ coverage 0
  ⇒ v1 fallback (relevant from Task 5 onward, both CLI + Tauri sides).
- Survey findings with verified file:line refs lived in an agent scratch dir
  (agent cache scratch, `ct-271-impl 2026-10-15/survey-271/findings.md`;
  pr283-review finding 3: no absolute home path in the repo — scratch prunes
  after 24h idle and may be gone; re-verify refs if needed).
