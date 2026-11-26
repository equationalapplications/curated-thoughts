# Plan: #381 — Run 2 confirmations hook on every rooted `start_pipeline`

Status: Draft (T3 review in progress)
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
  keep `start_pipeline(db_path.clone(), Some(canon_root.clone()))`. The
  `canon_root` binding stays used (it feeds `start_pipeline`), so no
  unused-variable fallout.
- Self-review gate: `cargo --manifest-path src-tauri/Cargo.toml clippy
  --all-targets` shows no NEW findings vs base.

### Task 2 — tests

Commit: `test(pipeline): pin the #381 hook contract — presence, stamping, negatives`

All in the `mod.rs` test module next to the drain harness
(`run_drain_waves`, line ~1838), reusing the harness patterns:

1. `rooted_start_pipeline_installs_confirmations_hook` — rooted handle:
   slot is `Some`.
2. `unrooted_start_pipeline_keeps_no_hook` — `vault_root=None`: slot is
   `None`.
3. `drain_stamps_confirmation_on_rooted_handle` — seed
   `stated/.confirmations.jsonl` with a confirmation for an ingested,
   unstamped fact; one drain crossing stamps the row `user_confirmed`.
4. `drain_on_vault_without_ledger_is_quiet_noop` — rooted handle, no
   ledger: crossing applies zero rows, returns success (owner check from
   the approval; also assert the hook result is `Ok` where the harness
   exposes it).
5. Follow existing fixture conventions (tempdir vaults; see
   `docs/test-fixture-conventions` / repo testdata conventions).

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
