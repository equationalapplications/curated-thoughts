# RULING request — best course of action for the CT wisdom gate after the 2026-10-18 live calibration

**This is a RULING request, not a review.** Read `docs/superpowers/rulings/2026-10-18-authority-fable-gate-course.md` (RULINGS THAT BIND) first — it carries the authority chain and the constraints. Do not return review findings; return the determination deliverables at the end.

## Provenance

- Requested by Kurt VanDusen (principal), 2026-10-18, after the live calibration below. Controller: Tessera.
- Repo: `curated-thoughts` (this checkout). Issue lineage: #265 (`ct wisdom match` contract, CTI live-delivery prerequisite) ← #271 (two-stage retrieval, shipped in 3.6.0 via PR #283, CLOSED) ← PR #270 (single-stage mechanism, shipped as-is).

## The situation (all numbers below are live receipts from today's runs)

**What shipped (3.6.0, live brain):** the wisdom gate's fact-layer matching was replaced by two-stage retrieval — semantic search over immutable-source chunk embeddings, then a hop to curated facts via `librarian_evidence` content hashes. The 2026-10-07 paired calibration had shown the OLD single-stage fact-card matching does not transfer to live traffic ("no floor yields usable hit@2 at FP ≤ 0.05").

**Bootstrap:** the #271 verify-scratch bootstrap ran on the live brain today and passed after a cleanup (48 hash-orphaned wisdom entries removed via Kurt-approved `ct wiki forget`; see issue #286 for the tooling gap it exposed). Stamp v1 written, `embed_key` backfilled on 320/320 docs, coverage 43/43, evidence refs 504/504 resolve, hops 333/333 resolve. This state is permanent and healthy.

**The grid (today, live brain, read-only paired runs):** 150 real-traffic probes (7 relevant with adjudicated `expect` sets, 143 FP; GLM-judged + Opus-adjudicated labels; probe file pinned sha `92942cff…`). Baseline = currently shipped default: single-stage, `external:qwen/qwen3-embedding-4b`, raw scheme, floor 0.70.

| cell | two-stage hit@2 | two-stage FP (of 143 FP probes) |
|---|---|---|
| restricted-cosine @ 0.30 | 0.571 (4/7) | 143 |
| restricted-cosine @ 0.35 | 0.571 | 141 |
| restricted-cosine @ 0.40 | 0.571 | 134 |
| chunk-hop @ 0.30 | 0.571 | 131 |
| chunk-hop @ 0.35 | 0.571 | 127 |
| chunk-hop @ 0.40 | 0.571 | 109 |
| **baseline single-stage @ 0.70** | **0.143 (1/7)** | **4** |

**Letter (pinned):** ts_hit2 ≥ ss_hit2 AND ts_fp ≤ ss_fp. Every cell FAILS on FP (best 109 vs 4) while passing hit@2 4× over. `pick` refused; **shipped default stays; no flip**. Artifacts: `calibration-run/grid-20261018-100351/*.json` + `pick-report.json` (in this repo, untracked).

**Shape of the failure:** retrieval is no longer the problem — two-stage finds the right facts for relevant probes 4× better than baseline at every floor. The problem is gate discrimination: chunk-level cosine over real in-domain traffic sits above every floor in the pinned 0.30–0.40 band for 76–100% of irrelevant messages. FP falls monotonically as the floor rises (143 → 109); the pinned grid deliberately stopped at 0.40. Note the FP probes are REAL EA/CT-adjacent traffic, not off-topic spam — "irrelevant" here means "no labeled fact matches", not "uninteresting".

**Caveats you should weigh:** only 7 relevant probes survived label adjudication (hit@2 moves in steps of 0.143); FP count is the gate-OPEN decision, which is a conservative proxy — the delivery product (`ct wisdom match` per #265) caps injection at `--max` (default 2) per turn, dedups against caller context, and always delivers corrections regardless of match.

## The open question

**What is the best course of action for CT's wisdom gate now?** The options on the table (you may determine an ordering, a combination, or a different course entirely):

- **A — Extend the grid above 0.40** (e.g. 0.45/0.50/0.55/0.60): mechanical, cheap (each cell ≈ 2.5 min; re-pin of methodology required from Kurt). The FP trend suggests the letter's FP clause may first become satisfiable somewhere up-band — but hit@2 may decay with floor too, and nobody has measured above 0.40.
- **B — Accept the negative result.** Shipped default stays; document; proceed to issue #282 (wisdom "activity query mode", the last new build on the board). Treat gate flip as blocked pending a better discrimination mechanism.
- **C — Product reframe:** flip to two-stage anyway on the argument that per-turn caps + dedup bound the harm of a high open rate, and re-pin the letter accordingly. (Requires Kurt to change his pinned letter — the current letter forbids it.)
- **D — Decouple ranking from gating:** two-stage determines WHICH facts are candidates (ranker, where it is 4× better); the OPEN decision stays on the shipped single-stage floor. Investigate whether `ct wisdom match` can serve capped, ranked candidates from the two-stage pool while the gate/open-rate stays at baseline levels.
- **E — Fix FP anatomy first:** before more floors, inspect WHICH 109–143 FP probes open and on which facts (per-probe audit fields exist in the artifacts). If opens cluster on a few facts or on a score-distribution artifact (e.g. floor semantics vs score scale, scheme mismatch, or the freshly rebuilt librarian stock), a targeted fix may dominate any floor choice.
- **F — Something else you identify** from the code/artifacts.

## Constraints

- Kurt's letter is "not worse than baseline" — relative, no absolute bar; he explicitly dropped absolute performance bars ("throughput now, performance later").
- Any grid/letter re-pin routes to Kurt with reasons; the ruling recommends, Kurt approves.
- The delivery product (per #265, in `curated-thoughts-integrations`) ships first against a faked `ct`; only its e2e run waits on this gate.
- Cost/time realism: a Fable-run calibration cell costs nothing (local embeds via OpenRouter, ~2.5 min/cell); engineering time is the scarce resource. CT is one repo among several active ones.

## Deliverables (exactly these, in order)

1. **Determination:** the single best course (or an explicit ordered sequence, e.g. "E then A with band X, then re-evaluate"), with the reasoning that survives the caveats above.
2. **Concrete parameters:** if the course involves floors/grid/letter changes — exact floor set, acceptance criterion (letter wording), and what evidence would stop the course early.
3. **Implementation consequence:** what the controller executes next, concretely (commands/tests/specs), and what NOT to do.
4. **Residual risk:** what the determination does NOT fix, and the failure mode that would falsify it.

You have full Read/Grep/Glob + git access to this checkout — verify anything you doubt against `calibration-run/grid-20261018-100351/`, `src-tauri/src/wisdom_match.rs`, `src-tauri/src/two_stage.rs`, `tools/src/bin/calibrate_two_stage.rs`, and git history. If you need material you cannot reach, say so explicitly in the determination rather than guessing.
