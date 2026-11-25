# Investigation — issue #376: engine-9.x follow-ups from the PR #375 T4 cycle

**Date:** 2026-11-25 · **Status:** Draft (Step 0 investigation)
**Branch:** `chore/engine9x-followups` · **Lane worktree:** `/mnt/kv-work/build/worktrees/curated-thoughts/engine9x-followups` (tip of investigation branch: `origin/main` = `10e9b006`)
**Upstream:** issue #376; findings originated in the PR #375 T4 Fable cycle (rounds r1–r4) and spec errata E8–E10 (spec `specs/engine-9x-adoption/spec.md` §8, landed with #375).

## Method

Every premise in issue #376 was re-verified against the live tree at `10e9b006`
by direct read of the named code paths (grep + sed excerpts below). No behavior
repro was needed: every item is a code-structure claim, not a runtime bug, and
each was confirmed by reading the exact lines the reviewers cited. Premise
verdicts: **7 REAL, 1 REAL-but-out-of-scope** (forgotten_at reader gap — r4
ruling: its own issue).

## Verified findings

### I1 — migrate repair reports print to stdout (REAL, trivial)

`src-tauri/src/db/connection.rs`:

- `:541` — V20 regrade report (`[ct::regrade] #186 V20: ...`) is `println!`
- `:694` — V22 canonical-path unify report (`[ct::repair] #204 V22: ...`) is `println!`
- `:918` — stated-ledger rederive report (`[ct::repair] #294 §8 rederive: ...`) is `println!`

Every sibling migration-log site in the same file (`:391, :406, :420, :439,
:448, :461, :554, :598, :679`) already uses `eprintln!`. On `--mcp` opens,
stdout is the JSON-RPC channel; if any of these repair branches fires during an
MCP session, three bytes-to-multi-KB log lines are injected into the protocol
stream. PR #375 fixed only the new V33 line.

**Fix direction:** change the three `println!` to `eprintln!`, text unchanged.
No semantics, no tests broken (verified: repo-wide grep for `\bprintln!(` in
non-test src finds ONLY these three sites plus `onboard/mod.rs`, whose
`println!`s are the interactive onboarding CLI and legitimately belong on
stdout; the `[ct::repair]`/`[ct::regrade]` markers appearing in
`evidence_regrade.rs` are sibling WARN texts already on `eprintln!`, and no
test asserts any marker).

### I2 — `engine9_missing` re-reads `PRAGMA table_info` redundantly (REAL, trivial)

`connection.rs:1494-1510`: the loop over `ENGINE9_MIRROR_COLUMNS` (4 entries:
3 columns on `llm_wiki_entries`, 1 on `llm_wiki_events`) calls
`ddl_compat::existing_columns(conn, table)` **per column**, so the fast path of
every open runs `PRAGMA table_info(llm_wiki_entries)` three times.
`apply_engine9_mirror` calls `engine9_missing` on every open, and a second time
under the lock when something is missing.

**Fix direction:** group `ENGINE9_MIRROR_COLUMNS` by table; one
`existing_columns` call per distinct table (2 calls total). Guard-test note:
`connection.rs:8048` asserts columns; the restructure must keep the
`(Vec<String>, Vec<String>)` return shape or update its callers (`:1550`,
`:1554` — both internal).

### I3 — helper-SQL mirrors sit outside the drift gate (REAL, medium)

`ddl_compat.rs:82 package_ddl_matches_rust()` compares the extracted
`setupDatabase` template against `okf_ddl::LLM_WIKI_PACKAGE_DDL` (order +
identity, `:98-:128`). But `okf_ddl.rs` carries FOUR hand-mirrored helper
groups created OUTSIDE the template:

- `:313` — temporal-index mirror (engine migration 13 / 7.8.0)
- `:344` — helper objects mirror (9.18.0 `setupDatabase` helpers)
- `:394` — source-store index mirror (9.18.0)
- `:421` — resurrect-trigger mirror (`createResurrectTrigger`)

