# Investigation — issue #376: engine-9.x follow-ups from the PR #375 T4 cycle

**Date:** 2026-11-25 · **Status:** CONVERGED after 5 review rounds (Opus med/low + Fable step-0 + 3 Fable deltas) — final Opus consistency delta pending, then spec
**Branch:** `chore/engine9x-followups` · **Lane worktree:** `/mnt/kv-work/build/worktrees/curated-thoughts/engine9x-followups` (base: `origin/main` = `10e9b006`)
**Upstream:** issue #376; findings originated in the PR #375 T4 Fable cycle (rounds r1–r4) and spec errata E8–E10 (spec `specs/engine-9x-adoption/spec.md` §8, landed with #375). Split-out: issue #378 (forgotten_at reader gap).

## Method

Every premise in issue #376 was re-verified against the live tree at `10e9b006`
by direct read of the named code paths. Review findings at each tier (Opus
medium REQUEST CHANGES → Opus-low delta APPROVE WITH NITS → Fable step-0
REQUEST CHANGES) were each re-verified against the tree and the installed
9.18.0 engine dist before folding (dist cited from the PRIMARY checkout's
node_modules — the lane worktree's `node_modules` is a symlink into it:
`~/code/github/equationalapplications/curated-thoughts/node_modules/.pnpm/@equationalapplications+core-llm-wiki@9.18.0/.../dist/index.js`). Premise verdicts: **7 REAL, 1
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
`:384` (events partial index), `:402`/`:410` (source store), `:443`
(resurrect trigger helper), and
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
(`schemaObjects`, defined `:128`, `type:name` projection `:135`): a **same-name SQL-body change** passes, because
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
(c) the earlier runtime-diff option is SUPERSEDED by (b′) — dropped.
**Coverage honesty (delta-r2 M4):** (b′) diffs steady-state
`sqlite_master.sql` between two FULLY BUILT brains, so like the dropped (c)
it sees only the FIRST gap class. Coverage matrix against the three gap
classes: (1) same-name SQL-body drift of surviving objects — (b′) YES,
(a) YES; (2) drop behavior (v12 `DROP INDEX`) — (b′) NO (the dropped object
is absent from steady state on both sides), (a) YES (compares the engine
source text); (3) conditional branches (column-existence guards, resurrect
trigger levels 0/1 — only the built variant exists in any given brain) —
(b′) NO for the unbuilt variants, (a) YES. So (b′) must either accept the
residual explicitly (drop + guard/variant drift stays uncaught) or pair
with dedicated assertions for class 2/3; (a) covers everything but costs
the inline-SQL refactor + a new extractor.

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
`stated_attestations`
(`src-tauri/src/db/migrations/0002_stated_verification.sql:12`),
`queue_denials` (`:19`), and `deposit_attribution`
(`src-tauri/src/db/migrations/0001_deposit_attribution.sql:13`) are `document_path`-keyed indexes derived
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
back, permanent. **Fix directions — a COMPOSITION is required (delta-r3 M3);
individual options are NOT equivalent:**
- **Rows removal (the leak half):** add the three `DELETE FROM` statements
  to the sweep itself, WITH THE STAMP LEFT IN PLACE (delta-r3 M3's
  composition; recommended). This removes A's rows inside the clear
  transaction — no RE-DERIVATION race with concurrent `--mcp` opens (the
  stamp survives, so no process re-derives A's rows back), though live
  writers can still insert rows (delta-r4 m3: writers outside the rebuild
  exist at `wisdom.rs:241`, `queue.rs:771`, `wisdom_deposit.rs:818`; a
  `--mcp` process still holding root A can insert A-keyed rows after the
  clear — see the delta-m7 live-writer question below, and note the same
  exposure exists TODAY without any fix). Repopulation for B is a
  separate step — timing NOT interchangeable, see delta-r4 M2 below.
- **Root fix (the wrong-root half):** option (a) — reorder the config write
  before the reopen — or option (d) — a root-override parameter on
  `open_with_config`. CORRECTION (delta-r3 M1, verified): the earlier
  objection to (a) was WRONG — `pending_config_align_to = Some(new_path)`
  is already set in BOTH branches before the reopen (`:2282` install,
  `:2302` clear), the error path already writes that config on any later
  failure (`:2373-2379`), and at `:2307` the old brain is never still on
  disk (cleared or replaced) — so a mid-switch failure ALREADY ends with
  the config naming the new vault. (a)'s real remaining costs are narrower:
  `set_vault_path` can itself fail before the reopen, and `vault_state`'s
  in-memory value changes earlier (delta-r4 m1 PARTIALLY ANSWERED:
  in-closure `run_pending` takes `(conn, db_path, now)` (`:2315`) and
  `start_pipeline` takes `new_root`'s canonical form (`:2321-2322`) —
  neither reads `vault_state`; the remaining unknown is concurrent readers
  on other threads — Tauri commands, the watcher). Position pin (delta-r4
  m2): the early config write must sit AFTER the point of no return
  (`:2282`/`:2302`) and BEFORE `:2307` — any earlier in the restore branch
  leaves config B over the outgoing brain on a failed install.
  (d) avoids those but adds a parameter to a ~25-call-site function.
  (c) defer-stamp-deletion is RULED OUT (delta-r2 M1: no further open
  in-session on the success path — the recovery open after a FAILURE is the
  exception, `:2384`, delta-r3 m2 — and the MCP-open race stands).
- Why BOTH halves: (rows-only) leaves the V22/okf wrong-root steps running
  on A; (root-only) fixes no leak at all — the rederive is skipped whenever
  the stamp exists regardless of root (`:907-915`), so (d) or (a) alone
  fails the pin test (delta-r3 M3).
**Root-dependent steps of the `:2307` open (delta-r2 M2 + delta-r3 M2
corrections, verified):** `migrate()`'s V22 `v22_unify_documents_path`
(rewrites `documents.path` to root form — no-op on a cleared brain, which
has no `documents` rows); the stated-ledger rederive (`:916`, stamp-gated —
skipped when the stamp exists, WHATEVER root is passed); and
`run_okf_migration(conn, vault_path)` — CORRECTED: it is DB-ROW-driven, not
a blind vault walk. In-place: `wiki_pages` is cleared (`queries.rs:274`), so
no rows → no-op. Restore branch with a stale backup predating the OKF
migration: B's `approved` `wiki_pages` rows are resolved against A's root —
`read_wiki_page_body` joins the RELATIVE page path onto the root
(`okf_migration.rs:59-71`, verified), so a same-named file under A supplies
A's text and missing files migrate with "(file missing)" bodies; worse,
`drop_pending_wiki_proposals` builds `remove_file` targets as
`A/.brain/proposed/<path>` (`:285-315`) — deleting files in the OUTGOING
vault — and the migration then stamps (`okf_migrated_at`, `:18`), making it
permanent.
**Confirmations ruling (delta M2, open question Q7 — PER BRANCH per
delta-r3 m3):** in-place: `llm_wiki_entries` is empty at switch (cleared; B
not yet ingested), so the confirmations leg applies 0 stamps and the
once-per-brain stamp would pin that forever — vault B's previously
confirmed facts show unconfirmed until re-confirmed. Restore: B's entries
are PRESENT (the backup carries them), so the entries-empty reasoning does
not apply there — the confirmations leg would actually work if the rebuild
ran against B's root. Options (delta-r4 M1: NO option deletes the stamp —
it survives by design in the adopted composition; a spec author working
from "stamp after first ingest" would reintroduce the delta-r2-M1 leak):
(a) explicit post-ingest confirmations re-run, stamp untouched; (b) name
the missing-stamps-in-place case a residual.
**Repopulation timing (delta-r4 M2 — the two candidates are NOT
interchangeable):** in-place needs TWO runs of `rebuild_rederive` — one
between the reopen (`:2307`) and the pipeline start (`:2322`) for the three
TABLE legs (they read only vault files, `stated_ledger.rs:366-442`; B's
first ingest must not run with empty `queue_denials`/`stated_attestations`/
`deposit_attribution` — verified: the queue joins
`deposit_attribution da ON da.document_path = d.path` (`queue.rs:347`) and
reads `queue_denials` for deny-suppression (`:361`), so empty tables mean
wrong attribution and un-suppressed re-queuing) — and one AFTER ingest for
the confirmations leg (needs entries; safe to re-run: the rebuild truncates
and rebuilds). Restore branch: the sweep never runs there (`:2301` is in
the `else`), so only the root fix acts, and only on stale backups; a
stamped backup keeps its own backup-era rows.
**Pin:** a switch-level test asserting the three tables hold B's rows and
the `stated_ledger_rederived` stamp value is UNCHANGED (stamp survives by
design).

