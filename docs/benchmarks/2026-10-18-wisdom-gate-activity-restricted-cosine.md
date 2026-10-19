# Wisdom gate activity-mode calibration — restricted-cosine @ 0.65, k=8 (2026-10-18)

Live-brain paired calibration of the ACTIVITY composite key (issue #282, spec §7),
executed per the same Task-15/16 runbook discipline as the #271 two-stage flip.
This record justifies the `WISDOM_GATE_FLOORS` row
`("external:qwen/qwen3-embedding-4b:activity:two-stage:restricted-cosine", 0.65)`
added by the flip. k stays on the `TWO_STAGE_K` `default` row (8) — k is not a
floor key, and 8 is both the measured value and the shipped default.

## Campaign

- **Brain:** a Kurt-approved scratch COPY of the live brain
  (`~/ct-calibration-scratch/brain-dir/`, consistent ro `.backup`). 14 facts
  relabeled `user_stated` in the COPY ONLY (13 from pinned ids + 1 by body
  match); the live brain was never written and has zero `user_stated` facts.
  This is a stand-in for Option 2 (in-app attestation), agreed with Kurt.
- **Probe set:** 196 probes — 42 positives (3 target shapes per fact:
  path/command/pattern) + 154 near-miss negatives. Per-kind splits:
  path 18+/56−, command 12+/52−, pattern 12+/46−. Full union
  `probes/activity-all-196.jsonl` (this dir), facts in `activity-facts.jsonl`.
- **Baseline (pinned):** message-mode v1, `ct wisdom match --max 1` @ registered
  raw floor 0.70.
- **Cells:** 24 paired runs — (rule × floor) × 3 kinds × 2 rules × 4 floors
  (0.55/0.60/0.65/0.70), same per-kind probe files, one build. `--query-kind`
  per kind implies: `v1_floor = None` at every pass site, the `user_stated`
  aperture in scoring, `user_stated`-only positives, hit@1 as the gated metric
  (spec rev-7 M2). Original build: `05dfe7bb` (post-#289 main); winning cells
  re-run at build `bad0cdbb91f388428d37b9aebb277a5383780419` with `CT_BUILD_SHA` exported so the flip
  artifact is version-tied to the shipping tree.

## Letter (pinned, issue #282 §7)

`act_hit1 ≥ base_hit1 && act_fp ≤ base_fp` — not worse on recall of the
actionable rule, not worse on false opens, FP counted per target over all
negatives. One shared `:activity` floor across all three kinds (Kurt ruling,
2026-10-18).

## Results (restricted-cosine arm, k=8, at floor 0.65)

| kind | base hit@1 | act hit@1 | base FP | act FP |
|---|---|---|---|---|
| path | 0.111 | 0.278 | 1 | 0 |
| command | 0.083 | 0.250 | 1 | 0 |
| pattern | 0.167 | 0.250 | 1 | 0 |

Per-kind `pick.json` (this dir): path and command pick rc@0.65 outright;
pattern's raw pick is rc@0.60 but rc@0.65 also passes there (hit@1 0.250, FP 0)
— so the single shared floor 0.65 passes every kind's letter. Rule (i) won
every passing grid; the tie-break never had to cross rules. chunk-hop (the
shipped message-mode rule) survives all kinds only at 0.55 (hit@1 0.167–0.250,
FP up to 3 on pattern) — dominated.

Floor-band shape on rule (i): FP 0 up to 0.65; 0.70 collapses hit@1
(0.083–0.167); 0.55 lets pattern FP 3 through. 0.65 is the conservative pick:
the highest floor with hit@1 ≥ 3× baseline everywhere at FP 0.

## Winner

**restricted-cosine @ floor 0.65, k=8** — one row, shared by all three kinds.

## Honesty caveats (repeated from the letter decision)

- Labels are 14 relabeled facts on a scratch copy (Kurt-approved stand-in),
  NOT app attestations; the live brain still has zero `user_stated` facts, so
  activity mode delivers nothing until Option 2 lands. The flip is still
  correct: it installs a calibrated, fail-closed gate.
- Per-kind positives are 12–18 (the ≥30 floor applies to the whole labeled
  set); k fixed at 8; embeds p50 ≈ 32 ms via OpenRouter.
- The letter measures not-worse-than-baseline; the concept-from-path headline
  case may still stay closed (spec §7 honesty clause).

## Post-flip obligations

Freeze snapshot committed beside this doc's floor row
(`src-tauri/tests/fixtures/wisdom_gate_bench/external_qwen_qwen3-embedding-4b_activity_two-stage_restricted-cosine/`,
expected.json + vectors.json.gz over the union probe set + facts);
`replay_activity_two_stage` un-ignored with its key corrected to the winner.

## Known limitations

- hit@1 moves in 1/12–1/18 steps per kind; small label sets, informational
  held-out splits only.
- If Kurt later asks to widen the aperture to vault-author facts, that is a
  spec change (§5) needing his sign-off — not part of this flip.
