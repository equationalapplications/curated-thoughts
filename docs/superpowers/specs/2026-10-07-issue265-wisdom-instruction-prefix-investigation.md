# Investigation: Qwen3 instruction-prefix for the wisdom gate (issue #265 follow-up)

**Date:** 2026-10-07 · **Session:** handoff `2026-10-07-handoff-sop-flow-wisdom-gate-prefix` · **Status:** CONVERGED (Step 0 complete — GLM-5.3 non-flash: CONVERGE
after 1 fix round; Opus: conclusion endorsed, round-2 text findings all
applied below; remaining items are explicitly blocking SPEC conditions)

## Question

CT's wisdom gate (`ct wisdom match`, #265, merged `e83156b`) catches only
**44%** of relevant messages at FP ≤ 0.05 (floor 0.70) with
`external:qwen/qwen3-embedding-4b` (`docs/benchmarks/2026-10-07-wisdom-gate-qwen3-embedding-4b.md`).
Hypothesis: Qwen3-Embedding is instruction-aware and expects a task
instruction prefix on **query-side** text; CT embeds raw text on both
sides. Adding the query-side prefix should lift hit@2 materially without
raising FP.

## Evidence 1 — Qwen3 docs (verified quotes, see scratch/step0-web-research.md)

- Exact template (HF model card Qwen3-Embedding-4B, Transformers/vLLM code):
  `f'Instruct: {task_description}\nQuery:{query}'` — `Instruct:` + space,
  newline, `Query:` with **no** space after the colon (card's TEI example
  inconsistently adds a space; code template is canonical).
- Document side: *"No need to add instruction for retrieval documents"* —
  raw text.
- Family-wide: 0.6B/4B/8B all listed `Instruction Aware: Yes`.
- Stated benefit (model card): omitting a query-side instruct drops
  retrieval performance ~1–5%; MTEB "Inst. Retri." for 4B = 11.56.
- OpenRouter: `/v1/embeddings` `input` is a plain string, no transform
  parameters exist → CT must prepend the prefix itself; empirically
  confirmed 2026-10-07: a prefixed multi-line input returned 2560-dim
  vectors, `prompt_tokens: 29` (raw request/response:
  scratch/embed-probe2.json). Repeat-call determinism: identical
  request twice → cos 1.0 (scratch/rep1.json, rep2.json), so the route
  is deterministic and the `input_type` 0.99993 difference is a real
  (tiny) transform, not sampling noise.

## Evidence 2 — code path (verified, see scratch/step0-code-path.md)

- Query side: `queries::wisdom_match_cmd` (tools/src/queries.rs:718-724)
  embeds `truncate_text(text)` (2000 chars) **verbatim** — no prefix.
- Document side: `embed_text_for_entry` (src-tauri/src/embed_sweep.rs:35-37)
  builds `"{title}\n\n{body}"` — no prefix. Same helper everywhere
  (sweep, write-time precompute, commit parity check).
- Calibrator matches production on both sides (facts via
  `embed_text_for_entry`, probes raw) — so the calibration numbers are a
  fair measurement of the production behavior.
- Blast radius of a production query-side prefix: **exactly one call
  site**, `queries.rs:724` (not shared with search/recall/MCP).
- Ordering note: production truncates BEFORE embedding; a prefix applied
  at :724 lands after truncation (prefix doesn't consume the 2000-char
  budget).
- CI gap: nothing enforces calibration-prefix == production-prefix (the
  bench replays frozen vectors and never embeds).

## Evidence 3 — baseline reproduction

Committed fixtures verified:
- facts.jsonl sha256 `63c0349c…14533f51` ✓ (matches expected.json + bench doc)
- probes.jsonl sha256 `6124b385…26a00ac1` ✓ (100 relevant / 100 irrelevant)

Frozen-vector bench replay (`wisdom_gate_bench.rs`, slow-tests):

```
NOT RUN this session — CI CANNOT run it either (M3: ci.yml enables only
test-utils,mcp-server; slow-tests never runs in CI). Replaced by two
equivalent reproductions now, and the PR must run the bench locally and
paste the output + add non-gated parity/scheme assertions:
```

1. Direct calibrator re-run on the committed fixtures, raw (cell A, real
   embedder): floor 0.70, hit@2 0.44, FP 0.05 — **exactly** matches
   `expected.json` and the bench doc.
2. Independent Python sweep over `vectors.json.gz` + `wisdom_match_with_floor`'s
   math (single matmul of normalized vectors, per-floor top-2 gating,
   ties → higher floor): best = (0.70, 0.44, 0.05) — **exact match**,
   including the full 71-floor curve shape.

