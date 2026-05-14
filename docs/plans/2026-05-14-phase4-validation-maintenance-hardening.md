# Phase 4 Validation, Maintenance, and Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Complete Phase 4 by hardening the Curated Thoughts runtime, automating maintenance, validating Code Graph impact radius with integration tests, and profiling search performance under desktop-scale load.

**Architecture:**
- The Rust backend must expose and emit maintenance job state for `heal`, `prune`, and `reindex` operations.
- The file watcher triggers debounced auto-heal for out-of-band file deletions.
- The SQLite brain persists `soft_deleted` records for 7 days, then hard-prunes stale `librarian_inferred` chunks.
- The frontend uses reactive status subscriptions and guarded maintenance controls to keep the UI safe during long-running background work.
- Semantic retrieval includes a capped in-memory vector cache (500 vectors per entity) and a profiling harness to measure query latency before migrating to native vector extensions.

**Tech Stack:** Tauri 2.x, React 19, Rust, SQLite, `@equationalapplications/core-llm-wiki` v4.x, Vitest, `@testing-library/react`, `@tauri-apps/api`

---

## File Structure

### New files
- `docs/plans/2026-05-14-phase4-validation-maintenance-hardening.md` — this implementation plan
- `src/__tests__/brain-prune.test.ts` — regression tests for hard-prune behavior
- `src/__tests__/impact-radius.test.ts` — integration-style test for code graph dependency traversal
- `src/hooks/useMaintenanceStatus.ts` — optional reactive maintenance event hook if needed for Phase 4 UI
- `src/components/settings/AutoMaintenancePanel.tsx` — status + manual maintenance controls if the existing dashboard needs Phase 4 extension
- `src/lib/searchProfiling.ts` — search latency profiling and vector cache cap tooling

### Modified files
- `src-tauri/src/lib.rs` — add or harden `run_wiki_prune`, `run_wiki_heal`, `run_wiki_reindex` commands, ensure status events emit for maintenance jobs, verify source normalization on all path-based commands
- `src-tauri/src/watcher.rs` or watcher module — add debounced auto-heal for file deletions and soft-delete cleanup triggers
- `src/lib/wiki.ts` — add or extend auto-maintenance helpers, prune scheduling, and vector cache caps on read operations
- `src/hooks/useWikiStatus.ts` — extend status model for `prune`/`reindex` if needed and surface maintenance state to UI
- `src/components/settings/MaintenanceDashboard.tsx` — add Prune schedule indicator, hard-delete warnings, and auto-heal status text
- `src/__tests__/wiki.test.ts` — add regression tests covering auto-heal invocation and vector-cache boundaries
- `src-tauri/src/librarian/mod.rs` — ensure `immutable_document` facts are still protected and `runHeal` only removes disconnected ghost notes

---

## Task 1: Validate Code Graph Impact Radius

**Objective:** Confirm that a Fact change resolves all affected Working Memory chunks up to 5 levels deep through the current recursive dependency graph.

- [ ] Add or extend integration tests in `src-tauri/tests/mcp_integration.rs` or the existing Rust suite to cover:
  - a Fact update that propagates through symbol callers/callees
  - expected lists of affected chunks with file and line references
  - 5-level dependency expansion using the existing recursive CTE
- [ ] Add a dedicated regression test in `src/__tests__/impact-radius.test.ts` to validate the JS/Rust boundary if a frontend-facing API is involved
- [ ] Run the integration suite and verify outputs against the expected dependency set

**Verification:** `cargo test --test mcp_integration` and `pnpm test -- impact-radius` pass with assertions for 5-level impact expansion.

---

## Task 2: Harden Database Heal and Prune Automation

**Objective:** Make the database self-healing and prune old soft-deleted inference entries safely.

