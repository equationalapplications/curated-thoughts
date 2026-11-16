# LEDGER — specs/2026-11-14-areas

SDD ledger for the `areas` implementation (PR #352, branch `areas`). Two disciplines: (1) every dispatched task gets a row (dispatch → result → SHA → adjudication); (2) working notes accumulate cross-task findings, re-checked before every dispatch. Transient scaffolding — deleted in the final cleanup commit before the ready flip (pr-spec-driven step 11).

## Task log

| Task | Status | SHA | Notes |
|---|---|---|---|
| Task 1 | DONE (review APPROVE WITH NITS 0/0/3m, minors deferred→Task 10 sweep) | c781601d | V30 migration + INSERT area bind; schema_guard forward-port ruled in (see plan Task 7) |
| Task 2 | DONE (review REQUEST CHANGES → fix round 1 clean: clippy exit 0; minors → Task 10 sweep) | 26e0bdb5 + 8392d6b8 | WisdomConfig + duplicate-dir drop + relative-path refusal (controller ruling) |
| Task 3 | DONE (review REQUEST CHANGES → fix round 1 checklist-verified: 25/25, clippy 0) | bb0fa604 + 647d0c2f | hub resolver; resolve_scoped amended to ScopedResolution three-way (controller ruling, review MAJOR-2) |
| Task 4 | DONE (review Approve w/ 1M → ruled pre-V30 degrade fix verified; minors folded) | 561405fa + 92605597 | band_predicate_where + WisdomItem band/area; pre-V30 SCOPED degrades never errors (controller ruling) |
| Task 5 | DONE (review Approve w/ 1M → ruled load-side diagnostics printed at CLI, fix round 1 checklist-verified: 26+9/0, clippy 0) | 1c1ed34c + 9df97d47 | SCOPED CLI wiring; minors m1–m4 folded; m5 reason-string tokens → Task 10 sweep |
| Task 6 | DONE (review REQUEST CHANGES 4M/6m → all 4 MAJORs ruled+fixed, fix round 1 checklist-verified: 1610/0 lib, 2/0 E2E, clippy 0) | add3c87b + d3031de3 + 08666f2c | write path: hub read, force-include, per-hub clause, LlmFact.area, drops; m2 drops-counter → Task 10 sweep |
| Task 7 | DONE (review REQUEST CHANGES 2M/4m → both MAJORs ruled+fixed, fix round 1 checklist-verified: 1617/0 lib, 4/0 outbox, clippy 0) | 792218d5 + 8d95180b + d913dbc0 | outbox payload area + six call sites + §8.4 summary (post-commit, warnings-routed); schema_guard untouched |
| Task 8 | DONE (review APPROVE WITH NITS 0M/5m → minors folded round 1, checklist-verified; controller-run gates on final head: 12/0 carrier, 1618/0 lib, clippy 0) | 712eee6f + 20f9eb75 + 91c3fa8b | repo_path carrier, hub: render, reply contract, status re-read; M1 survivor-id render kept (spec §6.8.3 errata note); status now truly read-only |

## Working notes

- CT is not a cargo workspace: `--manifest-path` on every cargo command (never `-p`).
- UNSCOPED byte-identity pins are the regression tripwire: message-mode `tool_dispatch.rs:2195` + four activity pins `tools/tests/ct_wisdom_match.rs:359/:378/:409/:427` must stay green and unchanged every task.
- `WisdomItem` PartialEq fixture fallout is budgeted in Task 4 (mechanical `None, None` additions to struct literals).
- Clippy gate = no NEW findings (base-vs-branch), not `-D warnings` absolute.
- Spec is at rev 11, Status Reviewed (approved 2026-11-15, msg `1571571325716271137`) after the converged T4 ladder (7 gates) + IOC series (3 consults). Verdicts: vault `records/investigations/2026-11-14-ct345-areas-write-path/`.
- PLAN rev 8 `561ac842` APPROVED by Kurt 2026-11-15 (Discord msg `1571709511411171430`, option 1) — tier-3 dual-rail cycle converged (GLM 0/0/0 + Fable AWN 0 blocker/0 major); implementation authorized; fold-authored decisions stand unchallenged. Task 9 + Tasks 10-11 remain gated on the two open owner rulings (rename selector, audit-ledger path).
- Measurement durable home: LEDGER + PR body (plan/LEDGER are transient; spec §19.3 intent preserved).
- Task 2 test-name note (review m7): plan Step-2 filter `wisdom_config` doesn't match `canonicalize_lenient_existing_ancestor_plus_remainder` — run BOTH filters (`... wisdom_config` and `... canonicalize_lenient`) or use the combined prefix in the Task 10 sweep.
