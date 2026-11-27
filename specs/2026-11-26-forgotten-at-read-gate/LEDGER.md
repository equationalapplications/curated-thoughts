# LEDGER — forgotten-at-read-gate (#378, PR #395)

One checkbox per plan task. Evidence link per tick. "What's in flight":
Tasks 0–6 of `plan.md`, serial (cargo discipline). Receipts live in
`.superpowers/sdd/issue-378/` (gitignored scratch).

- [ ] Task 0 — branch sync with origin/main + anchor re-verification (controller)
- [ ] Task 1 — shape probe field + `llm_wiki_events_related_forgotten_idx` mirror + schema guard + RED tests
- [ ] Task 2 — survival-guard arm in `entry_eligibility_where` + parity/survival/carve-out tests
- [ ] Task 3 — bypass closure (rank_wiki_entries, curated_get_wiki_entry, wiki_search_in_scheme, graph probes) via one shared helper
- [ ] Task 4 — event readers (`load_events`, `list_events`) drop forgotten events; queue.rs replay audit
- [ ] Task 5 — starvation + engine-agreement fixtures; full CI-exact suite + clippy closeout gates
- [ ] Task 6 — independent review loop; cleanup commit (spec → Implemented, delete plan/LEDGER); ready flip

## Working notes

- Plan approved by owner before any implementation dispatch (SOP gate).
- CARGO_TARGET_DIR authority: repo AGENTS.md (`<primary>/.cargo-shared-target`,
  absolute). Supersedes the worktree-local convention in older plans.
- queue.rs replay audit result: (pending Task 4).
