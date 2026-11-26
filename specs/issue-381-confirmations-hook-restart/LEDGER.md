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

- Wave 1 leg 1 (sonnet/low, session d417c937): wrote 4 Task-2 tests,
  stopped honestly at cargo-not-on-PATH — CAUSE: controller runner PATH
  export omitted ~/.cargo/bin and blocked the AGENTS.md-mandated
  CARGO_TARGET_DIR. Runner fixed (PATH + CARGO_TARGET_DIR=$REPO/.cargo-shared-target);
  same-session resume completed the wave. Spend: $0.40 (leg 1, 25 turns)
  + $0.83 (resume, 48 turns).
- Acceptance rulings: RED = 3 FAILED (absence) + 1 honest pass — the
  unrooted slot-None negative cannot fail on absence; deviation recorded
  in the RED log header. ACCEPTED.
- Deviation rulings: `cargo test --manifest-path` order (equivalent,
  this cargo rejects the brief's verbatim order — brief wording was
  wrong, not the child); clippy needs --features test-utils for
  --all-targets (wiki_maintenance test); commit split via restore-from-HEAD
  (git apply --cached was grant-denied) — final file states verified.
- Task 3 gates (controller-run, at d9de927f): lib suite 1747 passed /
  0 failed (baseline memory said 1744; measured count this session is
  authoritative); first run failed 1 env test (ddl_compat needs
  node_modules/@equationalapplications/core-llm-wiki) — fixed with
  CI=true pnpm install --frozen-lockfile (engine 9.18.0 restored, not a
  code issue); clippy src-tauri --all-targets --features test-utils = 0
  findings; clippy tools --all-targets = clean.

## Working notes

- Baseline suite count to be MEASURED at Task 3, not remembered (#379
  record says 1744; report actual).
- Cargo commands carry `--manifest-path`; the repo is not a workspace.
