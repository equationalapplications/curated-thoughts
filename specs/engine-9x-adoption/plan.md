# Plan: engine 9.x adoption — PR-A (schema-software-org 9.x readiness)

Status: Draft (plan author, 2026-11-24). Transient scaffolding — deleted in the
same-PR cleanup commit.
Spec: `specs/engine-9x-adoption/spec.md` (Approved 2026-11-24). Spec wins on any
conflict with this plan.
Scope: spec §3 PR-A only. PR-B (core bump, DDL regen, `ddl_compat` extractor)
is out of scope. PR-A must not break PR-B's landing surface.
Base: origin/main `39fc129f`. Branch `chore/sso-9x-readiness`. Worktree
`.worktrees/sso-9x`.

## 0. Ground rules for every task

### 0.1 Environment preamble (run once per shell)

Pins (`.superpowers/sdd/engine-9x-pr-a/pins.md`) name the canonical build
location for this machine. Q7 RULING 2026-11-24: AGENTS.md wins — use the
repo-root canonical dir. Export these before any cargo command:

```bash
export CARGO_TARGET_DIR=<absolute repo root>/.cargo-shared-target
export CARGO_PROFILE_DEV_DEBUG=0
export CARGO_PROFILE_TEST_DEBUG=0
```

- Every cargo gate runs through the heavy-build wrapper (F5): the invocation is `~/.hermes/bin/heavy-build --need-mb 3500 --mem-high 4500 --mem-max 6000 -- <the bare gate command>`. Receipts record BOTH the wrapped invocation line and the bare command. Small single-file checks may run bare.
- Pre-flight before each cargo gate: `df -h "<repo-root>"` must show ≥30G free (AGENTS.md floor).
- Below 30G free: STOP and report to the controller. Never delete caches.
- On linker `Bus error`, `ENOSPC`, or `os error 122`: STOP and report. This is an environment failure.
- `--manifest-path` goes AFTER the subcommand (cargo 1.96 rejects the other form).
- `tools/Cargo.toml` has no `test-utils` feature. Never pass `--features test-utils` there.
- Cargo work runs serial. Never run two cargo gates at once.
- Commit sequencing (F9b): Tasks 1 and 2 land as ONE commit (Task 1's drift test turns the old expectations RED; Task 2 re-pins them). Tasks 3–6 commit per task.

### 0.2 Canonical gate commands

Each gate is one bare command. Run each separately; capture exit code and tail.

- G-tauri: `cargo test --manifest-path src-tauri/Cargo.toml --lib --features test-utils -j 2 -- --test-threads=1`
- G-tools: `cargo test --manifest-path tools/Cargo.toml --lib -j 2 -- --test-threads=1`
- G-heal: `cargo test --manifest-path tools/Cargo.toml --test ct_heal -j 2 -- --test-threads=1`
- G-vitest: `pnpm vitest run`
- G-tsc: `pnpm typecheck`

A filtered Rust gate appends the test-name filter before `--`. Example:
`cargo test --manifest-path src-tauri/Cargo.toml --lib --features test-utils -j 2 entity_gate::tests -- --test-threads=1`

### 0.3 Baseline (controller-measured on `39fc129f`)

| Gate | Result |
|---|---|
| G-tools | 61 passed, 0 failed, exit 0 |
| G-tauri | 1642 passed, 0 failed, exit 0 (138.2s) |
| G-vitest | 96 files, 619 passed + 1 skipped, exit 0 |
| G-tsc | exit 0 |

Receipts: `.superpowers/sdd/engine-9x-pr-a/receipts/baseline-*.out`. Every
task's evidence must compare against these numbers. A count drop with no
matching planned deletion is a regression.

### 0.4 Evidence rule

Each task writes its receipts to `.superpowers/sdd/engine-9x-pr-a/receipts/`
as `t<N>-<gate>.out`. The LEDGER tick links the receipt path. A RED-first
step records the failing run too, as `t<N>-<gate>-red.out`.

### 0.5 Open questions (controller must rule; see §3)

Tasks name the open question that blocks them. The implementer must not
start a blocked step until the controller records a ruling in LEDGER.md.

## 1. Tasks

### Task 1 — sso pin bump, trial install, lockfile, vendored snapshots, drift test

Scope: spec §3 PR-A header + compatibility note; §3 change 5 (vendored
snapshot + drift test); §4 `package.json`, `pnpm-lock.yaml`.

Blocked by: Q9 (shared `node_modules` symlink).

Steps:

1. Before any package change, capture the INSTALLED 8.0.2 sso manifest as JSON.
   Write it to `src-tauri/src/db/testdata/schema-software-org-8.0.2.manifest.json`.
   Use `JSON.stringify(manifest, null, 2)` plus a trailing newline.
   Task 4's "8.x-era manifest" fixture and Task 1's diff read this file.
2. Edit `package.json` line 52 only: `@equationalapplications/schema-software-org` to `"9.18.0"`.
   Leave both overrides (lines 8-9) and lines 49-51 at `8.0.2`.
3. Run `pnpm install`. Save the full output as `t1-install.out`.
   Any unmet-dependency warning for `core-llm-wiki` 9.18.0 is EXPECTED (spec compat note). Record it verbatim.
4. Prove the override held: `pnpm why @equationalapplications/core-llm-wiki`. Every resolution must show 8.0.2.
5. Commit the resulting `pnpm-lock.yaml` change in the same commit as `package.json`.
   The lockfile diff must touch only the sso entry and its dependency edges. Any other churn must be reported.
6. Capture the INSTALLED 9.18.0 manifest as JSON (same formatting rule as step 1).
   Write it to `src-tauri/src/db/vendored/schema-software-org-9.18.0.manifest.json`.
   Reason for `vendored/` over the spec's example `testdata/`: production code `include_str!`s this file (Task 4 reseed source).
7. Cross-check: the vendored 9.18.0 JSON must deep-equal the controller's extracted tarball manifest.
   Tarball path: `.superpowers/sdd/engine-9x-pr-a/sso-9.18.0-tarball/dist/index.js` (no `package/` level).
8. Compute and record the 8.0.2→9.18.0 manifest diff in LEDGER.md (node slugs and edge `(type, source_type, target_type)` triples).
   Expected node diff: −`handoff`, −`session_recap`; +`statement`, `rule`, `decision`, `lesson`, `trap`, `area`.
   Expected edge diff: −2 `handoffFor`; +3 `about` (statement→area/software_application/organization); plus whatever else the diff shows (see Q2).
   Any diff beyond the expected set must be escalated before Task 4 starts.
9. Add the drift test to `src/__tests__/ontology-packages.test.ts` (a §4-listed file).
   It must import `schemaSoftwareOrgManifest` and deep-equal it against the vendored 9.18.0 JSON.
   The failure message must name the regenerate command from step 6.
   This test is ALSO the spec §5 runtime-load evidence (F8): extend it (or add a sibling case) that passes `schemaSoftwareOrgManifest` through the same manifest-loading code CT uses (`src/lib/ontology.ts` seed path) under the 8.0.2 core override. `pnpm build` in Task 6 remains bundling evidence only.
   (Former step 10 — Rust parse test beside the `include_str!` — MOVED to Task 4d per F4: the include does not exist until Task 4, and Task 1 has no cargo gate. Note for Task 4d: `parse_manifest_value` takes a `&serde_json::Value`, so the test must `serde_json::from_str` first.)

Gates:

- `pnpm why @equationalapplications/core-llm-wiki`
- `pnpm vitest run src/__tests__/ontology-packages.test.ts` (drift test GREEN; old expectations RED until Task 2 — record both)
- `pnpm typecheck`

Expected evidence: install log with the recorded unmet-range warning; `pnpm why` showing only 8.0.2; lockfile diff scope; vendored JSON paths; manifest diff table in LEDGER; drift test passing.

Ledger text: `T1 — sso 9.18.0 pin + trial install + lockfile locked; 8.0.2 and 9.18.0 manifest snapshots vendored; drift test green; manifest diff recorded`

### Task 2 — frontend test + copy updates

Scope: spec §3 changes 1 and 3 (frontend half); §5 PR-A tests 1 and smoke test.

Steps:

1. RED first: run the existing `ontology-packages.test.ts` against 9.18.0. Record the failures (17→21 nodes, 40→42 edges, creativework children).
2. Rewrite the assertions to pin the NEW contract, not just the absence of the old one:
   - node-type count 21; edge-type count 42 (re-derive both from the installed manifest; do not trust these numbers blind).
   - `creativework` children equal `['design_spec', 'procedure', 'reference_doc']`.
   - `statement` exists with no `parent_type`; its children equal `['decision', 'lesson', 'rule', 'trap']`.
   - `area` exists as a node type.
   - `about` edges with `source_type: 'statement'` target exactly `area`, `organization`, `software_application`.
   - No node type `handoff` or `session_recap`; no edge type `handoffFor`.
3. Edit `src/lib/ontology.ts:29`: `'Specs, handoffs, services'` → `'Specs, statements, services'` (RATIFIED wording).
4. Grep `src/` for any test asserting the old sub-label string. Update any hit in the same commit.

Gates:

- `pnpm vitest run src/__tests__/ontology-packages.test.ts`
- `pnpm vitest run`
- `pnpm typecheck`

Expected evidence: RED receipt, then GREEN receipt; full vitest at 96 files (or more, if a file was added), 0 failed; tsc exit 0.

Ledger text: `T2 — ontology-packages.test.ts pins the sso 9.x contract (statement kinds, area, about); ontology.ts copy → "Specs, statements, services"; vitest + tsc green`

### Task 3 — Rust vocabulary: EA_SEED_TYPES, pin test, ALIAS_TABLE, planner epoch, onboard copy, sweep

Scope: spec §3 changes 2, 3 (Rust half), 4; the ALIAS_TABLE extension from
"SCOPE OF THE RE-TYPE".

Blocked by: Q1 (seed count 20 vs 21) for the final pin value only.

Steps:

1. Re-derive the EA seed list from the INSTALLED 9.18.0 manifest (the vendored JSON), not from the spec's arithmetic.
   Rule (spec change 2): every declared slug EXCEPT the abstract parent `statement`.
   Record the derived list and its count in LEDGER. Expected: 20 slugs (see Q1).
2. RED first: change only the pin test `ea_seed_types_match_spec_section_1_1` (`src-tauri/src/db/entity_gate.rs:1984-2008`) to the derived list and count. Run it; record RED.
3. Update `EA_SEED_TYPES` (`src-tauri/src/db/entity_gate.rs:82`): drop `handoff`, `session_recap`; add `area`, `decision`, `lesson`, `rule`, `trap`.
   Keep the alphabetical order. Do NOT add `statement`. Update the doc comment's "17" and spec reference.
4. Extend `ALIAS_TABLE` (`src-tauri/src/db/entity_gate.rs:107`) with `("handoff", "decision")` and `("session_recap", "lesson")`.
   Honor the table's doc rule ("adding a key means adding the value"); check for an `into_iter` arm that must match.
5. Add alias-ladder tests: `handoff` lands `decision` and `session_recap` lands `lesson` against a 9.x manifest.
   Add the undeclared-target case: against a manifest without `decision`, `handoff` falls through to fallback.
6. Bump `ENSURE_PLANNER_EPOCH` (`src-tauri/src/db/entity_gate.rs:1085`) from `"r26"` to the next epoch (e.g. `"r27"`).
   Reason: the code's own rule requires a bump whenever the planner can return a different verdict for the same bytes. Changing `EA_SEED_TYPES` does exactly that. See Q10.
7. Update every test that builds a manifest from `EA_SEED_TYPES` (`entity_gate.rs` ~2177-3677). They read the const, so most need no edit. Run them and fix only real breaks.
8. Edit `src-tauri/src/onboard/mod.rs:128`: `specs, handoffs, services, procedures` → `specs, statements, services, procedures`.
9. Sweep the quoted literals `"handoff"` and `"session_recap"` (case-sensitive, code only). Record each disposition in LEDGER. Known hits at base:
   - `entity_gate.rs:87`, `:98` (const) → removed by step 3.
   - `entity_gate.rs:1990`, `:2001` (pin test) → re-pinned by step 2.
   - `tools/tests/ct_heal.rs:308`, `:319` (`seed_tier_fact_manifest`) → re-pin the fixture to the new seed list, built from `curated_thoughts::…::EA_SEED_TYPES` if the tools crate can see it. (F2, RULED: the "keep as 8.x-era fixture" option is INVALID — `init_brain_db` goes through `AppDb::open_with_config`, so `migrate()` stamps N before the fixture seeds, and the next open would never refresh it; against 20-slug `EA_SEED_TYPES` the old manifest then fails `is_ea_subset` and the tests would quietly exercise the foreign path.) 8.x-era refresh coverage lives in Task 4's tempfile tests.
   - `src/__tests__/ontology-packages.test.ts:23`, `:26` → handled by Task 2.
   - `docs/mcp-write-tools-okf-frontmatter.md` → prose examples of `entry_type`, not logic. Disposition: no change (out of the spec's "logic hits" scope).
   - CAUTION: `flag_handoffs_planned` in `src-tauri/src/db/evidence_reanchor.rs` and `tools/src/cmds.rs` is unrelated. Do not touch it.
   - `wisdom.rs`, `wisdom_match.rs`: prose comments only. No change.
   - Any NEW hit the sweep finds must be reported before it is changed.

Gates:

- `cargo test --manifest-path src-tauri/Cargo.toml --lib --features test-utils -j 2 entity_gate -- --test-threads=1`
- `cargo test --manifest-path tools/Cargo.toml --test ct_heal -j 2 -- --test-threads=1`
- `cargo test --manifest-path tools/Cargo.toml --lib -j 2 -- --test-threads=1`

Expected evidence: RED pin-test receipt; GREEN `entity_gate` receipt; `ct_heal` GREEN; tools lib 61 passed; sweep disposition table in LEDGER.

Ledger text: `T3 — EA_SEED_TYPES re-pinned from installed 9.18.0 manifest (count recorded); ALIAS_TABLE handoff→decision, session_recap→lesson; planner epoch bumped; onboard copy → statements; sweep dispositions recorded`

### Task 4 — migration step 1: stored-manifest MERGE refresh

Scope: spec §3 change 5; implementation-vehicle note (slot ordering); SAFETY REQUIREMENTS.

Blocked by: Q2 (`partOf` edge), Q6 (slot numbering), Q12 (MERGE eligibility), Q13 (runtime disk check).

#### 4a. Slot computation (spec: "the next free slot" is literal)

1. Sync first: `git fetch origin` then check whether origin/main moved (PR #368). If it moved, `git merge origin/main` (never rebase).
2. Compute N = (highest `INSERT OR IGNORE INTO schema_version (version) VALUES (<n>)` literal in `src-tauri/src/db/connection.rs`) + 1.
   At base this is expected to give N = 31; compute it, do not assume it.
3. Define two named consts in `connection.rs`: the refresh step at N, the re-type step at N + 1.
   Every stamp, gate, test, and log line reads the consts. No bare `31`/`32` literal anywhere.
4. Record N in LEDGER. If another lane takes N before merge, renumber the two consts (V27 renumber precedent, `connection.rs:831`).

#### 4b. Gate shape

1. The step runs when version N is NOT stamped. Check by exact membership (`SELECT 1 FROM schema_version WHERE version = N`), not `MAX(version) < N`.
   Reason: `MAX`-based gating would skip PR-A forever if a later slot lands first.
2. The step runs only when V22 has stamped (`MAX(version) >= 22`), like every stamp since V23. A deferred-V22 open logs a skip and does nothing.
3. Place the call in `migrate()` right after `apply_v30_area_column(conn)?` (`connection.rs:863`), before the `0001` migration file.

#### 4c. Safety requirements (HARD — each is its own checklist line)

- [ ] S1 Path probe: read the DB file path from `conn.path()`. `None` or empty (in-memory/unknown) → log a WARN, skip backup AND migration, do not stamp (precedent `connection.rs:441`, but stricter: the step does not run unbacked).
- [ ] S2 Disk check: read free space on the DB's filesystem (`fs4` is already a dependency) and the DB file size plus its `-wal`. Free space below that size plus a margin → treat as backup failure (S4). See Q13.
- [ ] S3 Backup: `VACUUM INTO` a timestamped file next to brain.db, BEFORE `BEGIN IMMEDIATE` (it cannot run inside a transaction). Never a bare file copy.
  Reuse or mirror `backup_before_apply` (`src-tauri/src/db/evidence_reanchor.rs:695`). Name: `<stem>.bak-pre-sso9-<ts>`.
  Log the backup path and a row-count sanity check (row counts of `llm_wiki_entity_manifests`, `llm_wiki_entries`, `llm_wiki_edges` in source vs backup).
- [ ] S4 Backup failure: return `Err` from `migrate()` so the open ABORTS. Do not stamp N. The next open retries.
- [ ] S5 Dry pass: before any write, compute the census and write it to the migration log (eprintln with a `[ct::migrate]` prefix). For step 1: manifests total, eligible (8.x EA pattern), foreign/untouched, malformed.
- [ ] S6 Transaction: `BEGIN IMMEDIATE`; re-check N unstamped under the lock (concurrent opens); apply; stamp N; `COMMIT`. Any error → `ROLLBACK` and propagate.
- [ ] S7 Idempotency: running the step body twice is a no-op. Pinned by a test (4e).

Backup sharing: step 2 (Task 5) may reuse a backup step 1 took in the same `migrate()` call. Otherwise step 2 takes its own.

#### 4d. MERGE semantics

1. Add the vendored include: `include_str!("vendored/schema-software-org-9.18.0.manifest.json")`.
2. Put the pure planner in `src-tauri/src/db/entity_gate.rs`, beside `plan_manifest_ensure` (keeps §4's file list).
   Signature shape: `plan_sso9_manifest_refresh(manifest_json: &str) -> Result<RefreshPlan>` with `Refresh { new_json } | Untouched(reason) | Malformed(err)`.
3. Eligibility (Q12): the manifest passes the OLD 17-slug EA subset check through `NodeVocabulary` (case-insensitive key).
   Keep the old list as a private const (e.g. `EA_SEED_TYPES_SSO_8X`) inside the planner. Not eligible → `Untouched`.
4. Edits on an eligible manifest, nothing else:
   - remove node types `handoff`, `session_recap` (membership key, both entry shapes: `{type: …}` objects and bare strings);
   - remove every edge with type `handoffFor`;
   - add `statement`, `rule`, `decision`, `lesson`, `trap`, `area` from the vendored JSON (with `description` and `parent_type`), skipping any already declared;
   - add the new edge shapes from the vendored JSON (`about` ×3 and `partOf` — Q2 RULED YES, spec E2), skipping any already declared by `(type, source_type, target_type)`;
   - preserve every other key and entry, and the order of array entries (F6: serde_json has no `preserve_order`, and the ensure already round-trips through `serde_json::Value` (`entity_gate.rs:825`), so byte-for-byte serialization is not achievable — tests must compare PARSED `Value`s for content+array-order equality; the compare-and-swap UPDATE stays as designed). Do not touch `fallback_node_type` here.
5. Apply: `UPDATE llm_wiki_entity_manifests SET manifest_json = ? WHERE entity_id = ? AND manifest_json = ?` (compare-and-swap, as the ensure does). Include the `tier_fact` row.
6. After `COMMIT`, call `ensure_manifest_vocabulary` for every refreshed entity id (spec: restore `document`/`process` and `fallback_node_type`).
   It opens its own transaction, so it must run after `COMMIT`, never inside the step's `BEGIN IMMEDIATE`. Log each outcome; a per-row failure is logged, not fatal (ensure is best-effort by design).

#### 4e. Tests (RED first where behavior is new)

All migration tests must use a tempfile-backed DB. `open_in_memory()` skips the step by S1.

- Planner unit tests: an 8.0.2 manifest (from the Task 1 testdata JSON) refreshes to the expected shape; a customised 17+extras manifest keeps its extras; a foreign manifest is `Untouched`; malformed JSON is `Malformed`; bare-string node types refresh.
- Spec test (§5 "Manifest-refresh test"): an entity holding an 8.x-era stored manifest passes `is_ea_subset` after the constants change, and classifies `decision`/`lesson` as declared.
- Backup test: after migrate on a tempfile brain, a `brain.db.bak-pre-sso9-*` file exists and opens with matching row counts.
- Backup-failure test: force the backup to fail (e.g. read-only target dir). `migrate()` returns `Err` and N is not stamped.
- In-memory test: `open_in_memory()` leaves N unstamped and manifests unchanged.
- Idempotency test: migrate twice; manifests, stamps, and memo rows are identical after the second run.
- Body-level idempotency test (F3): delete the stamp row (or invoke the step body with the stamp check bypassed) and run the body again on the already-migrated DB; assert zero rows change and the manifest/edge/memo snapshots are identical. The double-`migrate` test alone only exercises the stamp check, not the body.
- Ensure re-run test: a refreshed EA manifest gains `document`, `process`, and `fallback_node_type`.

Gates:

- `cargo test --manifest-path src-tauri/Cargo.toml --lib --features test-utils -j 2 entity_gate -- --test-threads=1`
- `cargo test --manifest-path src-tauri/Cargo.toml --lib --features test-utils -j 2 db::connection -- --test-threads=1`

Expected evidence: slot N recorded; RED receipts for new tests; GREEN receipts; a captured migration log line showing backup path, sanity counts, and census.

Ledger text: `T4 — migration step 1 (slot N, computed) MERGE-refreshes 8.x EA manifests from the vendored 9.18.0 snapshot; backup/abort/in-memory-skip/dry-pass/idempotency each pinned by a test; ensure re-run after commit`

### Task 5 — migration step 2: okf_type and edge re-type

Scope: spec §3 data re-typing decision (all sub-bullets); SAFETY REQUIREMENTS; §5 PR-A migration tests.

Blocked by: Q3 (`curated_entities.entity_type`), Q4 (endpoint type resolver), Q5 (undispositioned edge classes).

#### 5a. Gate shape and safety

Same as Task 4 (4b, 4c) at slot N + 1, with these additions:

- The step must run only when N (the refresh) is stamped. The refresh must precede the re-type.
- S5 dry-pass census for this step, written to the migration log before any write:
  - `llm_wiki_entries` rows per `okf_type` in (`handoff`, `session_recap`), live and tombstoned separately;
  - `llm_wiki_tasks` rows per same `okf_type`, live and tombstoned;
  - case-variant rows (`lower(trim(okf_type))` matches but the exact slug does not) — counted, not rewritten;
  - `handoffFor` edges by resolved target type (Q4 resolver);
  - per edge type, edges touching (source OR target) a row being re-typed;
  - pending `curated_proposal_items` whose payload carries `handoff`/`session_recap` types or `handoffFor` edges (counted, never rewritten);
  - `curated_entities` rows with `entity_type` in (`handoff`, `session_recap`) (counted; re-typed per E3 — see writes step 8).

#### 5b. Writes (single `BEGIN IMMEDIATE` transaction)

Order matters: compute every edge disposition BEFORE the `okf_type`/`entity_type` rewrites (all three tables) — the Q4 resolver reads `curated_entities.entity_type`, so re-typed rows must still carry their OLD type while edges are classified. After the rewrites they no longer do.

1. Snapshot the re-typed id set: entry ids with old `okf_type` in the two slugs PLUS `curated_entities.id` with `entity_type` in the two slugs (tombstones included in both). This set drives the edge steps below.
2. `handoffFor` → `about`, where BOTH the source_id is in the snapshot set AND the target type is `software_application` (or a declared `statement` target): `UPDATE OR IGNORE llm_wiki_edges SET edge_type = 'about' WHERE …` (F7: edges whose source is not in the set go to the untouched count — an untypeable source would create an undeclared shape).
   Then `DELETE` the leftover `handoffFor` rows that lost the UNIQUE race. Log the deduped count.
3. `handoffFor` with target type `service`: `DELETE`, log the count (RATIFIED drop-with-count).
4. Parent-family edges touching a snapshot-set row whose re-typed shape is undeclared: `DELETE`, log a count per edge type.
   Drop set per spec: `author`, `publisher`, `about`→person/place/event, `supersedes`.
   Keep set per spec: `about`→`organization` (becomes a declared `statement`→`organization` shape).
5. Any edge class outside the spec's keep/drop lists (Q5): leave untouched, log the count as `undispositioned`.
6. `UPDATE llm_wiki_entries SET okf_type = 'decision' WHERE okf_type = 'handoff'` and `'lesson'` for `'session_recap'`. Tombstones included (no `deleted_at` filter).
7. Same two UPDATEs on `llm_wiki_tasks.okf_type`, tombstones included.
8. `UPDATE curated_entities SET entity_type = 'decision' | 'lesson'` for the two dropped slugs (E3, F1) — AFTER the edge steps; tombstones included; log the count. Do NOT touch `concept`, `process`, `repository`, `job` or any other pre-existing type (E3 scope fence).
9. Stamp N + 1; `COMMIT`.
10. After COMMIT + dry-pass verify: delete the backup file created earlier in this step (F10 ruling — the backup has served its purpose once the step commits; kills the fresh-brain `.bak` litter without weakening the backup-before-dry-pass safety order, which is UNCHANGED per spec).

INTENT §12 constraints (spec): do NOT touch `llm_wiki_outbox`; do NOT rewrite `updated_at`; leave `ontology_checked_at` and `heal_checked_at` alone.
The PR body must record the §12 divergence: no engine bulk re-type operation exists, so these raw writes are the recorded boundary case.

#### 5c. Tests (RED first)

All on tempfile-backed DBs seeded as an 8.0.2-era brain.

- Re-type test (RATIFIED): seeded `handoff`/`session_recap` entries end as `decision`/`lesson`; mapped `about` edges exist.
- Tombstone test: a tombstoned `handoff` entry is re-typed to `decision`.
- Tasks test: `llm_wiki_tasks.okf_type` re-typed the same way.
- Collision test (spec §5): an entity holding BOTH `handoffFor` and `about` on the same (entity, source, target). Migration succeeds; the pre-existing `about` row survives; the `handoffFor` row is gone; the deduped count is logged.
- Service-drop test: `handoffFor` → `service` edge is deleted; the drop count is logged.
- Parent-family test: one edge per drop class is deleted with per-type counts; an `about`→`organization` edge survives.
- Curated-entity endpoint test (F1): a `handoffFor` edge whose SOURCE is a `curated_entities` row typed `handoff` re-maps to `about` (or is dropped per class), proving the snapshot set in writes step 1 includes curated entities and the edge steps see them.
- Untouched-columns test: `updated_at`, `ontology_checked_at`, `heal_checked_at`, and `llm_wiki_outbox` are byte-identical before and after.
- Trigger-coverage check (F11): before trusting the untouched-columns test, check `setupDatabase` at the 8.0.2 core tag for triggers on `llm_wiki_entries`, `llm_wiki_edges`, `curated_entities` (spec §2.4 names `createResurrectTrigger`). If any fire on the re-typed columns, recreate them in the Rust fixture first; otherwise record the absence check as the evidence.
- Entity-type test (E3): seeded `curated_entities` rows typed `handoff`/`session_recap` end as `decision`/`lesson`; rows typed `concept`/`process`/`repository`/`job` are untouched; the counts land in the census log.
- Order test: the re-type step does not run when N is unstamped.
- Idempotency test: migrate twice; second run changes zero rows and stamps nothing new.
- Body-level idempotency test (F3): same as Task 4e's — run the re-type body a second time with the stamp check bypassed; assert zero rows change.
- Census-log test: the dry-pass log line carries every census count (capture via the existing log/test pattern in `connection.rs` tests).
- Alias test (end-to-end): after migration, a pending payload typed `handoff` admits as `decision` through the gate.

Gates:

- `cargo test --manifest-path src-tauri/Cargo.toml --lib --features test-utils -j 2 db::connection -- --test-threads=1`
- `cargo test --manifest-path src-tauri/Cargo.toml --lib --features test-utils -j 2 entity_gate -- --test-threads=1`

Expected evidence: RED then GREEN receipts; a captured log showing census, backup path, deduped/dropped counts.

Ledger text: `T5 — migration step 2 (slot N+1) re-types entries+tasks+touched curated_entities (tombstones included) handoff→decision, session_recap→lesson; handoffFor→about via UPDATE OR IGNORE + dedupe; service and undeclared parent-family edges dropped with counts; safety + idempotency pinned`

### Task 6 — full-suite green, smoke, e2e fixture check, PR body inputs

Scope: spec §5 PR-A test list; the smoke test; post-session sweep.

Steps:

1. Re-sync with origin/main if it moved (merge, never rebase). Re-check slot N (Task 4a step 2) after the merge.
2. Run every gate in §0.2, one at a time, with the disk pre-flight before each cargo gate.
3. Smoke test (spec §5): run the `package.json` `build` script. It must import sso 9.18.0 and load its manifest under the 8.0.2 core override.
4. e2e fixture check: grep e2e/fixture paths for `handoff`, `session_recap`, `handoffFor`, `Specs, handoffs`. Base had zero hits; record the re-grep.
5. Draft the PR-body inputs in LEDGER: the §12 divergence note, the recorded unmet `core-llm-wiki` range, the sweep table, slot N, and the open-question rulings.
6. Post-session sweep (AGENTS.md): `du -sh "$REPO_ROOT/.cargo-shared-target"` (Q7 — the canonical dir, not /mnt/kv-work), check for stray `target/` dirs in the worktree, record `df -h` on both `/` and `/mnt/kv-work`.
7. Controller-owned, post-merge (Q9): run `pnpm install` in the MAIN checkout so its shared `node_modules` reconciles with the merged lockfile. Not an implementer step.

Gates:

- `cargo test --manifest-path src-tauri/Cargo.toml --lib --features test-utils -j 2 -- --test-threads=1`
- `cargo test --manifest-path tools/Cargo.toml --lib -j 2 -- --test-threads=1`
- `cargo test --manifest-path tools/Cargo.toml --test ct_heal -j 2 -- --test-threads=1`
- `pnpm vitest run`
- `pnpm typecheck`
- `pnpm build`

Expected evidence: G-tauri ≥ 1642 + new tests, 0 failed; G-tools 61 passed (unless Task 3 changed a tools lib test — record it); G-heal green; vitest 0 failed; tsc exit 0; build exit 0; disk sweep numbers.

Ledger text: `T6 — all gates green vs baseline (counts recorded); frontend build smoke passes under 8.0.2 override; e2e fixture re-grep clean; PR-body inputs drafted; disk sweep recorded`

## 2. Risks

1. **Build-cache disk pressure.** The canonical target is `<repo-root>/.cargo-shared-target` (Q7 ruling) on `/`, which had 176G free at the Q7 ruling — above the AGENTS.md 30G floor (F9: the pre-Q7 "floor already fails" text was stale). Every cargo gate still pre-flights `df -h "<repo-root>"`. Below the floor, STOP and report; cache deletion is a controller decision only. `/mnt/kv-work` (18G free) hosts only the superseded 86G tree from the baseline.
2. **Declared-vs-actual `core-llm-wiki` range mismatch.** sso 9.18.0 declares `core-llm-wiki` 9.18.0 exact; the override forces 8.0.2. Accepted, recorded state per the spec compat note. The lockfile and install log are its review record. PR-B resolves it.
3. **PR #368 merging mid-lane.** File-disjoint except possibly `connection.rs` migration slots. Sync by `git merge origin/main`, never rebase. Recompute slot N after every merge (Task 4a).
4. **Shared `node_modules`.** The worktree's `node_modules` symlinks to the main checkout (`../../node_modules`). A `pnpm install` here would bump sso for the main checkout and every other worktree. See Q9.
5. **Backup aborts the open.** Spec mandates abort-without-stamp on backup failure. A persistently full disk therefore blocks every open until space is freed. The log line must say exactly that and name the fix.
6. **Fresh-brain backup cost.** Every file-backed first open (including tests) takes a `VACUUM INTO` of a near-empty DB. Cheap, but it litters test temp dirs and fresh brain dirs with one backup file.
7. **Planner memo staleness.** Without the epoch bump (Task 3 step 6), memos recorded under the 17-slug set short-circuit the new verdict on unchanged bytes.
8. **Ensure re-run inside a transaction.** `ensure_manifest_vocabulary` opens its own transaction; calling it inside the step's `BEGIN IMMEDIATE` fails. Task 4d step 6 orders it after `COMMIT`.
9. **PR-B landing surface.** PR-A must not edit `okf_ddl.rs`, `schema_guard.rs`, `ddl_compat.rs`, or the overrides. The slot consts keep PR-B free to take the next free slot after PR-A.

## 3. Open questions and spec/brief conflicts (controller must rule)

> **RULED 2026-11-24 — all 13 questions resolved by the controller; see the
> rulings table in LEDGER.md and the spec's new §8 Errata (E1–E4), committed
> in this PR. Headlines: EA_SEED_TYPES pins 20 slugs (E1); the MERGE/drift
> surface includes `partOf` alongside the three `about` shapes (E2); the
> re-type adds `curated_entities.entity_type` for the two dropped slugs only
> (E3); implementation gates run under `CARGO_TARGET_DIR=<repo-root>/.cargo-shared-target`
> per AGENTS.md (Q7); T6 gains a post-merge `pnpm install` reconciliation of
> the main checkout (Q9). The Q texts below are retained verbatim for the
> review ladder's provenance.**

- **Q1 — EA seed count.** Spec change 2 and §5 say 21. Derivation per the spec's own rule (installed manifest slugs minus abstract `statement`) gives 20: 17 − 2 + 5. The installed manifest has 21 slugs only because it includes `statement`. Plan pins the derived value per "re-pins from the installed tarball, not this spec's arithmetic"; controller confirms.
- **Q2 — `partOf` edge in the MERGE.** Spec change 5 says add "the new edge shapes (`about`)". The 8.0.2 test pins 40 edges, 9.18.0 has 42, and 2 `handoffFor` shapes leave. So 4 shapes are new: 3 `about` plus `partOf` (area→software_application) by inspection. Spec is silent on `partOf`. Task 1 step 8 confirms the diff.
- **Q3 — `curated_entities.entity_type`.** The node-type gate writes `curated_entities.entity_type` (`entity_gate.rs:615-640`). The spec's re-type scope names only `llm_wiki_entries.okf_type` and `llm_wiki_tasks.okf_type`. Entities typed `handoff`/`session_recap` have no disposition. Plan counts them only.
- **Q4 — edge endpoint type resolution.** Edge endpoints may be fact ids or curated-entity ids. The spec says "classify by target type" but names no resolver. Proposed: `llm_wiki_entries.okf_type` when the id names an entry, else `curated_entities.entity_type`, else `unresolved` (census bucket, untouched).
- **Q5 — undispositioned edge classes.** Not covered by the spec: `handoffFor` to targets other than `software_application`/`service`; `itemReviewed` targeting a re-typed row (spec says review-sourced edges cannot touch these rows, but parent-aware `review`→`creativework` can target one); custom edge types. Proposed: leave untouched, log the count.
- **Q6 — slot numbering.** Spec says the chain "computes the next version as (max stamped version + 1) at open time". The code stamps literal per-step versions (`connection.rs:1403` stamps `30`). Plan computes N at implementation time into named consts (Task 4a). Brief's "not hardcoded 31/32 in migration code" is honored via the consts.
- **Q7 — cargo target location.** AGENTS.md mandates `<repo-root>/.cargo-shared-target`; pins name `/mnt/kv-work/build/cargo/curated-thoughts`. `.cargo-shared-target` does not exist in the worktree. Plan follows pins (the baseline used that path).
- **Q8 — tarball path.** The brief cites `sso-9.18.0-tarball/package/`; the extracted tree has no `package/` level (`sso-9.18.0-tarball/dist/index.js`). Pins also say "28 entries / 21 unique slugs"; `dist/index.js` holds exactly 21 node-type entries.
- **Q9 — shared `node_modules`.** The trial install in this worktree mutates the main checkout's `node_modules` through the symlink. Options: a worktree-local install (pnpm store hard-links keep it small), or accept the shared bump. Controller decides before Task 1 step 3.
- **Q10 — planner epoch bump.** Not in the spec; mandated by the code's own rule at `entity_gate.rs:1078-1085`. Flagged for visibility.
- **Q11 — spec chain-state text.** Spec says the chain "is at V29 on main and moves to V30 when PR #352 merges". Main already carries V30 (pins). Harmless; the slot rule covers it.
- **Q12 — MERGE eligibility.** Spec says "PRESERVE every other declared entry; any manifest carrying content beyond that pattern is left untouched". Plan reads "the pattern" as "passes the old 17-slug EA subset check" (the gate the spec names). Foreign manifests stay untouched.
- **Q13 — disk check timing.** Spec: "the implementer checks free disk … before running". Plan reads this as a runtime pre-check inside the step (S2), failing as a backup failure. Controller confirms.