**Restore branch (delta-r2 M3 + delta-r3 M2/M3, verified):** the non-in-place branch
(`lib.rs:2252-2292`) installs vault B's OWN backup
(`new_root/.brain/brain.db.bak` → `commit_staged_backup` at `:2274`) as the brain file
BEFORE the shared `:2307` reopen, and the config still names vault A then
(`set_vault_path` at `:2325` is shared, post-reopen). So the `:2307` open
runs against B's brain with A's roots in both branches. For B brains that
carry the stamps — V22 is a `schema_version` row (`connection.rs:699-700`),
the rederive is the `stated_ledger_rederived` `llm_wiki_meta` key, and the
OKF migration is the `okf_migrated_at` meta key (three separate mechanisms,
delta-r3 m5) — the open is a no-op — fine. For a STALE backup lacking the
stamps, the open runs with A's roots over B's rows: V22's `documents.path`
rewrite, the rederive (writes A's derived rows into B's brain), AND the
OKF migration (delta-r3 M2: reads A's wiki bodies for B's approved-page
rows and `remove_file`s A's `.brain/proposed/` files — deleting outgoing-
vault files — then stamps permanently). Today, no fix applied. I4(ii) must
cover this branch; the pin test needs these cases: (1) stale-backup
restore (V22 + rederive stamps absent), (2) the same backup also predating
the OKF migration, with an `approved` row AND a `pending_review` row
(delta-r3 M2 fix instruction).

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
**Bound note (delta-r2 m6):** if no engine operation runs against the
desktop brain after a switch, the at-rest residue is UNBOUNDED — the doc
must name which process (if any) runs the engine against the production
brain before Q6 is ruled on (planning-time check; CT's Rust paths never
trigger a drain).
**Blast radius (delta m4, verified):** CT's Rust paths — the production
search/recall implementation — reference NO FTS objects at all (repo-wide
grep over non-test Rust for `entries_fts`/`fts_map` is empty), so for CT the
residue is AT-REST only unless the engine's own search APIs run. Engine-side
search ranks via `bm25` MATCH over the FTS tables (`dist:7076-7078`,
`:7517-7519`, keyed by entity_id carried in the FTS columns); whether stale
hits surface depends on the join-back to `llm_wiki_entries` — planning-time
check before Q6 goes to the owner. Complete engine-created FTS inventory (verified against
FTS_FAMILY, `engine9-setup-compat.test.ts:188-215`): `llm_wiki_entries_fts`
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
exempt: `llm_wiki_meta` (survives INTACT — including the `stated_ledger_rederived`
stamp; delta-r4 M1 consistency: NO I4 fix deletes it — the three stated-
ledger TABLES move into the sweep, the stamp key does not),
`llm_wiki_outbox` (append-only, replica DELETEs are ADDED `:214-237`),
`llm_wiki_vector_removals` (marker-gated, #213/T9),
`llm_wiki_entity_manifests` (partial: tier vocabulary retained,
`queries.rs:1045-1048`), `schema_version` (watermark, `queries.rs:1050`),
`stall_strikes` (per-document strike ledger — ALREADY cleared,
`queries.rs:276`; delta-r3 m4), the engine FTS family (see (iii) ruling),
and the CT operational tables Fable M2 named for disposition:
`pipeline_heartbeat` (`schema.rs:244`), `pipeline_stalls` (`:256`),
`system_strikes` (`:286` — a DIFFERENT table from `stall_strikes`: the
single-row unattributed-strikes ledger, MIGRATION_V14; still needs an
explicit disposition), `deposit_kick_state`
(`connection.rs:793`) — each needs an explicit ruling; they look
vault-independent/operator-facing, but the guard cannot assume.
`wiki_pages_new` (delta m3) is TRANSIENT — created, populated, and RENAMED
to `wiki_pages` inside one migration (`schema.rs:87-99`), so it is absent at
steady state; the guard's static-inventory wording must say so (same
transient class: `documents_v15`, created `schema.rs:315`, renamed to
`documents` at `:338` — delta-r2 m3). Live-writer
window (delta m7): the switch stops the pipeline and outbox workers first
(`lib.rs:2208-2227`), but a concurrent `--mcp` connection is acknowledged
(`:2296`) — the spec must state whether a live writer can insert
vault-A-keyed rows into the three tables between the clear and the pipeline
restart, and if so how the switch serializes against MCP opens.
**Fourth residue class (delta-r4 M3, verified):** `deposit_kick_state` is
NOT vault-independent — its `path` is the vault-RELATIVE handle
(`wisdom_deposit.rs:1225`, "#325"), with live writers at `:261`/`:598` and
state values including `ingested`/`failed`/`queued_watcher`
(`connection.rs:793-799`). A vault-B deposit at the same relative path
inherits A's state, and rows for A's documents are residue of exactly the
(ii) class — a `DELETE FROM deposit_kick_state` belongs in the sweep
candidate list (spec rules it; default is NOT "exempt").
`pipeline_heartbeat.subject` and `pipeline_stalls.subject` also carry
per-document values (`schema.rs:249`, `:261`) — same analysis needed.
`system_strikes` alone is vault-independent (single counter row). Caveat
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
gets ≥22.13. THREE stale comments to fix (delta-r2 m4): `ci.yml:223-225`,
`scripts/runtime-smoke-clw.mjs:18`, AND `ci.yml:135` ("node 22.x (>= 22.5,
built-in sqlite)" — true for the built-in, but 22.5 was still flag-gated;
say ≥22.13 or "22.x current").

