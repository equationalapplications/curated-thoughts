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

## T1.5 rung 3 (claude-haiku-5-5, review-depth max @ b141e476) — REQUEST CHANGES

**Cost $0.1328** (24 turns). 1 BLOCKER + 3 MINOR + 2 open questions:

1. **BLOCKER — about→person/place/event arm hard-codes drops without the ungated/declared consult — FIXED.** Same irreversible-deletion class as the rung-2 MAJOR (ungated and custom-declared brains lose edges the live gate admits; backup removed on success). The arm now routes through `sso9_parent_shape_declared` exactly like the author/publisher/supersedes arms; the ratified default brain still drops (its refreshed manifest declares no about(decision|lesson, person|place|event) shape — verified against the vendored 9.18.0 snapshot). RED-first: `sso9_retype_ungated_entity_keeps_parent_family_edges` extended with the `e_about_person` assertion, verified failing against the hard-coded drop, green after the fix.
2. **MINOR — post-VACUUM sanity count can false-abort under a concurrent write — DISMISSED (design note).** The reviewer's own text concedes fail-closed/no-loss/retry. Downgrading to WARN-keep would weaken the S2/S3 backup-sanity guarantee; retry-once adds paths to a safety-critical step for a millisecond-window flake whose worst case is one delayed open (stale backups now pruned by the rung-2 fold, so no accumulation).
3. **MINOR — a malformed manifest blocks every open — POLICY CONFIRMED WITH OWNER PENDING.** This is the slyA fold working as designed (fail-closed beats irreversible loss); the reviewer calls it defensible and asks for owner sign-off. Surfaced in the merge ask.
4. **MINOR — review artifacts in the product repo — DISMISSED with precedent.** `.hermes/reviews/` is tracked on purpose: the PR228 archive (2026-09-24_pr228-*) established the convention on main.

Open questions (reviewer "could not confirm" → action items):
- **NULL-typed endpoints drop parent edges.** Controller code-truth: the live gate's endpoint ladder resolves endpoints by OWNING ENTITY (climbing to rung 4 = tier_fact; unmarked → None = ungated) — it never reads okf_type NULLness. The migration's unresolved-endpoint `_` arm returns false → drop, which is status-quo-conservative but potentially asymmetric with the gate for gated brains. Recorded as an OPEN QUESTION for Kurt + Opus T3; the retype census log will surface the actual affected count on the live brain before merge. No behavior change without a ruling.
- **Lockfile integrity hash** — unverified by runners; CI's frontend job and the trial-install recorded in the spec cover it.

"Checked and clean" receipts: step ordering/stamps, EA_SEED_TYPES 20-slug rule, TS vendored-drift test, ct_heal seed switch, backup-deleted-only-after-verification.

## Opus T3 (opus, full PR scope @ c772a0fa, seated real clone) — REQUEST CHANGES

**Cost $1.1258** (19 turns; floor ≥$2.5 NOT reached — the review CONVERGED rather than dying: complete verdict, 19 turns, found a MAJOR the depth rungs missed; shortfall disclosed in the merge ask rather than relaunched at 3x).

**1. MAJOR — declared-shape matching is not parent-aware — FIXED.** Premises verified in-tree: `decision`/`lesson`/`rule`/`trap` carry `parent_type: statement`; the statement node says source-`statement` rows "apply to every kind"; spec change-6 itself keeps about→organization as "a valid `statement`→`organization` shape" (parent-aware reasoning). Exact-triple matching therefore under-matched: a custom manifest declaring `about(statement, person)` intends the re-typed `(about, decision, person)` edge, which V32 then deleted irreversibly. FIX: `ManifestDeclaredEdges { triples, parents }` — `contains` expands each endpoint along its one-level parent chain (cycle-guarded) and matches any combination, parents read from the SAME manifest's node_types. RED-first test `sso9_retype_parent_declared_shape_keeps_the_edge` (verified failing under exact-match-only, green after). DEFAULT brain unchanged: the vendored vocabulary declares no about(statement→person) shape, so the ratified about→person/place/event drops still hold (existing standard tests pin this).

**2. MINOR — the ungated-keep's gate comparison doesn't hold — ACCEPTED, wording fixed, RULING PENDING.** Correct: the Rust edge gate checks edge-type NAMES only (EdgeVocabulary::contains), never endpoint shapes, so "the gate would admit this edge" is not the differentiator — every author edge is name-admitted on gated brains too. The ungated-keep is a POLICY choice (shape-level declaration per change-6, with ungated entities exempt by ruling). Code comments corrected to state the real criterion; the spec erratum ("E5") awaits Kurt's ruling at the merge ask — not written unilaterally (AGENTS.md: ask when the spec doesn't answer).

**3. MINOR — NULL okf_type entries never fall back to curated_entities — RULING PENDING (bundled with the rung-3 open question).** The entry-first rule is the resolver's documented contract; NULL endpoints currently drop (conservative). ~1,436 live rows affected in the worst case; the census log surfaces the actual count. Owner to rule: keep drop, or fall back on NULL.

"Checked and clean" receipts from T3: backup reservation + error kinds + prune prefix, per-row backup check covers all three destructive sets, every abort path rolls back including failed COMMIT, cascade legs (Strict(None) ⇒ keep), ALIAS_TABLE extension.

## CI incident on 5e1c61f4 (process lesson)

rust-ubuntu/rust-macos failed: edge_writer_gate found 8 vs expected 7. The 8th fixture INSERT (e_sup_sp) entered with the slx2 fold at b141e476; after that commit the gate was re-verified only via --lib + clippy, which compile but do not RUN the integration gate. Lesson recorded: after EVERY fold commit, re-run edge_writer_gate explicitly alongside --lib. Fixed in 62632da6 (baseline 7 → 8, fixtures only).

## Fresh CodeRabbit round on 5e1c61f4/62632da6 (2 findings, both REAL, both folded)

| Thread | Sev | Finding | Verdict |
|---|---|---|---|
| PRRT_kwDOSVmXas67t93k | MAJOR | A malformed NON-strict owner row aborted the re-type before tier_fact was consulted — `wiki_get_ontology` parses JSON before reading mode, so the gate's own Err leg falls through and the PARTITION manifest governs | FIXED: owner-row Err now falls through (WARN, gate-mirrored); only the APPLICABLE partition leg aborts. Tests: `sso9_retype_unreadable_partition_manifest_aborts_the_retype` (corrupt owner → succeeds via tier_fact; corrupt tier_fact → abort, backup preserved), RED-verified |
| PRRT_kwDOSVmXas67t93c | Minor | The promised NULL-endpoint census doesn't exist — planned_parent_family_drop mixes undeclared-resolved with unresolved endpoints; the 1,436-NULL-row figure is not the affected-edge count | FIXED: `Sso9ShapeVerdict::UnresolvedEndpoint` now counted separately (`parent_drop_unresolved_endpoint`) and reported in the retype log (`parent drops with an unresolved endpoint=N`) — the owner sees the actual affected-edge count at first open |

Process note: the edge_writer_gate baseline was stale twice during this round (7→8→9); root cause each time was re-verifying via --lib + clippy only, which compile but do not RUN the integration gate. Every future fold re-runs the gate explicitly.
