# LEDGER: recall-eligibility-provenance

| Task | Description | Status | Commit | Notes |
|------|-------------|--------|--------|-------|
| 1 | Pin `entry_eligibility_where` SQL | Done | 8b49f5e2 | golden pin green on unmodified code; calibrate_two_stage_harness 39 pass |
| 2 | Factor `temporal_arms_sql` | Done | d8b645d9 | wisdom_match filter 74 pass |
| 3 | `entry_recall_arms` public wrapper | Done | e8995e5c | wisdom_match+entry_recall_arms 76 pass; tests seen failing (compile) first |
| 4 | `kind_for` + shared recall item builder | Done | ef77929c | 5 pass (kind_for, kind_tier, confidence_ordinal, recall_item) |
| 5 | App `rank_wiki_entries` | Done | dfe24423 | tool_dispatch 47 pass; message_mode_output_byte_identical unchanged and green |
| 6 | `kind` on `WisdomItem` / corrections | Done | 78127ce0 | wisdom_match+message_mode 86 pass; harness 39 pass; typed-fact pin green |
| 7 | CLI/sidecar `rank_wiki_entries` | Done | (this commit) | ct_recall_eligibility 10, forgotten_read_gate 1, shared_queries 3, mcp bin 1 pass; sidecar test expectation unchanged (derived in comment) |
| 8 | `ct recall` wiki leg on zero chunks | Done | (this commit) | ct_search_recall_code 9 pass |
| 9 | Sweep, docs, cleanup | Done (cleanup pending controller) | (this commit) | app: 1818 lib incl. 1 env-failure; with --no-fail-fast, skipping ddl_compat::rust_llm_wiki_ddl_matches_core_llm_wiki_package (needs node_modules; worktree has none): 2163 pass / 0 fail; tools: 299 pass / 0 fail; clippy -D warnings clean (both crates); cargo fmt --check not clean repo-wide at base, not a CI gate |
