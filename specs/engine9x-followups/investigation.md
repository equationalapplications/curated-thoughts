# Investigation — issue #376: engine-9.x follow-ups from the PR #375 T4 cycle

**Date:** 2026-11-25 · **Status:** Draft — r1 Fable step-0 adjudicated (3 MAJOR + 8 minors, verified and folded)
**Branch:** `chore/engine9x-followups` · **Lane worktree:** `/mnt/kv-work/build/worktrees/curated-thoughts/engine9x-followups` (base: `origin/main` = `10e9b006`)
**Upstream:** issue #376; findings originated in the PR #375 T4 Fable cycle (rounds r1–r4) and spec errata E8–E10 (spec `specs/engine-9x-adoption/spec.md` §8, landed with #375). Split-out: issue #378 (forgotten_at reader gap).

## Method

Every premise in issue #376 was re-verified against the live tree at `10e9b006`
by direct read of the named code paths. Review findings at each tier (Opus
medium REQUEST CHANGES → Opus-low delta APPROVE WITH NITS → Fable step-0
REQUEST CHANGES) were each re-verified against the tree and the installed
9.18.0 engine dist before folding. Premise verdicts: **7 REAL, 1
REAL-but-out-of-scope** (#378). The Fable step-0 pass ESCALATED I4: two of its
MAJORs describe leaks that are live today, not gated-future debts.

## Verified findings

### I1 — migrate repair reports print to stdout (REAL, trivial)

`src-tauri/src/db/connection.rs`: `:541` (V20 regrade report), `:694` (V22
canonical-path unify report), `:918` (stated-ledger rederive report) are
`println!`; every sibling migration-log site (`:391, :406, :420, :439, :448,
:461, :554, :598, :679`) already uses `eprintln!`. On `--mcp` opens, stdout is
the JSON-RPC channel; three log lines (a few bytes to multiple KB each) would
be injected into the protocol stream if a repair branch fires mid-session. PR
#375 fixed only the new V33 line.

**Fix direction:** change the three `println!` to `eprintln!`, text unchanged.
Verified: repo-wide grep for `\bprintln!(` in non-test src finds ONLY these
three sites plus `onboard/mod.rs` (interactive CLI, legitimately stdout); the
`[ct::repair]`/`[ct::regrade]` markers in `evidence_regrade.rs` are sibling
WARN texts already on `eprintln!`; no test asserts any marker.

### I2 — `engine9_missing` re-reads `PRAGMA table_info` redundantly (REAL, trivial)

`connection.rs:1494-1514`: the loop over `ENGINE9_MIRROR_COLUMNS` (4 entries:
3 on `llm_wiki_entries`, 1 on `llm_wiki_events`) calls
`ddl_compat::existing_columns(conn, table)` **per column** — three
`PRAGMA table_info(llm_wiki_entries)` reads per fast-path open. Return shape
is constrained only by internal callers `:1550`/`:1554`; the `:8048`-area
test calls `existing_columns` directly and does not constrain the restructure.

**Fix direction:** group by table; one `existing_columns` call per distinct
table, return shape kept. Scope honesty (r2-m3, expanded per Fable m3): other
every-open PRAGMA readers are `okf_ddl.rs:325` (v13), `:356` (okf_v02),
`:384` (events partial index), `:402`/`:410` (source store), and
`engine9_missing`'s own `sqlite_master` COUNT-per-object loop (`:1503-1512`;
the object LIST comes from `package_ddl_objects` at `:1470`, which issues no
SQL). The I2 fix saves 2 of roughly 10 per-open PRAGMA reads — a hygiene fix,
NOT a hot-path win; presented as such. The object loop is left unchanged.

### I3 — helper-SQL body drift is not gated (REAL, medium — RE-SIZED DOWN from medium-large)

**Gap restated per Fable M1 (verified against
`src/__tests__/engine9-setup-compat.test.ts`):** the TS schema-stability test
(`:171-217`) runs real 9.18.0 `setup()` over `rustChainDb()` — the
Rust-generated dump — and asserts `dropped == []` / `added == FTS_FAMILY`.
Its own comment claims it catches `okf_ddl.rs` mirror drift, and for
**add / rename / drop** it does. But the comparison basis is `type:name` only
(`schemaObjects`, `:132`): a **same-name SQL-body change** passes, because
`CREATE INDEX IF NOT EXISTS` no-ops over the Rust-made object. Also
uncovered: the v12 `DROP INDEX` behavior, the column-existence guards, and
resurrect-trigger body levels 0/1. So the real gap is narrower than earlier
drafted: **same-name SQL-text drift, drop behavior, and conditional
branches**.

