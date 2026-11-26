# Spec: #381 — install the Run 2 confirmations hook on every rooted `start_pipeline`

Status: Implemented (PR #384; T4 Fable APPROVE at r2 delta)
Issue: #381. Found by T4 stage-close review of #379 (F1, MINOR).

## Problem

A vault switch installs the Run 2 confirmations drain hook on the
switch-path pipeline handle only. A restart or a recovery after a failed
switch builds a new pipeline handle. The new handle carries no hook.
Confirmations that arrive during the post-restart ingest are never
re-stamped from `stated/.confirmations.jsonl`. The loss persists until the
next switch or a heal.

## Root cause (verified in tree at `6f6d311e`)

- The worker fires the hook on every 1→0 drain crossing
  (`src-tauri/src/pipeline/mod.rs:218`–`247`). The hook slot defaults to
  `None` (`mod.rs:122`, `mod.rs:142`).
- The switch path installs the hook after `start_pipeline`
  (`src-tauri/src/lib.rs:2383`).
- The startup path calls `start_pipeline` and installs nothing
  (`src-tauri/src/lib.rs:4445`).
- The recovery path starts a replacement handle and installs nothing
  (`src-tauri/src/lib.rs:2093`).
- The open-time rederive is stamp-gated to once per brain
  (`src-tauri/src/db/connection.rs:910`–`932`), so a reopen never repairs
  the loss. #379 closed this loss class for the switch flow only.

## Proposed change

- `start_pipeline` must install the hook itself when `vault_root` is
  `Some`. The hook closure calls
  `db::stated_ledger::apply_stored_confirmations(conn, &vault_root)`.
- Delete the switch-path manual install (`lib.rs:2381`–`2385`). The
  switch already passes the same canonical root to `start_pipeline`, so
  behavior is identical.
- Unrooted pipelines keep no hook: no root, no ledger to read.
- No config change. No schema change. No new state.

## Behavior notes

- The leg is idempotent: `apply_stored_confirmations` dedups by hash
  (`src-tauri/src/db/stated_ledger.rs:452`). Extra firings are no-ops.
- Fire frequency does not change. A hooked worker already fires on every
  crossing today (switch path). More handles now carry the hook.
- Hook errors stay non-fatal: logged to stderr, next crossing retries.
  This matches the current worker contract.
- At startup the open-time rederive (Run 1) and the hook (Run 2) overlap
  harmlessly. Run 1 rebuilds the table legs once per brain; Run 2
  re-stamps confirmations only. They touch disjoint legs.

## Files touched

- `src-tauri/src/pipeline/mod.rs` — hook install inside `start_pipeline`;
  new tests near the drain harness (`run_drain_waves`, `mod.rs:1838`).
- `src-tauri/src/lib.rs` — delete the switch-path install block only.
- `src-tauri/src/db/stated_ledger.rs` — `apply_stored_confirmations`
  returns `Ok(0)` early on an empty ledger (skips the transaction and the
  wiki-wide hash scan; T4 m1) + a unit pin.

## Test plan

1. Slot test: a rooted `start_pipeline` handle exposes an installed hook;
   an unrooted one keeps `None`. Asserted in the `mod.rs` test module.
2. Two layers (the #379 harness builds a worker directly with no vault
   root, so it cannot exercise the install):
   - Direct-call unit pins: seed `stated/.confirmations.jsonl` + an
     unstamped fact, call the handle's installed hook on a connection,
     assert `user_confirmed`; a vault with no ledger is a quiet no-op.
   - End-to-end drain test (`drain_fires_installed_hook_end_to_end`): on a
     real rooted `start_pipeline` handle, send one counted ingest job and
     assert the WORKER's drain crossing stamps the row (bounded poll). This
     fails if the worker is not wired to the handle's confirmations slot.
3. Negative test: an unrooted handle keeps no hook, so no crossing can
   stamp anything.
4. Gates: `cargo test --manifest-path src-tauri/Cargo.toml --lib
   --features test-utils` (this cargo rejects `--manifest-path` before
   the subcommand) — final count 1749 with the lane's six tests; pre-lane
   base not separately measured — the remembered 1744 disagreed with the
   ledger by one; clippy on
   both crates, no new findings; no frontend or TS surface changes.

## Rejected alternatives

- Persist a "Run 2 owed" marker in `llm_wiki_meta`, read at startup:
  adds state plus a reader for a problem a stateless idempotent hook
  already solves. Rejected.
- Install the hook at the two missing call sites only: the next
  `start_pipeline` call site can forget again. Central install removes
  the failure mode. Rejected.

## Owner decisions

- D1 — install point. Recommended: central install inside
  `start_pipeline`, rooted only (this spec). Alternative: call-site
  installs at `lib.rs:2093` and `lib.rs:4445` only.
