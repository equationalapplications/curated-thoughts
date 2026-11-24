# LEDGER: engine 9.x adoption — PR-A (schema-software-org 9.x readiness)

Transient scaffolding — deleted in the same-PR cleanup commit.
Plan: `specs/engine-9x-adoption/plan.md`. Spec: `specs/engine-9x-adoption/spec.md` (Approved 2026-11-24).
Receipts dir: `.superpowers/sdd/engine-9x-pr-a/receipts/`.

**In flight:** Q1–Q13 ruled (see table); spec errata §8 committed in this PR; plan-review ladder next before Task 1.

## Baseline (39fc129f)

- [x] Baseline gates green — tools 61, src-tauri 1642, vitest 619 + 1 skip, tsc clean. Evidence: `receipts/baseline-39fc129f.log`, `receipts/baseline-*.out`

## Open-question rulings

| Q | Ruling | By / date |
|---|---|---|
| Q1 seed count 20 vs 21 | **20** — spec E1 errata; binding rule (statement NOT added) governs | controller, 2026-11-24 |
| Q2 `partOf` in MERGE | **Include** — spec E2 errata; new shapes = 3 `about` + `partOf`; drift test covers all four | controller, 2026-11-24 |
| Q3 `curated_entities.entity_type` | **Re-type the two dropped slugs only**, same mapping (handoff→decision, session_recap→lesson) — spec E3 errata; live census handoff ×1 / session_recap ×5; concept/process/repository/job out of scope | controller, 2026-11-24 |
| Q4 endpoint type resolver | **Accept proposed** — entry-id first, else `curated_entities.entity_type`, else `unresolved` census bucket untouched | controller, 2026-11-24 |
| Q5 undispositioned edge classes | **Accept proposed** — untouched + logged count | controller, 2026-11-24 |
| Q6 slot numbering via consts | **Accept** — implementation-time named consts; spec E4 errata records the literal-stamp convention | controller, 2026-11-24 |
| Q7 cargo target location | **AGENTS.md wins**: `CARGO_TARGET_DIR=<repo-root>/.cargo-shared-target` (absolute), ≥30G pre-flight (root 176G free). Baseline receipts on /mnt/kv-work remain valid evidence; implementation gates run on the canonical dir | controller, 2026-11-24 |
| Q8 tarball path / entry count | **Author is right on both** — controller pin corrected (21 entries / 21 slugs; controller's 28 was a parent_type regex artifact); copy is `sso-9.18.0-tarball/dist/index.js` | controller, 2026-11-24 |
| Q9 shared `node_modules` | **Accept shared symlink** for the trial install; NEW cleanup step in T6: after merge, `pnpm install` in the main checkout reconciles its node_modules with the merged lockfile | controller, 2026-11-24 |
| Q10 planner epoch bump | **Include** — code's own consistency rule (`entity_gate.rs:1078-1085`) | controller, 2026-11-24 |
| Q11 spec chain-state text | **Accept** — harmless; spec E4 errata | controller, 2026-11-24 |
| Q12 MERGE eligibility | **Accept** — "the pattern" = passes the named 17-slug EA subset check; foreign manifests untouched | controller, 2026-11-24 |
| Q13 runtime disk check | **Accept** — runtime pre-check inside the step, failing as a backup failure | controller, 2026-11-24 |

## Tasks

- [ ] T1 — sso 9.18.0 pin + trial install + lockfile locked; 8.0.2 and 9.18.0 manifest snapshots vendored; drift test green; manifest diff recorded. Evidence: _link_
- [ ] T2 — ontology-packages.test.ts pins the sso 9.x contract (statement kinds, area, about); ontology.ts copy → "Specs, statements, services"; vitest + tsc green. Evidence: _link_
- [ ] T3 — EA_SEED_TYPES re-pinned from installed 9.18.0 manifest (count recorded); ALIAS_TABLE handoff→decision, session_recap→lesson; planner epoch bumped; onboard copy → statements; sweep dispositions recorded. Evidence: _link_
- [ ] T4 — migration step 1 (slot N, computed) MERGE-refreshes 8.x EA manifests from the vendored 9.18.0 snapshot; backup/abort/in-memory-skip/dry-pass/idempotency each pinned by a test; ensure re-run after commit. Evidence: _link_
- [ ] T5 — migration step 2 (slot N+1) re-types entries+tasks+touched curated_entities (tombstones included) handoff→decision, session_recap→lesson; handoffFor→about via UPDATE OR IGNORE + dedupe; service and undeclared parent-family edges dropped with counts; safety + idempotency pinned. Evidence: _link_
- [ ] T6 — all gates green vs baseline (counts recorded); frontend build smoke passes under 8.0.2 override; e2e fixture re-grep clean; PR-body inputs drafted; disk sweep recorded. Evidence: _link_

## Recorded facts (filled during implementation)

- Slot N (refresh) / N+1 (re-type): _pending — computed in T4a_
- Derived EA seed list and count: _pending — T3 step 1_
- 8.0.2 → 9.18.0 manifest diff: _pending — T1 step 8_
- Sweep disposition table: _pending — T3 step 9_
- PR-body inputs (§12 divergence, unmet core range, rulings): _pending — T6 step 5_
