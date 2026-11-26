# Spec: engine-9.x follow-ups (vault-switch residue + drift gate + hygiene) — issue #376

**Status:** Implemented (T3 REQUEST_CHANGES folded; T4 APPROVE 2026-11-25) · awaiting owner merge · **Branch:** `chore/engine9x-followups`
**Backbone:** `specs/engine9x-followups/investigation.md` (same dir) — 8 review rounds (Opus medium/low, Fable step-0 + 4 deltas, final Opus consistency), every premise verified against the tree at `10e9b006` and the installed 9.18.0 dist. Line cites refer to that base.

## 1. Problem

`clear_vault_tables` (the vault-switch sweep, `queries.rs:207`) leaves live
residue behind, violating the #213 per-vault rule TODAY — not only when a
`storeSource` consumer lands:

1. **Stated-ledger tables:** `stated_attestations`, `queue_denials`,
   `deposit_attribution` are vault-derived, `document_path`-keyed indexes.
   Their only re-derivation writer (`rebuild_rederive`,
   `stated_ledger.rs:356`) is stamped once-per-brain
   (`stated_ledger_rederived` in `llm_wiki_meta`, `connection.rs:929-933`)
   and that stamp survives the sweep — so an in-place switch leaves vault
   A's attestations, denials, and deposit attribution visible under vault B.
2. **`deposit_kick_state`:** `path` is the vault-RELATIVE handle
   (`wisdom_deposit.rs:1225`); a vault-B deposit at the same relative path
   inherits A's `ingested`/`failed` state.
3. **FTS shadow tables:** the engine's `*_fts_ad` triggers only enqueue
   (dist `:2349-2350`); vault A's indexed text stays at rest
   (`entries_fts_content`, `fts_map`) until an engine drain that CT's
   production (Rust) paths never trigger — verified: no non-test Rust
   reference to `entries_fts`/`fts_map`.
4. **Wrong-root reopen:** the post-sweep reopen (`lib.rs:2307`) resolves the
   vault root from the config file, which still names vault A
   (`set_vault_path` runs later, `:2325`). Root-dependent steps of that
   open — V22 `documents.path` unify, the stamp-gated rederive, the
   DB-row-driven OKF migration (which `remove_file`s the root's
   `.brain/proposed/` files), and V5's root-hash chunk keying — run against
   the OUTGOING vault. On stale restore backups (missing stamps) this is
   live today.
5. Plus the issue's original hygiene items: three migrate reports on
   stdout (`connection.rs:541/:694/:918` — corrupts the `--mcp` JSON-RPC
   stream if a repair fires mid-session), redundant PRAGMA reads in
   `engine9_missing`, ungated same-name helper-SQL drift, missing
   `.gitattributes` (testdata fixture compare), stale CI sqlite-flag
   comments, and the unproven desktop-upgrade path (no FTS-carrying
   fixture).

## 2. Proposed change — ONE PR, step-ordered

### S1 — stdout → stderr (trivial)
`println!` → `eprintln!` at `connection.rs:541/:694/:918`, text unchanged.
(Verified: these are the only non-onboard `println!` sites in src.)

### S2 — PRAGMA grouping (trivial)
`engine9_missing` groups `ENGINE9_MIRROR_COLUMNS` by table: one
`existing_columns` call per distinct table (2, was 4), return shape kept.
Hygiene only (saves 2 of ~10 per-open PRAGMA reads); the object-existence
loop and the okf_ddl helpers' PRAGMA reads are explicitly out of scope.

### S3 — CI comment fixes (trivial)
True-history comment ("flag needed 22.5–22.12; unflagged 22.13+") in
`ci.yml:223-225`, `ci.yml:135`, and `scripts/runtime-smoke-clw.mjs:18`.
Flag KEPT (harmless no-op; no `engines` constraint added).

