# core-llm-wiki 7.9.0 adoption (schema sync)

**Date:** 2026-09-30
**Status:** Drafted on `feat/llm-wiki-7-9-adoption`
**Branch:** `feat/llm-wiki-7-9-adoption` (spec + plan + implementation ride one branch and one PR)
**Upstream:** expo-llm-wiki `v7.9.0` (`33f1af6`, 2026-09-30); prior adoption: `docs/superpowers/specs/2026-09-22-llm-wiki-7-7-adoption-design.md`

## Problem

Curated Thoughts (CT) pins the `@equationalapplications/*` llm-wiki packages at
`7.7.4` (core-okf override at `7.7.5`). Upstream `7.7.5`–`7.9.0` adds engine
migration 13 (temporal fact columns, event `occurred_at`, librarian watermark
columns, two partial indexes), the OKF unescape fix CT reported upstream
(#231), incremental MiniSearch sync fixes (#232/#235), and `syncSearchIndex()`.

CT cannot adopt blindly. Three CT invariants shape the design:

1. **The Rust startup schema guard demands the exact package column set.**
   `schema_guard.rs` hard-fails every open when `PRAGMA table_info` on the
   `llm_wiki_*` tables does not match its pinned expectations, and CLI/MCP
   opens never run the TS engine — so migration 13's columns must exist in
   Rust **before** the guard, via a mirrored V-migration (V17 precedent).
2. **`ddl_compat` compares statement sets.**
   `rust_llm_wiki_ddl_matches_core_llm_wiki_package` extracts the
   `setupDatabase` exec template from the installed package and compares it to
   `LLM_WIKI_PACKAGE_DDL`. The new columns go in the const's CREATE TABLEs
   inline (upstream does the same); the two temporal partial indexes stay
   **out** of the const — upstream creates them outside the template
   (`createTemporalIndexesIfColumnsExist`), so putting them in the const would
   fail the drift test as Rust-only statements. They get a mirror function
   instead (`apply_llm_wiki_v13_temporal_indexes`, V12 precedent).
3. **A rootless open defers the V22 stamp.** `migrate()` gates on
   `MAX(version)`, so the new V24 stamp must be conditional on V22 having
   stamped (V23 pattern). An unconditional stamp would make the next rooted
   open read max=24, skip `if version < 22`, and permanently skip V22's
   `documents.path` rewrite.

## Verified baseline (grepped on `main` @ `4baf903`)

- **Pin sites (all must move together):** `package.json` dependencies
  (`core-llm-wiki`, `react-llm-wiki`, `schema-org-llm-wiki`,
  `schema-software-org`, all `7.7.4`) and `pnpm.overrides` (`core-llm-wiki`
  `7.7.4`, `core-okf` `7.7.5`); `src-tauri/tests/engine_source_ref_gate.rs:51-62`
  (expected engine version ×3); `src-tauri/src/db/schema_guard.rs:8`
  (`PINNED_CORE_LLM_WIKI_VERSION`) plus the `LLM_WIKI_TABLES` column lists
  (entries 32 cols incl. `tier`, events 6, checkpoints 3).
- **DDL mirror:** `src-tauri/src/db/okf_ddl.rs::LLM_WIKI_PACKAGE_DDL` (verbatim
  `setupDatabase` with prefix applied) + `apply_llm_wiki_v12_edge_index`
  called from the `migrate()` tail; `verify_llm_wiki_schema(conn)` runs last
  (`connection.rs:755`).
- **Watermark asserts hold at 21** (rootless opens defer V22):
  `src-tauri/tests/okf_migration.rs:235` and the `open_in_memory` test in
  `connection.rs` (~:916). Current max stamp on a rooted open is 23
  (`connection.rs:704-723`).
- **Upstream delta (tags `v7.7.4..v7.9.0`):** exactly one DDL commit
  (`b087571`, migration 13). Additive-only, PRAGMA-guarded, no backfill:
  `llm_wiki_entries` += `valid_from INTEGER, valid_to INTEGER,
  superseded_by TEXT, superseded_at INTEGER`; `llm_wiki_events` +=
  `occurred_at INTEGER`; `llm_wiki_checkpoints` += `librarian_watermark_at
  INTEGER, librarian_watermark_id TEXT`; plus partial indexes
  `entries_superseded_idx` and `entries_temporal_idx` created outside the
  setup template. Fresh-install CREATE TABLEs include the columns inline.
- **No breaking changes; defaults preserve 7.7.4 behavior**
  (`strategy: 'legacy'`, `maintenance: 'auto'`, `compact: false`,
  `reportLlmUsage: false`). The one type-level change
  (`LibrarianResult.processedThrough: string → EventCursor`) has zero CT call
  sites (CT implements no `LibrarianStrategy` and runs its librarian in Rust).
- **Lockstep release:** react-llm-wiki / schema-org-llm-wiki /
  schema-software-org / core-okf all publish `7.9.0` against core-llm-wiki
  `7.9.0`; the override set must move together.
- **7.8.0 advisory floors** (upstream-monorepo overrides): `brace-expansion@5`
  floor raised to `5.0.11` (GHSA-qhr7-859c-m2p7, GHSA-6j4f-fj2g-mc7p). CT
  pins `brace-expansion@5: 5.0.9` and takes the same floor.

## 1. Version bumps and supply-chain floor [CT-REQ-BUMP-01]

All llm-wiki deps and overrides → exactly `7.9.0` (no ranges):
`package.json` dependencies ×4, `pnpm.overrides` ×2, and the
`brace-expansion@5` override `5.0.9 → 5.0.11` per upstream 7.8.0's advisory
floor. Lockfile regenerated with `CI=true npx -y pnpm@10.33.2 install`
(plain install first so `pnpm-lock.yaml` picks up the new overrides, then
`--frozen-lockfile` as the verification).

## 2. DDL mirror: migration 13 [CT-REQ-DDL-01]

`okf_ddl.rs`:

- `LLM_WIKI_PACKAGE_DDL` gains the 7 columns inline in the CREATE TABLEs,
  verbatim from upstream `packages/core/src/db/schema.ts` at `v7.9.0`
  (entries: `valid_from`, `valid_to`, `superseded_by`, `superseded_at`; events:
  `occurred_at`; checkpoints: `librarian_watermark_at`, `librarian_watermark_id`).
- New `apply_llm_wiki_v13_temporal_indexes(conn)`: PRAGMA-guarded creation of
  `llm_wiki_entries_superseded_idx` and `llm_wiki_entries_temporal_idx`
  (partial, exact upstream SQL), V12-mirror style and idempotent. Called from
  the `migrate()` tail next to the V12 mirror, before
  `verify_llm_wiki_schema`, so fresh Rust-created DBs and old DBs both end up
  with the same index shape the engine's `setup()` produces.

## 3. V24 mirror migration [CT-REQ-V24-01]

`connection.rs`, block placed after V23's stamp block:

- PRAGMA-guarded `ALTER TABLE … ADD COLUMN` for the 7 columns (V17 code
  pattern — per-column guards, hardcoded literals), running on **every** open
  so Rust-first (CLI/MCP) opens pass the schema guard.
- **Stamp gated on V22**: re-read `MAX(version)` (V23 already stamped by
  then), stamp 24 only when `stamped >= 22` — otherwise a rootless open's
  stamp would permanently skip V22 on the next rooted open (V23's
  doc-comment documents this exact trap).
- Idempotent in both directions: upstream migration 13's ALTERs are
  PRAGMA-guarded, so the engine's later pass no-ops on V24-upgraded columns,
  and vice versa.

## 4. Guard and gate re-pins [CT-REQ-GATE-01]

- `schema_guard.rs`: `PINNED_CORE_LLM_WIKI_VERSION = "7.9.0"`;
  `LLM_WIKI_TABLES` entries +4, events +1 (occurred_at), checkpoints +2.
- `engine_source_ref_gate.rs`: expected probe `engineVersion` `"7.7.4" →
  "7.9.0"` (comment, assert, failure message). The probe script itself is
  version-agnostic; it exercises upstream migration 13 during `setup()`, so a
  rewrite of CT rows would still surface in `changedRows`.
- Watermark asserts stay at `21` (rootless-open cap); both V-history comment
  blocks (`okf_migration.rs`, `connection.rs` test) gain a V24 entry.

## 5. Out of scope

Recorded here so no one assumes the capability exists:

- **`syncSearchIndex()`** (7.9.0) — relevant follow-up: CT's Rust writers
  (`promote_draft_cmd`, pipeline, heal/prune) bypass core write APIs, so
  newly-written facts stay out of keyword/hybrid retrieval until the next
  core write. Adopting the call is a separate spec (it needs a TS-side or
  IPC hook after Rust commits).
- Deferred write mode, `drain()`, `getPendingMaintenance`,
  `runPendingMaintenance` (CT maintenance is Rust-native).
- `'ops'` librarian strategy, temporal reads/`supersede()`/`history()`,
  `read({ asOf, tokenBudget })`, `formatContext` compact mode, `callLlm`
  gateway / `llm_usage` diagnostic — all opt-in upstream, none on a CT path.
- `undici`/`fast-uri` advisory floors — upstream-monorepo-only overrides; CT
  takes only the `brace-expansion@5` floor it actually pins.

## 6. Testing

**Rust** (run with `--features test-utils,mcp-server`, as CI does):

- `ddl_compat::rust_llm_wiki_ddl_matches_core_llm_wiki_package` passes
  against installed 7.9.0 (proves the DDL const ≡ package `setupDatabase`).
- Old-DB upgrade: a DB created at the pre-V24 shape (7.7.4 column set)
  upgrades through `migrate()` and passes `verify_llm_wiki_schema`; both
  temporal indexes exist afterwards; re-open is a no-op.
- Rootless open still caps the watermark at 21 (`okf_migration.rs` assert,
  unchanged) and V24 still adds the columns + indexes on the rootless path.
- `engine_source_ref_gate --ignored --test-threads=1` passes against the
  installed 7.9.0 package (CI runs this on every push).
- Full lib/tools/integration suites, clippy `-D warnings`, fmt.

**TypeScript:** `pnpm install --frozen-lockfile`, `pnpm run build`
(typecheck), `pnpm test`, `pnpm run lint`.

## Risks

- **Extraction-marker drift:** 7.9.0's bundled `dist/index.js` could shift
  the `ddl_compat` extraction markers; the source diff shows no change to the
  `setupDatabase` signature or exec shape, and the drift test fails loudly if
  hit (marker repair is in-scope for this PR).
- **Old DBs + engine migration 13:** safe — both sides PRAGMA-guard every
  ALTER, and engine `setup()` runs DDL before migrations.
- **Manual smoke:** optional, per the 7.7.4 precedent.
