# Spec: Issue #186 — evidence source_ref mangling: structural fix, provenance enforcement, data repair

**Date:** 2026-09-06
**Status:** Draft
**Branch:** `spec/issue186-source-ref-writer`
**Priority:** P1 — librarian re-runs are on HOLD until this lands; blocks the evidence-provenance backlog item and the wiki rebuild.
**Baseline:** `main` @ `bc2a283` (v2.5.1)
**Issue:** equationalapplications/curated-thoughts#186 ("Fixes #186" on the implementation PR)
**Evidence base:** explore-first report (this session, 68 tool calls, read-only):
`/tmp/issue186-explore-report.md` — all file:line anchors below are from that report and
must be re-verified by the implementer against the checkout at implementation time.

---

## §1 Problem (verified current state)

### 1.1 Root cause — CONFIRMED, with a correction to the issue's hypothesis

The Rust commit path writes **valid JSON** into `llm_wiki_entries.source_ref`
(`db/commit.rs:900` `evidence_json_with_hashes` → INSERT at `commit.rs:917-936`).
The destruction happens **afterwards, in the JS engine**:

- core-llm-wiki's `setup()` runs an **unconditional legacy-ref back-rewrite**:
  `findRowsForSourceRefMigration()` (dist/index.js:1363-1376) selects every row whose
  `source_ref` matches GLOB `'*[^-A-Za-z0-9._ ]*'` — **every JSON blob qualifies** —
  and rewrites each through `normalizeSourceRef` (dist:3905:
  `value.replace(/[^A-Za-z0-9._\- ]/g, "").trim().slice(0, 255)`).
- This runs on **every app launch** (`src/main.tsx:32`) and **every outbox-worker
  start/stop transition** (`src/lib/wiki.ts:332-352`), over the **shared brain.db**
  (`wiki_exec`/`wiki_run` → `src-tauri/src/lib.rs:2190/2207`).
- Fingerprints matching byte-for-byte: 260 live `librarian_inferred` rows are exactly
  **255 chars** (the JS `.slice(0,255)` cap; the Rust writer never truncates), and the
  mangling charset is exactly the normalizer's keep-set.
- Correction to #186: the trigger is **not** `ingestDocument` — CT never calls it
  (`docs/superpowers/specs/2026-09-01-memory-architecture-intent-implementation-design.md:40-42`).
  The producer-side call sites never see these blobs; the setup-time rewrite is the mangler.

**Consequence:** any fix that leaves structured JSON in `source_ref` is re-mangled at the
next `setup()`, on every install, until every deployed engine is fixed. The fix must make
CT's rows **engine-proof**, not merely fix the Rust writer.

### 1.2 Damage model (why repair is in scope)

- `proposal_id` key destroyed → `wiki_forget::forget_entries_by_source_refs`
  (`db/wiki_forget.rs:25`, exact-match `WHERE source_ref IN (…)`) can never match;
  proposal-based retraction is dead for these rows.
- Evidence array destroyed → `source_docs_from_ref` (`db/entities.rs:201-244`)
  `serde_json::from_str` fails → returns `[]` → provenance display silently empty.
- `source_ref_is_still_grounded` (`db/commit.rs:294`, logic at `:368-380`) **treats
  unparseable JSON-looking refs as still-grounded** — heal passes over them, so the
  damage is permanent and invisible without a dedicated repair.
- Existing detection is warn-only: `warn_on_malformed_source_refs`
  (`db/connection.rs:205`) counts but does not repair — and its `{`-prefix census misses
  mangled rows (they no longer start with `{`).
- ~260 rows damaged (Sep 3–5 waves). Repair source of truth exists:
  `curated_proposal_items.evidence` still holds full JSON evidence per item
  (`librarian/synthesis.rs` tests at 1594-1616); `curated_proposals.id` + chunk
  `content_hash` allow rebuilding the blob.

### 1.3 Secondary requirement (issue #186, same PR)