## Evidence 4 — the 2×2 experiment

Task description used for the instruction (tailored to the gate's job,
per the model card's advice to customize):

> `Given a user or agent message, retrieve stored wisdom entries that help answer or act on it`

Prefix string (canonical code-template spacing):
`Instruct: {task}\nQuery:` — prepended directly to probe text (query
side) and/or to `{title}\n\n{body}` (doc side).

| Cell | query-prefix | doc-prefix | floor | hit@2 | FP |
|---|---|---|---|---|---|
| A (baseline) | — | — | 0.70 | 0.44 | 0.05 |
| B | ✓ tailored | — | 0.70 | 0.50 | 0.05 |
| C | — | ✓ tailored | 0.73 | 0.39 | 0.05 |
| D | ✓ tailored | ✓ tailored | 0.74 | 0.58 | 0.04 |
| E | ✓ neutral | ✓ neutral | 0.64 | 0.57 | 0.05 |
| F | ✓ neutral | — | 0.65 | 0.49 | 0.05 |

Instruction strings:
- Tailored: `Instruct: Given a user or agent message, retrieve stored
  wisdom entries that help answer or act on it\nQuery:`
- Neutral: `Instruct: Given a web search query, retrieve relevant
  passages that answer the query\nQuery:` (the Qwen card's canonical
  retrieval example — topically unrelated to wisdom entries)

| Cell E/F artifact note: E and F were run with `--freeze` (freezeE/ and
| freezeF/ dirs): full sweep tables in `scratch/freezeE-run.json` /
| `scratch/freezeF-run.json`, frozen vectors `scratch/freeze{E,F}/vectors.json.gz`,
| expected.json inside each dir carries the prefix strings. Per-probe paired
| outcomes behind every McNemar test: `scratch/step0-paired-all.json` (A, D, E).
| Full hit/FP curves for A/D/E: `scratch/step0-curves-ADE.json`.

Cross-checks (all via an independent Python sweep of the same probe/fact
vectors, plus the calibrator's own sweep):
- At the FIXED baseline floor 0.70 (apples-to-apples): A (0.44, FP .05) ·
  B (0.50, FP .05) · C (0.57, FP .08) · D (0.77, FP .10) — D's raw
  separation is far better, it just needs its higher floor to control FP.
- Cell A/B floors ≤ 0.51 still show hit@2 ≥ 0.94 — the head of the curve
  is unchanged; the prefix effect lives in the middle band.

## Root cause (code level)

The gate scores a raw-query embedding against raw-document embeddings
with an instruction-aware model. The 2×2 + controls established:

- **Both-side prefixing beats baseline; query-only is unproven.**
  Paired McNemar at each cell's own floor: A vs E χ²=5.76 (p≈0.016),
  A vs D χ²=7.04 (p≈0.008), A vs B χ²=1.39 (ns), B vs D χ²=3.06 (ns).
  Family-wise caveat: across the five contrasts run, only A vs D
  survives Bonferroni (0.008×5=0.040; A vs E → 0.082). Bootstrap over
  probes with the floor re-selected in each resample (winner's-curse
  correction; scratch/step0-bootstrap.json): hit@2 95% CI —
  A [0.25, 0.57] · D [0.36, 0.76] · E [0.43, 0.79]. E and D
  statistically interchangeable; E-over-B rests on point estimates
  plus cell C's failure, not a significant paired test.
- **No detected wording difference** (was: "provably interchangeable").
  D vs E: χ²=0.00 — and per Opus r2 m-new-4, with only 5 discordant
  relevant probes the paired hit difference CI is about ±4.4pp, so the
  case for wording-insensitivity is reasonably strong on the hit side.
  GLM's fixture-leakage hypothesis is not supported by the control (the
  topically-unrelated string reproduces the full lift).
- Mechanism, corrected per Opus M5: the doc prefix does NOT simply
  "shift scores up" — E's floor (0.64) sits BELOW A's (0.70), while
  wording alone moves the floor 0.10 (D 0.74 vs E 0.64). The prefix
  reshapes each cell's score distribution; the calibrated floor is
  therefore **byte-exact-prefix-defining**: any wording change
  invalidates the frozen floor and forces recalibration.
- **Headroom, computed (Opus M5 definition)**: neither D nor E has any
  downward margin — one floor step (0.01) below either operating point
  breaches FP 0.05. Upward: hit@2 at floor+0.02 costs E −0.06 vs
  D −0.13. E's real advantage over D is the gentler upward slope and
  the lower absolute floor, not "max drift headroom" in any
  symmetric sense.
- **Off-label disclosure (Opus M4):** the doc-side prefix labels
  documents with `Query:` — the model card's "no instruction for
  retrieval documents" guidance makes this off-label use; the 2×2 says
  it works on this fixture set, but fixture-specific gains cannot be
  excluded (hence the pre-merge live-brain condition).
- Steepness near the operating floor (hit swings ~18pp over 0.03 of
  floor) is a property of this model+fixture in ALL cells including the
  baseline — embedding drift sensitivity is a pre-existing risk, not
  prefix-caused.

## Embedding-blob consumers (caller trace, scratch/step0-callers.md)

Production readers of `llm_wiki_entries.embedding_blob` — exactly two
query-vs-doc scorers; **no doc-to-doc scorer exists**:
1. **Wisdom gate** — `wisdom_match.rs:233-235/280` (query:
   `queries.rs:724`, raw). Consumer: `ct wisdom match` + the live
   wisdom-delivery integration.
2. **`wiki_search` / `wiki_context` MCP tools** —
   `wiki_graph.rs:298-327` via `embed_query` raw
   (`tool_dispatch.rs:1135/1123`). MISSED by every prior draft: if docs
   become prefixed and these queries stay raw, cross-scheme cosine
   degrades and rows scoring ≤ 0.0 silently vanish
   (`wiki_graph.rs:325-327`). Spec decision required: prefix these
   queries with the same string (they are natural-language queries —
   likely appropriate) or scope the scheme so they keep raw docs.

NOT affected (verified): `recall_chunks`, `semantic_search`,
`vault_semantic_search`, `vault_related_chunks`, `curated_recall_context`,
`curated_search_code` — all rank chunks via the separate
`embeddings` table (`queries.rs:75-82`, `search/mod.rs:151-156`).
`wisdom_deposit.rs:1110/1560` are test-only NULL-blob INSERTs — Opus's
doc-to-doc concern has no production counterpart.

Write-path facts that shape migration: the sweep only fills NULL blobs
(`embed_sweep.rs:53, 95`); write-time embed keys parity checks on
`embed_text_for_entry` output equality (`db/commit.rs:1534-1538,
1721-1725`); `WISDOM_GATE_FLOORS` pins 0.70 for the current key
(`wisdom_match.rs:52-58`).

## Fix directions

1. **Adopt BOTH-SIDE prefixing (cell E configuration)** — neutral
   canonical wording, both towers:
   - Query side (gate): `queries.rs:724`: prepend the query prefix
     (after `truncate_text`) on the gate path only.
   - Query side (wiki_search/wiki_context): same prefix at the
     `embed_query` call sites (`tool_dispatch.rs:1135/1123`) — REQUIRED
     if docs become prefixed (see consumers section); spec decision to
     confirm.
   - Doc side: with ONE `embedding_blob` column and no non-gate writer
     of `embed_text_for_entry` (all four callers write the same blob:
     `embed_sweep.rs:142`, `commit.rs:2260`, `db/wisdom.rs:67`,
     `lib.rs:2547`), a "new helper" protects nothing — per Opus r2
     MAJOR-1, the doc prefix necessarily applies to ALL blob readers,
     and the parity check (`commit.rs:1537`) must compare against the
     SAME function that produced the embedded text or it will either
     NULL out every write-time vector or validate text that was never
     embedded. The real spec choice for `wiki_search`/`wiki_context`:
     (a) prefix their queries too (recommended; they are
     natural-language queries), or (b) add a second raw-vector blob
     column with dual-write cost priced. No third option exists.
   - **Migration (M1 corrected + r2 MAJOR-2):** ONE "active scheme"
     value must atomically drive: the gate query prefix, the gate
     floor, the gate SELECT scheme-filter (`wisdom_match.rs:233-235`),
     the `wiki_search` query prefix, AND a scheme-filter added to
     `wiki_graph.rs:301` (which has none today). Precondition: every
     writer path switches in the same change, or the raw-scheme count
     never reaches zero. During the window, `wiki_search` results are
     degraded by design (accepted; entries written mid-window are
     invisible to the raw-filtered gate until cutover). Degradation
     semantics + crash idempotency remain SPEC BLOCKING (GLM r2).
   - **Shared-helper blast radius:** corrected by the caller trace —
     see "Embedding-blob consumers" section below. `recall_chunks` was
     wrongly accused earlier: it reads the separate `embeddings`/`chunks`
     tables and is NOT affected (verify against
     scratch/step0-callers.md).
2. **Recalibrate + freeze together** at E's floor (0.64), commit new
   `expected.json` + `vectors.json.gz` together, stamped with the new
   (profile, scheme) key; update `WISDOM_GATE_FLOORS` under that key.
3. **CI gap — corrected (Opus M3):** `wisdom_gate_bench.rs` is
   `#![cfg(feature = "slow-tests")]` and NO workflow enables slow-tests
   (`ci.yml:112/165` use `test-utils,mcp-server`) — "deferred to CI" was
   false. Fix: (a) run the bench locally and paste output in the PR,
   (b) put the prefix-parity + scheme assertions in a NON-feature-gated
   test so they always run, (c) optionally add a CI job with
   `--features slow-tests`.
4. **Latency re-measure** after cutover (p95 ≤ 1.5 s recipe, PR266
   closure).
5. **Keep the calibrator flags** (`--query-prefix`/`--doc-prefix` —
   both implemented in the worktree, `--doc-prefix` prefixes the OUTPUT
   of `embed_text_for_entry`, exactly mirroring the intended production
   helper) — they stamp the snapshot.

## Recommendation

Adopt both-side instruction prefixing, **cell E configuration**
(neutral wording, floor 0.64): hit@2 0.44 → 0.57 at FP ≤ 0.05
(bootstrap CI [0.43, 0.79] vs A's [0.25, 0.57]; floor re-selected per
resample). E vs A: χ²=5.76, p≈0.016 raw (Bonferroni-sensitive) — and
the PAIRED bootstrap (step0-bootstrap-paired-AE.json: same resampled
probes scored in both cells, floor re-selected per resample per cell,
seed 20261007, n=400) gives **Δhit@2 = E−A median +0.16, 95% CI
[+0.03, +0.35], P(Δ>0) = 0.97** — the improvement excludes zero even
under the winner's-curse correction. Wording
difference not detected (D vs E χ²=0.00, paired ±4.4pp); the neutral
string is preferred as the canonical documented example.

Conditions before merge (GLM round 1/2 + Opus, all blocking):
1. ~~Paired B-vs-D test~~ — DONE (χ²=3.06 ns; A-vs-B also ns).
2. ~~Neutral-instruction control~~ — DONE (cell E; leakage unsupported).
3. **Live/scratch-brain calibration pre-merge** — TO DO at spec/plan
   stage; per Opus r2 MAJOR-3 the pass condition must be a PAIRED
   comparison, not an absolute threshold: run cell A (raw, floor 0.70)
   and cell E (floor 0.64) on the SAME labelled live probes with
   ≥ 50 relevant and ≥ 100 irrelevant probes; **pass only if E hit@2 ≥
   A hit@2 (paired), E FP ≤ 0.05, using the shipping E string**.
   Probe labelling method + owner named in the spec. Failing → no
   merge, revisit scheme.
4. **Migration v2** (ONE active-scheme switch driving gate prefix +
   floor + both SELECT filters + wiki_search prefix; embed_scheme
   column; parity-check rewiring; degradation semantics; crash
   idempotency) — TO SPEC, blocking.
5. **Larger irrelevant-probe set (≥400)** — accepted as follow-up; the
   FP Clopper-Pearson 95% CI at n=100 is [0.016, 0.113], so post-merge
   monitoring (gate-open rate) is the practical FP guard until then —
   the spec must pin that guard to a number (expected baseline open
   rate + numeric tripwire + rollback owner), not leave it a phrase.
6. **Prefix parity is floor-defining** — the E floor is valid only for
   the byte-exact prefix string; the non-gated parity test (condition
   3b) must hard-code it.

## Review trail

- **GLM-5.3 (non-flash) critique, round 1: FIX_FIRST.** Full text:
  scratch/glm-critique-out.md. Findings: (1) A-vs-D tested but D-vs-B
  (the actual decision) not; (2) fixture-leakage hypothesis for the
  tailored instruction; (3) FP 5→4 is churn; (4) migration ships
  through untested mixed states under an unversioned model key; (5)
  shared-helper leak to search/recall consumers. Convergence demands
  1 and 2 executed above (controls B/D/E/F frozen + paired tests);
  demands 3–5 carried into the spec as blocking conditions.
- **GLM-5.3 (non-flash) round 2: CONVERGE.** Full text:
  scratch/glm-critique2-out.md. Demands 1–3 judged genuinely
  discharged; leakage hypothesis withdrawn. Residuals — all declared
  BINDING ON THE SPEC, not this doc: pin the re-embed mechanism and key
  representation (one of each named alternative), numeric monitoring
  tripwire + rollback owner, gate degradation semantics during the
  re-embed window (fail-closed vs skip; crash idempotency), live-brain
  calibration run with the SHIPPING E string and pre-declared pass
  thresholds, family-wise caveat (added above), B-vs-D wording fix
  (added above), pricing of the dual-scheme storage cost, and a canary
  re-embed hash in CI for provider-side drift (model-key versioning
  cannot see OpenRouter model changes; the frozen-vector bench never
  re-embeds).
- **Opus review: REQUEST CHANGES.** Full text:
  scratch/opus-review-step0-extracted.md. All findings addressed or
  re-staged:
  B1 (E/F artifacts — E/F were freeze-runs; artifact map added) ·
  M1 (storage/key — embed_scheme column mandated, contradictory
  one-column migration options replaced) · M2 (blast radius — caller
  trace run; wiki_search/wiki_context added as affected,
  wisdom_deposit concern dissolved as test-only) · M3 (CI never runs
  slow-tests — corrected; local bench + non-gated assertions required)
  · M4 (statistics honesty — bootstrap with floor re-selection added,
  "refuted/provably" softened, off-label disclosure added, live pass
  thresholds pre-declared in Recommendation) · M5 (mechanism + headroom
  — corrected; headroom computed: zero downward margin in D and E) ·
  m1 (live probe artifacts: scratch/embed-probe2.json, rep1/rep2.json;
  repeat-call cos 1.0 → route deterministic) · m2 (true migration
  intermediate — mixed raw+prefixed docs — unmeasured; covered by
  scheme-filter cutover design) · m3 (FP CI corrected to
  [0.016, 0.113]) · m4 (calibrator flags confirmed in worktree;
  UNCOMMITTED — they land with the PR).
  RESEARCH_REQUEST items 1–4 all satisfied (paired data files, caller
  trace, wisdom_deposit finding, live-probe artifacts + repeat probe).
- **Opus round 2: FIX_FIRST (text-only, no new runs).** Full text:
  scratch/opus-review-step0-r2-extracted.md. The E conclusion is
  endorsed; round-1 findings judged discharged. Three MAJOR text fixes
  + minors, ALL APPLIED: MAJOR-1 (the "new helper protects
  wiki_search" claim was wrong — one blob column, all four
  embed_text_for_entry callers write it; helper claim removed, real
  spec choice is query-prefixing vs dual-blob pricing; parity-check
  rewiring requirement added) · MAJOR-2 (ONE active-scheme switch must
  atomically drive gate prefix + floor + both SELECT filters +
  wiki_search prefix; wiki_graph.rs:301 needs a scheme-filter; window
  degradation stated) · MAJOR-3 (live pass condition converted to a
  PAIRED A-vs-E comparison; absolute 0.45 removed) · m-new-1
  ("deferred to CI" claim + "NOT RUN … equivalent" wording corrected;
  E/F "(control)" labels dropped; determinism artifact added) ·
  m-new-3 (input_type phrasing: real tiny transform, not noise) ·
  m-new-4 (paired Δ CI ±4.4pp replacing the loose ±10pp statement;
  paired A−E bootstrap RUN to close MAJOR-3's spirit:
  Δhit@2 +0.16, 95% CI [+0.03, +0.35], P>0 = 0.97 — improvement
  excludes zero). Confidence in E as the Step-0 conclusion: explicit.
- **OpenRouter `input_type` check (GLM §5):** accepted with HTTP 200 on
  both `search_query` and `search_document`, but response vectors are
  cos 0.99993 vs plain input on this route — effectively a no-op; CT's
  absence of the parameter is safe, hand-rolled prefixing is the only
  real lever. No double-apply risk.

## Open questions

1. ~~Is the both-side advantage stable or fixture-specific?~~ — wording
   sensitivity answered (D vs E χ²=0.00, leakage refuted); absolute
   generalization to live entries still rides on condition 3.
2. Does the doc-side prefix hold up on the LIVE brain's entries? →
   promoted to blocking condition 3 (pre-merge scratch-brain run).
3. ~~Task-description wording sensitivity~~ — answered: insensitive
   (χ²=0.00); neutral canonical string chosen.
4. Shared-helper scoping for recall_chunks / embed_query consumers —
   promoted into Migration v2 (blocking, spec-stage decision).
5. Steepness of the hit-vs-floor curve (~18pp / 0.03 floor) in ALL
   cells: pre-existing property of this model+fixture; larger probe set
   (condition 5) will smooth the estimate. No action this PR beyond
   monitoring.
