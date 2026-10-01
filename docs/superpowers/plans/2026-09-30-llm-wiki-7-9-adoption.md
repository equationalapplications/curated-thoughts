# core-llm-wiki 7.9.0 Adoption Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Upgrade Curated Thoughts from `@equationalapplications/*` llm-wiki 7.7.4 (core-okf 7.7.5) to 7.9.0: pin bump + engine migration 13 schema sync (DDL const, V24 mirror migration, temporal index mirror, guard + gate re-pins). Pure adoption — no new engine features.

**Tech Stack:** Tauri 2 + Rust (rusqlite), React 19 + TypeScript, pnpm 10.33.2, `@equationalapplications/core-llm-wiki` 7.9.0.

**Spec:** `docs/superpowers/specs/2026-09-30-llm-wiki-7-9-adoption-design.md`

## Global Constraints

- All llm-wiki packages and overrides pin exactly `7.9.0` (no ranges).
- All pin sites move together: `package.json` (deps + `pnpm.overrides`), `src-tauri/tests/engine_source_ref_gate.rs`, `src-tauri/src/db/schema_guard.rs::PINNED_CORE_LLM_WIKI_VERSION` + `LLM_WIKI_TABLES`.
- The temporal partial indexes stay OUT of `LLM_WIKI_PACKAGE_DDL` (upstream creates them outside the `setupDatabase` exec template; `ddl_compat` compares template statements only).
- V24's stamp is gated on V22 having stamped (V23 pattern); the watermark asserts in `okf_migration.rs` and `connection.rs` stay at 21.
- pnpm: `CI=true npx -y pnpm@10.33.2 …`; plain install first (lockfile must update), `--frozen-lockfile` only as verification.
- Rust gates (as CI): `cargo test --manifest-path src-tauri/Cargo.toml --features test-utils,mcp-server -- --test-threads=1`; `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --features test-utils -- -D warnings`; `cargo fmt --manifest-path src-tauri/Cargo.toml --check`.
- TS gates: `pnpm run build`, `pnpm test`, `pnpm run lint`.
- Regular commits on branch `feat/llm-wiki-7-9-adoption`; commit messages end with `Co-Authored-By: Claude Code <noreply@anthropic.com>`.
- Merge with `--merge` (regular merge, never squash).

## File Structure

| File | Change |
|---|---|
| `package.json`, `pnpm-lock.yaml` | 7.9.0 pins (+ `brace-expansion@5` → 5.0.11) |
| `src-tauri/src/db/okf_ddl.rs` | migration-13 columns inline + `apply_llm_wiki_v13_temporal_indexes` |
| `src-tauri/src/db/connection.rs` | V24 block (PRAGMA-guarded ALTERs, V22-conditional stamp) + index-mirror call + comment/test updates |
| `src-tauri/src/db/schema_guard.rs` | pinned version + column lists (+4/+1/+2) |
| `src-tauri/tests/engine_source_ref_gate.rs` | expected engine version → 7.9.0 (×3) |
| `src-tauri/tests/okf_migration.rs` | V-history comment entry (assert stays 21) |
| `docs/superpowers/specs/2026-09-30-llm-wiki-7-9-adoption-design.md` | spec (status → Implemented when done) |
| `docs/superpowers/plans/2026-09-30-llm-wiki-7-9-adoption.md` | this plan |

---

### Task 1: Bump pins to 7.9.0 and regenerate the lockfile

- [ ] **Step 1: Edit `package.json`**
  - dependencies: `core-llm-wiki`, `react-llm-wiki`, `schema-org-llm-wiki`, `schema-software-org` → `"7.9.0"`
  - `pnpm.overrides`: `core-llm-wiki` → `"7.9.0"`, `core-okf` → `"7.9.0"` (from 7.7.4 / 7.7.5)
  - `pnpm.overrides`: `brace-expansion@5` → `"5.0.11"` (upstream 7.8.0 advisory floor)
- [ ] **Step 2: Regenerate lockfile** — `CI=true npx -y pnpm@10.33.2 install` (NO `--frozen-lockfile` — the lockfile must update to the new overrides first).
- [ ] **Step 3: Verify** — `pnpm install --frozen-lockfile` exits clean; `node -p "require('./node_modules/@equationalapplications/core-llm-wiki/package.json').version"` prints `7.9.0`; `git diff pnpm-lock.yaml` shows only expected movement.
- [ ] **Step 4: Commit** — `chore(deps): bump @equationalapplications llm-wiki packages 7.7.4 -> 7.9.0`

### Task 2: Mirror migration 13 in the Rust DDL (`okf_ddl.rs`)

- [ ] **Step 1: Columns inline.** In `LLM_WIKI_PACKAGE_DDL`, extend verbatim from upstream `packages/core/src/db/schema.ts` @ v7.9.0:
  - `llm_wiki_entries`: after `embedding_attempts INTEGER NOT NULL DEFAULT 0` add `valid_from INTEGER, valid_to INTEGER, superseded_by TEXT, superseded_at INTEGER`
  - `llm_wiki_events`: after `created_at INTEGER NOT NULL` add `occurred_at INTEGER`
  - `llm_wiki_checkpoints`: after `memory_checkpoint INTEGER NOT NULL DEFAULT 0` add `librarian_watermark_at INTEGER, librarian_watermark_id TEXT`
