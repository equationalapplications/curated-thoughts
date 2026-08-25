# Spec: Librarian LLM completer robustness (timeout + reasoning effort)

**Date:** 2026-08-24
**Repo:** equationalapplications/curated-thoughts
**Target:** `src-tauri/src/librarian/synthesis.rs` (`HttpLlmCompleter`)
**Status:** Implementation already validated live (librarian run4b); this spec formalizes it for PR review per spec-driven workflow.

## Problem

The librarian's `HttpLlmCompleter` used `reqwest::blocking::Client::new()`,
which carries a ~30s default timeout. When the configured generation provider
is a reasoning model on OpenRouter (e.g. `stealth/ox-alpha`), synthesis calls
on long documents routinely exceed 30s:

- Librarian run4 (unpatched): 3+ docs failed in the first hour with
  "error decoding response body: operation timed out" — including core vault
  docs (`curated-thoughts-improvement-backlog.md`, `hermes-memory-offload.md`).
  Failures are logged to errors.log but the doc's synthesis is silently lost
  for that pass.
- Additionally, reasoning models spend completion budget in a `reasoning`
  field before producing `content`. For bulk librarian work the deliberation
  adds latency without improving output; CT only needs strict JSON.

## Goals

1. A librarian run against an external OpenRouter model completes with zero
   transport timeouts on documents of any size in the current corpus.
2. Bulk runs complete ~an order of magnitude faster when the provider is a
   reasoning model, without degrading synthesis JSON validity.
3. No behavior change for sidecar/Ollama providers beyond the (harmless)
   extra body field and longer timeout.

## Non-goals

- Retry/backoff logic for HTTP 429/5xx (separate backlog item — see
  `procedures/curated-thoughts-improvement-backlog.md`, "single-retry JSON path").
- Persisting the real provider model name into `curated_proposals.model`
  (separate backlog item: "proposal model mislabeling").

## Design

Single function changed: `HttpLlmCompleter::complete`.

1. **Timeouts.** Replace `Client::new()` with `Client::builder()`:
   - request timeout: **600s** (longest observed valid synthesis ≈ 3 min;
     10× headroom)
   - connect timeout: **30s**
2. **Reasoning effort cap.** Add to the JSON body:
   `"reasoning": { "effort": "low" }`. OpenRouter forwards this to capable
   models; non-reasoning models ignore it (verified harmless on
   `openai/gpt-4o-mini` style endpoints). The synthesis prompt requires
   mechanical strict-JSON extraction — low effort is appropriate.

No API/config surface changes. No schema changes.

## Acceptance criteria

- [x] Live probe: ox-alpha with `effort: low` returns valid strict JSON,
      0 reasoning tokens.
- [x] Run4b (patched binary): 46 docs in 5.5 min, zero timeouts/errors
      (vs unpatched pace of ~56/hour WITH timeouts).
- [ ] Full-corpus validation: run4b completes 1540/1540 with 0 transport
      failures and `llm_wiki_entries > 0` after the synthesize-mode folder
      is processed.
- [ ] `cargo test --features test-utils` green (completer tests use mocks;
      no mock changes needed).
- [ ] CodeRabbit review addressed per pr-spec-driven procedure.

## Risks

- 600s timeout means a hung provider stalls one document up to 10 min before
  failing. Acceptable for batch runs; not for interactive paths (this code
  path is only the librarian/synthesis completer).
- If a future external provider rejects unknown body fields strictly, the
  `reasoning` key could cause a 400. Mitigation if observed: send the key
  only when `model_name` matches known reasoning models — do NOT pre-build
  this; keep it simple until evidence demands it.
