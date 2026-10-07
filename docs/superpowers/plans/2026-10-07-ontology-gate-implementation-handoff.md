# Handoff — finish the ontology gate wave-1 implementation (Task 9 onward)

**Written:** 2026-10-07 (third revision — supersedes the second ("resume at
Task 3 fix round 1") and first versions).
**For:** a controller session that runs the remaining work with subagents via
`superpowers:subagent-driven-development`. You coordinate; subagents write
the code. This file only adds what the skill can't know.

## 1. Where things stand

- Repo `curated-thoughts`, branch `spec/ontology-node-type-gate-and-heal`,
  PR #269. **Spec (binding authority):**
  `docs/superpowers/specs/2026-10-03-ontology-node-type-gate-and-heal-design.md`
  (r21). **Plan:**
  `docs/superpowers/plans/2026-10-03-ontology-node-type-gate-and-heal.md`
  (its Status table may lag — trust this file and the ledger).
- Tasks 0–8 are COMPLETE, reviewed clean, and **pushed**:

  | Task | Commits | Notes |
  |---|---|---|
  | 0 Schema DDL (V26) + r21 addendum | `c327558..65578c4`, `6f5ada2` | |
  | 1 Config core + 1b clippy cleanup | `f9998dc..7da4603`, `95d8c36..dd10873` | |
  | 2 Manifest vocab + NodeVocabulary + ImmediateTx + SourceResolution | `3864595..63127e8` | |
  | 3 Write-time gate at four insert sites | `63127e8..1ec104d` | 1 fix round (full §2.3 ladder + watermark stamp + DRY) |
  | 4 OKF `doc_kind` rename | `1ec104d..ec8822f` | serde-only wire preservation |
  | 5 Ontology heal | `ec8822f..c56806a` | 1 fix round (§6 test rows + unreadable-manifest arm); incl. rung-3 config fix |
  | 6 Duplicate merge sweep | `c56806a..0fdeb9f` | 1 fix round (`redirects_rewritten`) |
  | 7 Read/write redirect resolution | `0fdeb9f..aadefd3` | 1 fix round, 4 findings (outbox loser-key, connections, export edges, archive tx) |
  | 8 CLI + MCP + CI tools step | `aadefd3..8315d5d` | 1 fix round (R10: `ontology_retype_pass`, sweep writes no bookkeeping) |

- **CI:** verified green through tip `aadefd3`. Tip is now `8315d5d` — CI
  was IN PROGRESS at handoff time; **first resume action: verify CI on the
  tip** (`gh pr view 269 --json mergeStateStatus,statusCheckRollup,headRefOid`;
  confirm checks on the tip SHA, mergeStateStatus CLEAN). Known flake
  pattern: pipeline watchdog `seqlock_holds_under_concurrent_transitions`
  (rust-macos) failed once and passed on re-run (R9) — re-run once before
  investigating.
- **Task 9 is DISPATCHED and possibly still running**: implementer (sonnet)
  brief at
  `.superpowers/sdd/2026-10-03-ontology-node-type-gate-and-heal/task-9-brief.md`,
  report target `task-9-report.md`, BASE `8315d5d`. On resume: if
  `task-9-report.md` exists and a commit sits on top of `8315d5d`, handle
  its DONE per the skill; if not, re-dispatch Task 9 from its brief
  (context: Task 2 owns the `&mut Connection` signature work, Task 3 owns
  the abort-vs-skip decision — Task 9 is loud-observability + §3 migration
  data work ONLY; NO issue creation — single tracking issue is Task 10's).
- **Task 10 remains after Task 9**, then the final whole-branch review on
  Opus, then `superpowers:finishing-a-development-branch`. **Do not merge.**
- Before you start: `git log --oneline -5` and `git status`. If git has
  moved past what this file says, trust git and the ledger (§2) over this
  file.

## 2. Ledger

`.superpowers/sdd/2026-10-03-ontology-node-type-gate-and-heal/progress.md`
is current through Task 8's completion and rulings R1–R10. It is
git-ignored — it exists on this machine; recreate from git + this file if
missing. READ IT FIRST: it carries every parked minor (the final review
must triage them), every ruling, and the deferred-minor list per task.
The workspace holds all task briefs (Tasks 2–10 pre-extracted — do not
re-extract), reports, and review packages.

## 3. How to run it

1. Invoke `superpowers:subagent-driven-development` and follow it exactly.
2. Never run two implementers in parallel. Never fix code yourself in the
   controller. Briefs are per-task files; never paste the whole plan into a
   dispatch.
3. After each task: review package → task review → fix rounds (max 5) →
   ledger completion line → **push** → **verify CI yourself**.
4. After Task 10: final whole-branch review on Opus (point it at the
   ledger's deferred-minor lines), ONE fix wave max, then
   `superpowers:finishing-a-development-branch`. **Do not merge the PR.**
   Stop there and report to the user.

## 4. Context every implementer dispatch must carry

- Locate code by SYMBOL; line numbers drift. `rN-*` tags are provenance,
  not requirements. Decisions are CLOSED (spec header) — flag conflicts,
  never re-litigate. Implementers never dispatch subagents.
- **R7:** any text citing a "per-config-path cache at config/mod.rs:182"
  is a FALSE claim (doc-comment tail) — the implementer dispatch and any
  review must ignore that rationale wherever it appears.
- **Task 10:** ONE tracking issue (GitHub write — fine under standing
  authorization, note it in ledger) folding in issue #272's outcome if the
  spec's r18-m4 wants the migration census tracked there; spec Status →
  implemented; **add one §2.11 sentence documenting `edge_types` riding
  along verbatim on `--entity strict`** (Task 8 parked minor 5); PR body
  carries §3 rollout notes; the live-ThinkPad census is a step for the
  USER — never fabricate the numbers.

## 5. Gates, commits, CI

```
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --features test-utils -- -D warnings
cargo clippy --manifest-path tools/Cargo.toml --all-targets -- -D warnings
cargo test  --manifest-path src-tauri/Cargo.toml --features test-utils,mcp-server -- --test-threads=1
cargo fmt   --manifest-path src-tauri/Cargo.toml -- --check
cargo test  --manifest-path tools/Cargo.toml -- --test-threads=1
```

- Clippy is BLOCKING in CI. Full suites take many minutes — long timeouts,
  run once before committing.
- Commit trailer: `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`
  or the model actually used.
- **Push after each completed task** (standing authorization). **Never
  force-push** without asking. **Never merge.** Never squash.
- **Verify CI yourself** — `gh pr view 269 --json mergeStateStatus,statusCheckRollup,headRefOid`;
  confirm checks are on the tip SHA and mergeStateStatus is CLEAN.
  CI failure on a pushed task = that task's next fix round — but FIRST
  check the two known non-branch flakes (§1): watchdog seqlock test
  (rust-macos, passed on re-run once) and codeql Analyze (rust)
  concurrency mutual-cancels (re-run the failed job).

## 6. Model choice (explicit every dispatch)

| Role | Model |
|---|---|
| Implementer Tasks 9, 10 | `sonnet` |
| Task reviewers / scoped re-reviews | `sonnet` |
| Fix rounds 4–5 escalation | `opus` |
| Final whole-branch review | `opus` |

## 7. Stop and ask the user only for

Force-push, deleting branches/data, dropping non-empty tables; merging,
releasing, publishing; a plan defect so deep every way forward is a guess.
Everything else: write a `Ruling:` line and keep going.

## 8. Traps already hit (don't repeat)

- Line drift — cite symbols. CI clippy — run all five gates.
- TempDir guards: rename unused to `_tmp`, never bare `_`.
- Old-schema DBs: read-only `ct wiki sweep`/`ct heal` paths report
  "schema pending (read-only)", never error.
- `drift_walk_identity_with_ingest` is `#[ignore]`d (issue #272) — it is
  a DETERMINISTIC macOS /var vs /private/var fixture bug, NOT a
  live-~/.brain flake (earlier sessions mislabeled it).
- Tools-crate path tests: check env before "fixing" code.
- `entity_type_origin` is first-origin-wins; heal retype of an entity with
  an existing row writes nothing — correct, not a bug.
- Verify CI on the tip SHA; never trust a subagent's "green".
- **429 usage-limit failure mode:** long-running subagent reviews can die
  on API 429 after emitting their report — the report text in the task
  notification is still the findings of record; persist it to a file and
  continue the loop.
- Monitor gotcha: a CI rollup read mid-refresh shows null statuses that
  pass an IN_PROGRESS grep — treat null as pending (bne023s5v incident);
  also confirm the monitor's `headRefOid` filter uses the FULL sha.

## 9. When you finish

Report to the user: commits per task; CI state on the tip SHA; every
`Ruling:` line from the ledger in order (R1–R10 so far) with cost-if-wrong;
deferred minors the final review left open; what needs the user (live
census for the PR body, merge decision, manual smoke test). Then delete
the SDD workspace and mark this handoff obsolete in the plan's Status
table (or delete it — plans are ephemeral).