- [ ] Implement debounced auto-heal in the file watcher module so deleted files trigger `runHeal()` after a short delay
- [ ] Ensure `runHeal()` only removes ghost notes and never touches immutable `Fact` sources
- [ ] Implement `runPrune()` to hard-delete `librarian_inferred` entries that have been soft-deleted for more than 7 days
- [ ] Add a cleanup scheduler or a maintenance entrypoint that can be invoked manually or periodically from the frontend
- [ ] Add tests in `src/__tests__/brain-prune.test.ts` verifying:
  - a `librarian_inferred` row soft-deleted older than 7 days is removed
  - a fresh soft-delete is preserved until the threshold passes
  - `immutable_document` rows are never hard-pruned

**Verification:** backend prune tests pass and manual `runPrune()` behavior is validated.

---

## Task 3: Profile Semantic Search and Cap Vector Memory

**Objective:** Measure current search latency under desktop-scale chunk load and enforce strict vector cache limits.

- [ ] Add profiling utilities in `src/lib/searchProfiling.ts` that:
  - execute representative semantic queries against the current local search stack
  - capture latency for query, vector scoring, and candidate expansion
  - log or surface the results in development mode
- [ ] Implement a 500-vector per entity cap in the local vector cache used during semantic reads
- [ ] Add or extend tests verifying the cache cap and eviction behavior
- [ ] Document whether current latency requires migrating from cosine similarity to a native vector extension such as `sqlite-vec`

**Verification:** profiling reports are available and the cache cap prevents unbounded memory growth in local runs.

---

## Task 4: Production Hardening and Security

**Objective:** Ensure path normalization, UI safety, and trust boundary enforcement are complete for desktop use.

- [ ] Audit path-based Rust commands and file watcher events for source normalization and path injection vulnerabilities
- [ ] Verify the MCP server only accepts trusted local requests and that status event emission does not leak unsafe state
- [ ] Add UI guards in `src/components/settings/MaintenanceDashboard.tsx` or the new maintenance panel so:
  - `runPrune` is clearly described as permanent
  - `Change Vault` and destructive actions are disabled when the system is busy
  - auto-heal status is visible when watcher repairs are pending or running
- [ ] Ensure the `wiki-status-change` event payload includes `ingesting`, `librarian`, `heal`, and `prune` if the UI needs it
- [ ] Add tests or runtime assertions for `wiki-status-change` event correctness if not already covered

**Verification:** UI busy-state gating and source normalization checks are in place; no path-based command can operate on an unsanitized vault path.

---

## Task 5: Release and Documentation

**Objective:** Finalize Phase 4 with docs and release-ready notes.

- [ ] Add or update `docs/superpowers/specs/2026-05-13-phase4-three-tier-memory.md` summary notes if the plan diverges
- [ ] Document the new maintenance workflow in `README.md` or `CHANGELOG.md` as needed
- [ ] Confirm the final branch state is stable after tests

**Verification:** new plan file exists in `docs/plans`, and the Phase 4 behavior is documented clearly for future work.

---

## Deliverables

| Feature | Component | Purpose |
| --- | --- | --- |
| Impact radius validation | `src-tauri/tests/mcp_integration.rs` | confirms recursive dependency graph reach |
| Auto-heal from watcher | `src-tauri/src/watcher.rs` | removes ghost notes after external deletions |
| Hard prune retention | `src-tauri/src/lib.rs` | keeps `brain.db` lean by removing old soft-deletes |
| Vector cache cap | `src/lib/searchProfiling.ts` | limits RAM to 500 vectors per entity |
| Reactive maintenance status | `src/hooks/useWikiStatus.ts` | keeps UI safe during maintenance jobs |
| Maintenance UI | `src/components/settings/MaintenanceDashboard.tsx` | displays and controls maintenance operations |

---

### Notes

- The plan assumes Phase 3 infrastructure (`useWikiStatus`, MaintenanceDashboard, auto-heal wiring) is already implemented.
- If this repo uses a different naming convention for backend commands or watcher modules, adapt the file references accordingly.
- Keep Phase 4 changes narrowly focused on validation, maintenance automation, profiling, and hardening to avoid scope creep.
