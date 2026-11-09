# Spec: Deposit kick records honest failure (issue #298)

**Status:** Implemented
**Branch:** `fix/deposit-kick-honest-failure`
**Issue:** #298 — deposit lane: `wisdom_deposit` reaches `ingested` but librarian
synthesis never fires.

## 1. Problem

A deposit whose librarian synthesis fails (LLM transport error or synthesis
output validation error) is recorded in `deposit_kick_state` as **`ingested`**
with no error, no facts, no proposals, no `librarian_evidence`, and no outbox
rows. The agent's `wisdom_deposit_status` then reports the honest-state
vocabulary's success state for a deposit that produced nothing — INTENT rule 3
("agents should get an honest answer") is violated by the ledger itself.

Observed live 2026-10-23 on v3.10.0: two deposits reached
`documents.status=indexed` + `deposit_kick_state='ingested'`; 8+ hours later
zero artifacts for either document. The control note through the watcher lane
minted facts minutes later, and `ct librarian run --yes` minted entries for
other documents but never for the deposits.

## 2. Root cause (code-established 2026-11-09)

`run_synthesis_with_completer` (`src-tauri/src/librarian/synthesis.rs`)
degrades two failure classes to `Ok(())` after appending one
`<vault>/.brain/errors.log` line each:

1. **LLM call failure** — `call_llm_with_retry` errors (transport, timeout,
   missing content in the response body).
2. **Synthesis output validation failure** — `parse_and_validate` errors
   (unparseable JSON after retry, or schema/semantic validation rejection).

`run_kick_with` (`src-tauri/src/wisdom_deposit.rs:562-566`) maps any `Ok` from
the librarian leg to `set_kick_state(..., "ingested", None)`. Because the two
fail-safe arms return `Ok`, the ledger records success. Neither arm stamps the
synthesis watermark, so `ct librarian run` retries the document on its next
pass and fails the same silent way — consistent with the issue's observation
that the run minted ~11 general entries while skipping the deposits.

Which of the two failure classes fired on the repro machine is recorded in that
machine's `<vault>/.brain/errors.log` at 17:14–17:17Z 2026-10-23 (`synthesis
JSON failure` vs `synthesis validation failure`); confirming the line is a
verification step (§7), not a blocker — the honesty defect is the same either
way.

## 3. Non-goals

- Not changing **which** deposits synthesize (folder rules, tiers, frontmatter
  gates are out of scope; gates 1–4 in `generate_summary_inner` were audited
  and are correct for the deposit lane).
- Not changing the legitimate zero-facts path: when the LLM answers with
  valid, schema-valid output that yields zero proposals, that is a real
  "nothing to extract" — it stays `Ok(())`, stamps the watermark, and the
  deposit reads `ingested` with `facts: []`.
- Not changing the issue-#299 benign skips (trigger document vanished
  mid-synthesis): they stay `Ok(())` by design — the delete+heal pass owns
  that state.

## 4. Decisions

**D1 — typed failure, honest ledger.** The two fail-safe arms return a typed
`SynthesisFailure` error (new public error type in `synthesis.rs`) instead of
`Ok(())`. `run_kick_with`'s existing `Err` arm already does the right thing:
`set_kick_state(..., "failed", Some(error))`. No new ledger state — the CHECK
constraint vocabulary (`pending|chunked|ingested|failed|no_ingest_host|
queued_watcher`) is unchanged, and `wisdom_deposit_status` already surfaces
`error`.

**D2 — exactly-once journaling.** The arms keep calling
`write_synthesis_error` (the errors.log journal). Callers must not re-log a
`SynthesisFailure`:

- The pipeline worker (`pipeline/mod.rs` `run_inner`) currently writes
  `librarian error {path}: {e}` via `write_error_log` on any librarian `Err`;
  it downcasts `SynthesisFailure` and skips its own write (the arm already
  journaled), keeping one line in `errors.log` instead of two.
- `run_kick_with` never wrote errors.log itself; unchanged — the ledger row is
  its record.

**D3 — error content.** The typed error carries the same message text the
arms write to errors.log (`synthesis JSON failure for {path}: …` /
`synthesis validation failure for {path}: …`), so the ledger `error` column,
the errors.log line, and any stderr output are one copy-paste.

**D4 — non-goals are pinned by tests.** The zero-facts watermark path and the
#299 vanished-source skips keep returning `Ok(())`; both behaviors get explicit
regression assertions so a future "make everything loud" change cannot silently
reverse them.

## 5. Tests

1. **RED→GREEN (the #298 repro):** extend the `wisdom_deposit.rs` kick test
   seam — `run_kick_with` with a completer that returns unparseable text
   (validation-failure class) must leave the ledger at `failed` with the
   synthesis message in `error`, not `ingested`. Same-shape companion test
   with a completer that errors outright (transport class).
2. **Pipeline-lane dedupe:** a librarian leg failing with `SynthesisFailure`
   writes ONE errors.log line (the arm's), not two.
3. **D4 pins:** (a) valid-but-zero-proposals output still stamps the watermark
   and ends the kick `ingested`; (b) the #299 vanished-trigger skip still
   returns `Ok`.
4. Existing suites must stay green: `librarian_evidence_e2e`,
   `wisdom_deposit` tests, `pipeline.rs`, `folder_rules.rs`.

## 6. Risks

- The pipeline lane now treats synthesis failure as `Err` internally; the
  worker's job-level handling on that path is a logged error (it already was
  for real librarian errors) — no counter/pending-state change.
- `ct librarian run` prints per-document failures instead of silence — a
  behavior improvement consistent with the ledger, checked during
  implementation.

## 7. Verification

- `cargo test -p curated-thoughts --lib` + the named integration suites, plus
  `--features test-utils` per the repo's CI gate reality.
- Human confirmation step (repro machine, Linux box): grep
  `<vault>/.brain/errors.log` for `synthesis (JSON|validation) failure` at
  2026-10-23T17:14–17:17Z; post the class ("JSON" = transport/parse,
  "validation" = schema) onto #298 as the trigger confirmation.
