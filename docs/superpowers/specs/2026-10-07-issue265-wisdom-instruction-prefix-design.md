# Wisdom-gate instruction prefix: both-side Qwen3 conditioning + scheme cutover (issue #265 follow-up)

**Date:** 2026-10-07
**Status:** PROPOSED
**Branch:** `feat/issue265-query-prefix-test`
**Parent spec:** `docs/superpowers/specs/2026-10-06-issue265-wisdom-match-design.md`
**Investigation (Step 0, converged):** session scratch
`step0-investigation-draft.md` — GLM-5.3 (non-flash) CONVERGE after one
fix round; Opus conclusion endorsed after two rounds (round-1 findings
addressed, round-2 text fixes applied). Review trail with all artifacts
lives in that file; this spec carries the decisions forward.

## Problem

The wisdom gate catches only 44% of relevant messages at FP ≤ 0.05
(floor 0.70) because `qwen/qwen3-embedding-4b` is instruction-aware and
CT embeds raw text on both sides. The 2×2 + controls established:

- Both-side instruction prefixing (cell E) reaches **hit@2 0.57 at
  floor 0.64** (vs 0.44 @ 0.70 raw). Paired bootstrap (floor
  re-selected per resample): **Δhit@2 = E−A median +0.16, 95% CI
  [+0.03, +0.35], P(Δ>0)=0.97**; McNemar χ²=5.76 (raw p≈0.016,
  Bonferroni-sensitive).
- Query-only prefixing is NOT established (A-vs-B χ²=1.39 ns).
- Instruction wording is not detected to matter (D-vs-E χ²=0.00,
  paired ±4.4pp); the floor is **byte-exact-prefix-defining**.
- The doc-side prefix is OFF-LABEL (model card: no instruction for
  retrieval documents) — works on fixtures; live-corpus validation is
  a merge blocker (below).

## Decision

Adopt **cell E**: neutral canonical instruction, BOTH sides:

```
Instruct: Given a web search query, retrieve relevant passages that answer the query
Query:
```

prepended verbatim (single string, `\n` between the two lines) to:
- gate queries (after `truncate_text`),
- `wiki_search`/`wiki_context` queries (Opus r2 MAJOR-2: both blob
  readers must share the scheme),
- all `llm_wiki_entries.embedding_blob` documents (via
  `embed_text_for_entry` output).

New floor: **0.64**, pinned under the NEW scheme key.

## Scope decisions (review-driven; do not relitigate without new evidence)

1. **One scheme, all readers.** There is ONE `embedding_blob` column
   and every reader must agree on the scheme. No "gate-only prefix,"
   no helper-based protection (Opus r2 MAJOR-1).
2. **`embed_scheme` column.** `llm_wiki_entries` gains a nullable
   `embed_scheme` TEXT column (`NULL`/`raw` for existing rows, `instr1`
   for prefixed). The gate SELECT (`wisdom_match.rs:233-235`) and a NEW
   filter in `wiki_graph.rs`'s scoring query (`wiki_graph.rs:301`,
   none today) both filter `embed_scheme = <active>`.
3. **ONE active-scheme switch (Opus r2 MAJOR-2).** A single constant /
   config value atomically determines: the prefix string, the gate
   floor key, the gate SELECT filter, the `wiki_search` SELECT filter,
   and the query-prefix application at both call sites
   (`queries.rs:724`, `tool_dispatch.rs` embed_query sites). No
   combination may be expressible that mixes schemes under one floor.
4. **Parity check rewiring.** Write-time parity (`commit.rs:1534-1538,
   1721-1725`) must compare against the SAME function that produced the
   embedded text (prefixed) — otherwise every write-time embed either
   NULLs out or validates never-embedded text.
5. **Migration window semantics (GLM r2 blocker; Opus r2 MAJOR-2).**
   Spec + implementation must define, concretely:
   - The sweep gains a scheme-filtered mode (re-embed rows whose
     `embed_scheme != active`); existing raw blobs stay readable until
     rewritten.
   - While raw-scheme rows exist: gate and wiki_search serve the OLD
     raw scheme (floor 0.70, raw queries); prefixed writes accumulate
     under `instr1`.
   - Cutover when raw count = 0: flip the active scheme atomically
     (floor 0.64, prefixed queries, `instr1` filter).
   - Crash mid-sweep must be idempotent (per-row scheme stamp makes
     resume trivial: unfinished rows still show old scheme).
   - Mixed-scheme scoring must be IMPOSSIBLE by construction (SELECT
     filters), not by discipline.

## Implementation outline (branch tasks; Rust tasks SERIAL)

1. `embed_scheme` column (V+1 migration), `active scheme` constant
   module: prefix string, scheme name, floor key.
2. `embed_text_for_entry` + gate/wiki_search query paths apply the
   active prefix when the active scheme is `instr1` (single switch).
3. Scheme-filtered sweep mode + backfill; cutover check.
4. `WISDOM_GATE_FLOORS`: add the new key (`external:qwen/qwen3-embedding-4b:instr1`
   or equivalent — key representation finalized in code review), floor
   0.64; recalibrate + commit new `expected.json` + `vectors.json.gz`
   together (calibrator `--query-prefix`/`--doc-prefix` flags, already
   prototyped on this branch, land committed).
5. Tests: non-gated (always-run) assertions that (a) the production
   prefix constants equal the snapshot's `query_prefix`/`doc_prefix`
   keys, (b) both SELECTs filter on the active scheme, (c) parity-check
   uses the same text function. Wisdom-gate bench run locally, output
   pasted in the PR (CI cannot run slow-tests — M3).
6. Latency re-measure after cutover on the ThinkPad (p95 ≤ 1.5 s recipe,
   PR266 closure).

## Merge blockers (from the converged Step 0 — verbatim conditions)

1. **Live/scratch-brain paired calibration** (PR266 pattern, OpenRouter
   spend pre-approved by Kurt for gate work): run cell A (raw, floor
   0.70) vs cell E (floor 0.64) on the SAME labelled live probes
   (≥ 50 relevant, ≥ 100 irrelevant; labelling method + owner named in
   the plan). **Pass only if E hit@2 ≥ A hit@2 paired AND E FP ≤ 0.05,
   E string only.** Fail → no merge, revisit scheme.
2. **Migration v2 completeness** — every item under "Migration window
   semantics" implemented and tested (no prose-only answers).
3. **Monitoring tripwire** — numeric gate-open-rate baseline + tripwire
   + rollback owner pinned in the plan (FP CI at fixture n=100 is
   [0.016, 0.113]; ≥ 400 irrelevant probes is an accepted follow-up,
   not a substitute).
4. **Provider-drift canary** — GLM r2: the frozen-vector bench never
   re-embeds, so a silent OpenRouter model change ships undetected.
   Plan must include a periodic canary re-embed hash check (CI or
   cron) before/with this landing.

## Non-goals

- No probe-set expansion in this PR (tracked follow-up).
- No change to `recall_chunks`/`semantic_search` (separate
  `embeddings` table — verified unaffected).
- No retry/re-rank logic — the floor still abstains per rule 7.
