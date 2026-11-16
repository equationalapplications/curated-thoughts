# LEDGER — specs/2026-11-14-areas

SDD ledger for the `areas` implementation (PR #352, branch `areas`). Two disciplines: (1) every dispatched task gets a row (dispatch → result → SHA → adjudication); (2) working notes accumulate cross-task findings, re-checked before every dispatch. Transient scaffolding — deleted in the final cleanup commit before the ready flip (pr-spec-driven step 11).

## Task log

| Task | Status | SHA | Notes |
|---|---|---|---|
| Task 1 | dispatched 2026-11-15 (post plan approval) | pending | V30 migration + INSERT area bind |

## Working notes

- CT is not a cargo workspace: `--manifest-path` on every cargo command (never `-p`).
- UNSCOPED byte-identity pins are the regression tripwire: message-mode `tool_dispatch.rs:2195` + four activity pins `tools/tests/ct_wisdom_match.rs:359/:378/:409/:427` must stay green and unchanged every task.
- `WisdomItem` PartialEq fixture fallout is budgeted in Task 4 (mechanical `None, None` additions to struct literals).
- Clippy gate = no NEW findings (base-vs-branch), not `-D warnings` absolute.
- Spec is at rev 11, Status Reviewed (approved 2026-11-15, msg `1571571325716271137`) after the converged T4 ladder (7 gates) + IOC series (3 consults). Verdicts: vault `records/investigations/2026-11-14-ct345-areas-write-path/`.
- PLAN rev 8 `561ac842` APPROVED by Kurt 2026-11-15 (Discord msg `1571709511411171430`, option 1) — tier-3 dual-rail cycle converged (GLM 0/0/0 + Fable AWN 0 blocker/0 major); implementation authorized; fold-authored decisions stand unchallenged. Task 9 + Tasks 10-11 remain gated on the two open owner rulings (rename selector, audit-ledger path).
- Measurement durable home: LEDGER + PR body (plan/LEDGER are transient; spec §19.3 intent preserved).
