# Wisdom gate two-stage calibration — chunk-hop @ 0.65 (2026-10-18)

Live-brain paired calibration of the two-stage wisdom gate (issue #271 two-stage
retrieval, shipped in 3.6.0), executed per the Task-16 ordered runbook
(`scripts/calibrate-two-stage.sh` semantics). This record justifies the
`WISDOM_GATE_FLOORS` row `("external:qwen/qwen3-embedding-4b:two-stage:chunk-hop", 0.65)`
and the `TWO_STAGE_K` row `("external:qwen/qwen3-embedding-4b", 8)` added by the
flip (issue #265).

## Campaign

- **Probe set:** 150 real-traffic probes — 7 relevant (GLM-judged, Opus-adjudicated
  KEEP/DROP) + 143 FP. sha256 `92942cffaad12911dd9110cb076c39e47a7fcbc8e841b546c79e76ce01b9fdd0`
  (`records/investigations/2026-10-17-issue271-flip-prep/probes-150-final.jsonl` in the
  curated-thoughts-integrations vault mirror).
- **Baseline (single-stage, currently shipped):** `external:qwen/qwen3-embedding-4b`, raw
  scheme, floor 0.70 — hit@2 0.143 (1/7), FP 4/143, opens 5.
- **Bootstrap:** verify-scratch on the live brain — stamp v1, embed_key backfill 320/320
  docs, coverage 43/43, evidence refs 504/504 resolve, hops 333/333 resolve.
- **Cells:** 14 paired runs, one per (rule × floor × k), same probe file, same build
  (`05dfe7bb`), read-only over the live brain. Grid directory:
  `calibration-run/grid-20261018-100351/`.

## Letter (pinned, issue #271 §7)

`two_stage_passes = ts_hit2 ≥ ss_hit2 && ts_fp ≤ ss_fp` (equal boundary passes).

## Results (chunk-hop arm, k=8)

| floor | hit@2 | FP (of 143) | opens | letter |
|---|---|---|---|---|
| 0.30 | 0.571 | 131 | — | FAIL |
| 0.35 | 0.571 | 127 | — | FAIL |
| 0.40 | 0.571 | 109 | — | FAIL |
| 0.45 | 0.429 | 86 | — | FAIL |
| 0.50 | 0.286 | 44 | — | FAIL |
| 0.55 | 0.143 | 16 | — | FAIL |
| 0.60 | 0.143 | 4 | — | PASS (tie on both clauses) |
| **0.65** | **0.143** | **0** | **1** | **PASS — winner (lower FP)** |
| 0.70 | 0.000 | 0 | 0 | FAIL |
| 0.75 | 0.000 | 0 | 0 | FAIL |
| 0.80 | 0.000 | 0 | 0 | FAIL |

The restricted-cosine arm (floors 0.30–0.40, measured first) fails at every floor: it
opens on 109–143 FP probes. Per the Fable course-of-action ruling (2026-10-18), the arm
is structurally barred from passing except degenerately at floor 0.70 (its open set is a
superset of single-stage's) and was excluded from the extended grid with Kurt's re-pin
approval.

## Winner

**chunk-hop @ floor 0.65, k=8**: hit@2 0.143 — equal to baseline (the letter's exact
boundary) — with FP **0** vs baseline 4 and 1 open vs baseline 5. The gate finds the
same relevant facts while opening strictly less often. The retrieval upside measured at
lower floors (hit@2 0.571 at 0.30–0.40) is deliberately traded away to keep FP at zero;
revisiting that trade-off is deferred until the relevant-probe label set grows beyond 7.

## Post-flip obligations (runbook output, Task 15/16)

Freeze snapshot committed beside this doc's floor row
(`src-tauri/tests/fixtures/wisdom_gate_bench/external_qwen_qwen3-embedding-4b_two-stage_chunk-hop/`,
expected.json + vectors.json.gz); `replay_two_stage` un-ignored. The shipped tree flips
to the two-stage default only when this lands via the flip PR.

## Known limitations

- hit@2 moves in steps of 0.143 (7 relevant probes); the held-out splits are
  informational only. Growing the adjudicated relevant set is the "performance later"
  follow-up (Fable ruling residual risk).
- 33 of the 143 FP probes carried Oct-7 `relevant` prelabels with empty final `expect`
  sets — the FP denominator likely overstates real-world harm.