**Fix direction (updated per delta-r2 m4):** KEEP the flag (harmless no-op
on ≥22.13) and fix all THREE stale comments — `ci.yml:223-225`,
`scripts/runtime-smoke-clw.mjs:18`, and `ci.yml:135` — stating the true
history (flag needed 22.5–22.12; unflagged 22.13+; accurate text exists at
`engine9-setup-compat.test.ts:28-30`). Honest rationale: the flag protects
nobody in CI (contributors do not execute ci.yml); it is kept purely to
avoid churning a working invocation, not as protection.

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
| I7 | fix all three stale comments (`ci.yml:223`, `ci.yml:135`, smoke script), keep flag | trivial | 1 |
| I6 | `.gitattributes` scoped to testdata `*.sql` + `*.db binary` | trivial | 2 |
| I4(ii) | TWO-HALF composition: sweep adds 3 DELETEs (stamp left in place — no re-derivation race) + root fix ((a) early config write, positioned after point-of-no-return, or (d) override) for the V22/okf steps; repopulation TWO-RUN in-place (tables pre-pipeline, confirmations post-ingest); covers in-place AND stale-backup restore; confirmations ruling (Q7, per-branch) | medium | 2 |
| I4(i) | source-store DELETEs (E10 fold) | trivial | 2 |
| I3 | helper-SQL body-drift gate — (a) consts+new extractor vs (b′) TS-test SQL diff | medium | 2 |
| I4(iii) | FTS drain window — Q6 narrowed (at-rest for CT paths; join-back check at planning) | ruling | 3 |
| I4(iv) | brain-wide guard test with per-table dispositions (incl. `deposit_kick_state` — fourth residue class) | small-medium | 3 |
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
   live leak on in-place switch (I4(ii)). Repopulation for B is a
   composition decision (delta-r4 M2): table legs between reopen and
   pipeline start, confirmations leg post-ingest — the stamp is never
   deleted (supersedes the earlier "ordering-aware stamp-deletion"
   framing).
