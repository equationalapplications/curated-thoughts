# Spec: engine 9.x adoption (expo-llm-wiki 8.0.2 → 9.18.0)

Status: Draft
Owner: Kurt VanDusen / Hermes Agent
Date: 2026-11-24
Related issues: #342 (vector_removals ledger, non-blocking), #348 (sso 9.x node types, blocking), #353 (source_pointer mirror, blocking for the guard).
Authority check: this spec was checked against the repo's `INTENT.md` on 2026-11-24. INTENT imposes no rule on node-type vocabulary or pin bumps; its engine-mirror rule (§12, "The Rust side follows the engine's words") is exactly what PR-B implements. INTENT wins on any conflict.
Related spec: this is the "engine bump lane" deferred by `specs/2026-11-14-areas/spec.md` §14.1. That file lives on the UNMERGED areas branch (PR #352, branch `areas`), not on main; the citation names intent, and the areas lane owns the file. The `area` ingest-mapping decision (§14.2 there) stays deferred; it is not part of this spec.

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

Compatibility note (verified 2026-11-24): the published `schema-software-org` 9.18.0 tarball declares an exact dependency on `core-llm-wiki` 9.18.0. The CT pnpm override forces `core-llm-wiki` to 8.0.2 in every resolution, so the install resolves, but the declared range goes unsatisfied. The only core surface sso imports is the `OntologyManifest` type family, and `packages/core/src/types.ts` at tag v8.0.2 (lines 52-66) already defines `OntologyNodeType.parent_type` and the manifest shape sso 9.18.0 uses. PR-A must include a trial install with the bumped pin and lock the resulting `pnpm-lock.yaml` resolution in the PR, so the mismatched declared range is a recorded, reviewed state rather than a surprise at merge time.

Changes:

1. Update `src/__tests__/ontology-packages.test.ts` expectations: `creativework` children lose `handoff` and `session_recap`; add assertions for the new surface (`statement` with kinds `rule`, `decision`, `lesson`, `trap`; `area` node type; `about` edges) so the test pins the NEW contract, not just the absence of the old one.
2. Update `src-tauri/src/db/entity_gate.rs` `EA_SEED_TYPES`: remove `handoff` and `session_recap`; add the statement kinds and `area` per the 9.18.0 sso manifest. Update the pin test at `entity_gate.rs:1984-2008` to the new slug list and new count (the count is whatever the 9.18.0 manifest declares — the implementer must read the installed tarball and pin the actual list, not this spec's guess).
3. Update copy: `src/lib/ontology.ts:29` and `src-tauri/src/onboard/mod.rs:128` replace "handoffs" with the 9.x vocabulary wording (e.g. "Specs, statements, services" — final wording in implementation).
4. Sweep `src-tauri/src/onboard/mod.rs`, `src-tauri/src/wisdom_match.rs`, `src-tauri/src/db/wisdom.rs`, `src-tauri/src/db/evidence_reanchor.rs`, and `tools/src/cmds.rs` for `handoff`/`session_recap` logic branches (grep hits exist in all five) and state each disposition: re-map to the new statement kinds, or keep as legacy-data-only handling.

Data re-typing proposal (Kurt's decision — this spec does NOT implement a migration until he picks a mapping):

- Existing brain.db facts typed `handoff` or `session_recap` reference node types that no longer exist in the 9.18.0 manifest. KIND-STORAGE VERIFICATION (owner review item 2): the storage mechanism for statement kinds EXISTS at 8.0.2. In sso 9.x the four kinds are four manifest NODE TYPES (`rule`, `decision`, `lesson`, `trap` — `packages/schema-software-org/src/index.ts:45-46` at tag v9.18.0: "Statement kinds: one abstract parent, four content-defined children"), not a separate column. They live in the same `okf_type TEXT` column the engine has carried since the OKF migration (expo-llm-wiki `packages/core/src/db/schema.ts:22` at BOTH tags v8.0.2 and v9.18.0; mirrored verbatim in `src-tauri/src/db/okf_ddl.rs` `LLM_WIKI_PACKAGE_DDL` and in `schema_guard.rs` expectations). Neither tag adds a `kind` column to entries. So re-typing is a plain `UPDATE llm_wiki_entries SET okf_type = :slug` — the 8.0.2 build can hold `decision`/`lesson` slugs even though the 8.0.2 sso manifest does not declare them (the column is free-form TEXT; per-entry validation is the node-type gate, which PR-A updates in the same change). Proposed mapping:
  - `handoffFor` edges → `about` edges (same source node, same target node). EDGE ENDPOINT VERIFICATION (verified 2026-11-24 against the v9.18.0 sso manifest): `handoffFor` at v8.0.2 declares two shapes — `handoff` → `software_application` and `handoff` → `service`. The 9.18.0 `about` edges with source `statement` declare exactly three targets — `area`, `software_application`, `organization` (`packages/schema-software-org/src/index.ts:97-99` at v9.18.0). A `statement` → `service` edge has NO declared `about` shape, so mapped edges whose original target was a `service` node cannot become `about` rows without the node-type gate refusing them. The migration must classify every `handoffFor` edge by target type and dispose of `service`-targeted edges explicitly (drop with a logged count, or hold as an owner decision) — never rewrite them blind.
  - `handoff` → `decision` (a handoff is a decision about where work stands); `session_recap` → `lesson`. Since the kinds ARE node types, these are single `okf_type` rewrites — no composite "statement + kind" encoding.
- Implementation vehicle: a CT-side one-time migration on CT's OWN chain in `src-tauri/src/db/connection.rs` (CT numbering is independent of engine numbering; the chain is at V29 on main and moves to V30 when PR #352, the areas lane, merges). The data migration rides as a new local migration step on top of whatever the base branch carries at merge time — no hardcoded number in this spec. PR #352 owns V30; this spec's step takes the next free slot. The areas spec (`specs/2026-11-14-areas/spec.md` §14.1) already flags "heal for handoff and session_recap rows" as bump-lane work; this is it.
- REJECTED degrade variant: re-type to plain `statement` and defer kind enrichment to PR-B. Pointless: the kind IS the `okf_type` slug (see verification above), so `decision` costs the same one UPDATE as `statement` and preserves fidelity.
- Alternative if Kurt prefers no data rewrite: keep old-type rows frozen, exclude them from the node-type gate, and let heal re-anchor them. This loses type fidelity but touches nothing.
- DECISION REQUIRED from Kurt at spec review: re-type with the mapping above (evidence supports this — the storage mechanism exists at 8.0.2), or freeze-and-exclude. The only open product question is the semantic mapping (handoff→decision, session_recap→lesson), not feasibility.

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

Rust (repo is NOT a cargo workspace — always `--manifest-path`; export `CARGO_TARGET_DIR=<absolute repo root>/.cargo-shared-target` per the AGENTS.md single-build-location rule). Run `df -h "$(dirname "$CARGO_TARGET_DIR")"` before the first build. It must show at least 30G free:

- `cargo --manifest-path src-tauri/Cargo.toml test --lib --features test-utils` — covers the `ea_seed_types_match_spec_section_1_1` pin test, `schema_guard` expectations, and `okf_migration`.
- `cargo --manifest-path tools/Cargo.toml test --lib --features test-utils` — covers tool-side vocabulary.
- `cargo --manifest-path tools/Cargo.toml test --test ct_heal --features test-utils` — covers the heal vocabulary assertion at `tools/tests/ct_heal.rs:319`.

Frontend:

- `pnpm test -- src/__tests__/ontology-packages.test.ts` (or the repo's `package.json` test script scoped to that file) — new sso 9.x contract assertions.

New tests required in PR-B (both DB paths):

1. Fresh-install path: create a brand-new DB through the new `LLM_WIKI_PACKAGE_DDL`, run `schema_guard::verify` against the 9.18.0 expectations. Must pass.
2. Existing-DB path: take a DB fixture created under the 8.0.2 DDL, run CT's migration chain (including the new engine-mirror migrations), then run `schema_guard::verify`. Must pass. This test is the whole point of the same-commit rule — it fails if the DDL mirror and the pin ever drift apart again.
3. Round-trip: open the migrated fixture with the OLD pinned build (CI job or a pinned-version test) and assert the guard bails with the documented "unexpected columns" message — the intended forward-incompat behavior, documented rather than silent. Mechanism: PR-B must check in an 8.0.2 expectation fixture (a snapshot of the current `TableExpectation` sets plus the 8.0.2 `LLM_WIKI_PACKAGE_DDL` text, e.g. under `src-tauri/src/db/testdata/`), and the test runs `schema_guard::verify` with the NEW guard pointed at that fixture's DB. The old expectations therefore live in the fixture, not in the deleted guard code.

PR-A tests:

- Updated `ontology-packages.test.ts` asserts the 9.18.0 sso manifest (statement kinds, `area`, `about`; no `handoff`/`session_recap`).
- Updated `entity_gate.rs` pin test asserts the new slug list and count.
- If Kurt approves re-typing: a migration test seeds `handoff`/`session_recap` rows in an 8.0.2-era fixture and asserts `okf_type` = `decision`/`lesson` and the mapped `about` edges after migration.

## 6. Rejected alternatives

1. **Option (b) for guard ordering — CT pre-adds engine columns before the bump.** The exact-equality guard rejects "unexpected columns" on the pre-bump build. Broken intermediate state. Option (a) chosen.
2. **Bumping everything in one PR.** Mixes a TS-only surface change (sso) with the DDL-mirror change. PR-A lands independently, ships value, and shrinks PR-B's blast radius. Split.
3. **Splitting the DDL regen from the pin bump into separate commits/PRs.** Every intermediate commit would fail `schema_guard::verify` on existing DBs. Same commit, by construction.
4. **Hand-editing `LLM_WIKI_PACKAGE_DDL`.** The file header says verbatim. Regenerate from `setupDatabase`.
5. **Freezing `handoff`/`session_recap` rows with no decision (option c — defer migration to PR-B).** Leaves rows typed as node types no manifest declares through the entire PR-A window — the node-type gate and heal cannot classify them the day PR-A merges. Feasibility is settled (kind storage verified at 8.0.2), so deferral buys nothing.
6. **Degrade migration in PR-A: old types → plain `statement`, kind enrichment in PR-B (option b).** Pointless intermediate: the kind is the `okf_type` slug itself, so the `statement`-only rewrite and the `decision`/`lesson` rewrite are the same one-UPDATE migration. Two passes double the migration code for zero fidelity gain.
7. **Waiting for the age gate on these packages.** Internal EA packages are exempt (owner ruling 2026-11-24).

## 7. Out of scope

1. The `area` column ingest mapping (areas spec §14.2 — separate decision).
2. Any new feature built on 9.x engine behavior beyond the mirrors listed here.
3. Other pinned packages (`@tiptap/*`, `@blocknote/*`, `@mantine/*`).