None is drift-checked: a future pin bump that changes the engine's helper SQL
would leave the Rust mirrors stale with no gate failing. The TS-side
schema-stability test (landed in #375) pins the TS `setup()` footprint only —
it cannot see Rust mirrors. The existing extractor
(`ddl_compat.rs:31 extract_setup_database_ddl`) parses only the first
`execAsync` template block; helper SQL lives in separate engine functions.

**Fix direction:** extend the drift gate to cover the four helper groups —
extract each engine helper function's SQL from
`node_modules/@equationalapplications/core-llm-wiki/dist/index.js` and compare
against the corresponding Rust const (identity or normalized-statement
comparison, same shape as the template gate), wired into the same test
(`ddl_compat.rs:221` calls the gate in tests).
**Open question Q1:** comparison depth — verbatim SQL identity per statement
(strongest; breaks on cosmetic engine changes) vs normalized statement set
(whitespace/prefix-normalized). Template gate precedent is order+identity.
**Open question Q2:** the 7.8.0 temporal-index mirror is gated on columns
existing — the extractor must compare the SQL text, not runtime gating.

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
(b) add a structural guard test that fails when any `llm_wiki_*` table
(excluding an explicit exemption list: `llm_wiki_meta`, `llm_wiki_vector_removals`,
`llm_wiki_entity_manifests` partial-clear, checkpoint ledger rows) is neither
cleared by `clear_vault_tables` nor listed exempt — so future tables cannot
silently miss the sweep.
**Open question Q3:** the guard's table inventory — derive at test runtime from
`sqlite_master` on an in-memory brain (robust to new tables, may over-fire on
engine-internal tables) vs a static const list (explicit, rots). Precedent
`schema_guard.rs` uses a static registry.

**Related E10 debt, no code change:** `bundle_apply.rs:979
clear_one_entity_content` deletes entry rows and re-inserts from bundle
payloads — the new entries columns (`source_pointer`, `prior_valid_to`,
`derived_events`) lose their values on the wipe-and-reinsert path. Same
fold-when-consumer-lands class as E10; the I4 guard does not cover it (bundle
apply, not vault switch). Recorded in the spec's out-of-scope section; stays on
#376's E10 checklist.

### I5 — `forgotten_at` not honored by Rust readers (REAL — SEPARATE ISSUE)

Verified: the only Rust references to `forgotten_at` are the mirror column
declaration, the index mirror (`okf_ddl.rs:377-387`), and test assertions. No
Rust reader of `llm_wiki_events` filters `forgotten_at IS NULL` (the engine
does so in 11 places), so once anything calls `forget()`, Rust MCP/recall reads
would serve forgotten events. Per the r4 ruling this touches INTENT §12 and
**deserves its own issue** — action: file it referencing this investigation;
out of this lane's scope.

### I6 — no `.gitattributes`; byte-compare is line-ending fragile (REAL, trivial)

`connection.rs` byte-compares the schema dump; the repo has NO `.gitattributes`
(verified `ls .gitattributes` → absent). Only Windows checkouts with
`core.autocrlf` are affected; CI (Linux/macOS) is not.

**Fix direction:** minimal scoped `.gitattributes` — the files whose byte
comparisons matter (`src-tauri/src/db/connection.rs`, `src-tauri/src/db/okf_ddl.rs`,
`src-tauri/src/db/ddl_compat.rs`) pinned to `text eol=lf` (smallest change
surface; repo-wide `* text=auto` would churn every file's blame).
**Open question Q4:** scoped-file vs repo-wide normalization.

### I7 — `ci.yml` runtime-smoke sqlite flag stale (REAL, trivial)

`.github/workflows/ci.yml:223-225`: comment says "Node 22 needs
--experimental-sqlite"; on current Node 22 (≥22.5) `node:sqlite` is built in
unflagged (proven in #375's CI where the frontend test ran `node:sqlite` green
without any flag). The flag is still accepted but misleading.

**Fix direction:** drop the flag from the smoke step, update the comment;
verify `node scripts/runtime-smoke-clw.mjs` locally on the pinned Node before
pushing.

### I8 — desktop-upgrade-path test coverage gap (REAL, medium, test-only)

No test runs the 9.18.0 TS `setup()` over a brain that already carries the
8.0.2 FTS family + `schema_version=15` + data, and no test runs the V33 ALTERs
over populated tables. Everything is additive and existence-guarded so it is
expected to work — but unproven (the TS-proof fixture's schema dump carries no
FTS objects; limitation documented in the test header).

**Fix direction:** capture a second fixture from a brain the 8.0.2 TS engine
has set up AND written to (generate deterministically: install
core-llm-wiki@8.0.2 in a scratch dir, run `setup()` + writes through node, save
the brain as a committed binary fixture like the existing 412KB V32 one), then
(a) a TS test running 9.18.0 `setup()` over it, (b) a Rust test running V33
ALTERs over populated tables. Fixture hygiene rules from #375 apply (0
local-path strings, README provenance).
**Open question Q5:** in-lane vs separate issue. In-lane keeps the upgrade-path
proof with the drift gate that touches the same SQL; separate issue if fixture
generation proves heavy.

## Proposed fix directions / open questions (summary)

| # | Item | Size | Priority |
|---|------|------|----------|
| I1 | stdout → stderr ×3 | trivial | 1 |
| I2 | PRAGMA grouping | trivial | 1 |
| I7 | ci.yml flag comment | trivial | 1 |
| I6 | .gitattributes (scoped) | trivial | 2 |
| I4 | vault-clear source store + guard test | small | 2 |
| I3 | helper-SQL drift gate | medium | 2 |
| I8 | desktop-upgrade fixture + tests | medium | 3 |
| I5 | forgotten_at Rust reader | — | separate issue (filed) |

## Out of scope

- I5 (its own issue, per r4 M2b — INTENT §12 semantic).
- The E10 clear_one_entity_content debt (fold when a storeSource consumer
  lands; no consumer exists).
- Any pin bump or engine upgrade — this lane only hardens what 9.18.0 already
  landed.

## Resource-state note (lane open, per directive)

Owner directive: follow resource procedures. Sweep at lane open: root 208G
free (52%), kv-work 75G free (33%) — both gates pass; `/tmp` tmpfs 3.6G
essentially empty; no stray `target/` under the repo; no repo-root
`.cargo-shared-target` present (first heavy gate this lane will be a cold
~15–25 min rebuild under the canonical AGENTS.md location); no stale lane
worktrees (only empty per-repo dirs on kv-work). Cargo-capable children will
carry the full env exports in their dispatch context.