3. **D2 matrix:** surviving-rows precedent is pinned by tests
   (`queries.rs:1050-1053` schema_version watermark; `:1045-1048` tier
   vocabulary). The authoritative matrix lives in the #213 vault-switch
   spec; the I4(iv) guard enumerates dispositions table-by-table for ladder
   review rather than re-litigating the matrix.
4. **delta-r3 answers:** `read_wiki_page_body` joins the RELATIVE
   `wiki_pages.path` onto the vault root (`okf_migration.rs:59-71`) — the
   M2 wrong-root body read is real. `okf_migrated_at` is the OKF stamp key
   (`:18`). `stall_strikes` (per-document, cleared at `queries.rs:276`) and
   `system_strikes` (single-row ledger, MIGRATION_V14) are distinct tables.
   Remaining planning-time checks: readers of `vault_state` on other
   threads (Tauri commands, watcher) during a switch (option (a) cost);
   engine search join-back for Q6.
5. **delta-r4 answers:** `deposit_kick_state.path` is the vault-RELATIVE
   handle (`wisdom_deposit.rs:1225` "#325"; writers `:261`/`:598`; states
   at `connection.rs:793-799`) → fourth live-residue class. Queue
   consumers verified: `queue.rs:347` joins `deposit_attribution` on
   `document_path`; `:361` reads `queue_denials` for deny-suppression —
   confirming delta-r4 M2's ingest-time consequence. RR3 (vault_state
   readers outside `switch_vault`) partially answered (delta-r4 m1) —
   remaining: concurrent threads. RR4 (dist join-back for Q6): carried to
   planning.

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
- r5 (2026-11-25): Fable delta-r2 ($1.47/19 turns) REQUEST CHANGES — M1
  (option (c) ruled out standalone; MCP-open race on stamp-deleting variants;
  (b)-with-stamp-left avoids the race), M2 (root-dependent steps of the
  `:2307` open enumerated — V22 no-op on cleared brain, okf_migration walks
  vault A on pre-migration brains; (a)-vs-(b) discriminator), M3 (restore
  branch: stale backup lacking stamps gets A's roots applied to B's rows
  TODAY — second pin-test case), M4 (I3 coverage matrix: (b′) sees only gap
  class 1), m1–m7 — all verified against the tree and adopted; option (d)
  (root-override reopen) added as the recommended fix direction. Delta-r3
  verification next.
