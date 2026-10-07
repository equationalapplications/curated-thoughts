# Handoff — finish the ontology gate wave-1 implementation (Task 3 fix loop, then Tasks 4–10)

**Written:** 2026-10-07 (second revision — supersedes the "resume at Task 2"
version; that state is superseded).
**For:** a controller session that runs the remaining work with subagents via
`superpowers:subagent-driven-development`. You coordinate; subagents write
the code. This file only adds what the skill can't know.

## 1. Where things stand

- Repo `curated-thoughts`, branch `spec/ontology-node-type-gate-and-heal`,
  PR #269. **Spec (binding authority):**
  `docs/superpowers/specs/2026-10-03-ontology-node-type-gate-and-heal-design.md`
  (r21). **Plan:**
  `docs/superpowers/plans/2026-10-03-ontology-node-type-gate-and-heal.md`
  (revised 2026-10-07; its Status table may lag Task 2/3 — trust this file
  and the ledger).
- Done and **pushed**, CI verified CLEAN on tip `63127e8` (8/8 checks
  SUCCESS incl. blocking rust-ubuntu clippy):

  | Task | Commits |
  |---|---|
  | 0 Schema DDL (V26) | `c327558..65578c4` |
  | 0 r21 addendum | `6f5ada2` |
  | 1 Config core | `f9998dc..7da4603` |
  | 1b Clippy cleanup | `95d8c36..dd10873` |
  | 2 Manifest vocab + NodeVocabulary + ImmediateTx + SourceResolution | `3864595..63127e8` (review clean after 1 fix round: trailer added, resolver core moved to `entities.rs` per R4, engine-rewrite opt-out test added) |

- **Task 3 is MID-LOOP.** Implementer produced `8c3e83b` (local, **NOT
  pushed**; 1192 tests pass, gates green per its report). The task review
  returned **NEEDS FIXES**:
  - **C1:** `stamp_initial_watermark` built but called from no production
    path (r13-MAJOR-3 requires the stamp at first gate/heal resolution).
  - **I2:** rungs 2/3 of the §2.3 ladder are not walked at any of the four
    production insert sites — only rung 1 + rung 4. **Ruled a real gap
    (R6)** — the fix wires the full ladder through all four sites.
  - **I3:** DRY — four near-identical inline resolvers + two near-identical
    ledger writers; extract shared helpers in `entity_gate.rs`.
  - **I4:** the implementer report's "config cache at config/mod.rs:182"
    justification is false; the fix report must not carry it forward.
  - Minors parked (dead bindings/params) — do NOT fix in round 1.
  - **Full findings with fix guidance:**
    `.superpowers/sdd/2026-10-03-ontology-node-type-gate-and-heal/task-3-review.md`
- **Resume at:** Task 3 fix round 1 — dispatch the fix from
  `task-3-review.md` to the Task 3 implementer's report context (report:
  `task-3-report.md` in the same workspace), then scoped re-review, then
  push `8c3e83b` + fix commit(s) together, verify CI, ledger, and continue
  at Task 4.
- Before you start: `git log --oneline -5` and `git status`. If git has
  moved past what this file says, trust git and the ledger (§2) over this
  file.

## 2. Ledger

`.superpowers/sdd/2026-10-03-ontology-node-type-gate-and-heal/progress.md`
is current through Task 3's review verdict and rulings R1–R6. It is
git-ignored — it exists on this machine; recreate from git + this file if
missing. Read it first. The workspace also holds all task briefs
(`task-N-brief.md`, Tasks 2–10 pre-extracted from the REVISED plan — do not
re-extract), reports, and review packages.

## 3. How to run it

1. Invoke `superpowers:subagent-driven-development` and follow it exactly.
2. Never run two implementers in parallel. Never fix code yourself in the
   controller. Briefs are per-task files; never paste the whole plan into a
   dispatch.
3. After each task: review package → task review → fix rounds (max 5) →
   ledger completion line → **push** → **verify CI yourself** (§5).
4. After Task 10: final whole-branch review on Opus, then
   `superpowers:finishing-a-development-branch`. **Do not merge the PR.**
   Stop there and report to the user.

## 4. Context every implementer dispatch must carry

- Locate code by SYMBOL; line numbers drift (R2/R3). `rN-*` tags are
  provenance, not requirements. Decisions are CLOSED (spec header) — flag
  conflicts, never re-litigate. Implementers never dispatch subagents.
