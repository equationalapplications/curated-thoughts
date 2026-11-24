# Spec: engine 9.x adoption (expo-llm-wiki 8.0.2 → 9.18.0)

Status: Draft
Owner: Kurt VanDusen / Hermes Agent
Date: 2026-11-24
Related issues: #342 (vector_removals ledger, non-blocking), #348 (sso 9.x node types, blocking), #353 (source_pointer mirror, blocking for the guard).
Related spec: this is the "engine bump lane" deferred by `specs/2026-11-14-areas/spec.md` §14.1. The `area` ingest-mapping decision (§14.2 there) stays deferred; it is not part of this spec.

## 1. Problem

Curated Thoughts pins six internal packages at 8.0.2 while expo-llm-wiki is at 9.18.0. The pins live in two places:

- `package.json` pnpm overrides, lines 8-9: `@equationalapplications/core-llm-wiki` and `@equationalapplications/core-okf`, both `"8.0.2"`.
- `package.json` dependencies, lines 49-52: `core-llm-wiki`, `react-llm-wiki`, `schema-org-llm-wiki`, `schema-software-org`, all `"8.0.2"`.

The pins stall features that other EA repos already use. Owner directive (Kurt, Discord 2026-11-24): "Adopt the latest version of expo-llm-wiki packages. Always respect 14 day dependency cool down age gate. Own the open PRs that are not cool down blocked." Internal EA packages are age-gate exempt (SOP amended 2026-11-24).

Three open CT issues are prerequisites or companions:

1. #348 — schema-software-org 9.x removes `handoff` and `session_recap` node types and `handoffFor` edges. CT asserts the old set in tests and Rust constants. BLOCKING.
2. #353 — engine migration v16 adds `entries.source_pointer`. CT mirrors the engine DDL verbatim and pins an exact column set, so CT must mirror it in the same PR that bumps the core pin. BLOCKING for the guard.
3. #342 — engine 8.4.0 adds a `vector_removals` ledger with DELETE/enqueue triggers. Feature parity. NON-BLOCKING.

## 2. Root cause and evidence

CT mirrors the engine's SQLite DDL verbatim and pins it with an exact-equality guard. Any package bump changes the engine DDL, so the bump and the Rust mirror must land in one commit or existing brain.db files fail to open.

Evidence (each anchor verified against origin/main at e1e75165 on 2026-11-24):

1. `src-tauri/src/db/okf_ddl.rs:10` defines `LLM_WIKI_PACKAGE_DDL` as "verbatim `setupDatabase` SQL from `@equationalapplications/core-llm-wiki`". Verified.
2. `src-tauri/src/db/schema_guard.rs:8` pins `PINNED_CORE_LLM_WIKI_VERSION: &str = "8.0.2"`. Verified.
3. `src-tauri/src/db/schema_guard.rs` `verify_table` (~line 190) compares `PRAGMA table_info` column sets as `BTreeSet` equality and bails on "missing columns" / "unexpected columns" (~lines 203-212). Verified. Any DB opened by a new build against an old file, or an old build against a new file, fails startup.
4. The engine changed the schema additively between 8.0.2 and 9.18.0: migration v16 adds `entries.source_pointer` (9.1.0), v17 adds a `source_store` table, v18 adds `entries.prior_valid_to`, `derived_events`, and a partial index, v19 adds `events.forgotten_at`, and 8.4.0 adds the `vector_removals` ledger with DELETE/enqueue triggers on entries. Package `schema_version` moves 15→19; CREATE TABLE count moves 10→13.
5. The TS import surface is additive-only between 8.0.2 and 9.18.0. All 13 symbols CT imports still exist. Imports live at `src/lib/wiki.ts:1`, `src/lib/ontology.ts:1-7`, `src/lib/wikiAdapter.ts:2`, `src/main.tsx:3`, and type-only imports in `DraftsPanel`, `HealthReportPanel`, and `ontologySeed`. The only semver break (9.0.0 removes `prompts.librarianSystemPrompt`) has zero CT hits. Verified by grep.
6. The one semver break CT does hit is schema-software-org 9.0.0: it removes the `handoff` and `session_recap` node types and `handoffFor` edges; it adds statement kinds (`rule`, `decision`, `lesson`, `trap` under abstract `statement`), the `area` node type, and `about` edges. CT asserts the old set:
   - `src/__tests__/ontology-packages.test.ts:20-27` asserts `creativework` children include `handoff` and `session_recap`. Verified.
   - `src-tauri/src/db/entity_gate.rs:87` lists `handoff` and `:98` lists `session_recap` in `EA_SEED_TYPES` (17 slugs). Verified. The 17-slug pin test `ea_seed_types_match_spec_section_1_1` sits at `src-tauri/src/db/entity_gate.rs:1984-2008`. Verified.
   - Copy text: `src/lib/ontology.ts:29` (`'Specs, handoffs, services'`) and `src-tauri/src/onboard/mod.rs:128` (`specs, handoffs, services, procedures`). Verified.
   - `tools/tests/ct_heal.rs:319` asserts `session_recap` in the heal vocabulary. Verified by grep.
