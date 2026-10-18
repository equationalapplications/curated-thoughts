# RULINGS THAT BIND — Fable gate-course ruling, 2026-10-18

**Authority chain**

1. **Kurt VanDusen (principal, Equational Applications LLC), 2026-10-18, Discord:** "I authorize a special run of the Fable reviewer to determine the best course of action for CT for our give situation. Please allow Fable to browse the repo freely and give it good context on our intentions, issues, and open questions." — this is the delegation for THIS ruling. You (Fable) are the designated adjudicator for this question.
2. Controller: Tessera (Equational's executive officer agent) will execute the determination you deliver, and present it to Kurt as the recommended course. **Kurt remains final approver** — the determination binds the controller's recommendation and next actions, not Kurt's veto.
3. Pre-existing binding rulings that still stand unless your determination explicitly supersedes them WITH reasons (Kurt retains veto on all):
   - The pinned acceptance letter (Kurt, 2026-10-09): "two-stage passes if it is not worse than single-stage: hit@2 ≥ single-stage hit@2 AND FP ≤ single-stage FP. No absolute performance floor." — Kurt explicitly dropped the earlier ≥0.30 hit@2 bar: "throughput now, performance later" (backlog directive).
   - Calibration methodology was pre-pinned before the run (probe set sha256 `92942cff…`, grid = 2 rules × floors 0.30/0.35/0.40 × k=8, baseline `external:qwen/qwen3-embedding-4b` raw @ 0.70) — changing the grid or letter is a RE-PINNING decision that has historically required Kurt's approval; your ruling may recommend it, and the controller will route it to Kurt with your reasons attached.
   - Issue #265's contract: `ct wisdom match` must deliver relevance-gated, capped, deduped wisdom to integrations (per-turn limits bound injection volume; `--max` default 2; corrections always delivered). The per-turn caps are a real product control, not hypothetical.
4. **Live-probe evidence outranks doc interpretation.** The calibration artifacts under `calibration-run/grid-20261018-100351/` in this repo are ground truth; re-derive anything you doubt from them. The reviewer tools include Read/Grep/Glob/git over this entire checkout — you are authorized and expected to browse the code (`src-tauri/src/wisdom_match.rs`, `src-tauri/src/two_stage.rs`, `tools/src/bin/calibrate_two_stage.rs`), the artifacts, and git history freely.

**What this ruling request is:** a determination of the best course of action (not a document review). Do NOT return review findings on the documents' prose. Return the deliverables the ruling request lists, exactly.
