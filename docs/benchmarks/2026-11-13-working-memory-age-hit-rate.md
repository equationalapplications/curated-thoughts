# Working-memory age-out hit-rate gate (issue #347)

Date: 2026-11-13 · Lane: `working-memory-age` · Plan Task 5

Method: **stored embeddings only** — no external model, no live embedder. The
corpus is constructed so relevance is known by construction, and the gate runs
as a test (`src-tauri/tests/ct_hit_rate_gate.rs`, run with `--nocapture` to
reproduce the `WMA-BENCH` lines).

## Fixture

- 8 queries, unit vectors `e_j` (dim 16).
- Per query: 1 relevant fresh doc in the unaged `notes/` folder with cosine
  0.9 to its query, plus 5 aged distractor docs in the `records/` folder at
  cosines 0.95 / 0.94 / 0.93 / 0.92 / 0.91 — all above the fresh doc, so the
  distractors crowd out the relevant doc when the filter is off.
- `ingest.folder_ages: {"records": 0}`; the aged docs' `ingest_runs.run_at`
  is backdated 400 days (`UPDATE ingest_runs SET run_at = unixepoch()
  - 400*86400`), since scratch brains are freshly indexed and the filter
  never fires without the backdating fixture.
- 48 documents total; ranking via `search::semantic_search` (full scan +
  cosine, the production path); hit@5 = fraction of queries whose relevant
  fresh doc appears in the top-5.

## Results (from the test's `WMA-BENCH` output)

| metric | filter inactive | filter active |
|---|---|---|
| hit@5 | 0.000 | 1.000 |
| aged rows in top-5 (across queries) | 320 | 0 |
| aged docs excluded | no | **yes** |
| unaged ranking identical between configs | — | **yes** |

**hit@5 delta: +1.000** (0.000 → 1.000).

## Gates

1. **Aged docs excluded when the filter is active** — PASS (0 aged rows in
   any top-5).
2. **Unaged hit rate unchanged** — PASS: the unaged ranking is byte-identical
   with the filter on and off; the filter is strictly subtractive (removes
   aged rows, never reorders or drops unaged ones).
3. **Filter recovers crowded-out fresh docs** — PASS (hit@5 0 → 1).

## Interpretation

The synthetic ladder is a worst case: aged-but-recently-relevant distractors
outrank every fresh doc. Production effect sizes will be smaller; the gate
exists to prove the predicate fires on real backdated history and that
unaged retrieval is untouched, not to measure a production lift. The
LongMemEval battery (external-model hit rates) remains controller-side /
`benchmark-pending` per the plan.