### S4 — `.gitattributes` (trivial)
New file, scoped: `src-tauri/src/db/testdata/*.sql text eol=lf` +
`src-tauri/src/db/testdata/*.db binary`. PR includes `git ls-files --eol`
verification and a Windows re-checkout note in the fixture README.

### S5 — vault-switch sweep completion (the leak fix)
`clear_vault_tables` gains, inside the existing transaction:
- `DELETE FROM llm_wiki_source_documents/passages` (E10 fold; no-ops today,
  verified no REFERENCES targets);
- `DELETE FROM stated_attestations / queue_denials / deposit_attribution`;
- `DELETE FROM deposit_kick_state` (Q9 recommendation; relative-key class).
**The `llm_wiki_meta` stamp is NOT touched** — no code path deletes
`stated_ledger_rederived` (deleting it re-derives vault A via the
config-naming race, delta-r2 M1). `llm_wiki_outbox`,
`llm_wiki_vector_removals`, `schema_version`, `llm_wiki_entity_manifests`
(partial), keep their existing dispositions.

**Root fix (Q8, recommendation: option (a)):** in `switch_vault`, write the
config BEFORE the reopen — `set_vault_path(&new_path)` immediately after the
point of no return (`:2302` in-place; `:2274` install in restore) and set
`pending_config_align_to = None` (the error-path align at `:2373-2379`
becomes a no-op safety net; keep it). This fixes every root-dependent step
of the `:2307` open (V22, rederive-if-stamped, OKF, V5) without new API.
Option (d) (root-override parameter on the ~25-call-site
`open_with_config`) stays the fallback if implementation hits friction.
Rejected: stamp deletion in any form (delta-r2 M1 race → permanent
re-leak); root-fix-only (fixes no leak — the rederive skips on the stamp
regardless of root, delta-r3 M3).

**Repopulation (Q7, recommendation: explicit, two-run in-place):**
- Run 1 — after the reopen, before the pipeline starts (`:2322`):
  `rebuild_rederive(conn, new_root_canonical)` — the three TABLE legs read
  only vault files; B's first ingest must not run against empty
  `queue_denials`/`deposit_attribution` (verified consumers:
  `queue.rs:347/:361`).
- Run 2 — confirmations leg only, after ingest, via a **worker-side drain
  hook (a barrier that is ADDED — verified: no completion event exists to
  hook; the pipeline is a single worker thread draining a synced channel
  with `pending: Arc<AtomicUsize>` decremented per counted job,
  `pipeline/mod.rs:84-146`, and ingest is event-driven via
  `enqueue_vault_event`, so no "initial full scan" barrier exists).** The
  worker, after completing a counted job, when `pending` transitions to 0
  AND the channel stays empty through a short drain probe
  (`recv_timeout` window), executes the confirmations-only leg (new narrow
  entry point on `stated_ledger`; `PipelineJob` itself is unchanged).
  Re-run is idempotent (truncate-and-rebuild) and loses no live-written
  row (`queue.rs:753-775` appends to the vault ledger before the table
  insert); the hook fires on EVERY 1→0 drain crossing (debounced by the
  probe), so entries created by late-arriving jobs are stamped on the next
  crossing. Stamp untouched throughout.
- Restore branch: the sweep never runs there; only the root fix acts, and
  only on stale backups. **Pre-V5 backups are declared unsupported**
  (error message points at re-creating the vault) rather than widening the
  root fix to cover V5's chunk re-keying.

