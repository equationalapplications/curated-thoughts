# LEDGER — #381 confirmations-hook-restart

Lane: PR #384, branch `fix/381-confirmations-hook-restart`, base `6f6d311e`.
Controller: Tessera (Hermes HQ thread), 2026-11-25 session.

## Gates and rulings of record

- Spec APPROVED by Kurt 2026-11-25 (ready-to-paste approval in-thread):
  D1 = central install inside `start_pipeline`, rooted only; switch-path
  manual install deleted.
- Owner implementation check resolved in code: missing
  `stated/.confirmations.jsonl` → `read_lines` `Ok(Vec::new())`
  (`src-tauri/src/db/stated_ledger.rs:128`) → quiet `Ok(0)` no-op.
  Pinned by Task 2 test 4.
- Verify-first pass: five code anchors confirmed at `6f6d311e` (see spec
  §Root cause). No implemented-already surprise.

## Dispatches

(appended per task)

## Working notes

- Baseline suite count to be MEASURED at Task 3, not remembered (#379
  record says 1744; report actual).
- Cargo commands carry `--manifest-path`; the repo is not a workspace.