7. `schema-software-org` has NO DDL mirror in `src-tauri/src/db/okf_ddl.rs` — grep for `handoff|session_recap|schema-software` in that file returns zero hits. Verified. So PR-A can bump sso alone without touching the Rust schema guard.
8. `src/tests/ontology.test.ts` from the impact report DOES NOT EXIST — corrected. The only frontend assertion of the removed node types is `src/__tests__/ontology-packages.test.ts:20-27`. The Rust-side assertions are `entity_gate.rs` (above) and `tools/tests/ct_heal.rs:319`.

## 3. Proposed staged change

Three PRs, in order. PR-A and PR-B are blocking; PR-C may fold into PR-B.

### PR-A: schema-software-org 9.x readiness (resolves #348)

Bump `@equationalapplications/schema-software-org` alone to 9.18.0 in `package.json` dependencies. The guard is untouched: sso has no DDL mirror (evidence §2.7), and core stays at 8.0.2 so `schema_guard.rs` still matches.

Changes:

1. Update `src/__tests__/ontology-packages.test.ts` expectations: `creativework` children lose `handoff` and `session_recap`; add assertions for the new surface (`statement` with kinds `rule`, `decision`, `lesson`, `trap`; `area` node type; `about` edges) so the test pins the NEW contract, not just the absence of the old one.
2. Update `src-tauri/src/db/entity_gate.rs` `EA_SEED_TYPES`: remove `handoff` and `session_recap`; add the statement kinds and `area` per the 9.18.0 sso manifest. Update the pin test at `entity_gate.rs:1984-2008` to the new slug list and new count (the count is whatever the 9.18.0 manifest declares — the implementer must read the installed tarball and pin the actual list, not this spec's guess).
3. Update copy: `src/lib/ontology.ts:29` and `src-tauri/src/onboard/mod.rs:128` replace "handoffs" with the 9.x vocabulary wording (e.g. "Specs, statements, services" — final wording in implementation).
4. Sweep `src-tauri/src/onboard/mod.rs`, `src-tauri/src/wisdom_match.rs`, `src-tauri/src/db/wisdom.rs`, `src-tauri/src/db/evidence_reanchor.rs`, and `tools/src/cmds.rs` for `handoff`/`session_recap` logic branches (grep hits exist in all five) and state each disposition: re-map to the new statement kinds, or keep as legacy-data-only handling.

Data re-typing proposal (Kurt's decision — this spec does NOT implement a migration until he picks a mapping):

- Existing brain.db facts typed `handoff` or `session_recap` reference node types that no longer exist in the 9.18.0 manifest. Proposed mapping:
  - `handoff` → `statement` with kind `decision` (a handoff is a decision about where work stands), or degrade to plain `statement`.
  - `session_recap` → `statement` with kind `lesson`.
  - `handoffFor` edges → `about` edges (same source, same target).
- Implementation vehicle: a CT-side one-time migration in `src-tauri/src/db/connection.rs` (CT's own migration chain, currently at version 19 locally — CT numbering is independent of engine numbering) gated on rows with the old types, plus heal coverage. The areas spec (`specs/2026-11-14-areas/spec.md` §14.1) already flags "heal for handoff and session_recap rows" as bump-lane work; this is it.
- Alternative if Kurt prefers no data rewrite: keep old-type rows frozen, exclude them from the node-type gate, and let heal re-anchor them. This loses type fidelity but touches nothing.
- DECISION REQUIRED from Kurt at spec review: re-type (mapping above or a different one), or freeze-and-exclude.

### PR-B: core/okf/react/schema-org pin bump to 9.18.0 (resolves #353)

Bump all four remaining packages to 9.18.0 — `core-llm-wiki`, `core-okf` (pnpm overrides, lines 8-9) and `core-llm-wiki`, `react-llm-wiki`, `schema-org-llm-wiki` (dependencies, lines 49-52) — AND regenerate the Rust mirror in the SAME commit.

Guard-ordering resolution: option (a) — ride ALL engine schema mirrors (v16 `source_pointer`, v17 `source_store`, v18 `prior_valid_to`/`derived_events`/partial index, v19 `forgotten_at`, and the 8.4.0 `vector_removals` ledger if PR-C folds in) in the same commit as the pin bump.

1. Regenerate `src-tauri/src/db/okf_ddl.rs` `LLM_WIKI_PACKAGE_DDL` verbatim from the 9.18.0 `core-llm-wiki` `setupDatabase` output, prefix applied, exactly as the file header demands. No hand edits.
2. Bump `src-tauri/src/db/schema_guard.rs:8` `PINNED_CORE_LLM_WIKI_VERSION` to `"9.18.0"`.
3. Update the exact-equality `TableExpectation` sets in `schema_guard.rs` (verify_table, ~line 190) to the 9.18.0 column sets: new columns on `entries`, `events`; new tables `source_store`, `derived_events`, `vector_removals` (13 tables total, schema_version 19).
4. CT's own `okf_migration` (`src-tauri/src/db/okf_migration.rs`) must not conflict: verify it does not create columns the engine now creates itself; adjust or remove any CT-side pre-add now that the engine owns them.
5. Bump CT's local migration chain in `src-tauri/src/db/connection.rs` as needed for any CT-owned columns (existing files get them via CT migrations; fresh installs get them from the package DDL — both paths must converge on identical column sets, which is exactly what the guard asserts).

Why option (a) over option (b) (CT pre-adds columns before the bump): pre-adding means CT ships columns the pinned engine DDL does not have, which the SAME exact-equality guard rejects in the other direction ("unexpected columns") until the bump lands. That is a broken intermediate state for zero benefit. Rejected alternative — recorded.

### PR-C: vector_removals ledger adoption (resolves #342; may fold into PR-B)

Wire the engine's `vector_removals` ledger: mirror its DDL and DELETE/enqueue triggers in `okf_ddl.rs` (if not already folded into PR-B) and consume the ledger where CT currently does ad-hoc vector cleanup. If the consumption surface turns out larger than a small patch, land the DDL mirror in PR-B and split consumption into this follow-up PR. Non-blocking.

## 4. Files touched (real paths, verified on origin/main)

PR-A:

- `package.json` (one dependency line)
- `pnpm-lock.yaml`
- `src/__tests__/ontology-packages.test.ts`
- `src-tauri/src/db/entity_gate.rs`
- `src-tauri/src/onboard/mod.rs`
- `src/lib/ontology.ts`
- sweep dispositions only if logic branches exist: `src-tauri/src/wisdom_match.rs`, `src-tauri/src/db/wisdom.rs`, `src-tauri/src/db/evidence_reanchor.rs`, `tools/src/cmds.rs`, `tools/tests/ct_heal.rs`

PR-B:

- `package.json` (two override lines, three dependency lines)
- `pnpm-lock.yaml`
- `src-tauri/src/db/okf_ddl.rs`
- `src-tauri/src/db/schema_guard.rs`
- `src-tauri/src/db/okf_migration.rs` (only if step 4 applies)
- `src-tauri/src/db/connection.rs` (only if step 5 applies)

PR-C (if standalone):

- `src-tauri/src/db/okf_ddl.rs`
- the vector-consumption surface, identified at implementation time

Note: `src/tests/ontology.test.ts` from the impact report does not exist; it is dropped from every file list.

## 5. Test plan

Rust (repo is NOT a cargo workspace — always `--manifest-path`; export `CARGO_TARGET_DIR=/mnt/kv-work/build/cargo/curated-thoughts`):

- `cargo --manifest-path src-tauri/Cargo.toml test --lib --features test-utils` — covers the `ea_seed_types_match_spec_section_1_1` pin test, `schema_guard` expectations, and `okf_migration`.
- `cargo --manifest-path tools/Cargo.toml test --lib --features test-utils` — covers tool-side vocabulary.
- `cargo --manifest-path tools/Cargo.toml test --test ct_heal --features test-utils` — covers the heal vocabulary assertion at `tools/tests/ct_heal.rs:319`.

Frontend:

- `pnpm test -- src/__tests__/ontology-packages.test.ts` (or the repo's `package.json` test script scoped to that file) — new sso 9.x contract assertions.

New tests required in PR-B (both DB paths):

1. Fresh-install path: create a brand-new DB through the new `LLM_WIKI_PACKAGE_DDL`, run `schema_guard::verify` against the 9.18.0 expectations. Must pass.
2. Existing-DB path: take a DB fixture created under the 8.0.2 DDL, run CT's migration chain (including the new engine-mirror migrations), then run `schema_guard::verify`. Must pass. This test is the whole point of the same-commit rule — it fails if the DDL mirror and the pin ever drift apart again.
3. Round-trip: open the migrated fixture with the OLD pinned build (CI job or a pinned-version test) and assert the guard bails with the documented "unexpected columns" message — the intended forward-incompat behavior, documented rather than silent.

PR-A tests:

- Updated `ontology-packages.test.ts` asserts the 9.18.0 sso manifest (statement kinds, `area`, `about`; no `handoff`/`session_recap`).
- Updated `entity_gate.rs` pin test asserts the new slug list and count.
- If Kurt approves re-typing: a migration test seeds `handoff`/`session_recap` rows in an 8.0.2-era fixture and asserts the mapped types and `about` edges after migration, including a handoff→statement-with-kind row.

## 6. Rejected alternatives

1. **Option (b) for guard ordering — CT pre-adds engine columns before the bump.** The exact-equality guard rejects "unexpected columns" on the pre-bump build. Broken intermediate state. Option (a) chosen.
2. **Bumping everything in one PR.** Mixes a TS-only surface change (sso) with the DDL-mirror change. PR-A lands independently, ships value, and shrinks PR-B's blast radius. Split.
3. **Splitting the DDL regen from the pin bump into separate commits/PRs.** Every intermediate commit would fail `schema_guard::verify` on existing DBs. Same commit, by construction.
4. **Hand-editing `LLM_WIKI_PACKAGE_DDL`.** The file header says verbatim. Regenerate from `setupDatabase`.
5. **Freezing `handoff`/`session_recap` rows with no decision.** Leaves rows typed as node types no manifest declares — the node-type gate and heal cannot classify them. Kurt must pick re-typing or freeze-and-exclude; this spec records the option, not the decision.
6. **Waiting for the age gate on these packages.** Internal EA packages are exempt (owner ruling 2026-11-24).

## 7. Out of scope

1. The `area` column ingest mapping (areas spec §14.2 — separate decision).
2. Any new feature built on 9.x engine behavior beyond the mirrors listed here.
3. Other pinned packages (`@tiptap/*`, `@blocknote/*`, `@mantine/*`).