### S6 — brain-wide guard test (structural)
New `#[test]`: every table in a built brain must be swept,
partially-cleared-with-reason, or exempted-with-reason. Static disposition
list (schema_guard precedent), cross-checked at test time against
`sqlite_master` of the in-memory brain AND the FTS family inventory
(carried as a const mirroring the TS test's FTS_FAMILY — Rust in-memory
brains lack FTS objects, verified). Named dispositions: `llm_wiki_meta`
(intact), `llm_wiki_outbox` (append-only), `llm_wiki_vector_removals`
(#213/T9), `llm_wiki_entity_manifests` (tier vocabulary), `schema_version`
(watermark), `system_strikes` (vault-independent counter),
`pipeline_heartbeat`/`pipeline_stalls` (per-document `subject` — Q9
sub-ruling at implementation: recommend keep + note), FTS family (S7),
transient `wiki_pages_new`/`documents_v15` (absent at steady state),
`librarian_evidence` orphan caveat documented.

### S7 — FTS window (Q6, recommendation: accept + document)
No code change in this lane: the residue is at-rest only for CT (Rust
paths never read FTS; engine search join-back is a planning-time check).
`clear_vault_tables` gains a doc comment stating the window and the
engine-owned boundary; if the planning check shows engine search CAN
surface stale hits, a follow-up issue is opened instead of expanding this
lane.

### S8 — helper-SQL body-drift gate (Q1/Q2, recommendation: (b′) + documented residual)
Extend `engine9-setup-compat.test.ts`: after `setup()`, compare
whitespace-normalized `sqlite_master.sql` for INDEX and TRIGGER objects
between `rustChainDb()` and a fresh engine-only brain (tables excluded —
ALTER-added columns change stored CREATE TABLE text). **Engine-ownership
filter (CodeRabbit thread): the fresh engine-only brain's index/trigger
name set is the ground truth for what the diff covers.** An object present
in the engine brain is ENGINE-OWNED: missing from the rust brain, or
present with a drifted body → FAIL. An `llm_wiki_*`-prefixed object on the
rust side that is ABSENT from the engine brain is CT-OWNED and must appear
on an explicit allow-list in the test (expected members: the
`llm_wiki_vector_removals*` trigger/index family — marker-gated, CT-owned
per #213/T9; the plan-time check enumerates rust-side-only llm_wiki
objects on a built brain to prove the allow-list is complete — an
unlisted object FAILS). Non-`llm_wiki` objects (CT-local tables/indexes)
are outside the diff entirely. Normalized comparison (template-gate
precedent, `normalize_ddl_statement` spirit). Documented residual
(coverage matrix in the investigation): v12 DROP behavior and unbuilt
conditional variants (resurrect levels 0/1, column guards) are NOT
covered — dedicated assertions deferred until engine churn makes them
worthwhile. Planning-time check: whether the migration-only v18/v19
partial indexes appear on both sides of the diff (they are engine-created
during setup migration replay; if absent, a dedicated assertion lands with
this step).

### S9 — desktop-upgrade fixture (Q5, recommendation: in-lane)
Generate a second fixture deterministically (core-llm-wiki@8.0.2 in a
scratch dir on kv-work, `setup()` + FTS-producing writes through node,
committed per testdata README provenance, 0 local-path strings). Then:
(a) TS test — 9.18.0 `setup()` over it; (b) Rust test — V33 ALTERs over
populated tables. If fixture generation proves heavy at plan time, this
step splits to its own issue rather than blocking the lane.

## 3. Files touched (paths verified on base)

`src-tauri/src/db/connection.rs` (S1, S2, S5 stamp-intact invariant test),
`src-tauri/src/lib.rs` (S5 reorder + run-1 hook + run-2 trigger),
`src-tauri/src/db/queries.rs` (S5 sweep DELETEs + doc comment S7),
`src-tauri/src/db/stated_ledger.rs` (expose a narrow confirmations-only
entry point for run 2, if needed),
`.gitattributes` (new), `.github/workflows/ci.yml` +
`scripts/runtime-smoke-clw.mjs` (S3),
`src/__tests__/engine9-setup-compat.test.ts` (S8),
`src-tauri/src/db/testdata/` (S9 fixture + README),
`specs/engine9x-followups/` (spec + investigation ride the PR; pruned at
merge per convention).

## 4. Test plan

- Sweep dispositions unit tests (each new DELETE; stamp-unchanged
  assertion; outbox append-only invariant).
- Switch-level pin tests (three cases from the investigation): (1) in-place
  A→B: three tables + `deposit_kick_state` hold only B's data, stamp
  unchanged; (2) stale-backup restore: no A-derived rows, V22 untouched;
  (3) pre-OKF-migration backup: no A wiki bodies, no A `.brain/proposed/`
  deletions.
- `engine9_missing` grouping (column coverage unchanged; `:8048`-area test
  green).
- TS b′ diff test + negative control (mutated helper SQL fails).
- I8 tests over the new fixture (setup upgrade + V33 ALTERs over populated
  tables).
- Stdout hygiene: the three sites asserted via source grep in CI? No —
  covered by review + the existing eprintln siblings; no new test.
- Gates (controller-run, heavy-build wrapper per the resource procedure —
  cold rebuild expected, canonical `.cargo-shared-target`): fmt → clippy
  `-D warnings` both manifests (src-tauri with `--features
  test-utils,mcp-server`) → full suites (`--test-threads=1`, tools +
  integration) → vitest + tsc. Receipts transient.

## 5. Rejected alternatives

- Stamp deletion anywhere in the switch path (delta-r2 M1 config race →
  permanent re-leak).
- Deferring stamp deletion to post-alignment (delta-r2 M1: no in-session
  open on success; recovery open is the exception).
- Root fix alone ((a)/(d) without sweep DELETEs): fixes no leak — rederive
  skips on the stamp regardless of root.
- Explicit `rebuild_rederive` alone without root fix: leaves V22/OKF/V5
  running against A.
- Repo-wide `* text=auto` normalization (churn; scoped is sufficient).
- I3 option (a) (inline-SQL refactor + new extractor) — revisit if engine
  churn increases; (b′) + residual is the proportionate gate.
- Forcing FTS reconciliation in the sweep (touches the #213 engine-owned
  boundary; at-rest only for CT) — follow-up issue if the planning check
  says otherwise.

## 6. Out of scope

Issue #378 (`forgotten_at` Rust readers; INTENT §12); the E10
`clear_one_entity_content` entries-columns debt (folds when a storeSource
consumer lands); `package_ddl_objects` per-object queries and other
every-open PRAGMA readers; any pin bump; pre-V5 backup support.

## 7. Decisions (Q1–Q10)

**Q5–Q9 APPROVED by owner (PR #379 spec review, 2026-11-25, with the three
required revisions applied: S5 Run-2 drain-hook barrier, S8 engine-ownership
filter, Q10 residual + follow-up issue). Cleared to plan.**

| Q | Decision | Ruling | Decider |
|---|----------|----------------|---------|
| Q1 | I3 comparison depth | normalized per-statement | ladder |
| Q2 | I3 object coverage (v18/v19 visibility) | planning-time check, dedicated assertion if needed | plan |
| Q3 | Guard inventory source | static list + runtime cross-check + FTS const | ladder |
| Q4 | `.gitattributes` scope | RESOLVED: scoped testdata | — |
| Q5 | I8 fixture in-lane? | APPROVED: IN-LANE (split if heavy at plan time) | owner |
| Q6 | FTS window | APPROVED: accept + document (+ follow-up if join-back check fails) | owner |
| Q7 | Confirmations ruling | APPROVED: explicit post-ingest re-run, stamp untouched — via the S5 worker-side drain-hook barrier (required revision applied) | owner |
| Q8 | Root fix mechanism | APPROVED: (a) early config write, after point of no return | owner |
| Q9 | `deposit_kick_state` + subject columns | APPROVED: DELETE in sweep; heartbeat/stall subjects keep + note | owner |
| Q10 | `--mcp` live-writer window (post-commit old-root writers can insert A-keyed rows after the sweep) | RESOLVED: out-of-scope residual — the sweep transaction does NOT close this window; **#380** carries it (rejection-guard or serialization design) | owner (ruled 2026-11-25) |