Every inferred fact must carry **≥1 chunk-anchored evidence item** (chunk exists in
`chunks`). The Sep 6 audit showed live synthesis currently produces dangling references
(8/8 `prop_*` targets missing, 16/61 chunk targets missing, 24/69 structurally
orphaned) — enforcement must happen at insert time, and the repair must decide what
happens to orphaned historical rows.

---

## §2 Approach

**Two tracks.** This spec covers the CT-side structural fix (the PR). An upstream
engine-side guard is filed as a separate core-llm-wiki issue (non-blocking; see §7).

**Central invariant:** `llm_wiki_entries.source_ref` may only ever contain values that
`normalizeSourceRef` maps to themselves — charset `[A-Za-z0-9._- ]`, trimmed, ≤255 chars
— or NULL. Structured evidence moves out of that column into CT-owned storage.

### 2.1 New CT-owned evidence table

```sql
CREATE TABLE IF NOT EXISTS librarian_evidence (
  entry_id      TEXT PRIMARY KEY REFERENCES llm_wiki_entries(id) ON DELETE CASCADE,
  proposal_id   TEXT NOT NULL,
  evidence_json TEXT NOT NULL CHECK(json_valid(evidence_json)),
  unanchored    INTEGER NOT NULL DEFAULT 0,
  created_at    INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS librarian_evidence_proposal_idx ON librarian_evidence(proposal_id);
```

