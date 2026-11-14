# LEDGER — working-memory-age

One checkbox per plan task. Evidence link per tick. "What's in flight": Tasks 1-6 of `plan.md`, serial (cargo discipline).

- [x] Task 1 — `ingest.folder_ages` config key (serde, known keys, salvage, tie diagnostic larger-wins, `age_days_for`) + TDD tests — **b18b1be1** (unit tests in `src-tauri/src/config/mod.rs`; `logs/w1-config.log`)
- [x] Task 2 — `ct_folder_age_days` scalar function + `register_ct_functions` + rusqlite `functions` feature + unit tests — **89f98b68** (`src-tauri/src/ct_sql_functions.rs`; `logs/w2-ctsql.log`)
- [x] Task 3 — shared WHERE fragment in both SQL twins + `semantic_search` + `related_chunks`; shape assertions; parity; empty-config byte-identity test — **83c21bb1** (`logs/w3-search.log`, `logs/w3-shape.log`, `logs/w3-shared.log`)
- [x] Task 4 — registration at every filtered consumer + completeness test + behavior tests (hide/reappear, pre-V10 NULL arm) — **d6f241de**; `src-tauri/tests/ct_sql_registration.rs` (6 entry points on fresh connections + reversibility + pre-V10 arm, `logs/wma-tauri-newtests.log`), `tools/tests/ct_age_registration.rs` (recall_chunks + MCP-bin SQL, `logs/wma-tools-test2.log`); full gates: `logs/task4-tauri-full2.log` (lib 1564 passed), `logs/task4-tools-full.log` (TOOLS_EXIT=0)
- [x] Task 5 — backdating fixture + hit-rate gate (stored embeddings; LongMemEval battery controller-side or `benchmark-pending`) — **9220850e**; `src-tauri/tests/ct_hit_rate_gate.rs` + `docs/benchmarks/2026-11-13-working-memory-age-hit-rate.md`: hit@5 0.000 → 1.000 (+1.000), aged rows across full result sets (≤50/query) 320 → 0 (top-5-restricted: 40 → 0), unaged ranking byte-identical
- [ ] Task 6 — status flip + cleanup commit + post-session disk sweep (controller-owned)