- r6 (2026-11-25): Fable delta-r3 ($1.36/15 turns) REQUEST CHANGES — M1
  (option (a) objection CORRECTED: `pending_config_align_to` is already set
  pre-reopen and the error path already aligns the config — verified
  `lib.rs:2282/:2302/:2372-2380`), M2 (okf migration is DB-row-driven;
  in-place no-op; restore-stale-backup reads A's wiki bodies via the
  relative-path join and `remove_file`s A's `.brain/proposed/` files —
  verified `okf_migration.rs:59-71/:285-315`), M3 (option (d)/(a) alone fix
  NO leak — rederive skips on stamp regardless of root; TWO-HALF
  composition adopted: sweep DELETEs + root fix + explicit/Q7 rebuild),
  m1–m5 — all verified and adopted. Delta-r4 verification next.
- r7 (2026-11-25): Fable delta-r4 ($1.27/13 turns) REQUEST CHANGES — M1
  (stale stamp-deletion text in 4 places contradicted the adopted
  composition — consistency sweep done: I4(iv) exemption, Pin, Research
  answer 2, Q7 options all rewritten stamp-intact), M2 (repopulation
  timing NOT interchangeable — two-run structure for in-place stated,
  queue-consumer consequence verified `queue.rs:347/:361`), M3
  (`deposit_kick_state` is path-keyed vault-relative — fourth live-residue
  class, verified `wisdom_deposit.rs:1225`), m1–m4 — all verified and
  adopted. **INVESTIGATION CONVERGED**: all factual premises settled; the
  remaining items are explicit spec-stage decisions (Q1–Q7) and
  planning-time checks. Final Opus-medium consistency delta before the
  spec; further fix-composition iteration moves into the spec + plan
  stage where it belongs.
