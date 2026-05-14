# Phase 3 Implementation Plan

**Date:** May 13, 2026
**Scope:** React & Desktop Optimization for Phase 3
**Folder:** `docs/plans`

## Objectives

Phase 3 will finish the React frontend and desktop integration for Curated Thoughts by:

- adding live backend job status and reactive UI locking,
- replacing standard search with a tiered, graph-aware reactive memory read,
- ensuring emoji-safe chunk rendering in review flows,
- adding maintenance controls for healing/pruning/re-indexing,
- and implementing backend GDPR/security cleanup commands.

## Plan

### 1. Verify current baseline

- Review existing Phase 1/2 implementation of:
  - `src/hooks/useWikiStatus.ts`
  - `src/components/settings/MaintenanceDashboard.tsx`
  - `src/components/settings/SettingsModal.tsx`
  - `src/__tests__/useWikiStatus.test.ts`
  - `src/lib/wiki.ts`, `src-tauri/src/*` for command wiring
- Confirm whether `subscribeEntityStatus`, `runPrune`, `forget`, `runHeal`, and watcher auto-heal are already partially implemented or need new work.

### 2. Implement live status subscription

- Create or enhance `src/hooks/useWikiStatus.ts`
  - subscribe to backend status events via `subscribeEntityStatus` or equivalent
  - track states: `ingesting`, `librarian`, `healing`
  - expose derived `busy` state for UI gating
- Update UI with `useWikiStatus` to disable:
  - vault selection / change vault controls
  - manual re-index buttons
  - maintenance commands while any active job is running
- Add test coverage in `src/__tests__/useWikiStatus.test.ts`

### 3. Add `useMemoryRead` reactive hook

- Create `src/hooks/useMemoryRead.ts`
  - take query input and return reactive results
  - apply a 300ms debounce
  - maintain a 500-vector in-memory cache per entity
- Implement tier weights in retrieval:
  - Fact = 1.5x
  - Wisdom = 1.0x
  - Working = 0.6x
- Add code graph neighbor expansion:
  - when a semantic match is found, fetch callers/callees
  - merge structural neighbors into returned results
- Replace or augment current search flow with the new reactive hook
- Add tests for debounce, caching, tier multipliers, and neighbor expansion

### 4. Ensure emoji-safe chunk rendering

- Update `src/components/review/MemoryChunk.tsx`
  - preserve surrogate pairs and multi-byte emoji characters when rendering or splitting chunks
  - use `core-llm-wiki` emoji-safe chunking logic if available
- Validate display of emoji-heavy and AST-symbol-heavy review content

### 5. Add maintenance dashboard support

- Enhance `src/components/settings/MaintenanceDashboard.tsx`
  - surface manual controls for `Heal`, `Prune`, and `Full Re-index`
  - show live wiki job status and busy state
  - disable commands while active operations are in-progress
- Wire `MaintenanceDashboard` into `src/components/settings/SettingsModal.tsx`
- Ensure frontend calls the correct backend commands for each maintenance action

### 6. Backend cleanup, pruning, and GDPR support

- Extend Rust backend in `src-tauri/src/`:
  - add/extend watcher logic to trigger `runHeal` 3 seconds after document deletions
  - add `runPrune` to hard-delete soft-deleted `librarian_inferred` chunks older than 7 days
  - add `forget` for right-to-be-forgotten deletion
  - sanitize and normalize source file paths to prevent path injection or bad vault paths
- Confirm `runHeal` and `runLibrarian` preserve facts/wisdom immutability
- Expose new or updated backend commands through the frontend API layer

### 7. Validation and testing

- Add or update unit tests for:
  - `useWikiStatus`
  - `useMemoryRead`
  - backend maintenance command behavior
- Perform manual integration checks for:
  - vault switching being blocked during active jobs
  - maintenance controls disabling correctly
  - emoji-safe review chunk rendering
  - automatic heal after document deletion
  - prune removing old soft-deleted inferred chunks
- Optionally add `mcp_integration` coverage for the new status and prune flows

## Deliverables

- `src/hooks/useWikiStatus.ts`
- `src/hooks/useMemoryRead.ts`
- `src/components/review/MemoryChunk.tsx`
- `src/components/settings/MaintenanceDashboard.tsx`
- `src/components/settings/SettingsModal.tsx`
- Rust backend updates for watcher auto-heal, `runPrune`, `forget`, and path normalization
- tests covering reactive status, search behavior, and maintenance flows

## Notes

This plan is aligned with the Phase 3 design goals in `docs/superpowers/specs/2026-05-13-phase3-three-tier-memory.md` and is intended to be the working implementation roadmap for the `kv/fixes` branch.
