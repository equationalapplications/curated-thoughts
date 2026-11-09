# Deposit Kick Honest Failure (#298) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A deposit whose librarian synthesis fails (LLM transport or output
validation) records `deposit_kick_state='failed'` with the error text instead
of `ingested`.

**Architecture:** New typed error `SynthesisFailure` returned by the two
fail-safe arms in `run_synthesis_with_completer`; `run_kick_with`'s existing
`Err` arm does the ledger write; the pipeline worker downcasts to avoid
double-journaling into `errors.log`.

**Tech Stack:** Rust, rusqlite, anyhow (manual `Display`/`Error` impls, same
pattern as `pipeline::IngestRefused` and `db::commit::SourceVanished` — no
thiserror).

**Spec:** `specs/2026-11-09-deposit-kick-honest-failure/spec.md`

## Global Constraints

- Before any cargo command: `export CARGO_TARGET_DIR=<repo-root>/.cargo-shared-target`
  (ABSOLUTE path); `df -h` on that filesystem must show ≥30G free before full
  suites. Linker `Bus error`/`os error 122` = disk full → STOP, never delete
  caches.
- Tests run with `--features test-utils` where the suite needs it (CI's real
  gates use `--features test-utils[,mcp-server]`).
- No new `deposit_kick_state` state values (CHECK constraint vocabulary is
  fixed); `failed` + `error` column is the target state.
- Do not touch the live brain; tests use `tempfile` + `with_brain` scopes.
- Never put the `[skip ci]` marker literal in commit messages or PR titles.

## Review Focus

- **Embedder-stub regime:** kick tests run with the test embedder stub; if a
  new test sees chunks fail to stage for an unrelated reason it must fail
  loudly, not look like the fix — each kick test asserts the ingest leg
  succeeded (`documents.status='indexed'`, 1 chunk) BEFORE asserting the
  ledger state.
- **Arm-1 vs arm-2 classification:** garbage text must land in arm 1
  (`call_llm_with_retry` → JSON failure); type-mismatched-but-parseable JSON
  must land in arm 2 (`parse_and_validate` → validation failure). Tests assert
  the message prefix, which pins which arm fired.
- **#299 regressions:** the vanished-trigger skip must stay `Ok(())` —
  existing arbiter tests (`b224d939`, Tier-1.5 wave) must stay green; do not
  convert those paths.
- **errors.log exactly-once:** the pipeline lane's downcast guard must keep
  eprintln (console) unconditional and guard ONLY `write_error_log`.
- **`ct librarian run` lane:** confirm per-document `Err` handling prints and
  continues (grep `generate_summary` in `tools/src/cmds.rs`); expected: no
  change needed.

---

### Task 1: `SynthesisFailure` type + honest kick ledger on validation failure

**Files:**
- Modify: `src-tauri/src/librarian/synthesis.rs` (arms at ~1306-1327; add type
  near `LlmCompleter`)
- Modify: `src-tauri/src/librarian/mod.rs` (re-export)
- Test: `src-tauri/src/wisdom_deposit.rs` (`mod tests`, near the existing kick
  tests ~2274)

**Interfaces:**
- Produces: `pub struct SynthesisFailure(pub String)` with `Display`
  forwarding to the string and `impl std::error::Error`; re-exported as
  `crate::librarian::SynthesisFailure`. The string is the exact errors.log
  line body (`synthesis JSON failure for {path}: …` / `synthesis validation
  failure for {path}: …`), per spec D3.
- Consumes: the existing kick test harness in `wisdom_deposit.rs` (`wisdom_ctx`,
  `with_brain`, `run_kick_with` with `Some(&completer)`).

