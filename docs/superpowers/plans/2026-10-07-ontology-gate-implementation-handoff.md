# Handoff — finish the ontology gate wave-1 implementation (Tasks 2–10)

**Written:** 2026-10-07, by the session that revised spec r21 and the plan.
**For:** a controller session (Sonnet-class is fine) that runs the remaining
tasks with subagents. You coordinate; subagents write the code.

## 1. Where things stand

- Repo `curated-thoughts`, branch `spec/ontology-node-type-gate-and-heal`,
  PR #269. Main was merged in at `db480d6` (v3.3.0).
- **Spec (binding authority):**
  `docs/superpowers/specs/2026-10-03-ontology-node-type-gate-and-heal-design.md`
  (r21). **Plan:** `docs/superpowers/plans/2026-10-03-ontology-node-type-gate-and-heal.md`
  (revised 2026-10-07 to match r21; its "Status" table is current).
- Done and pushed:

  | Task | Commits |
  |---|---|
  | 0 Schema DDL (V26 tables, clear list) | `c327558..65578c4` |
  | 0 r21 addendum (ledger `reason` column, `OriginReason` enum) | `6f5ada2` |
  | 1 Config core | `f9998dc..7da4603` |
  | 1b Clippy cleanup (CI clippy had been red since `7da4603`) | `dd10873` |

- **Resume at Task 2.** Nothing of Task 2 exists yet (an implementer was
  stopped before writing anything; the tree was clean at `dd10873`).
- Before you start, run `git log --oneline -5` and `git status`. If the
  branch has moved past what this file says, trust git and the ledger
  (§3) over this file.

## 2. How to run it

1. Invoke the skill `superpowers:subagent-driven-development` and follow
   it exactly. This file only adds what the skill can't know.
2. Workspace: the skill's `scripts/sdd-workspace <plan>` prints
   `.superpowers/sdd/2026-10-03-ontology-node-type-gate-and-heal/`.
   A ledger (`progress.md`) already exists there on the original machine.
   It names Tasks 0, 0-addendum, 1 and 1b as complete, and rulings R1–R3.
   **It is git-ignored.** On a different machine it won't exist; create
   it from §3 below.
3. Get every task's brief with the skill's `scripts/task-brief <plan> N`,
   from the REVISED plan. Never reuse a brief made before 2026-10-07.
4. Dispatch with an explicit model every time (see §5). Never run two
   implementers in parallel. Never fix code yourself in the controller.
5. After each task: build the review package, run the task review, run fix
   rounds if needed, and append the ledger completion line. Then commit and
   push (§6).
6. After Task 10: run the final whole-branch review on Opus, then
   `superpowers:finishing-a-development-branch`. **Do not merge the PR.**
   Stop there and report to the user.

## 3. Ledger seed (if `progress.md` is missing)

```
# SDD ledger — plan: docs/superpowers/plans/2026-10-03-ontology-node-type-gate-and-heal.md
Task 0: complete (commits c327558..65578c4, pre-ledger; per git log)
Task 1: complete (commits f9998dc..7da4603, pre-ledger; per git log)
Task 0 r21 addendum: complete (commit 6f5ada2)
Task 1b: complete (commits 95d8c36..dd10873, review clean)
Task 1b: minor (deferred): ~18 `// tmp stays alive…` comments in config/mod.rs tests still say `tmp` after the `_tmp` rename
Task 1b: minor (deferred): one untouched `let _ = tmp;` in the config cache test, harmless no-op
- R1 Ruling: inserted Task 1b clippy cleanup before Task 2 — CI clippy (blocking) red since 7da4603 — cost if wrong: one small commit.
- R2 Ruling: plan/spec line numbers in files edited since 9c2281b are approximate; locate by symbol — cost if wrong: none.
- R3 Ruling: connection.rs citations switched to symbols (AppDb::open_with_config, migrate_brain_db) — drifted twice in a day — cost if wrong: none.
```

The preflight conflict scan was already run against the plan, and every
task-pair row was consistent. You don't need to repeat it. Add new rulings
as `Ruling:` lines.

## 4. Context every implementer dispatch must carry

The brief can't know these. Put the ones that apply to the task in the
dispatch. Point at the spec by section; don't paste it.

- **All tasks:**
  - Locate code by SYMBOL. Line numbers are approximate in edited files
    (ruling R2).
  - Inline `rN-*` tags in the spec are provenance, not requirements.
  - Decisions are CLOSED (spec header: Kurt's rulings). The implementer
    flags conflicts; it never re-litigates them.
  - Implementers never dispatch subagents or reviewers.
- **Task 2:**
  - New module `src-tauri/src/db/entity_gate.rs` (wire it in `db/mod.rs`).
    The source-resolution core goes in `db/entities.rs`.
  - The ensure step runs in the `AppDb::open_with_config` path, in the
    order migrate → ensure → `run_okf_migration`. It is best-effort: log,
    never fail the open. Pin the order with a test.
  - The ensure memo is keyed `(entity_id, sha256(manifest_json))`. It is
    process-local and recorded only after the write commits.
  - Added `document`/`process` entries are OBJECTS:
    `{"type": "...", "description": "..."}`. Use short neutral
    descriptions. Edit the raw `manifest_json` Value in place; never
    round-trip it through `WikiManifest`.
  - The 17 EA seed slugs for the subset guard are listed verbatim in spec
    §1.1.
  - `EdgeVocabulary` (the pattern to mirror) lives in `db/commit.rs`.
  - The insert helper returns the admit outcome so that Task 3 can write
    the ledger row in the same transaction.
  - Build the pieces and their tests only. No gate calls at the insert
    sites, and no ledger writes: those belong to Task 3.
- **Task 3:**
  - The plan's ledger-reason table (in Task 3's bullets) is exact.
  - Every ledger write uses `tauri_app_lib::db::schema::OriginReason` with
    `INSERT OR IGNORE`.
  - `original_type` NULL means "no label". Never write `''`.
  - The `ImmediateTx` transaction holds DB work only.
- **Task 5:** the R2.3.5 r21 scope bullet and the surfacing-by-reason rule
  are in the plan. `alias_retype` rows are written in the same per-row
  transaction as the retype.
- **Task 6:**
  - Archived members are never merge candidates.
  - Auto-merge requires non-empty, normalized-equal SUMMARIES.
  - The report lists every redirect row written.
- **Task 7:**
  - `live_entities` VIEW (not per-query `NOT EXISTS`) plus the source-scan
    allowlist test. The allowlist seed is in the plan.
  - `get_entity(loser)` follows the redirect even to an archived survivor.
  - Merge reversal test.
- **Task 8:**
  - Before adding the `tools` test step to CI, run the whole tools suite
    locally. Quarantine environment-dependent tests with `#[ignore]` plus
    an issue link.
  - The ontology writer must be absent from the MCP catalog.
