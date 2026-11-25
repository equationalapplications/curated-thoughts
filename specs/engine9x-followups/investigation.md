# Investigation — issue #376: engine-9.x follow-ups from the PR #375 T4 cycle

**Date:** 2026-11-25 · **Status:** Draft — Fable step-0 + delta adjudicated (delta M1/M2 folded; delta-r2 verification next)
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

**(ii) Stated-ledger derived tables (LIVE leak — verified; fix needs ordering care per Fable-delta M1/M2):**
`stated_attestations` (migrations/0002:12), `queue_denials` (`:19`), and
`deposit_attribution` (0001:13) are `document_path`-keyed indexes derived
FROM THE VAULT. `rebuild_rederive` (`stated_ledger.rs:356`) is their only
re-derivation writer and truncates+rebuilds all three from the vault's
ledger/deposit files (`:391 DELETE FROM stated_attestations`, `:420 DELETE
FROM queue_denials`, `:439 DELETE FROM deposit_attribution`) — and has a
FOURTH output (delta M2): the confirmations leg (`:379-381`) applies
`stated/.confirmations.jsonl` as `user_confirmed` stamps on
`llm_wiki_entries` rows matched by content hash; no ingest path applies
confirmations (the only other caller is the live confirm path, `:270`).
Today the three tables leak: the rebuild is stamped once-per-brain
(`connection.rs:907-934`; stamp insert at `:929-933`) and the stamp survives
the clear (only `alias_remap_completed` is deleted from `llm_wiki_meta`,
`queries.rs:300-303`), so on an in-place switch vault A's attestations,
denials, and deposit attribution stay visible under vault B.

**Why the naive fix is WRONG (delta M1, verified):** deleting the stamp in
`clear_vault_tables` would make the FIRST rooted open after the clear re-run
`rebuild_rederive` — but `open_with_config` (`connection.rs:3020`) resolves
the vault root from the CONFIG FILE (`VaultConfig::new(config_path).vault_root()`),
and in the in-place branch the config is still pointing at vault A at that
moment: the reopen is `lib.rs:2307`, while `vault_state.set_vault_path(&new_path)`
— which writes the config (`vault/config.rs:85-91`, `config.write`) — only
runs later (`lib.rs:2325-2330`, after `pending_config_align_to` handling).
So the naive fix re-derives the OUTGOING vault and re-stamps: the leak comes
back, permanent. **Fix directions (each is new machinery; I4(ii) re-sized
medium):**
(a) reorder the config write before the reopen in `switch_vault`;
(b) call `rebuild_rederive(conn, new_root)` explicitly in `switch_vault`
after the aligned reopen;
(c) defer the stamp deletion until the config is aligned.
**Confirmations ruling (delta M2, open question Q7):** at switch time
`llm_wiki_entries` is empty (cleared; B not yet ingested), so the
confirmations leg applies 0 stamps and the once-per-brain stamp would pin
that forever — vault B's previously confirmed facts show unconfirmed until
re-confirmed. Today's behavior has the same hole (no rederive runs on
switch at all). Options: set the rebuild stamp only after B's first ingest
completes, or re-run the confirmations leg post-ingest, or name it a
residual. Drop any "re-derives everything / no new machinery" claim.
**Pin:** a switch-level test asserting the three tables hold B's rows (not
just that the stamp is gone).

