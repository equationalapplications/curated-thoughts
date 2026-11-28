# LEDGER: wisdom-brief

| Task | Status | Commit | Notes |
|------|--------|--------|-------|
| 0 docs rev 3 | done | 2c851a8c | Fable delta folded |
| 1 has_area | done | 6ce3fe69 | recall_arms_reports_has_area 1 passed; wisdom_match lib filter 88 passed |
| 2 resolve_brief_scope | done | 474cad82 | hub_resolver 38 passed (28 existing unmodified + 10 brief_scope); test file only appended |
| 3 order/fit | done | 592f6315 | wisdom_brief unit 10 passed (red phase: first run failed to compile, missing fns) |
| 4 index | done | 71f0abdc | wisdom_brief unit 20 passed (10 new index tests) |
| 5 build_brief_at | done | 0c7b474f | wisdom_brief unit 34 passed (14 new incl. repo-wins when org merged into repo) |
| 6 CLI brief | done | 03d79225 | ct_wisdom_brief 14 passed; ct_wisdom_match 26 passed after helper move (seed_fact already in common; moved seeded_fact_with_entity, seed_entity, with_scoped_brain) |
| 7 delivery_log | done | 69b6a327 | delivery_log unit 11 passed; no unwrap/expect outside cfg(test) |
| 8 log brief | done | dc25256f | ct_wisdom_brief 19 passed |
| 9 log match | done | a69b4cfc | ct_wisdom_match 30 passed (26 existing unmodified + 4 new); activity_helper_parity 3; message_mode_output_byte_identical 1 |
| 10 timeline | done | 9ee2250c | timeline_excludes_wisdom_delivery_rows 1 passed; db::events lib filter 15 passed |
| 11 INTENT line | done | 34bc8147 | one added line plus blank separator |
| 12 perf | done | - | scratch brains (fixture facts, no embeddings; throwaway harness deleted), DEBUG ct binary, 20 runs each, log off: n=50 p95 27 ms; n=1000 p95 34 ms; n=10000 p50 130 ms p95 137 ms (gate <= 250). Byte-identical over 20 runs, used_chars <= 2200, zero superseded/expired/forgotten ids at every size |
