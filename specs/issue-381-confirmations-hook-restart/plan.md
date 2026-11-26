# Plan: #381 — Run 2 confirmations hook on every rooted `start_pipeline`

Status: Approved (owner 2026-11-25) — implementation in progress
Spec: `specs/issue-381-confirmations-hook-restart/spec.md` — APPROVED by
Kurt 2026-11-25 (D1: central install, rooted only). Owner check folded:
`read_lines` returns `Ok(Vec::new())` on a missing ledger
(`src-tauri/src/db/stated_ledger.rs:128`), so the hook is a quiet `Ok(0)`
no-op in vaults without a ledger — pinned by Task 2's fourth test.

## Baseline

- Branch base: `6f6d311e` (post-#383 main). Local suite baseline: src-tauri
  lib 1744 tests (per the #379 lane record); report the real count at Task
  3 gates, not the remembered one.
- Both crates are standalone: every cargo command carries `--manifest-path`.
  No `-p` form anywhere in this plan.

## Tasks

### Task 1 — central install + switch-site removal

Commit: `fix(pipeline): install the Run 2 confirmations hook inside start_pipeline (#381)`

- In `src-tauri/src/pipeline/mod.rs` `start_pipeline` (line ~1430): when
  `vault_root` is `Some(root)`, build the hook closure
  `|conn| db::stated_ledger::apply_stored_confirmations(conn, &root).map(|_| ())`
  and write it into the `stated_confirmations` slot before the handle is
  returned. Root is canonicalized by every current caller; `start_pipeline`
  must not re-canonicalize (keeps one canonicalization per path, matches
  the switch site's existing contract).
- Reuse the existing `ConfirmationsHook` type and the pub
  `install_stated_confirmations` method (call it on a locally built
  `PipelineHandle` after construction, or write the slot directly before
  construction — implementer's choice; both are equivalent because the
  worker holds a clone of the slot `Arc`). No signature changes.
- In `src-tauri/src/lib.rs` (~2381–2385): delete the manual install block;
  keep `start_pipeline(db_path.clone(), Some(canon_root))`, dropping the
  now-redundant `.clone()`. Move the deleted block's
  `// Run 2 (spec §2 S5): confirmations that arrive after the …` comment
  to the new install site inside `start_pipeline` so the rationale
  survives (r1-m3).
- Update the `install_stated_confirmations` doc comment
  (`mod.rs:1386–1387`): "installed once per switch epoch" stops being
  true — it is now installed by every rooted `start_pipeline` (r1-m3).
- Self-review gate: `cargo --manifest-path src-tauri/Cargo.toml clippy
  --all-targets` shows no NEW findings vs base.

### Task 2 — tests

Commit: `test(pipeline): pin the #381 hook contract — presence, stamping, negatives`

All in the `mod.rs` test module next to the drain harness. Fixture shape
follows the harness itself (`run_drain_waves`, `mod.rs:1839–1849`):
`tempfile::TempDir` brain + work dirs, `temp_env::with_vars` with
`CURATED_EMBED_STUB=constant8` (r1-m2 — there is no fixture-conventions
doc to cite).

**Design rule (r1-M1):** the harness (`run_drain_waves`) builds a
`PipelineWorker` directly with `vault_root = None` and installs its own
counting hook (`mod.rs:1858–1875`) — it exercises neither
`start_pipeline`'s new install code nor the production closure. Tests
that pin the #381 contract therefore must NOT drive a drain through the
harness: they build a real rooted handle with `start_pipeline`, then lock
`handle.stated_confirmations` and call the installed hook directly on a
connection to the same database, asserting on DB state. Firing cadence
is already pinned by `drain_hook_fires_on_every_drain_crossing`
(`mod.rs:1914`) and does not need re-pinning here. Cleanup in every test
that creates a handle: drop `handle.tx` and join `handle.join` (both
pub, `mod.rs:1367–1368`) so no worker thread outlives the temp database.

1. `rooted_start_pipeline_installs_confirmations_hook` — rooted handle:
   slot is `Some`.
2. `unrooted_start_pipeline_keeps_no_hook` — `vault_root=None`: slot is
   `None`. This covers spec Test 3's intent: with no hook installed, no
   crossing can stamp anything — state the mapping in a comment (r1-m1).
3. `installed_hook_stamps_confirmation` — seed
   `stated/.confirmations.jsonl` with a confirmation for an ingested,
   unstamped fact; call the installed hook directly on a connection to
   the same DB; assert `Ok` and the row is now `user_confirmed`.
4. `hook_on_vault_without_ledger_is_quiet_noop` — rooted handle, no
   ledger: call the installed hook on a connection; assert `Ok` and
   unchanged row state (owner check from the approval;
   `read_lines` returns `Ok(Vec::new())`, `stated_ledger.rs:128`).
6. Implementer note (r2 APPROVE): tests 3 and 4 must create the schema
   with `open_app_db` before seeding, as the harness does at
   `mod.rs:1852` — the seeded fact row needs tables to land in.

### Task 3 — gates

- `cargo --manifest-path src-tauri/Cargo.toml test --lib --features
  test-utils` — full suite green; report the count delta vs the measured
  baseline.
- `cargo --manifest-path src-tauri/Cargo.toml clippy --all-targets` and
  `cargo --manifest-path tools/Cargo.toml clippy --all-targets` — no new
  findings.
- No frontend/TS surface: no vitest/tsc run required (spec §Test plan).

## Risks

- Hook install ordering: the worker holds a clone of the slot `Arc`, so
  install timing (before/after spawn) cannot lose the hook. Low.
- Extra ledger reads on every crossing for all rooted pipelines: the
  switch path already pays this; the leg dedups by hash and the missing
  ledger is a no-op. Accepted (spec §Behavior notes).
- Double canonicalization drift: `start_pipeline` re-rooting the path
  would diverge from hash keys minted elsewhere. Mitigated by the
  no-re-canonicalize rule in Task 1.
- Pre-existing clippy debt: gate is "no NEW findings", not `-D warnings`.

## Out of scope

- #380 (post-commit old-root writers window) and #382 (moved-restore
  attestation keys) — separate lanes.
- Any change to fire frequency or the worker's crossing contract.
