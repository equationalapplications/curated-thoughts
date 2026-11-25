# CT PR #369 — impl-review round 3 (fresh CodeRabbit round on 2294df04, 2026-11-25 ~01:20Z)

All findings verified against the tree at `2294df04` before adjudication. 7 findings: 6 REAL + 1 adjudicated REAL after re-trace (rOZe initially looked FP; the resolver's lowercase made it real — see below). 0 FP.

| Thread | Severity | Finding | Verdict | Fix |
|---|---|---|---|---|
| PRRT_kwDOSVmXas67rOZU | MAJOR | Backup reservation: `create_new` failure proceeds to `VACUUM INTO` on the winner's path; the loser's failure-cleanup can delete the winner's completed backup (`reserved` true only for the winner) | REAL (verified connection.rs:1529-1563) | Bail when `create_new` fails; cleanup unconditional on the owned path |
| PRRT_kwDOSVmXas67rOZa | MAJOR | `sso9_declared_edges` checks the entity's own manifest row only; the gate cascade (`resolve_strict_edge_vocabulary_with`, commit.rs:398-428) inherits the `tier_fact` partition manifest — "the typical production case". Inherited-declared parent edges were deleted | REAL (verified commit.rs cascade + dedicated test `edge_fact_endpoints_inherit_owning_entitys_strict_gate`) | Mirror the two-leg cascade (entity strict → tier_fact strict → None) in `sso9_declared_edges` via `wiki_get_ontology` |
| PRRT_kwDOSVmXas67rOZe | MINOR | Declared-shape check mispredicts mixed-case endpoints: `sso9_resolve_type` lowercases BEFORE the retype map, so a stored `Handoff` target predicts `decision` — but the SQL retype matches EXACT slugs and leaves `Handoff` in place. Kept edge lands on an undeclared shape | REAL (premise re-verified: resolver tail line 1747 `.map(|t| t.trim().to_lowercase())`; SQL at 2094+ exact `WHERE okf_type = ?1`; mixed-case rows deliberately in fixture `hv`='Handoff', retype scope pinned by `entity_types_rewrite_only_the_two_dropped_slugs` + assertion `okf(hv)=='Handoff'`) | Resolve the RAW stored type in the parent-shape path; apply the map to raw, then normalize for lookup. Classification sites keep the lowercased resolver. Exact-slug retype scope NOT touched |
| PRRT_kwDOSVmXas67rOZh | MINOR | Backup-count guard armed only on drops; a remap-eligible row committed in the backup→lock window reaches the dedupe DELETE with no backup copy | REAL (plan capture is inside BEGIN IMMEDIATE — the only window is backup→lock) | Subsumed by the per-row backup content check (below) |
| PRRT_kwDOSVmXas67rOZt | MAJOR | Count equality misses equal-count drift (late delete-one + insert-one): the inserted row lands in a drop/remap set and is deleted with no backup copy | REAL | Replace the count guard with per-row content verification: every planned destructive row (remap + service_drop + parent_drop) must exist in the backup with identical (entity_id, source_id, target_id, edge_type); missing/changed → abort unapplied, fresh backup on retry |
| PRRT_kwDOSVmXas67rOZx | MINOR | Test asserts `t.0 == "handoffFor"` but `refresh_edge_triple` returns `NodeVocabulary::key`-normalized values — the retired-edge guard can never fire | REAL (mechanical) | Compare against `NodeVocabulary::key("handoffFor")` |
| PRRT_kwDOSVmXas67rOZz | MINOR | `edge_types: null` passes eligibility (17 seed slugs are all node types), the missing-key init is skipped, `as_array_mut()` on Null skips the E2 merge wholesale → stamped manifest lacks all four required shapes | REAL (verified: eligibility seeds all node slugs; `parse_manifest_value` reads any non-array as empty) | Normalize any non-array `edge_types` (missing, null, scalar) to `[]` before the merge — the exact set of forms the reader treats as empty |

## RED evidence (before fixes)
4 new tests, all failing against `2294df04` + tests-only commit:
- `sso9_retype_equal_count_late_swap_aborts_the_retype` (rOZt)
- `sso9_retype_remap_only_late_write_aborts_the_retype` (rOZh)
- `sso9_retype_keeps_parent_edges_declared_by_the_inherited_partition_manifest` (rOZa)
- `sso9_retype_parent_shape_mixed_case_target_is_predicted_as_stored` (rOZe)

Existing `sso9_retype_late_edge_write_between_backup_and_delete_aborts_the_retype` re-covered by the per-row check (insert-only case → planned row missing from backup); its message assertion updated with the fix commit.

## Adjudication notes
- rOZa preserves the explicit-owner strict match (leg 1 wins) and keeps genuinely-undeclared drops; the standard fixture's expectations are unchanged (the refreshed tier_fact manifest declares no `author(decision, …)` shape — verified against `vendored/schema-software-org-9.18.0.manifest.json`).
- rOZe: CodeRabbit's proposed remedy wording ("map before normalizing, exact lowercase slugs still map") == implemented; the alternative remedy direction (case-insensitive SQL retype) was REJECTED — it contradicts the pinned exact-slug scope.

## T1.5 rung 1 verdict (claude-haiku-5-5, review-depth high, full PR scope @ 16bde2d0)

**APPROVE WITH NITS** — 0 BLOCKER / 0 MAJOR / 4 MINOR. Cost **$0.1393** (23 turns).
Raw runner log archived locally only (*.log is gitignored); verdict JSON archived alongside this file.

Nits and dispositions:
1. sso9_shape_check first()-only — **FIXED** (all refreshed rows verified; RED-first test `sso9_shape_check_covers_every_refreshed_row`, verified failing against the old body then green).
2. backup-on-fresh-install — **DISMISSED** (reviewer's own verdict: matches spec S2/S4, design note not defect).
3. VENDORED_PATH cwd-relative — **FIXED** (resolved from the test file; avoids Vite's new-URL asset transform which silently yields `undefined` for dynamic paths; verified from repo root AND from src/).
4. spec status line — **DISMISSED** (branch named IS this PR's branch; AGENTS.md draft rule satisfied: status reads Implemented, commit 0aba3516).

Reviewer "unverified" items, controller-verified: edge_writer_gate connection.rs=7 PASSES; lib 1699/1699 green; clippy --all-targets -D warnings EXIT 0.

## Fresh CodeRabbit round on 16bde2d0 (3 findings — reviewed the round-3 fold) + T1.5 rung 2 (sonnet xhigh)

### CodeRabbit fresh round (02:24Z)
| Thread | Sev | Finding | Verdict |
|---|---|---|---|
| PRRT_kwDOSVmXas67slyA | MAJOR | Unreadable owner manifest falls through to tier_fact and classifies shapes undeclared → irreversible drop | REAL — FIXED: an Err leg now BAILS the re-type before any write (rollback, backup retained, operator repairs); gate fall-through untouched. RED test `sso9_retype_unreadable_owner_manifest_aborts_the_retype` |
| PRRT_kwDOSVmXas67slx2 | Minor | Resolver TRIMS the stored value → `'handoff '` mispredicted as `decision` though the exact-slug SQL leaves it stored | REAL — FIXED: resolver returns stored VERBATIM; classification sites trim+lowercase on their own side. RED test `sso9_retype_parent_shape_trailing_space_target_is_predicted_as_stored` |
| PRRT_kwDOSVmXas67slxp | Minor | Reservation error conflates permission failures with "another opener holds this slot" | REAL — FIXED: only `ErrorKind::AlreadyExists` reports the collision; other errors report destination + underlying error (early return kept) |

### T1.5 rung 2 (claude-sonnet-5-5, review-depth xhigh, effort high @ 70ad5e71) — REQUEST CHANGES
**Cost $0.7316** (29 turns). 1 MAJOR + 3 MINOR:

1. **Ungated entities (MAJOR) — PARTIALLY REAL, FIXED.** The strict-with-empty-`edge_types` scenario is MOOT post-refresh: the E2 merge (owx8 fold + rOZz) fills every eligible refreshed manifest with the four required shapes, so the refreshed gate is ACTIVE there (reviewer missed that interaction — dismissed with receipts). The remaining sub-cases are REAL: no strict manifest on either leg, or a non-eligible manifest with an empty edge vocabulary, leave `declared` empty/None while the live gate is DISARMED (`ontology_leg` Strict(None): "ungated means most permissive") — the re-type then deleted edges the gate admits, irreversibly. FIX: empty declared set OR no applicable manifest ⇒ KEEP (ungated), mirroring the gate. RED tests `sso9_retype_ungated_entity_keeps_parent_family_edges`, `sso9_retype_strict_empty_edge_vocabulary_keeps_parent_family_edges`. Spec change-6 says "UNDECLARED … DROPPED" — an UNGATED shape is not an undeclared shape; interpretation recorded here for Kurt's merge-ask review.
2. **Stale backup accumulation (MINOR) — FIXED.** Success path now prunes stale `<stem>.bak-pre-sso9-<ts>` siblings, keeping the newest 3 (SSO9_BACKUP_KEEP); only the success path prunes. RED test `sso9_success_prunes_stale_backups_keeping_the_newest_three`.
3. **Leading-space stored types (MINOR) — ALREADY FIXED in-tree** by the slx2 fold above (verbatim resolver); the rung reviewed 70ad5e71 before those fixes were pushed. Duplicate — dismissed with receipts.
4. **Receipt .json isn't JSON (MINOR) — FIXED**: renamed to `-verdict.md` (tool writes markdown).

Rung-2 "checked and clean" receipts: eligibility/idempotency, refresh node-set (statement excluded from EA_SEED_TYPES → 20 slugs), ensure-fold memo transactionality, empty-file VACUUM INTO target, unique-collision UPDATE OR IGNORE + per-id DELETE, DDL columns, redirect_scan allowlist, TS vendored-drift + VENDORED_PATH resolution.