The six helper mirror functions in `okf_ddl.rs` (outside the `setupDatabase`
template gate `ddl_compat.rs:82`, which normalizes via
`normalize_ddl_statement` then identity-checks the template in dist order):

- `:306` `apply_llm_wiki_v12_edge_index` — engine migration 12 (CREATE + DROP INDEX)
- `:324` `apply_llm_wiki_v13_temporal_indexes` — engine migration 13 / 7.8.0 (and a `setupDatabase` helper)
- `:353` `apply_llm_wiki_okf_v02_indexes` — mirrors `createOkfV02IndexesIfColumnsExist`
- `:379` `apply_llm_wiki_v18_v19_partial_indexes` — from engine MIGRATION code (v18/v19), not `setupDatabase`
- `:401` `apply_llm_wiki_source_store_indexes` — 9.18.0 (mirrors two engine functions)
- `:432` `apply_llm_wiki_resurrect_trigger` — mirrors `createResurrectTrigger` (body levels 0/1/2)

None exists as a Rust const — SQL is inline (`okf_v02` builds it with
`format!` in a 2×3 loop, `:356-360`). Extractor correction (Fable m1):
`extract_setup_database_ddl` (`ddl_compat.rs:31`, collecting every template
block inside the `setupDatabase` body, `:57-73`, test `:314`) deliberately
EXCLUDES the helper functions (`:26-29`) — option (a) needs a NEW extractor
keyed on engine function name.