- **Task 10:**
  - File ONE tracking issue. This is a GitHub write, which is fine under
    the standing authorization, but say so in the ledger.
  - Spec Status → implemented.
  - The PR body carries the §3 rollout notes. The live-ThinkPad census in
    the PR body needs the user's real brain: note it as a step for the
    user, and never fabricate the numbers.

## 5. Model choice (be explicit every dispatch)

| Role | Model |
|---|---|
| Implementer, Tasks 2, 3, 5, 6, 7 (design-heavy, multi-file) | `opus` |
| Implementer, Tasks 4, 8, 9 (rename / CLI wiring / logging) | `sonnet` |
| Implementer, Task 10 (docs, status, issue) | `sonnet` |
| Task reviewers and scoped re-reviews | `sonnet` (use `opus` to review Tasks 2, 3, 5) |
| Fix rounds 4–5 (escalation) | `opus` |
| Final whole-branch review | `opus` |

## 6. Gates, commits, CI

Run from the repo root. These are CI-exact. A task is not complete until
all of them pass. Implementers run them; you verify the clippy ones
yourself when a reviewer marks them ⚠️.

```
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --features test-utils -- -D warnings
cargo clippy --manifest-path tools/Cargo.toml --all-targets -- -D warnings
cargo test  --manifest-path src-tauri/Cargo.toml --features test-utils,mcp-server -- --test-threads=1
cargo fmt   --manifest-path src-tauri/Cargo.toml -- --check
```

From Task 5 onward, also run:
`cargo test --manifest-path tools/Cargo.toml -- --test-threads=1`.

- **Clippy is BLOCKING** in CI (job `rust-ubuntu`, step "Clippy"). A local
  clippy newer than CI's can flag extra lints. Fix them anyway; CI's set is
  a subset.
- The full src-tauri suite takes many minutes. Tell implementers to use
  long timeouts and to run it once, before committing.
- Commit trailer: `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`
  (or the model actually used, per your session's attribution reminder).
- **Push** after each completed task. Pushing to this PR branch is under
  the user's standing commit+push authorization. **Never force-push**
  without asking. **Never merge** the PR. Never squash.
- **Verify CI yourself; never trust a subagent's "green".** After pushing,
  check:

  ```
  gh pr view 269 --json mergeStateStatus,statusCheckRollup
  ```

  A CONFLICTING PR runs zero checks silently. Check that the check-runs
  are on the tip SHA. If CI fails on a pushed task, it becomes that task's
  next fix round. Don't move on with red CI.

## 7. Stop and ask the user only for

- Anything destructive or irreversible: force-push, deleting branches or
  data, dropping a non-empty table.
- Merging, releasing, or publishing.
- A plan defect so deep that every way forward is a guess.

Everything else, you decide: write a `Ruling:` line and keep going.

## 8. Traps already hit (don't repeat)

- **Line drift.** `connection.rs` anchors moved twice in one day. Cite and
  locate by symbol.
- **CI clippy.** Task 1 shipped with CI red because only tests were run.
  Run all four gates.
- **TempDir guards.** Rename an unused guard to `_tmp`, never to bare `_`
  (a bare `_` drops it immediately).
- **Old-schema databases.** The `ct wiki sweep` and `ct heal` read-only
  paths can hit an old-schema DB (`no such table: entity_redirects`).
  Task 5's plan text covers this: report "schema pending (read-only)"
  instead of erroring.
- **Shared-brain test flakes.** Tools-crate path tests flake when a live
  `~/.brain` is present. If a tools test fails only on the dev machine,
  check that before "fixing" code.
- **Report-only ledger rows.** `entity_type_origin` is first-origin-wins.
  A heal retype of an entity that already has a row writes nothing. That
  is correct, not a bug.

## 9. When you finish

Report to the user:

1. Commits per task.
2. CI state on the tip SHA.
3. Every `Ruling:` line from the ledger, in order, each with its "cost if
   wrong".
4. The deferred minors the final review left open.
5. What still needs the user: the live census for the PR body, the merge
   decision, and the manual smoke test.

After the final review is clean, delete the SDD workspace directory. Then
mark this handoff file obsolete in the plan's Status table, or delete it
(plans are ephemeral).
