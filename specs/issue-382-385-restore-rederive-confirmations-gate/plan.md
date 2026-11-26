# Plan: #382+#385 — restore rederive + confirmations dual gate + hygiene

**Spec (authority):** `specs/issue-382-385-restore-rederive-confirmations-gate/spec.md` — spec wins on conflict. Owner rulings Q1=1 (§2-A as written), Q2=2 (dual gate §2-B) are FINAL.
**Base:** `211c1e79` (origin/main). **Branch:** `fix/382-385-restore-rederive-confirmations-gate` (checked out in the PRIMARY clone for this lane — switch primary back to `main` at closeout).
**Working rules for implementers:** foreground-only commands; no pushes (controller owns pushes); RED-first — capture the failing test output BEFORE the fix (test-code defects get fixed before the captured run; the RED must fail for implementation-absence only); report written incrementally to the pinned path BEFORE the final message; STOP on silence — if the spec is silent on anything material, STOP and write the report as the deliverable; never invent a mechanism.
**Line-cite warning:** every `file:line` in the spec/plan is an anchor from `211c1e79`; landed tasks shift them — grep the symbol names, not the lines.

## Task 1 — §2-A: restore rederives unconditionally, unstamp on failure (#382)

1. RED: extend `src-tauri/src/db/switch_pin_tests.rs` (unit tests in the lib target): a moved-root restore pin — brain at root A with ≥1 stamped attestation; snapshot backup (`lib.rs:1030-1056` capture path); restore at root B; assert `stated_attestations` keys carry B-rooted paths and the rederive ran. Must FAIL on unfixed code (today's restore leaves A-rooted keys). Also pin the failure story: inject rederive failure → stamp row absent → reopen rederives via the open-time gate (`connection.rs:905-960`).
2. GREEN: in `switch_vault`'s restore arm (after `commit_staged_backup` publishes and the config is aligned — anchor: the `set_vault_path`-before-reopen block), call `rebuild_rederive(conn, canonical_new_root)` (`stated_ledger.rs:356`; canonicalizes its own arg; Run-1 call precedent `lib.rs:2346`). On Err: delete the `stated_ledger_rederived` stamp (unstamp seam precedent `switch_pin_tests.rs:78-84`, repair-flow precedent `connection.rs:4529`) and log to `errors.log` like Run 1 does (`lib.rs:2356-2363`) — do NOT declare permanent failure; the open-time gate retries.
3. Gate: `cargo test --manifest-path src-tauri/Cargo.toml --lib db::switch_pin_tests` green + no NEW failures in `--lib db::`.
4. Commit (byte-exact): `fix(switch): rederive stated-ledger keys after restore, unstamp on failure (spec §2-A, #382)`

## Task 2 — §2-B: dual gate (stat memo + entries-written dirty flag) (#385 m1)

1. Mechanism (verified at base): `PipelineJob` has ONE variant `Ingest { path, force, count_pending }` (`pipeline/mod.rs:32-70`) — every Ingest job writes entries; there are no pure-read jobs. The confirmations slot is `ConfirmationsSlot = Arc<Mutex<Option<ConfirmationsHook>>>` (`mod.rs:1426-1453`); `respawn_worker` reuses it. Hook installed for every rooted start at `mod.rs:1456-1471`, closure captures `root` (`:1469`); hook fires on drain-crossing timeouts (`mod.rs:211-261`, `probe_next` at `:414`).
2. RED: unit tests in `pipeline/mod.rs` `#[cfg(test)]`: (a) second hook fire with unchanged ledger AND no completed Ingest since the last apply → scan skipped (assert via a counting seam — e.g. count `apply_stored_confirmations` invocations or wrap the scan leg); (b) ledger unchanged but an Ingest job completed since last apply → scan runs (the owner's multi-wave case); (c) ledger stat changed → scan runs; (d) failed apply → stays dirty → next fire retries.
3. GREEN: introduce a shared gate state (e.g. `ConfirmationsGate { memo: Mutex<Option<(u64, u64)>>, dirty: AtomicBool }`) created in `start_pipeline`, shared between the worker (sets `dirty` when an Ingest job completes — the completion path around `mod.rs:400-414` is the anchor) and the installed hook (checks memo+dirty, calls `apply_stored_confirmations`, updates memo, clears dirty ONLY on success). First fire (no memo) always applies. Keep the empty-ledger early return (`stated_ledger.rs:457-461`) untouched.
4. STOP-flag: if the dirty-set site is ambiguous (a completion path that may or may not write entries), STOP and report — do not guess.
5. Gate: `cargo test --manifest-path src-tauri/Cargo.toml --lib pipeline::` green + Task 1's tests still green.
6. Commit (byte-exact): `perf(confirmations): skip idle-crossing scan unless ledger changed or entries written (spec §2-B, #385)`

## Task 3 — §2-C: hygiene (#385 m4)

1. Extract `seed_confirmation_ledger(root, title, body)` (or the exact shape the duplicated blocks imply — `pipeline/mod.rs:2045-2056` and `:2076-2087` at base) and use it at both sites.
2. Pins through `apply_stored_confirmations`: zero-byte ledger file → `Ok(0)`-class result, no panic, no scan; blank-lines-only ledger → same (both take `read_lines` `:127-146`).
3. Narrow `install_stated_confirmations` (`pipeline/mod.rs:1389` at base) from `pub` to private (`fn`) — sole caller `:1468` is in-module; repo-wide grep shows no external callers (recon-verified; re-grep before narrowing).
4. Gate: `cargo test --manifest-path src-tauri/Cargo.toml --lib pipeline:: db::stated_ledger` green; full lib target compiles (visibility narrowing compiles = proof).
5. Commit (byte-exact): `refactor(confirmations): dedupe ledger-seeding helper, pin empty-ledger cases, narrow install_stated_confirmations (spec §2-C, #385)`

## Task 4 — closeout (controller)

Full unfiltered gates at final head: `cargo test --manifest-path src-tauri/Cargo.toml --lib` (exact counts vs ledger baseline) + `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --features test-utils -- -D warnings`; stage-close review ladder (Tier 1.5 → Opus T3 → Fable T4; spec-ful lane so T4 runs); plan/LEDGER pruning per Single PR delivery; `gh pr ready` flip; switch primary back to `main` post-merge.

## Baselines (controller, before Task 1 dispatch)

Recorded in the SDD ledger (`.superpowers/sdd/issue-382-385/progress.md`, gitignored) from controller runs at base: lib-test exact counts, clippy result. Gate logs live in the SDD dir. Heavy runs go through `heavy-build -- bash <script>` with `CARGO_TARGET_DIR=$PWD/.cargo-shared-target` (AGENTS.md canonical) + `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0`.