**Fix directions:**
(a) lift each helper's SQL into consts + a new function-name-keyed extractor
for the engine helpers/migrations, then normalize+compare (template-gate
shape). Medium-large: new extractor + inline-SQL refactor.
(b′) **extend the existing TS test** (Fable M1's cheapest path): compare
whitespace-normalized `sqlite_master.sql` for index/trigger objects between
the Rust-chain brain and a fresh engine-only brain, in the same harness
(`rustChainDb()` + `engine(db).setup()` already exist). EXCLUDE tables —
ALTER-added columns change stored `CREATE TABLE` text on migrated fixtures.
Re-sized **medium**: no new extractor, harness exists.
(c) the earlier runtime-diff option is SUPERSEDED by (b′) and never covered
the v12 DROP or trigger levels 0/1 anyway — dropped.

**Open question Q1:** comparison depth — normalized per-statement identity
(template-gate precedent) vs raw text. **Open question Q2 (now: which
objects):** (b′) covers index/trigger body text on setup-created objects;
whether the v18/v19 partial indexes (migration-only) are visible to the b′
diff on both sides needs a planning-time check or a dedicated assertion.
**Direction choice (a) vs (b′) is the spec's central decision.**

### I4 — vault switch leaves live leaks behind (REAL, medium — ESCALATED per Fable M2/M3)

`queries.rs:207 clear_vault_tables` clears the llm_wiki_* content tables and
the CT-local sweep, but residue remains in three verified classes:

**(i) Source store (gated-future, E10 class):** no DELETEs for
`llm_wiki_source_documents` / `llm_wiki_source_passages`. CT writes no rows
today (v17 constraint), so nothing leaks YET; the leak is live the day a
`storeSource` consumer lands (#213). Fix: add the two DELETEs now — no-ops,
and verified no `REFERENCES` of any form targets either table (Fable m7:
the schema uses column-level `REFERENCES`, so an empty `FOREIGN KEY` grep
proves nothing; the column-level grep IS empty; FKs are off on that raw
connection anyway). Tables-exist invariant (Fable m6): the DELETEs assume
both tables exist, which holds because the every-open V33 chain runs at app
start before a switch can be driven; state it in the spec.

**(ii) Stated-ledger derived tables (LIVE leak — verified):**
`stated_attestations` (migrations/0002:12), `queue_denials` (`:19`), and
`deposit_attribution` (0001:13) are `document_path`-keyed indexes derived
FROM THE VAULT. `rebuild_rederive` (`stated_ledger.rs:356`) is their only
re-derivation writer and truncates+rebuilds all three from the vault's
ledger/deposit files (`:409 DELETE FROM stated_attestations`, `:431 DELETE
FROM queue_denials`, `:439 DELETE FROM deposit_attribution`). But it is
wired at `connection.rs:916` to run **ONCE per brain lifetime**, stamped
`stated_ledger_rederived` in `llm_wiki_meta` (`:934`) — and `llm_wiki_meta`
deliberately SURVIVES the clear. Therefore, on an in-place vault switch (the
non-file-replacement branch, `lib.rs:2297-2301`), all three tables keep
VAULT A's rows while vault B is active: attestations, denials (which "pin
suppression"), and deposit attribution of the outgoing vault leak into the
incoming session. Fix direction: clear the three tables in
`clear_vault_tables` AND delete the `stated_ledger_rederived` stamp row from
`llm_wiki_meta`, so the next rooted open re-derives everything from the NEW
vault's files — the existing once-per-brain mechanism then does the rebuild;
no new machinery. Keep-rows precedent: `schema_version` survival is pinned
by test (`queries.rs:1050-1053`); the three tables take
cleared-with-everything dispositions under the same #213 per-vault rule as
proposals.

**(iii) FTS family (LIVE residue — mechanism verified in engine dist):**
the raw `DELETE FROM llm_wiki_entries` fires the engine's `*_fts_ad`
triggers, which only ENQUEUE into `*_fts_pending` (dist `:2349-2350`) — no
synchronous purge of `llm_wiki_entries_fts_content` or `llm_wiki_fts_map`.
The engine drains the pending ledger in `drainTurn` chunks (dist
`:6177-6240`; `drain`/`drainPassages` at `:6117`/`:7010`) during engine
operations — not visibly gated on `setup()` (setup sets `secure_delete=ON`
and does not gate on drain). After a switch, vault A's indexed text remains
in the FTS shadow tables until the engine's next drain turn reconciles the
emptied `llm_wiki_entries` against the pending queue; `secure_delete=ON`
zeroes at drain time, but the WINDOW exists and a search in it can serve
stale hits. Complete engine-created FTS inventory (verified against
FTS_FAMILY, `engine9-setup-compat.test.ts:190-215`): `llm_wiki_entries_fts`
+ `_content/_data/_docsize/_idx/_config`, `llm_wiki_fts_map`,
`llm_wiki_fts_pending`, triggers `*_fts_ad/_ai/_au`, and the same family
for `llm_wiki_source_passages_fts` (+ `_fts_map`, `_fts_pending`,
`_ad/_ai` triggers). A Rust in-memory brain has NONE of these objects, so a
Rust-side guard cannot see them — the guard needs a second inventory source
(the FTS_FAMILY list or the I8 fixture). **Open question Q6 (owner/ladder):**
accept the engine-drain window and document it, or force reconciliation
after the clear (engine-side drain on reopen, or explicit FTS-object cleanup
in the sweep — the latter touches the #213 "engine-owned" boundary and
secure_delete semantics). rusqlite FTS5 support is NOT a factor for the
clear path (the `_ad` triggers touch ordinary tables; the drain is TS-side).

**(iv) Guard test (structural):** brain-wide guard — every table in the
schema must be cleared, partially-cleared-with-reason, or exempted-with-
reason. Verified inventory so far — sweep tables (`queries.rs:214-303`);
exempt: `llm_wiki_meta` (survives; BUT the (ii) fix deletes the
`stated_ledger_rederived` row — a named exception, pinned by its own test),
`llm_wiki_outbox` (append-only, replica DELETEs are ADDED `:214-237`),
`llm_wiki_vector_removals` (marker-gated, #213/T9),
`llm_wiki_entity_manifests` (partial: tier vocabulary retained,
`queries.rs:1045-1048`), `schema_version` (watermark, `queries.rs:1050`),
the engine FTS family (see (iii) ruling), and the CT operational tables
Fable M2 named for disposition: `pipeline_heartbeat` (`schema.rs:244`),
`pipeline_stalls` (`:256`), `system_strikes` (`:286`), `deposit_kick_state`
(`connection.rs:793`) — each needs an explicit ruling; they look
vault-independent/operator-facing, but the guard cannot assume. Caveat
(Fable M2): `librarian_evidence` is cleared only via
`WHERE entry_id IN (SELECT id FROM llm_wiki_entries)` — pre-existing orphan
evidence rows would survive; the spec should note whether orphans are
possible (write paths insert evidence only alongside entries in one tx).

**Related E10 debt, no code change:** `bundle_apply.rs:979
clear_one_entity_content` drops the new entries columns' values on the
wipe-and-reinsert path. Fold-when-consumer-lands; the I4 guard does not
cover it (bundle apply, not vault switch). Stays on #376's checklist.

### I5 — `forgotten_at` not honored by Rust readers (REAL — SEPARATE ISSUE #378, filed)

No Rust READ path filters `forgotten_at IS NULL` (the engine does so in 11
places). Rust references: mirror column declaration, index mirror
(`okf_ddl.rs:377-387`), schema_guard registry entry (`schema_guard.rs:111` —
not a reader), test assertions. Own issue per r4 M2b (INTENT §12).

### I6 — no `.gitattributes`; the testdata fixture compare is line-ending fragile (REAL, trivial)

The byte compare (`connection.rs:8316-8330`,
`engine9_migrated_schema_fixture_is_current`) reads
`src-tauri/src/db/testdata/ct-engine9-migrated.llm_wiki.schema.sql` with
`read_to_string` and asserts `on_disk == dump`; the dump is built with
literal `\n` escapes (`:8300-8307`), so the fragile artifact is the committed
testdata file (CRLF on Windows autocrlf checkouts fails the gate). The repo
has no `.gitattributes`. testdata also carries the 412 KiB (421,888 bytes)
binary brain fixture (`ct-brain-v32-core-8.0.2.db`) that git must never
text-normalize. No Windows CI leg exists (no Windows test step in the
workflows — Fable-verified), so only local Windows checkouts are affected.

**Fix direction (Fable m5 hardened):** scope to the ONE fragile artifact —
`src-tauri/src/db/testdata/*.sql text eol=lf` plus `*.db binary`; attribute
matching is last-line-wins, so with the scoped `*.sql` pattern there is no
overlap with `*.db binary`, and the file stays minimal. Acceptance:
`git check-attr` / `git ls-files --eol` checks in the PR; note that existing
Windows clones need a re-checkout after the attribute lands. Q4
(repo-wide vs scoped) RESOLVED: scoped.

### I7 — `ci.yml` runtime-smoke sqlite flag comment stale (REAL, trivial)

`ci.yml:223-225`: comment says "Node 22 needs --experimental-sqlite".
Verified (nodejs.org v22.13.0 release notes — "unflag sqlite module",
nodejs/node#55890 — and the node:sqlite API history): flagged in 22.5.0,
unflagged in 22.13.0. CI installs floating `node-version: "22"` via
`.github/actions/setup-node-pnpm` (no `.nvmrc`, no `engines`), so CI always
gets ≥22.13.

**Fix direction (Fable m4 corrected):** KEEP the flag (harmless no-op on
≥22.13) and fix BOTH stale comments — `ci.yml:223` AND
`scripts/runtime-smoke-clw.mjs:18` (same stale wording; accurate text exists
at `engine9-setup-compat.test.ts:28-30`). Honest rationale: the flag
protects nobody in CI (contributors do not execute ci.yml); it is kept
purely to avoid churning a working invocation, not as protection.

### I8 — desktop-upgrade-path test coverage gap (REAL, medium, test-only)

No test runs 9.18.0 TS `setup()` over a brain carrying the 8.0.2 FTS family +
`schema_version=15` + data, and no test runs the V33 ALTERs over populated
tables (the existing fixture brain, 412 KiB `ct-brain-v32-core-8.0.2.db`,
was captured pre-FTS-writes). **Fix direction:** generate a second fixture
deterministically (install core-llm-wiki@8.0.2 in a scratch dir, `setup()` +
FTS-producing writes through node, commit per the testdata README provenance
pattern), then (a) TS test: 9.18.0 `setup()` over it, (b) Rust test: V33
ALTERs over populated tables. Hygiene: 0 local-path strings, README
provenance. **Open question Q5:** in-lane (keeps the upgrade-path proof with
the drift-gate work) vs separate issue.

## Proposed fix directions / open questions (summary)

| # | Item | Size | Priority |
|---|------|------|----------|
| I1 | stdout → stderr ×3 | trivial | 1 |
| I2 | PRAGMA grouping (column loop only; hygiene, not perf) | trivial | 1 |
| I7 | fix both stale comments, keep flag | trivial | 1 |
| I6 | `.gitattributes` scoped to testdata `*.sql` + `*.db binary` | trivial | 2 |
| I4(ii) | clear 3 stated-ledger tables + delete rebuild stamp — LIVE leak | small-medium | 2 |
| I4(i) | source-store DELETEs (E10 fold) | trivial | 2 |
| I3 | helper-SQL body-drift gate — (a) consts+new extractor vs (b′) TS-test SQL diff | medium | 2 |
| I4(iii) | FTS drain window — ruling needed (Q6) | ruling | 3 |
| I4(iv) | brain-wide guard test with per-table dispositions | small-medium | 3 |
| I8 | desktop-upgrade fixture + tests (Q5) | medium | 3 |
| I5 | forgotten_at Rust reader | — | separate issue #378 |

Open questions: Q1 (I3 comparison depth), Q2 (I3 object coverage incl. the
migration-only v18/v19 indexes under (b′)), Q3 (guard inventory: runtime
sqlite_master + engine-FTS second source vs static list), Q4 — RESOLVED
(scoped), Q5 (I8 in-lane?), Q6 (FTS window: accept+document vs force
reconciliation).

## Out of scope

- I5 / issue #378 (own lane; INTENT §12 semantics).
- The E10 clear_one_entity_content debt (fold when a storeSource consumer
  lands).
- The object-existence `sqlite_master` loop in `engine9_missing` and the
  other every-open PRAGMA readers (recorded in I2 scope note).
- Any pin bump or engine upgrade.

## Research answers (Fable step-0 RESEARCH_REQUESTs)

1. **FTS trigger bodies / drain:** `_ad` triggers enqueue into
   `*_fts_pending` only (dist `:2349-2350`); no synchronous purge of
   `_fts_content`/`fts_map`. Drain runs in engine `drainTurn` chunks during
   engine operations (`:6117/:7010/:7210-7291`); not visibly gated on
   `setup()`. rusqlite FTS5 irrelevant to the clear path (triggers touch
   ordinary tables; drain is TS-side).
2. **Stated-ledger lifecycle:** `rebuild_rederive` truncates+rebuilds all
   three `document_path`-keyed tables from vault files
   (`stated_ledger.rs:409/:431/:439`) but is stamped once-per-brain
   (`connection.rs:916`; the stamp survives the clear) → live leak on
   in-place switch (I4(ii)).
3. **D2 matrix:** surviving-rows precedent is pinned by tests
   (`queries.rs:1050-1053` schema_version watermark; `:1045-1048` tier
   vocabulary). The authoritative matrix lives in the #213 vault-switch
   spec; the I4(iv) guard enumerates dispositions table-by-table for ladder
   review rather than re-litigating the matrix.

## Resource-state note (lane open, per directive)

Owner directive: follow resource procedures. Sweep at lane open: root 208G
free (52%), kv-work 75G free (33%); `/tmp` tmpfs empty; no stray `target/`;
no repo-root `.cargo-shared-target` (first heavy gate this lane will be a
cold ~15–25 min rebuild under the canonical AGENTS.md location); no stale
lane worktrees. Cargo-capable children carry full env exports in dispatch
context.

## Revision history

- r0 (2026-11-25): initial investigation; 7 premises verified (`dca512d7`).
- r1 (2026-11-25): Opus-medium ($0.90/26 turns) REQUEST CHANGES — I3 six-group
  inventory + mechanism, I6 testdata target, I4 exemption fix, 4 minors — all
  verified and adopted (`09a151ba`).
- r2 (2026-11-25): Opus-low delta ($0.31/3 turns) APPROVE WITH NITS — query-loop
  attribution, fixture size units, Node sources — adopted (`b52eb863`).
- r3 (2026-11-25): Fable step-0 ($2.58/29 turns) REQUEST CHANGES — M1 (I3 gap
  restated: name-only TS gate; option b′ added; re-sized medium), M2 (I4(ii)
  three-table LIVE leak — verified: once-per-brain stamp survives clear), M3
  (FTS inventory completed; guard blindness; drain window), m1–m8 — all
  verified against tree + 9.18.0 dist and adopted; RESEARCH_REQUESTs answered
  (section above); I4 escalated small → medium; Q4 resolved; Q6 added. Fable
  delta next.
