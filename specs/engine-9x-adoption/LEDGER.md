# LEDGER: engine 9.x adoption — PR-A (schema-software-org 9.x readiness)

Transient scaffolding — deleted in the same-PR cleanup commit.
Plan: `specs/engine-9x-adoption/plan.md`. Spec: `specs/engine-9x-adoption/spec.md` (Approved 2026-11-24).
Receipts dir: `.superpowers/sdd/engine-9x-pr-a/receipts/`.

**In flight:** LADDER CLOSED 2026-11-24 — APPROVE-WITH-NITS (rung 10 PASS $0.34; V1 MINOR folded controller-side). Ladder spend $9.55; plan total with author $11.73. PLAN CONVERGED. **Draft PR #369 opened** (chore/sso-9x-readiness → main; carries spec errata + plan + LEDGER; body records the 4 divergences). IMPLEMENTATION WAVE starting: Tasks 1+2 as one commit (T1 first for RED, T2 re-pins), then 3–6 per task.

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

- [x] T1 — sso 9.18.0 pin + trial install + lockfile locked; 8.0.2 and 9.18.0 manifest snapshots vendored; drift test green; manifest diff recorded. Evidence: commit 9e3bca1a; receipts/t12-reconcile.out, t12-gates.out, t12-vitest-ontology-red.out; controller re-verified: vendored JSON deep-equals tarball manifest, full vitest 0 fail (625+1skip), tsc clean
- [x] T2 — ontology-packages.test.ts pins the sso 9.x contract (statement kinds, area, about); ontology.ts copy → "Specs, statements, services"; vitest + tsc green. Evidence: commit 9e3bca1a (same commit per §0.1 sequencing); controller re-ran vitest (exit 0) + typecheck (exit 0)
- [x] T3 — EA_SEED_TYPES re-pinned from installed 9.18.0 manifest (count recorded); ALIAS_TABLE handoff→decision, session_recap→lesson; planner epoch bumped; onboard copy → statements; sweep dispositions recorded. Evidence: commit 88165341; controller verified: RED-first receipt in transcript (exit 101), entity_gate lib 56 pass (incl. 2 new alias tests), ct_heal 16 pass, tools lib 61 = baseline; pin test asserts len 20; epoch r26→r27; ct_heal fixture via tauri_app_lib::db::entity_gate::EA_SEED_TYPES; T3 files rustfmt-clean (repo-wide fmt drift pre-existing, not a CT gate). Note: t3 receipt files not written (sandbox blocked redirects into repo); transcript is the record
- [x] T4 — migration step 1 (slot N, computed) MERGE-refreshes 8.x EA manifests from the vendored 9.18.0 snapshot; backup/abort/in-memory-skip/dry-pass/idempotency each pinned by a test; ensure re-run after commit. Evidence: commit 390dde93 ($1.43/68 turns); slot N=31/N+1=32 named consts in connection.rs; sso9 filter 16 ✓, entity_gate 65 ✓, db::connection 68 ✓; controller synced origin/main (merge, clean; #368 fix + 3.14.x releases; no chain movement → slots stand) and re-ran sso9 filter ✓ + FULL src-tauri lib 1662 ✓ 0 fail on the merged tree. Deviations accepted: gates ran bare cargo with --target-dir/--config flags (wrapper grant shape denied; memory-equivalent flags used); no RED receipt (denials pre-code; first run 11✓/5✗ all test-harness defects, production code unchanged by the fixes); stray 5G target/ at repo root REMOVED by controller after lsof-clear; receipt t4-gates.out hand-written
- [x] T5 — migration step 2 (slot N+1) re-types entries+tasks+touched curated_entities (tombstones included) handoff→decision, session_recap→lesson; handoffFor→about via UPDATE OR IGNORE + dedupe; service and undeclared parent-family edges dropped with counts; safety + idempotency pinned. Evidence: commit c772171d ($1.59/54 turns); controller re-ran both gates: sso9 filter 41 ✓ (16→41 with the 5c battery), db::connection 93 ✓; full 5c battery incl. (c-step2)/(c-step2b) firing forms; Task 4e backup tests rewritten per step 11 (success-path existence no longer asserted); trigger-coverage checked (8.0.2 has only FTS triggers on entries, not on okf_type — re-checked by a test). Deviations accepted: no RED-first receipt (impl+tests first-run green; firing tests prove the verifiers fail); heavy-build denied → bare cargo with explicit flags; receipt t5-gates.out hand-written
- [x] T6 — all gates green vs baseline (counts recorded); frontend build smoke passes under 8.0.2 override; e2e fixture re-grep clean; PR-body inputs drafted; disk sweep recorded. Evidence: commit (this closeout); receipts/t6-gates.out; see Recorded facts

## Recorded facts (filled during implementation)

- Slot N (refresh) / N+1 (re-type): **N=31 / N+1=32** (named consts in `connection.rs`; no chain movement after the origin/main merge)
- Derived EA seed list and count: **20** — action, area, creativework, decision, design_spec, event, lesson, organization, person, place, procedure, product, project, reference_doc, review, role, rule, service, software_application, trap (`entity_gate.rs` `EA_SEED_TYPES`; spec E1)
- 8.0.2 → 9.18.0 manifest diff: see T1 evidence (commit 9e3bca1a, `receipts/t12-reconcile.out`); new edge shapes = 3 `about` + `partOf` (E2); `handoffFor` ×2 shapes leave
- Sweep disposition table: lives in the Task 3 report (commit 88165341); not reproduced here — handoff→decision, session_recap→lesson (spec E3), census handoff ×1 / session_recap ×5; carry the table into the PR body from that report
- PR-body inputs (T6 step 5):
  - §12 divergence: the migration's raw writes to `llm_wiki_entries.okf_type`, `llm_wiki_edges.edge_type` and the dropped `service`-targeted edge DELETEs are the recorded one-time-migration boundary case (engine exposes no bulk re-typing op); no outbox touch, no `updated_at` rewrite, heal stamps untouched (spec §12 disposition). The four `connection.rs` base-table `curated_entities` reads are allowlisted in `redirect_scan.rs` (commit 848bd462) because spec E3 requires tombstone-inclusive reads — same deliberate-reader class as `commit.rs`.
  - Unmet `core-llm-wiki` range: the T1 install emitted NO peer/range warning — the 8.0.2 override satisfies the declared range silently. sso 9.18.0 declares `core-llm-wiki` 9.18.0 exact; the override forces 8.0.2. Accepted recorded state; PR-B resolves it. Lockfile is the review record.
  - Runtime-load evidence for the manifest is the T1 drift/seed-wrapper vitest case; `pnpm build` is bundling evidence only (F8 delta).
  - Rulings: see the "Open-question rulings" table above (Q1–Q13) and spec §8 Errata E1–E4.
  - Post-merge (controller-owned, Q9): `pnpm install` in the main checkout.
- T6 gate results (branch at 848bd462): G-tauri 1687/1687 exit 0 (controller-verified after 848bd462; my earlier run 1686/1687, the redirect_scan allowlist failure, since fixed); G-tools 61 passed; G-heal (ct_heal) 16 passed; vitest 625 passed + 1 skipped, 0 failed; tsc exit 0; `pnpm build` ok (vite built, chunk-size warning only); e2e re-grep (handoff|session_recap|handoffFor|Specs, handoffs) 0 hits.
- Disk sweep: `.cargo-shared-target` 11G; no stray `target/` dirs; `/` 455G total, 159G free (64%); `/mnt/kv-work` 17G free (86% used).
