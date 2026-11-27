# LEDGER — forgotten-at-read-gate (#378, PR #395)

One checkbox per plan task. Evidence link per tick. "What's in flight":
Tasks 0–6 of `plan.md`, serial (cargo discipline). Receipts live in
`.superpowers/sdd/issue-378/` (gitignored scratch).

- [x] Task 0 — branch sync with origin/main + anchor re-verification (controller) — **843b334a**
- [ ] Task 1 — shape probe field + `llm_wiki_events_related_forgotten_idx` mirror + schema guard + RED tests — **63b50d60** (controller-verified: targeted 2/2, clippy exit 0, full lib **1775/0** = baseline 1773 + 2 new; push readback origin==HEAD ✓; fixture hand-edit accepted — matches generator format, drift gate green; interim `#[allow(dead_code)]` on the probe field to be removed in Task 2). SoR rung UNAVAILABLE — Jev HTTP 401, credential present (len 108) but rejected = stale/revoked; flagged to owner; non-binding rung, ladder proceeds without it per charter.
- [x] Task 2 — survival-guard arm in `entry_eligibility_where` + parity/survival/carve-out tests — **9e94debb** (controller-verified: forgotten filter 6/0, pre_v19 1/0, clippy exit 0 with dead_code allow REMOVED, full lib **1780/0** = 1775 + exactly 5 new; push readback origin==HEAD ✓; `forgotten_entry_arm(t)` landed as the single arm definition per ruling R2; two_stage.rs +18 is test-only candidate_set parity per plan step (a); OQ5 attestation carve-out correctly absent per spec default). SoR rung still UNAVAILABLE (Jev 401, flagged to owner).
- [ ] Task 3 — bypass closure (rank_wiki_entries, curated_get_wiki_entry, wiki_search_in_scheme, graph probes) via one shared helper
- [ ] Task 4 — event readers (`load_events`, `list_events`) drop forgotten events; queue.rs replay audit
- [ ] Task 5 — starvation + engine-agreement fixtures; full CI-exact suite + clippy closeout gates
- [ ] Task 6 — independent review loop; cleanup commit (spec → Implemented, delete plan/LEDGER); ready flip

## Working notes

- Plan approved by owner before any implementation dispatch (SOP gate).
- CARGO_TARGET_DIR authority: repo AGENTS.md (`<primary>/.cargo-shared-target`,
  absolute). Supersedes the worktree-local convention in older plans.
- queue.rs replay audit result: (pending Task 4).