- Written in the **same transaction** as the `llm_wiki_entries` INSERT.
- `evidence_json` is the exact `evidence_json_with_hashes` payload (unchanged shape).
- The `json_valid` CHECK is the database-level guardrail (Kurt's suggestion, adapted):
  JSON is universally required in this column, so the constraint rejects any future
  mangling **loudly at write time** — in the CT-owned table the engine never touches.
- **FK CASCADE is not relied upon.** SQLite enforces foreign keys only with
  `PRAGMA foreign_keys=ON` per connection, and brain.db has multiple connections
  (Rust `DbState` + engine via `wiki_exec`/`wiki_run`) whose pragma state we cannot
  guarantee. All deletion paths therefore issue **explicit**
  `DELETE FROM librarian_evidence WHERE entry_id IN (…)` alongside the entry deletes.
  **This explicitly includes existing hard-delete paths that predate this spec**
  (CodeRabbit/Kurt, Sep 6): `prune_old_librarian_inferred` (src-tauri/src/lib.rs:1752)
  and every other path that deletes `llm_wiki_entries` rows by
  `source_type = 'librarian_inferred'` (heal purge, wiki_forget, repair orphan
  deletion, proposal retraction) must add the paired evidence-row DELETE — audit at
  implementation time and add wherever missing. The CASCADE clause stays as
  documentation of intent only.
- Implementer verifies JSON1 availability (`SELECT json_valid('{}')`) on both the
  bundled Tauri SQLite and via the engine connection once, in the E2E test.

### 2.2 `source_ref` becomes a normalizer-idempotent token

For `librarian_inferred` rows: `source_ref := "librarian-" + <hex hash of entry id>`
(lowercase hex digest, e.g. first 32 hex chars of SHA-256 — **derived by hash, not by
slicing the proposal id**, so no assumption about id format). The token is
**per-entry unique**: the old JSON refs could differ between facts of one proposal
(different evidence subsets), so a per-proposal token would silently change
dedupe/supersede collision semantics — the hash-of-entry-id scheme preserves
distinctness. Charset-legal, well under 255, `normalizeSourceRef(token) === token`.
Document-sourced rows keep their existing refs: after the engine's first setup pass any
legacy path-like ref is already at a normalizer fixed point (that rewrite is the
migration working as intended — **implementer must verify** no CT write path introduces
new non-fixed-point path refs). The GLOB selector never matches the token → the setup
rewrite becomes a no-op for CT rows.

### 2.3 Consumer migration

Consumers that currently parse JSON out of `source_ref` move to `librarian_evidence`
(joined by `entry_id`); keying/matching consumers move to the token:

| Consumer | Today | After |
| --- | --- | --- |
| `source_docs_from_ref` (entities.rs:201-244) | parse JSON from ref | join `librarian_evidence` |
| `source_ref_is_still_grounded` (commit.rs:294) | parse JSON, find chunks | **branch by row type**: token rows → join `librarian_evidence`, verify chunk ids exist; path-ref document rows → existing behavior unchanged. **Phase-1 carve-out (Round-2 review, Sep 6): token rows with `unanchored=1` are treated as still-grounded while the flag is set** — otherwise `heal_invalid_sources` (lib.rs:412) soft-deletes them and `prune_old_librarian_inferred` hard-deletes them 7 days later, destroying the drop-rate data the Phase-1 policy exists to measure. The Phase-2 re-grade (§2.4) is the ONLY path that purges still-unanchored rows, and it does so deliberately, after export |
| `wiki_forget::forget_entries_by_source_refs` (wiki_forget.rs:25) | exact-match JSON ref | exact-match token. **Token routing (CodeRabbit/Kurt, Sep 6): the token is derived from the entry id, so a caller holding only a `proposal_id` cannot construct it directly.** Retraction must first query `librarian_evidence` by `proposal_id` to resolve the target `entry_id`s, then derive the tokens (hash of entry id) and pass those to the exact-match forget. Implementer sweeps all existing retraction callers for this two-step shape |
| commit-path dedupe/supersede by `source_ref` | match JSON ref | match token (Round-2 review: dedupe in this codebase keys on normalized body, not source_ref — this row is precautionary; implementer verifies which consumers actually exact-match the ref) |
| `get_chunk_ids_for_wiki_entry` (lib.rs:2528) | path-normalize the JSON blob, returns `[]` | join `librarian_evidence` chunk ids (currently dead for JSON rows — reviving it is the natural fix) |
| outbox payload (`push_entries_outbox`, commit.rs:938-960) | carries full JSON | unchanged — CT-owned, JSON is fine there. **Verified, not assumed**: implementer confirms no outbox drain path (including the engine's apply side) ever writes the payload's source_ref back into `llm_wiki_entries`; if one exists it joins the migration |
| **bundle export** (`bundle_io.rs:49` SELECTs source_ref into the export) | carries full JSON | export also includes the row's `librarian_evidence` row (bundle export becomes brain-complete for librarian facts: entries + evidence + chunks + proposals), and the token replaces JSON in the entries columns (Round-2 review: bundle_io/bundle_apply is a second write path into `llm_wiki_entries` the spec must cover) |
| **bundle apply** (`bundle_apply.rs:358-405` INSERTs facts with the bundle's source_ref) | inserts JSON ref | inserts the token + its evidence row together. **Token-without-evidence grounding rule**: a token row whose `librarian_evidence` row is missing is treated as **still-grounded** (defensive, consistent with the existing parse-error/DB-error policy in `source_ref_is_still_grounded`) — never auto-purged, surfaced as a census warning instead |
| TS/frontend readers of `source_ref` | unverified | implementer sweeps frontend for parse sites; any that JSON-parse the ref for librarian rows join the token migration |

### 2.4 Provenance enforcement (insert-time)

**Phased policy (Kurt, Sep 6):**

- **Phase 1 — write-with-flag (ships with this PR, active for the initial supervised
  live re-run):** inferred facts whose evidence has zero existing-chunk anchors are
  still written, but flagged (column on `librarian_evidence`, e.g.
  `unanchored INTEGER NOT NULL DEFAULT 0`, set to 1) and counted in the run summary.
  Purpose: measure the exact drop rate of unanchored facts against real synthesis
  output before committing to the strict policy — the Sep 6 audit suggests it may be
  a large fraction, and destroying that data sight-unseen would be irreversible.
- **Phase 2 — skip+log (permanent default, flipped after the baseline is measured):**
  once the supervised re-run confirms the drop rate, the default flips to fail-closed
  skip+log (unanchored inferred facts do not enter the table; logged with proposal id
  and reason, counted in the run summary). Flagged rows from Phase 1 are re-graded:
  still-unanchored ones are exported and purged (same treatment as repair orphans).
  **Phase-2 flip includes reverting the heal/prune carve-outs** (§2.3): once no new
  `unanchored=1` rows are being written and the re-grade has purged the old ones,
  grounding for token rows is strictly evidence-based again.

The flag is also the natural input to deciding whether synthesis's chunk selection
itself needs a follow-up fix (dangling refs at the source).

### 2.5 Repair migration (the ~260 damaged rows)

1. **Census** (extends `warn_on_malformed_source_refs`): a `librarian_inferred` row is
   damaged **iff its `source_ref` does not match the token shape
   `^librarian-[0-9a-f]{32}$`** (positive token-shape test, not prefix heuristics —
   Round-2 review: `evidence_json_with_hashes` serializes `proposal_id` first and the
   normalizer keeps underscores, so mangled blobs actually begin `proposal_id…`, not
   `evidenceproposal_id…`; the earlier prefix list would have matched nothing and the
   repair would silently miss rows). Census and ALL subsequent mutation are
   explicitly restricted to `source_type = 'librarian_inferred'` (CodeRabbit/Kurt,
   Sep 6): a legitimate document-sourced `source_ref` can itself hit the 255-char cap
   (long vault paths normalize to exactly 255), so shape/length heuristics alone
   could classify valid document entries as damaged and delete good data. Every census
   query, repair UPDATE, and orphan DELETE in this migration carries the
   `source_type = 'librarian_inferred'` predicate — no exceptions.
2. **Backup**: export affected rows (+ their re-derived evidence) to
   `<brain>/repair-export-186/` before any mutation.
3. **Re-derive**: for each damaged row whose `curated_proposals` row survives, rebuild
   `evidence_json` from `curated_proposal_items.evidence`, insert into
   `librarian_evidence`, rewrite `source_ref` to the token. The entry→item mapping was
   destroyed by the mangling, so the **reconstruction rule is: attach the proposal's
   FULL item evidence to each of its surviving entries** (all items of the proposal,
   not a per-entry subset). The rebuilt blob therefore may not byte-equal the original
   per-entry blob — acceptable: grounding verification re-checks chunk existence at
   repair time, and superseding is per-entry. **Fallback-to-delete**: a damaged row
   whose proposal survives but whose `curated_proposal_items` rows are all gone gets
   exported and deleted (same treatment as orphans) — no stuck class remains.
4. **Orphans** (proposal or all anchor chunks gone — the 8/8 + 16/61 + 24/69 classes):
   export then **delete**. They are ungrounded by definition; keeping them re-arms the
   heal-purge bait problem this issue exists to end.
   **Export hazard (Kurt, Sep 6)** — a supported export is officially defined as
   **brain-complete**: it must include entries, evidence, chunks, and proposals. A
   partial export (entries without chunks) would make legitimately-anchored facts look
   like orphans, and the orphan deletion above would destroy good data. Guard: the
   repair migration **asserts the database contains a complete chunk schema before
   executing any orphan deletion** — all expected tables present (`chunks`,
   `documents`, `embeddings`, **and the proposals tables**, per the brain-complete
   definition), with **non-emptiness required only for `chunks` and `documents`**
   (embedding tables are legitimately empty on any DB whose embed sweep has not yet
   run — requiring non-empty embeddings would false-positive on healthy databases
   and block orphan deletion forever in the fail-safe direction, Round-2 review).
   If the assertion fails, orphan deletion is skipped entirely (repair of
   re-derivable rows still proceeds) and the failure is reported loudly. Fallback if
   the assertion cannot be reliably expressed at migration time: introduce an
   `import_pending` state on imported graphs that heal and orphan-deletion respect
   until the import is confirmed brain-complete.
5. **Idempotent**: re-running the migration is a no-op (token rows and existing
   `librarian_evidence` rows are left untouched).

### 2.6 End-to-end regression test (the #169 lesson)

The test that PR #169 lacked — drives the **real** synthesis persistence path, not
fixtures:

- mockito External generation provider (`tests/folder_rules.rs:147-190` pattern:
  mock `POST /v1/chat/completions` returning the synthesis JSON, `LlmConfig` with
  `GenerationProviderKind::External` written into `CURATED_BRAIN_DIR`)
  + `CURATED_EMBED_STUB=constant8` + `TestApp` brain-dir redirect (no
  `CT_ALLOW_LIVE_BRAIN`).
- Run the real librarian synthesis command → then assert:
  1. every `librarian_inferred` `source_ref` is a **fixed point** of the normalizer
     semantics (re-implement the JS regex in the test; assert
     `normalize(token) === token` and length ≤ 255);
  2. **phase-aware evidence assertion** (Round-2 review): every inferred fact has a
     `librarian_evidence` row whose `evidence_json` parses; anchored facts have ≥1
     `chunk_id` present in `chunks`; unanchored facts carry `unanchored=1` and are
     counted in the run summary (under Phase-1 write-with-flag, a fact with zero live
     anchors is a legitimate, expected write — the old blanket "≥1 chunk present"
     assertion contradicted §2.4);
  3. **engine-simulation pass**: run the GLOB selector + normalize + rewrite semantics
     over the table exactly as `setup()` does → **zero rows change**. Supplemental
     fast check — NOT the acceptance gate (see engine-in-the-loop gate below).
- **Engine-in-the-loop gate (the acceptance gate)**: a test harness copies a scratch
  brain.db seeded with CT-shaped rows, points node at the **actual installed engine**
  (`@equationalapplications/core-llm-wiki` as installed — NOT a re-implementation;
  with installed-vs-pinned version skew, only the real engine is authoritative), runs
  its `setup()`, and asserts zero source_ref changes to CT rows. **The gate must read
  and record the active engine version** (`node -e "console.log(require(
  '@equationalapplications/core-llm-wiki/package.json').version)"` or equivalent) in
  its output/assertions, so drift between installed and pinned versions is visible in
  every test run, never silent (Kurt, Sep 6). Run on main pre-fix
  (marked `#[ignore]`, demonstrated via `cargo test -- --ignored`) it doubles as the
  real-repro proof that the shipped engine mangles JSON refs; post-fix it is the
  "bug is dead" gate.

Additional tests: repair-migration test (fixture with mangled-shape + orphan rows →
repaired/deleted/exported correctly, idempotent on re-run; plus a
**brain-complete assertion fixture** — orphan deletion skipped when the chunk schema
is partial); provenance tests per §2.4 phases (Phase 1: dangling-chunk evidence →
row written with `unanchored=1`, counted; Phase 2: skipped+logged);
`json_valid` CHECK rejection test (mangled write to `librarian_evidence` fails loudly);
**census-scope test** — a document-sourced row with a 255-char path ref is NOT
classified as damaged and NOT touched by the repair migration (the CodeRabbit census
gap made into a pinned regression); **retraction-routing test** — retracting by
`proposal_id` correctly resolves entry_ids via `librarian_evidence` and forgets by
derived token; **paired-delete test** — `prune_old_librarian_inferred` and each
audited hard-delete path leaves zero orphaned `librarian_evidence` rows.

---

## §3 Rejected alternatives

- **Engine-side fix only** (exempt JSON-parseable refs from
  `findRowsForSourceRefMigration`): correct long-term, and worth doing upstream — but
  every existing install keeps mangling until upgraded, and version skew is not
  hypothetical: **installed core-llm-wiki is 6.0.1 while package.json pins 7.1.0**
  (verified in `node_modules`). CT's data must be engine-proof regardless. Filed
  upstream, non-blocking (§7).
- **`CHECK(json_valid(source_ref))` directly on `llm_wiki_entries`**: rejects
  legitimate path-like refs (document-sourced facts) and the engine's NULL writes; the
  table's DDL is co-owned with the engine (mirrored in engine dist), so a CT-side CHECK
  risks breaking engine assumptions. The guardrail lands on the new CT-owned column,
  where JSON is universally required.
- **Repair-only (no structural change)**: repaired JSON rows are re-mangled at the next
  `setup()` — the rewrite is unconditional. Disqualified outright.
- **Keep JSON in `source_ref`, rely on a future engine release**: violates the §2
  invariant by design; any engine ≤ current destroys the data.

---

## §4 Data flow (after)

```
synthesis resolve_evidence (unchanged)
  → NewProposalItem.evidence (unchanged)
commit path (one transaction):
  [provenance gate: annotate unanchored = 0|1 per Phase-1 policy (never blocks the write)]
  INSERT llm_wiki_entries (source_ref = "librarian-<hex entry-id hash>")
  INSERT librarian_evidence (evidence_json = full JSON,              ← json_valid CHECK
                             unanchored = 0|1 per Phase-1 policy)    ← Kurt, Sep 6
  push_entries_outbox (full JSON payload, unchanged)
engine setup():
  GLOB '*[^-A-Za-z0-9._ ]*' matches nothing CT wrote → rewrite is a no-op
heal / grounding / provenance display:
  join librarian_evidence by entry_id
forget / retraction:
  evidence-row delete + exact-match on the token
```

## §5 Error handling

- `librarian_evidence` insert failure → the whole entry transaction rolls back (the
  fact is not written). Fail-closed, consistent with the CHECK guardrail.
- Synthesis evidence with zero existing-chunk anchors → Phase 1: written with
  `unanchored=1`, logged with proposal id, counted in the run summary. Phase 2
  (post-baseline): skipped and logged. See §2.4.
- Repair migration: backup-before-mutate; orphan deletion **gated on the
  brain-complete schema assertion** (§2.5.4) and only after successful export;
  idempotent re-runs.

## §6 Testing summary

See §2.6 — the **engine-in-the-loop gate** (real installed engine, scratch brain.db,
zero CT-row changes post-setup) is the acceptance gate; the E2E synthesis test,
engine-simulation pass, repair, provenance-skip, CHECK-rejection, and token-idempotence
property tests round it out. All tests run under `CURATED_BRAIN_DIR` redirect (the #178
live-brain guard panics otherwise).

## §7 Resolved decisions & remaining out-of-scope items

1. **Upstream engine guard** *(out of scope, follow-up)*: file the core-llm-wiki issue
   (setup back-rewrite must exempt structured/JSON-parseable refs). Non-blocking for
   this PR.
2. **Version skew** *(RESOLVED, Kurt Sep 6)*: installed 6.0.1 vs pinned 7.1.0 — the
   engine version reconciliation stays a **separate ops task**, but the
   engine-in-the-loop gate **must read and record the active engine version** in its
   output assertions so drift is never silent (§2.6).
3. **Orphaned historical rows** *(RESOLVED)*: export-then-delete, gated by the
   brain-complete schema assertion (§2.5.4).
4. **Post-landing librarian re-run** *(planned)*: one supervised re-run at real LLM
   cost, per the issue's HOLD note — scheduled after merge; doubles as the Phase-1
   drop-rate measurement run.
5. **Insert-time skip policy** *(RESOLVED, Kurt Sep 6)*: phased — write-with-flag for
   the initial supervised live run (measure the unanchored drop rate), then flip the
   permanent default to skip+log once the baseline is confirmed (§2.4).
6. **Mock vs live** *(RESOLVED, Kurt Sep 6)*: mock for CI. The supervised live run is
   the **live acceptance for requirement 3** (issue #186's "live synthesis output"),
   executed **post-merge** per the HOLD release condition and doubling as the Phase-1
   drop-rate measurement — distinct from the **pre-merge engine-in-the-loop gate**
   (§2.6), which remains the CI acceptance gate. Two gates, two names, no conflation.
7. `get_chunk_ids_for_wiki_entry` revival (§2.3) — still open for the implementer to
   confirm live callers vs. deprecate.
