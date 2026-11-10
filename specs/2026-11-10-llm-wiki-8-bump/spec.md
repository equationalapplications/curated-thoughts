# Adopt @equationalapplications llm-wiki 8.0.2 — behavior-preserving bump

- **Date:** 2026-11-10
- **Status:** Draft
- **Branch:** `deps/llm-wiki-8-0-2`
- **Priority:** High. 8.0.0 flips two engine defaults CT relies on; every new
  CT feature built on 7.13.0 assumptions drifts further from upstream.

## Problem

CT pins five `@equationalapplications` packages at 7.13.0 — `core-llm-wiki`,
`react-llm-wiki`, `schema-org-llm-wiki`, `schema-software-org` in
`dependencies`, plus pnpm `overrides` for `core-llm-wiki` (7.13.0) and
`core-okf` (7.9.0, lockstep-lagged since #252 — core 7.13.0 declares
core-okf 7.13.0 exactly). Upstream shipped 7.14.0 → 8.0.2 between
2026-11-01 and 2026-11-10.

This is an investigation-verified account of what 8.0.2 changes **for CT**
(read from the 7.13.0 and 8.0.2 tarballs and upstream release notes, not
from memory):

1. **The only breaking deltas that touch CT are two default flips** in
   `WikiConfig` (both options existed in 7.13.0; only defaults moved):
   - `librarian` — default `{ strategy: 'legacy' }` → `{ strategy: 'ops' }`
     (spec 2026-09-29 §5.1 dispatcher).
   - `maintenance` — default `'auto'` → `'deferred'`. In `'deferred'`,
     `write()` only records events; nothing is extracted or healed until
     the host calls `runPendingMaintenance()`. CT passes **neither** key in
     `makeWikiOptions` (`src/lib/wiki.ts:347`), so an unpinned bump would
     silently stop write()-time extraction and heal in the app's TS-engine
     lane. Upstream also documents a data-loss trap: `'deferred'` paired
     with `strategy: 'legacy'` advances the watermark past only the newest
     50 events per pass, so a larger deferred backlog is never extracted.
2. **v7.17/v8.0 removed the deprecated `wiki_*` tool names** — but from the
   `core-llm-tools` package, which CT does **not** import. CT's MCP server
   exposes tools *named* `wiki_traverse_graph` / `wiki_get_ontology`
   (`src-tauri/src/mcp_server.rs`) backed by CT's own Rust implementations
   (`crate::wiki_graph`). Those names are CT's public contract with CTI;
   upstream's removal cannot break this build and renaming them is out of
   scope (see Decisions).
3. **Schema is untouched.** `setupDatabase` DDL diffed byte-identical
   between the 7.13.0 and 8.0.2 `dist/index.js`. Therefore
   `LLM_WIKI_PACKAGE_DDL` (`okf_ddl.rs`), the `schema_guard` column
   expectations, and the engine-migration mirrors (engine V12, engine
   migration 13 → CT V24) all stand. The
   `rust_llm_wiki_ddl_matches_core_llm_wiki_package` test is the gate that
   proves this on the installed package.
4. **The TS API surface CT consumes is unchanged.** `react-llm-wiki` 8.0.2
   does `export * from '@equationalapplications/core-llm-wiki'`, so every
   symbol CT imports through it (`createWiki`, `WikiBusyError`,
   `ClassifyRequest/Response`, `WikiDiagnostic`, `WikiLintReport`,
   `WikiOptions`, `WikiProvider`, `WikiFact`, `SQLiteAdapter`) still flows.
   `schemaOrgWarmAgentManifest` and `schemaSoftwareOrgManifest` exports
   verified present in both schema packages at 8.0.2. New APIs
   (`remember()`, `runPendingMaintenance()`, `PendingMaintenance`,
   `MaintenanceJob`, `RememberInput`) are additive.
5. **The FTS convergence host contract stands.** `config.setupBudgetMs`
   (the 7.13.0 quantum budget `src/lib/wiki.ts` converges against) still
   exists in 8.0.2 with the same semantics; `minisearch` stays `^7.2.0`.