- [ ] **Step 1: Write the failing test** in `wisdom_deposit.rs` tests — mirror
  the seeding of `kick_ingest_stages_documents_path_in_canonical_form`
  (tempdir vault, `wisdom_ctx`, deposit via
  `dispatch_wisdom_deposit_awaiting_kick`, then `run_kick_with` in
  `spawn_blocking` with `Some(&completer)`). Name:
  `kick_records_failed_when_synthesis_validation_fails`. The completer returns
  `Ok(r#"{"proposals": {"oops": true}}"#.into())` — parseable JSON, wrong
  schema, so `parse_and_validate` fails (arm 2). Assert BEFORE the ledger
  check: `documents.status='indexed'` for the canonical doc key and chunk
  count 1; then assert `deposit_kick_state` for the rel path has
  `state='failed'` and `error LIKE 'synthesis validation failure%'`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p curated-thoughts --lib kick_records_failed_when_synthesis_validation_fails`
Expected: FAIL — state is `ingested` (the bug).

- [ ] **Step 3: Implement** — add `SynthesisFailure` (manual `Display` +
  `Error` impls, `SourceVanished` pattern); in `run_synthesis_with_completer`
  replace both `return Ok(())` fail-safe arms (LLM-call arm and
  `parse_and_validate` arm) with `return Err(anyhow::Error::new(SynthesisFailure(msg)))`
  keeping the `write_synthesis_error(vault_root, &msg)` call. Re-export from
  `librarian/mod.rs`.

- [ ] **Step 4: Run test to verify it passes** (same command). Expected: PASS.

- [ ] **Step 5: Commit** — `fix(librarian): synthesis failures carry typed SynthesisFailure (#298)`

### Task 2: transport-failure class records failed

**Files:**
- Test: `src-tauri/src/wisdom_deposit.rs` (same harness)

**Interfaces:**
- Consumes: `SynthesisFailure` from Task 1 (no new production code expected).

- [ ] **Step 1: Write the failing test**
  `kick_records_failed_when_synthesis_transport_fails` — completer returns
  `Err(anyhow!("connection refused"))`. Assertions: ingest leg staged, then
  `state='failed'`, `error LIKE 'synthesis JSON failure%connection refused%'`.

- [ ] **Step 2: Run — expect PASS already** (the transport arm shares Task 1's
  conversion). If it FAILS with `ingested`, the arms were not both converted —
  fix before proceeding.

- [ ] **Step 3: Commit** — `test(librarian): transport-failure kick records failed (#298)`

### Task 3: pipeline lane does not double-journal

**Files:**
- Modify: `src-tauri/src/pipeline/mod.rs:244-256` (`run_inner` librarian leg)
- Test: `src-tauri/src/pipeline/mod.rs` (`mod tests`)

**Interfaces:**
- Consumes: `crate::librarian::SynthesisFailure` (Task 1).
- Produces: `fn librarian_error_needs_journal(e: &anyhow::Error) -> bool` —
  `false` iff `e.downcast_ref::<SynthesisFailure>().is_some()`.

- [ ] **Step 1: Write the failing test** `librarian_error_needs_journal_skips_synthesis_failure`:
  `assert!(librarian_error_needs_journal(&anyhow!("boom")))` and
  `assert!(!librarian_error_needs_journal(&anyhow::Error::new(SynthesisFailure("synthesis validation failure for x".into()))))`.

- [ ] **Step 2: Run to verify it fails** (`function not found`).

- [ ] **Step 3: Implement** the helper + guard `write_error_log` with it in
  `run_inner`'s librarian `Err` branch; keep the `eprintln!` unconditional.

- [ ] **Step 4: Run to verify it passes.**

- [ ] **Step 5: Commit** — `fix(pipeline): SynthesisFailure skips errors.log re-journal (#298)`

### Task 4: D4 pins — zero-facts stays ingested; verify ct lane

**Files:**
- Test: `src-tauri/src/wisdom_deposit.rs` (if no existing test covers it)
- Check only: `tools/src/cmds.rs`

- [ ] **Step 1: Search** `wisdom_deposit.rs` tests and `librarian_evidence_e2e.rs`
  for a zero-proposals kick test (`"proposals": []` completer). If one exists,
  skip to Step 3; otherwise write `kick_zero_proposals_stays_ingested_and_watermarks`:
  completer returns `Ok(r#"{"proposals": []}"#.into())`; assert
  `state='ingested'`, `error IS NULL`, and the documents row's `synth_hash`
  is NOT NULL.

- [ ] **Step 2: Run it** — expected PASS already (pin, not fix).

- [ ] **Step 3: Verify the ct lane** — grep `generate_summary` in
  `tools/src/cmds.rs`; confirm the per-document `Err` path prints and
  continues. Record the finding in the LEDGER; change nothing unless it
  aborts the run.

- [ ] **Step 4: Commit (only if Step 1 added a test)** — `test(librarian): pin zero-proposals kick as ingested + watermarked (#298)`

### Task 5: Full gates + ledger

- [ ] **Step 1:** `cargo test -p curated-thoughts --lib --features test-utils`
  — all green. Then the named integration suites:
  `cargo test -p curated-thoughts --test librarian_evidence_e2e --test pipeline --test folder_rules --features test-utils`.
- [ ] **Step 2:** `cargo clippy -p curated-thoughts --lib --features test-utils -- -D warnings`
  and `cargo fmt --check` (clippy is NOT CI-gated but is a repo standard).
- [ ] **Step 3:** Update `specs/2026-11-09-deposit-kick-honest-failure/LEDGER.md`
  (rulings, ct-lane finding, any deviations). Spec status → `Implemented`.
- [ ] **Step 4:** Commit — `docs(spec): deposit-kick-honest-failure ledger + implemented status`
