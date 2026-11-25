# Investigation — issue #376: engine-9.x follow-ups from the PR #375 T4 cycle

**Date:** 2026-11-25 · **Status:** Draft — r1 Opus adjudicated (7 findings, all adopted)
**Branch:** `chore/engine9x-followups` · **Lane worktree:** `/mnt/kv-work/build/worktrees/curated-thoughts/engine9x-followups` (base: `origin/main` = `10e9b006`)
**Upstream:** issue #376; findings originated in the PR #375 T4 Fable cycle (rounds r1–r4) and spec errata E8–E10 (spec `specs/engine-9x-adoption/spec.md` §8, landed with #375). Split-out: issue #378 (forgotten_at reader gap).

## Method

Every premise in issue #376 was re-verified against the live tree at `10e9b006`
by direct read of the named code paths (grep + sed excerpts below). No behavior
repro was needed: every item is a code-structure claim, not a runtime bug, and
each was confirmed by reading the exact lines the reviewers cited. Premise
verdicts: **7 REAL, 1 REAL-but-out-of-scope** (forgotten_at reader gap — r4
ruling: its own issue, filed as #378). Opus r1 review findings (M1–M3, m1–m4)
were each re-verified against the tree before folding; all adopted.

## Verified findings

### I1 — migrate repair reports print to stdout (REAL, trivial)

`src-tauri/src/db/connection.rs`:

- `:541` — V20 regrade report (`[ct::regrade] #186 V20: ...`) is `println!`
- `:694` — V22 canonical-path unify report (`[ct::repair] #204 V22: ...`) is `println!`
- `:918` — stated-ledger rederive report (`[ct::repair] #294 §8 rederive: ...`) is `println!`

Every sibling migration-log site in the same file (`:391, :406, :420, :439,
:448, :461, :554, :598, :679`) already uses `eprintln!`. On `--mcp` opens,
stdout is the JSON-RPC channel; if any of these repair branches fires during an
MCP session, three log lines (a few bytes to multiple KB each) are injected
into the protocol stream. PR #375 fixed only the new V33 line.

**Fix direction:** change the three `println!` to `eprintln!`, text unchanged.
No semantics, no tests broken (verified: repo-wide grep for `\bprintln!(` in
non-test src finds ONLY these three sites plus `onboard/mod.rs`, whose
`println!`s are the interactive onboarding CLI and legitimately belong on
stdout; the `[ct::repair]`/`[ct::regrade]` markers appearing in
`evidence_regrade.rs` are sibling WARN texts already on `eprintln!`, and no
test asserts any marker).

### I2 — `engine9_missing` re-reads `PRAGMA table_info` redundantly (REAL, trivial)

`connection.rs:1494-1514`: the loop over `ENGINE9_MIRROR_COLUMNS` (4 entries:
3 columns on `llm_wiki_entries`, 1 on `llm_wiki_events`) calls
`ddl_compat::existing_columns(conn, table)` **per column**, so the fast path of
every open runs `PRAGMA table_info(llm_wiki_entries)` three times.
`apply_engine9_mirror` calls `engine9_missing` on every open, and a second time
under the lock when something is missing. The return shape is constrained only
by its two internal callers (`:1550`, `:1554`) — the `:8048`-area test calls
`existing_columns` directly and does NOT constrain the restructure (r1-m2).

**Fix direction:** group `ENGINE9_MIRROR_COLUMNS` by table; one
`existing_columns` call per distinct table (2 calls total), return shape kept.
Scope note (r1-m2, reworded per r2-N1): other every-open PRAGMA callers exist
(`okf_ddl.rs:325` v13 helper, `:356` okf_v02, `:443` resurrect helper), and
`engine9_missing` itself ALSO issues one `sqlite_master` COUNT query per
package-DDL object — the object list comes from `package_ddl_objects`
(`connection.rs:1470`, no SQL of its own), but the query loop lives inside
`engine9_missing` (`:1503-1511`) and is likely the bigger fast-path cost.
This lane scopes I2 to the column grouping ONLY and leaves the object
existence loop unchanged.

### I3 — helper-SQL mirrors sit outside the drift gate (REAL, medium-large)

`ddl_compat.rs:82 package_ddl_matches_rust()` compares the extracted
`setupDatabase` template against `okf_ddl::LLM_WIKI_PACKAGE_DDL`
(statement-normalized via `ddl_compat.rs:8 normalize_ddl_statement`, then
identity-checked in dist order). But `okf_ddl.rs` carries **SIX** every-open
helper mirror functions outside the template (r1-M1 corrected inventory):

- `:306` `apply_llm_wiki_v12_edge_index` — engine migration 12 (CREATE + DROP INDEX)
- `:324` `apply_llm_wiki_v13_temporal_indexes` — engine migration 13 / 7.8.0
- `:353` `apply_llm_wiki_okf_v02_indexes` — mirrors `createOkfV02IndexesIfColumnsExist`
- `:379` `apply_llm_wiki_v18_v19_partial_indexes` — `entries_deleted_idx` / `events_forgotten_idx`; **from engine MIGRATION code (v18/v19), not `setupDatabase` helpers**
- `:401` `apply_llm_wiki_source_store_indexes` — 9.18.0 source-store indexes
- `:432` `apply_llm_wiki_resurrect_trigger` — mirrors `createResurrectTrigger` (three body levels 0/1/2)

None of these exists as a Rust const — the SQL is written inline (`okf_v02`
builds it with `format!` inside a 2×3 loop, `:356-360`). The TS-side
schema-stability test (landed in #375) pins the TS `setup()` footprint only —
it cannot see Rust mirrors. The existing extractor
(`ddl_compat.rs:31 extract_setup_database_ddl`) parses only the first
`execAsync` template block.

**Failure scenario:** a pin bump changes `createEntriesDeletedIndex` or the
v12 edge index; no gate fails; the Rust mirrors drift silently — exactly what
I3 exists to catch.

**Fix directions (r1-M1):**
(a) lift each helper's SQL into consts/templates so a gate can compare them,
then extract the corresponding engine source (`setupDatabase` helper functions
for four groups; engine migration bodies for v12 and v18/v19) and
normalize+compare, same shape as the template gate. This refactor sizes I3 at
medium-large.
(b) OR compare at runtime: run TS `setup()` + migrations on a scratch brain,
run a Rust open, and diff `sqlite_master` (helper object names + SQL text)
between the two — settles Q2 for free and needs no extractor work. CI has node
+ the package installed, so this works in CI too.

**Open question Q1:** comparison depth — per-statement normalized identity
(template-gate precedent: `normalize_ddl_statement` then identity) vs raw
text. **Open question Q2:** extractor scope — the v18/v19 group comes from
migration code, so its extraction needs a different source region than the
template extractor (option (b) sidesteps extraction entirely). **Direction
choice (a) vs (b) is the spec's central decision** — recommend deciding in the
spec with a small spike if (a)'s extraction proves brittle.

### I4 — vault switch does not clear the source store (REAL, small)

`queries.rs:207 clear_vault_tables` deletes entries (with replica DELETEs),
tasks, edges, events, source_ref_index, checkpoints, curated_*, embeddings,
chunks, ingest_runs, documents, wiki_pages, folder_rules, stall_strikes, and
the entity-level manifest rows — but NOT `llm_wiki_source_documents` /
`llm_wiki_source_passages`. CT writes no rows into either table (v17
constraint, documented at `connection.rs` V33 docblock), so the tables are
empty today and nothing leaks **yet**; the leak is live the day a `storeSource`
consumer lands (#213 per-vault rule). `llm_wiki_vector_removals` stays out
deliberately (T9 tests pin it) — no change there.

**Fix direction (r4 M2 offers two; recommend BOTH):**
(a) add the two `DELETE FROM llm_wiki_source_documents/passages` statements to
the sweep now — no-ops on empty tables, no v17-invariant interaction, and
verified: no FK in the schema references either table (grep over `REFERENCES
llm_wiki_source` and `FOREIGN KEY` in okf_ddl.rs is empty), so ordering is
free;
(b) add a structural guard test that fails when any table in the brain schema
is neither cleared by `clear_vault_tables` nor listed exempt **with a
per-table reason** (r1-M3: widen beyond `llm_wiki_*` — most of the sweep is
CT-local tables, and a new CT-local table leaks just as silently).

**Guard exemption list (r1-M3 corrected):** `llm_wiki_outbox` —
`clear_vault_tables` deliberately APPENDS replica DELETEs to it
(`queries.rs:214-237`) and never clears it (append-only by design);
`llm_wiki_vector_removals` — marker-gated per the #213/T9 decision;
`llm_wiki_entity_manifests` — partially cleared (tier vocabulary retained);
`llm_wiki_meta` — deliberately survives (drift watermark). NOT exempt:
`llm_wiki_checkpoints` — it IS cleared unconditionally today (`queries.rs:258`),
so exempting it would hide a future regression that drops that DELETE
(r1-M3 caught the prior draft wrongly exempting it).

**Open question Q3:** the guard's table inventory — derive at test runtime from
`sqlite_master` on an in-memory brain (robust to new tables; must filter FTS
shadow tables `*_fts_data|_idx|_docsize|_config`, which engine `setup()`
-created brains carry) vs a static const list (explicit, rots). Precedent
`schema_guard.rs` uses a static registry.

**Related E10 debt, no code change:** `bundle_apply.rs:979
clear_one_entity_content` deletes entry rows and re-inserts from bundle
payloads — the new entries columns (`source_pointer`, `prior_valid_to`,
`derived_events`) lose their values on the wipe-and-reinsert path. Same
fold-when-consumer-lands class as E10; the I4 guard does not cover it (bundle
apply, not vault switch). Recorded in the spec's out-of-scope section; stays on
#376's E10 checklist.

### I5 — `forgotten_at` not honored by Rust readers (REAL — SEPARATE ISSUE, filed as #378)

Verified: no Rust READ-path code filters `forgotten_at IS NULL` (the engine
does so in 11 places), so once anything calls `forget()`, Rust MCP/recall reads
would serve forgotten events. Rust references that exist: the mirror column
declaration, the index mirror (`okf_ddl.rs:377-387`), the schema_guard registry
entry (`schema_guard.rs:111` — not a reader, r1-m3), and test assertions. Per
the r4 ruling this touches INTENT §12 and has its own issue (#378) — out of
this lane's scope.

### I6 — no `.gitattributes`; byte-compare is line-ending fragile (REAL, trivial)

The actual byte compare (r1-M2 corrected): `connection.rs:8320-8328`
(`engine9_migrated_schema_fixture_is_current`) reads
`src-tauri/src/db/testdata/ct-engine9-migrated.llm_wiki.schema.sql` with
`read_to_string` and asserts `on_disk == dump`. The dump is built with literal
`\n` escape sequences (`:8300-8307`), so the `.rs` source itself carries no
sensitive line breaks — the fragile artifact is the committed testdata file,
which on a Windows checkout with `core.autocrlf=true` arrives CRLF and fails
the gate even when content is identical. The repo has NO `.gitattributes`
(verified absent). Only Windows checkouts are affected; CI (Linux/macOS) is
not. testdata also carries the 412 KiB (421,888 bytes) binary brain fixture
(`ct-brain-v32-core-8.0.2.db`) that git must never text-normalize.

**Fix direction:** minimal scoped `.gitattributes` —
`src-tauri/src/db/testdata/** text eol=lf` with an explicit `*.db binary`
(r1-M2). Q4's scoped-vs-repo-wide argument still holds; recommend scoped
(smallest change surface).

### I7 — `ci.yml` runtime-smoke sqlite flag stale (REAL, trivial)

`.github/workflows/ci.yml:223-225`: comment says "Node 22 needs
--experimental-sqlite". Verified against Node release notes (r1-m1; sources:
[nodejs.org v22.13.0 release notes](https://nodejs.org/en/blog/release/v22.13.0)
— "doc,lib,src,test: unflag sqlite module" — and the
[node:sqlite API history](https://nodejs.org/docs/latest-v22.x/api/sqlite.html)):
`node:sqlite` shipped behind the flag in **22.5.0** and was unflagged in
**22.13.0** (2025-01-07, nodejs/node#55890). CI installs a floating
`node-version: "22"`
via `.github/actions/setup-node-pnpm` (no `.nvmrc`, no `engines` field), so CI
itself always gets ≥22.13 — but a contributor on a local Node 22.5–22.12
running the smoke script without the flag would fail to import `node:sqlite`.

**Fix direction:** correct the comment to state the true history (flag needed
22.5–22.12; unflagged 22.13+) and KEEP the flag — it is a harmless no-op on
≥22.13 and protects the 22.5–22.12 contributors without adding an `engines`
constraint (r1-m1 option (a), smallest safe change).

### I8 — desktop-upgrade-path test coverage gap (REAL, medium, test-only)

No test runs the 9.18.0 TS `setup()` over a brain that already carries the
8.0.2 FTS family + `schema_version=15` + data, and no test runs the V33 ALTERs
over populated tables. Everything is additive and existence-guarded so it is
expected to work — but unproven (the TS-proof fixture's schema dump carries no
FTS objects; limitation documented in the test header). testdata already holds
the 8.0.2 fixture brain (`ct-brain-v32-core-8.0.2.db`, 421KB) but it was
captured pre-FTS-writes — the FTS family is absent.

**Fix direction:** capture a second fixture from a brain the 8.0.2 TS engine
has set up AND written to (generate deterministically: install
core-llm-wiki@8.0.2 in a scratch dir, run `setup()` + FTS-producing writes
through node, save the brain as a committed binary fixture per the existing
testdata README provenance pattern), then (a) a TS test running 9.18.0
`setup()` over it, (b) a Rust test running V33 ALTERs over populated tables.
Fixture hygiene rules from #375 apply (0 local-path strings, README
provenance). **Open question Q5:** in-lane vs separate issue. In-lane keeps
the upgrade-path proof with the drift-gate work that touches the same SQL;
separate issue if fixture generation proves heavy.

## Proposed fix directions / open questions (summary)

| # | Item | Size | Priority |
|---|------|------|----------|
| I1 | stdout → stderr ×3 | trivial | 1 |
| I2 | PRAGMA grouping (engine9_missing scope only) | trivial | 1 |
| I7 | ci.yml comment corrected, flag kept | trivial | 1 |
| I6 | .gitattributes (testdata-scoped + `*.db binary`) | trivial | 2 |
| I4 | vault-clear source store + brain-wide guard test | small | 2 |
| I3 | helper-SQL drift gate — direction (a) consts+extract vs (b) runtime sqlite_master diff | medium-large | 2 |
| I8 | desktop-upgrade fixture + tests | medium | 3 |
| I5 | forgotten_at Rust reader | — | separate issue #378 (filed) |

Open questions carried to the spec: Q1 (I3 comparison depth), Q2 (I3
extraction scope / option a-vs-b), Q3 (guard inventory: runtime sqlite_master
with FTS-shadow filter vs static list), Q4 (.gitattributes scope — recommend
scoped), Q5 (I8 in-lane vs separate).

## Out of scope

- I5 / issue #378 (its own lane per r4 M2b — INTENT §12 semantics).
- The E10 clear_one_entity_content debt (fold when a storeSource consumer
  lands; no consumer exists).
- The per-object `sqlite_master` queries in `package_ddl_objects` and the
  v13/okf_v02/resurrect PRAGMA reads (I2 scope note records them; not this
  lane).
- Any pin bump or engine upgrade — this lane only hardens what 9.18.0 already
  landed.

## Resource-state note (lane open, per directive)

Owner directive: follow resource procedures. Sweep at lane open: root 208G
free (52%), kv-work 75G free (33%) — both gates pass; `/tmp` tmpfs 3.6G
essentially empty; no stray `target/` under the repo; no repo-root
`.cargo-shared-target` present (first heavy gate this lane will be a cold
~15–25 min rebuild under the canonical AGENTS.md location); no stale lane
worktrees. Cargo-capable children will carry the full env exports in their
dispatch context.

## Revision history

- r0 (2026-11-25): initial investigation; 7 premises verified; committed `dca512d7`.
- r1 (2026-11-25): Opus-medium review (`inv-opus-r1`, $0.90/26 turns) — REQUEST CHANGES;
  findings M1 (I3 six-group inventory + mechanism), M2 (I6 testdata is the fragile file), M3
  (I4 exemption list both directions + brain-wide guard), m1 (I7 Node 22.13.0 — verified
  against nodejs.org release notes), m2 (I2 scope), m3 (I5 schema_guard entry), m4
  (wording + Q1 precedent note) — ALL verified against the tree and adopted.
- r2 (2026-11-25): Opus-low delta (`inv-opus-r2`, $0.31/3 turns) — APPROVE WITH NITS;
  nits N1 (per-object query loop attribution: inside `engine9_missing`, not
  `package_ddl_objects`), N2 (fixture size stated as 412 KiB / 421,888 bytes), N3
  (Node release-notes links added) — all verified and adopted. Investigation CONVERGED;
  proceeding to the Fable step-0 tier.