- **Task 3-specific (fix round):** rung 2/3 machinery already exists from
  Task 1 (`IngestConfig::ontology_lookup`, `OntologyDegradedState`,
  `GateResolutionContext`); the fix threads it into the four resolvers.
  `ImmediateTx` holds DB work only — if the watermark stamp can't reach the
  config hash inside `ImmediateTx::begin` without violating the hold-time
  rule, wire it at each `resolve_*_gate` instead and say so.
- **Task 5:** R2.3.5 scope bullet + surfacing-by-reason rule are in the
  plan; `alias_retype` rows share the per-row transaction.
- **Task 6:** archived members never merge candidates; auto-merge needs
  non-empty normalized-equal SUMMARIES; report lists every redirect row.
- **Task 7:** `live_entities` VIEW + source-scan allowlist (seed in plan;
  allowlist must include merge_dedup); `get_entity(loser)` follows the
  redirect even to an archived survivor; merge reversal test.
- **Task 8:** run the whole tools suite locally BEFORE adding the CI tools
  step; quarantine env-dependent tests with `#[ignore]` + issue link;
  ontology writer absent from the MCP catalog.
- **Task 9:** only the loud logging/diagnostic at the
  `let _ = run_okf_migration` site; Task 3 owns the abort decision (done).
- **Task 10:** ONE tracking issue (GitHub write — fine under standing
  authorization, note it in ledger); spec Status → implemented; PR body
  carries §3 rollout notes; the live-ThinkPad census is a step for the USER
  — never fabricate the numbers.

## 5. Gates, commits, CI

```
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --features test-utils -- -D warnings
cargo clippy --manifest-path tools/Cargo.toml --all-targets -- -D warnings
cargo test  --manifest-path src-tauri/Cargo.toml --features test-utils,mcp-server -- --test-threads=1
cargo fmt   --manifest-path src-tauri/Cargo.toml -- --check
```

From Task 5 onward also:
`cargo test --manifest-path tools/Cargo.toml -- --test-threads=1`.

- Clippy is BLOCKING in CI. Full src-tauri suite takes many minutes — long
  timeouts, run once before committing.
- Commit trailer:
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>` (or the model
  actually used).
- **Push after each completed task** (standing authorization). **Never
  force-push** without asking. **Never merge.** Never squash.
- **Verify CI yourself** — `gh pr view 269 --json mergeStateStatus,statusCheckRollup,headRefOid`;
  confirm checks are on the tip SHA and mergeStateStatus is CLEAN
  (CONFLICTING runs zero checks silently). CI failure on a pushed task =
  that task's next fix round.

## 6. Model choice (explicit every dispatch)

| Role | Model |
|---|---|
| Implementer Tasks 3 (fix rounds 1–3 resume original), 5, 6, 7 | `opus` |
| Implementer Tasks 4, 8, 9, 10 | `sonnet` |
| Task reviewers / scoped re-reviews | `sonnet` (`opus` for Tasks 3, 5 reviews) |
| Fix rounds 4–5 escalation | `opus` |
| Final whole-branch review | `opus` |

## 7. Stop and ask the user only for

Force-push, deleting branches/data, dropping non-empty tables; merging,
releasing, publishing; a plan defect so deep every way forward is a guess.
Everything else: write a `Ruling:` line and keep going.

## 8. Traps already hit (don't repeat)

- Line drift — cite symbols. CI clippy — run all four gates.
- TempDir guards: rename unused to `_tmp`, never bare `_`.
- Old-schema DBs: read-only `ct wiki sweep`/`ct heal` paths report
  "schema pending (read-only)", never error.
- Tools-crate path tests flake when live `~/.brain` exists — check env
  before "fixing" code.
- `entity_type_origin` is first-origin-wins; a heal retype of an entity
  with an existing row writes nothing — correct, not a bug.
- Verify CI on the tip SHA; never trust a subagent's "green"
  ([[feedback-verify-ci-not-agent-claims-2026-09-02]]).
- **429 usage-limit failure mode:** long-running subagent reviews can die
  on API 429 after emitting their report — the report text in the task
  notification is still the findings of record; persist it to a file (as
  done for `task-3-review.md`) and continue the loop.

## 9. When you finish

Report to the user: commits per task; CI state on the tip SHA; every
`Ruling:` line from the ledger in order with cost-if-wrong; deferred minors
the final review left open; what needs the user (live census for the PR
body, merge decision, manual smoke test). Then delete the SDD workspace and
mark this handoff obsolete in the plan's Status table (or delete it — plans
are ephemeral).
