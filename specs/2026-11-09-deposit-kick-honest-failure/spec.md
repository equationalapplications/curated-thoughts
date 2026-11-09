# Spec: Deposit kick records honest failure (issue #298)

**Status:** Implemented
**Branch:** `fix/deposit-kick-honest-failure`
**Issue:** #298 — deposit lane: `wisdom_deposit` reaches `ingested` but librarian
synthesis never fires.

## 1. Problem

A deposit whose librarian synthesis fails (LLM transport error or synthesis
output validation error) is recorded in `deposit_kick_state` as **`ingested`**
with no error, no facts, no proposals, no `librarian_evidence`, and no outbox
rows. The agent's `wisdom_deposit_status` then reports `kick: "ingested"` for
a deposit that produced nothing — INTENT rule 3 ("agents should get an honest
answer") is violated by the ledger itself.

Observed live 2026-10-23 on v3.10.0: two deposits reached
`documents.status=indexed` + `deposit_kick_state='ingested'`; 8+ hours later
zero artifacts for either document. The control note through the watcher lane
minted facts minutes later, and `ct librarian run --yes` minted entries for
other documents but never for the deposits.

## 2. Failure mechanism (code-established 2026-11-09)

`run_synthesis_with_completer` (`src-tauri/src/librarian/synthesis.rs`)
has two fail-safe arms that return `Ok(())` after appending one `errors.log`
line (see §8 for where that line lands):

1. **`synthesis JSON failure`** — `call_llm_with_retry` errors: transport,
   timeout, missing content in the response body, or output that is still
   not schema-valid JSON after the retry.
2. **`synthesis validation failure`** — `parse_and_validate` errors. This arm
   is unreachable today (§8).

`run_kick_with` (`src-tauri/src/wisdom_deposit.rs:562-566`) maps any `Ok` from
the librarian leg to `set_kick_state(..., "ingested", None)`. Because the two
fail-safe arms return `Ok`, the ledger records success. Neither arm stamps the
synthesis watermark, so `ct librarian run` retries the document on its next
pass and fails the same silent way — consistent with the issue's observation
that the run minted ~11 general entries while skipping the deposits.

This is the candidate mechanism for the live incident, not a proven cause. A
second path gives the same symptom and is unchanged here: schema-valid output
whose proposals are all dropped by `validate_llm_output` (unknown
`existing_id`, unknown `op`, empty body) stamps the watermark and returns
`Ok(())`, so the kick reads `ingested` with zero artifacts and later
`ct librarian run` passes skip the document. §7 tells the two apart. The
honesty defect in the fail-safe arms is real either way.

## 3. Non-goals

- Not changing **which** deposits synthesize (folder rules, tiers, frontmatter
  gates are out of scope; gates 1–4 in `generate_summary_inner` were audited
  and are correct for the deposit lane).
- Not changing the zero-result path: when the LLM answers with valid,
  schema-valid output that yields zero validated proposals, it stays `Ok(())`,
  stamps the watermark, and the status reads `kick: "ingested"` with
  `facts: []`. That covers a real "nothing to extract" and also output whose
  proposals were all dropped during validation; the pin tests only
  `{"proposals": []}`.
- Not recording failures in the watcher lane: when a watcher holds the vault
  lock the kick writes `queued_watcher`, and the pipeline worker never writes
  the kick ledger, so a synthesis failure there stays `queued_watcher`.
- Not changing the issue-#299 benign skips (trigger document vanished
  mid-synthesis): they stay `Ok(())` by design — the delete+heal pass owns
  that state.
- Not changing where synthesis errors are journaled (#324), or the status
  probe's document-key mismatch that leaves successful deposits reading
  `state: pending` (#325).

## 4. Decisions

**D1 — typed failure, honest ledger.** The two fail-safe arms return a typed
`SynthesisFailure` error (new public error type in `synthesis.rs`) instead of
`Ok(())`. `run_kick_with`'s existing `Err` arm already does the right thing:
`set_kick_state(..., "failed", Some(error))`. No new ledger state — the CHECK
constraint vocabulary (`pending|chunked|ingested|failed|no_ingest_host|
queued_watcher`) is unchanged, and `wisdom_deposit_status` already surfaces
`error`.

**D2 — at most one journal write per failure.** The arms keep calling
`write_synthesis_error` (the errors.log journal). Callers must not re-log a
`SynthesisFailure`:

- The pipeline worker (`pipeline/mod.rs` `run_inner`) currently writes
  `librarian error {path}: {e}` via `write_error_log` on any librarian `Err`;
  it downcasts `SynthesisFailure` and skips its own write (the arm already
  attempted its write). `write_synthesis_error` is best-effort and the two
  writers can target different files (§8), so this means at most one write
  per failure, not one line in any particular log.
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
   (JSON arm) must leave the ledger at `failed` with the synthesis message in
   `error`, not `ingested`, and `deposit_status_row` must answer `state:
   failed` with the same `error`. Same-shape companion test with a completer
   that errors outright (transport class).
2. **Pipeline-lane dedupe:** `librarian_error_needs_journal` is false for
   `SynthesisFailure` and true for every other error. The worker-level
   line-count test moves to #324, because it only proves something once the
   two writers share a file.
3. **D4 pins:** (a) valid-but-zero-proposals output still stamps the watermark
   and ends the kick `ingested`, with `kick: "ingested"` and `facts: []` in
   the status; (b) the #299 vanished-trigger skip still returns `Ok`.
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
- Human confirmation step (repro machine, Linux box), to post onto #298:
  1. Run `SELECT path, synth_hash FROM documents WHERE path LIKE
     '%/immutable-source-files/agents/%'` and find the two deposits. A
     non-NULL `synth_hash` means the zero-result path (§2), so this change did
     not fix that incident and #298 stays open for it.
  2. If `synth_hash` is NULL, run `find <vault> -path '*/.brain/errors.log'`
     and grep each result for `synthesis JSON failure` plus the deposit's
     filename, with no time bound (17:14Z and 17:17Z are the deposit times;
     failures are logged later). The text after the path shows the cause
     (transport error or invalid JSON).

## 8. Rulings recorded during review

- **The validation arm is unreachable.** `validate_llm_output` has no error
  path, and `call_llm_with_retry` has already run the same fence-strip and
  parse into the same type that `parse_and_validate` repeats. The arm still
  returns `SynthesisFailure`, so a future hard-fail validator is honest, but
  no test can reach it today. Only `synthesis JSON failure` can appear in a
  journal.
- **Journal location is out of scope (#324).** The arms write to
  `<source grandparent>/.brain/errors.log`, so a deposit at
  `immutable-source-files/agents/x.md` logs under
  `<vault>/immutable-source-files/.brain/`. D2's skip is a true dedupe only
  when that is also the worker's file.
- **The agent-visible failure is pinned; the agent-visible success is not
  (#325).** A failed kick reads `state: failed` with its `error`. A successful
  kick reads `state: pending` with `facts: []` because the status probes miss
  the canonical document key.