- [ ] **Step 2: Index mirror fn.** Add `apply_llm_wiki_v13_temporal_indexes(conn) -> rusqlite::Result<()>`: PRAGMA `table_info(llm_wiki_entries)`; when `superseded_by` exists create `llm_wiki_entries_superseded_idx ON llm_wiki_entries(entity_id, superseded_by) WHERE superseded_by IS NOT NULL`; when `valid_from` AND `valid_to` exist create `llm_wiki_entries_temporal_idx ON llm_wiki_entries(entity_id) WHERE valid_from IS NOT NULL OR valid_to IS NOT NULL` (exact upstream SQL, `IF NOT EXISTS`). Doc-comment: mirror of engine migration 13's `createTemporalIndexesIfColumnsExist`, runs on every Rust open so CLI/MCP-first brains get the same index shape; idempotent after the engine's own pass.
- [ ] **Step 3: Verify** — `cargo test --manifest-path src-tauri/Cargo.toml --lib ddl_compat` FAILS only if the const drifted from the installed package (it must pass after Step 1 is verbatim).

### Task 3: V24 mirror migration + tail wiring (`connection.rs`)

- [ ] **Step 1: V24 block** after V23's stamp block (before the `version < 8` taxonomy fix): PRAGMA-guarded `ALTER TABLE llm_wiki_{entries,events,checkpoints} ADD COLUMN` ×7 (V17 code pattern: build the `existing` column vec per table, hardcoded `(column, declared_type)` literals). Runs on every open. Then re-read `MAX(version)` and stamp 24 only when `stamped >= 22` (V23 pattern + doc-comment explaining the rootless-open trap).
- [ ] **Step 2: Tail call** — `crate::db::okf_ddl::apply_llm_wiki_v13_temporal_indexes(conn)?;` next to the existing `apply_llm_wiki_v12_edge_index` call, before `verify_llm_wiki_schema(conn)?`.
- [ ] **Step 3: Old-DB upgrade test** (lib tests): create an in-memory DB, stamp it to the pre-V24 world by creating `llm_wiki_entries/events/checkpoints` WITHOUT the new columns (or rewind columns on a migrated DB), run `migrate(None)`, then assert `verify_llm_wiki_schema` passes and both temporal indexes exist; re-run `migrate` → no-op. Mirror the existing V22/V23 upgrade-test style (`connection.rs` ~:2818, "V22 then V23 must be stamped when the migration runs").
- [ ] **Step 4: Comment-only updates** — the `open_in_memory` watermark test's V-history comment block gains a V24 entry; the assert stays `assert_eq!(max_version, 21)`.
- [ ] **Step 5: Verify** — `cargo test --manifest-path src-tauri/Cargo.toml --lib --features test-utils -- --test-threads=1`

### Task 4: Guard and gate re-pins

- [ ] **Step 1: `schema_guard.rs`** — `PINNED_CORE_LLM_WIKI_VERSION = "7.9.0"`; `LLM_WIKI_TABLES`: entries += `valid_from, valid_to, superseded_by, superseded_at`; events += `occurred_at`; checkpoints += `librarian_watermark_at, librarian_watermark_id`.
- [ ] **Step 2: `engine_source_ref_gate.rs`** — comment (:51), assert value (:61), failure message (:62): `"7.7.4"` → `"7.9.0"` (message also names 7.9.0).
- [ ] **Step 3: `okf_migration.rs`** — V-history comment block gains the V24 entry ("mirrors core-llm-wiki engine migration 13; stamps only after V22"); `assert_eq!(max_version, 21)` unchanged.
- [ ] **Step 4: Verify** — `cargo test --manifest-path src-tauri/Cargo.toml --test okf_migration --test engine_source_ref_gate --features test-utils -- --test-threads=1` (engine gate also needs `--ignored` + a real `pnpm install`; CI runs it as `--ignored --test-threads=1`).

### Task 5: Full gates

- [ ] `cargo fmt --manifest-path src-tauri/Cargo.toml --check`
- [ ] `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --features test-utils -- -D warnings`
- [ ] `cargo test --manifest-path src-tauri/Cargo.toml --features test-utils,mcp-server -- --test-threads=1`
- [ ] `cargo test --manifest-path src-tauri/Cargo.toml --test engine_source_ref_gate -- --ignored --test-threads=1`
- [ ] `pnpm run build && pnpm test && pnpm run lint`
- [ ] Known node-26 vitest flake (6 tests, `localStorage`) reproduces on `main` — don't blame this branch.

### Task 6: Spec status, commit, push, PR

- [ ] Mark the spec Status line `Implemented on feat/llm-wiki-7-9-adoption — pending PR review`; commit docs.
- [ ] Push; `gh pr create` (regular merge policy — `--merge` only, never squash). Body documents: the 7.7.4→7.9.0 delta, the V24/conditional-stamp rationale, the out-of-const index decision, `brace-expansion@5` floor, and the `syncSearchIndex` follow-up pointer.
- [ ] Verify on the tip SHA: `gh pr view --json mergeable,mergeStateStatus` + check-runs (never trust the PR body / "green" claims).

---

**Deferred follow-up (out of scope here):** adopt `WikiMemory.syncSearchIndex()` — CT's Rust writers bypass core write APIs, so Rust-written facts stay out of keyword/hybrid retrieval until the next core write. Needs its own spec (TS hook or IPC after Rust commits).