**(iii) FTS family (LIVE residue — mechanism verified in engine dist):**
the raw `DELETE FROM llm_wiki_entries` fires the engine's `*_fts_ad`
triggers, which only ENQUEUE into `*_fts_pending` (dist `:2349-2350`) — no
synchronous purge of `llm_wiki_entries_fts_content` or `llm_wiki_fts_map`.
The engine drains the pending ledger in `drainTurn` chunks (dist
`:6177-6240`; `drain`/`drainPassages` at `:6117`/`:7010`) during engine
operations — not visibly gated on `setup()` (setup sets `secure_delete=ON`
and does not gate on drain). Citation note (delta m2): both earlier cites are
real but different layers — `drainTurn` wrapper `:6177-6240`; strategy
`drain` implementations `:6117`/`:7010`/`:7210-7291`. After a switch, vault
A's indexed text remains in the FTS shadow tables until the engine's next
drain turn reconciles the emptied `llm_wiki_entries` against the pending
queue; `secure_delete=ON` zeroes at drain time, but the WINDOW exists.
**Blast radius (delta m4, verified):** CT's Rust paths — the production
search/recall implementation — reference NO FTS objects at all (repo-wide
grep over non-test Rust for `entries_fts`/`fts_map` is empty), so for CT the
residue is AT-REST only unless the engine's own search APIs run. Engine-side
search ranks via `bm25` MATCH over the FTS tables (`dist:7076-7078`,
`:7517-7519`, keyed by entity_id carried in the FTS columns); whether stale
hits surface depends on the join-back to `llm_wiki_entries` — planning-time
check before Q6 goes to the owner. Complete engine-created FTS inventory (verified against
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
vault-independent/operator-facing, but the guard cannot assume.
`wiki_pages_new` (delta m3) is TRANSIENT — created, populated, and RENAMED
to `wiki_pages` inside one migration (`schema.rs:87-99`), so it is absent at
steady state; the guard's static-inventory wording must say so. Live-writer
window (delta m7): the switch stops the pipeline and outbox workers first
(`lib.rs:2208-2227`), but a concurrent `--mcp` connection is acknowledged
(`:2296`) — the spec must state whether a live writer can insert
vault-A-keyed rows into the three tables between the clear and the pipeline
restart, and if so how the switch serializes against MCP opens. Caveat
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
| I4(ii) | stated-ledger leak fix — ordering-aware (config write vs reopen) + confirmations ruling (Q7) | medium | 2 |
| I4(i) | source-store DELETEs (E10 fold) | trivial | 2 |
| I3 | helper-SQL body-drift gate — (a) consts+new extractor vs (b′) TS-test SQL diff | medium | 2 |
| I4(iii) | FTS drain window — Q6 narrowed (at-rest for CT paths; join-back check at planning) | ruling | 3 |
| I4(iv) | brain-wide guard test with per-table dispositions | small-medium | 3 |
| I8 | desktop-upgrade fixture + tests (Q5) | medium | 3 |
| I5 | forgotten_at Rust reader | — | separate issue #378 |

Open questions: Q1 (I3 comparison depth), Q2 (I3 object coverage incl. the
migration-only v18/v19 indexes under (b′)), Q3 (guard inventory: runtime
sqlite_master + engine-FTS second source vs static list), Q4 — RESOLVED
(scoped), Q5 (I8 in-lane?), Q6 (FTS window — NARROWED by the m4 blast-radius
finding: at-rest for CT production paths; finalize after the planning-time
join-back check), Q7 (NEW, delta M2: confirmations leg at switch — stamp
after first ingest vs post-ingest re-run vs named residual).

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
   `_fts_content`/`fts_map`. Drain runs via the `drainTurn` wrapper
   (`:6177-6240`) into the strategy `drain` implementations
   (`:6117/:7010/:7210-7291`) during engine operations; not visibly gated on
   `setup()`. Engine search = `bm25` MATCH over the FTS tables
   (`:7076-7078`, `:7517-7519`); CT's Rust search paths reference no FTS
   objects (grep-verified). rusqlite FTS5 irrelevant to the clear path
   (triggers touch ordinary tables; drain is TS-side).
2. **Stated-ledger lifecycle:** `rebuild_rederive` truncates+rebuilds the
   three `document_path`-keyed tables (`stated_ledger.rs:391/:420/:439`) AND
   applies confirmations to `llm_wiki_entries` (`:379-381`), but is stamped
   once-per-brain (`connection.rs:907-934`; the stamp survives the clear) →
   live leak on in-place switch (I4(ii)). The post-clear reopen resolves the
   vault root from the config file, which still names vault A until
   `set_vault_path` runs later — so any stamp-deletion fix must be
   ordering-aware (delta M1).
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
- r4 (2026-11-25): Fable delta ($1.29/16 turns) REQUEST CHANGES — M1 (naive
  stamp-deletion fix RE-DERIVES VAULT A: `open_with_config` reads the config,
  which still names vault A at the `lib.rs:2307` reopen; `set_vault_path`
  writes it only at `:2325` — VERIFIED), M2 (confirmations leg is a fourth
  rebuild output; entries empty at switch → Q7 opened), m1 (line cites
  :391/:420/:929-933), m2 (drain cite layers), m3 (`wiki_pages_new`
  transient), m4 (FTS blast radius: CT Rust search has no FTS references —
  Q6 narrowed), m5 (table row), m6 (revision labels), m7 (live-writer
  window) — all verified and adopted; I4(ii) re-sized medium. Fable delta-r2
  verification next.
