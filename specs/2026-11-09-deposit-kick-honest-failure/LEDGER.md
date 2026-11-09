# Ledger — deposit-kick-honest-failure

Plan: `plan.md` · Spec: `spec.md` · Branch: `fix/deposit-kick-honest-failure` · BASE `d0fc79a3`

## Rulings

- **Ruling (investigation):** Root cause confirmed at code level — the two
  fail-safe arms in `run_synthesis_with_completer` journaled to errors.log and
  returned `Ok(())`; `run_kick_with` maps `Ok` → `deposit_kick_state='ingested'`.
  The repro machine's trigger class (transport vs validation) is recorded in
  that machine's `errors.log` at 2026-10-23T17:14–17:17Z; confirming it is a
  human verification step, not a blocker (the honesty defect is identical).
- **Ruling (Task 1):** the plan's arm-2 stimulus (parseable JSON, wrong schema)
  actually fires arm 1 — `parse_json_only` validates the full
  `LlmSynthesisOutput` schema, and `validate_llm_output` has zero hard-fail
  paths (arm 2 is unreachable defensive code). Test pins arm 1 with a non-JSON
  stimulus; the `SynthesisFailure` conversion is line-identical in both arms.
  Cost if wrong: a future hard-fail in the validator is untested at kick level.
- **Ruling (Task 4):** `ct librarian run` needed no behavior change —
  `run_librarian_docs` counts the typed `Err` arm once; the Ok-arm log-growth
  check stays as defense. Only comment drift fixed.
- **Ruling (fmt):** repo main carries ~74 style-2024 rustfmt hunks (CI does not
  gate fmt). Only this PR's own lines are fmt-clean; pre-existing drift in
  touched files (e.g. the `{LlmCompleter, run_synthesis_with_completer}`
  version-sort) left untouched to keep the diff reviewable.

## Test outcomes

- RED: `kick_records_failed_when_synthesis_validation_fails` → `left: "ingested", right: "failed"` (the #298 bug in miniature) → GREEN after the typed-error conversion.
- `kick_records_failed_when_synthesis_transport_fails` GREEN immediately (shares the conversion, per plan).
- `librarian_error_needs_journal_skips_synthesis_failure` RED (`cannot find function`) → GREEN.
- `kick_zero_proposals_stays_ingested_and_watermarks` GREEN (pin).
- Updated old-contract tests: `failed_synthesis_leaves_watermark_unset`,
  `malformed_json_retries_then_writes_no_proposal` now assert the typed Err.
- Full lib suite `--features test-utils`: **1521 passed, 0 failed**.
- Integration: `librarian_evidence_e2e` 5/5, `folder_rules` 1/1, `pipeline`
  7/8 — `unforced_edit_that_only_removes_chunks_skips_the_embed_round_trip`
  fails ONLY in parallel runs (pre-existing live-`~/.brain` env race, passes
  `--test-threads=1`; same class as the PR #188 flake — not this diff).
- clippy `-D warnings` clean on `--lib --features test-utils`.