6. Transitive movement is exactly one package: `core-okf` (declared exact
   by core). No new transitive crates or npm packages otherwise.

The dependency age gate does not apply: it is Dependabot's own cooldown
(`.github/dependabot.yml:21`, "applies to direct Dependabot updates only"),
and this family is the controller's internal library — exempt per ruling
2026-11-10.

## Change

1. `package.json` — bump the four `dependencies` entries to `8.0.2`; move
   both `pnpm.overrides` entries (`core-llm-wiki`, `core-okf`) to `8.0.2`.
2. `src-tauri/src/db/schema_guard.rs` — `PINNED_CORE_LLM_WIKI_VERSION` →
   `"8.0.2"`. The `engine_source_ref_gate.rs` test reads this constant and
   asserts it equals the *installed* version, so it needs no edit — it is
   the gate that catches a half-moved bump.
3. `src/lib/wiki.ts` — `makeWikiOptions` explicitly pins
   `librarian: { strategy: 'legacy' }` and `maintenance: 'auto'` under
   `config`, preserving the exact 7.13.0 engine behavior across the bump.
   The comment records why: the 8.0.0 default flip, the deferred+legacy
   backlog trap, and that adopting `ops`/`deferred` is future work.
4. `src/lib/wiki.ts` — update the three "core-llm-wiki 7.13.0 host
   contract" comment stamps to say the contract (introduced 7.13.0) was
   reverified on 8.0.2.
5. `pnpm install` regenerates the lockfile (`npx -y pnpm@10.33.2`, per the
   #208 gotcha that global pnpm 9.15.9 otherwise prompts interactively).

## Decisions

- **Behavior-preserving bump, not `ops`/`deferred` adoption.** A version
  bump must not change when extraction runs. Adopting the ops librarian
  and deferred maintenance changes app-side ingest timing, requires
  `runPendingMaintenance()` wiring and status surfaces, and per INTENT §4
  behavior changes are benchmark-gated. That is a separate future spec.
- **`core-okf` override moves to 8.0.2** rather than being removed or left
  at 7.9.0. It matches what core 8.0.2 declares exactly; leaving it at
  7.9.0 would force a downgrade under a core that may use newer okf APIs;
  removing it would drop the deliberate-pin guard the overrides exist to
  provide (#208, #252 precedent).
- **No CT tool renames.** Upstream's `wiki_*` removal binds hosts that
  import `core-llm-tools`. CT satisfies the spirit of the "hosts migrate
  first" condition (#324) by importing nothing that was removed; its own
  tool names are CTI's contract and move only in a dedicated CTI-coordinated
  change.
- **No adoption of `remember()`.** INTENT rule 1: agents never write facts
  directly; a host-side direct-write API is unnecessary for CT's
  file-first deposit pipeline.

## Verification

- `pnpm run typecheck`, `pnpm run lint`, `pnpm test` (vitest) — with the
  documented local-env caveat: `useProposalNotifications` (×5) and
  `SettingsScreen` tab smoke (×1) fail on clean main under local node 26
  (#208); CI runs node 22 and is authoritative for those.
- `cargo test --features test-utils,mcp-server` with the canonical
  absolute `CARGO_TARGET_DIR` (AGENTS.md), serial: must include
  `rust_llm_wiki_ddl_matches_core_llm_wiki_package` (DDL proof),
  `schema_guard` tests (pinned version in error text), and
  `engine_source_ref_gate` (installed == pinned == 8.0.2; requires
  `pnpm install` first).
- `grep -rn "7\.13\.0"` over tracked source returns no version stamps
  outside historical/spec references.

## Out of scope

- Ops-librarian / deferred-maintenance adoption (`runPendingMaintenance`
  wiring, backlog surfaces, benchmark gates).
- `remember()` host API adoption.
- Renaming CT's MCP tool names to upstream's canonical set
  (`graph_traverse` etc.) — CTI-coordinated change.
- Direct adoption work for `core-okf` 8.0.2 beyond the aligned override.
