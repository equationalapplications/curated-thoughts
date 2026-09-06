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
  guarantee. All deletion paths (forget, heal purge, repair orphan deletion) therefore
  issue **explicit** `DELETE FROM librarian_evidence WHERE entry_id IN (…)` alongside
  the entry deletes. The CASCADE clause stays as documentation of intent only.
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
| `source_ref_is_still_grounded` (commit.rs:294) | parse JSON, find chunks | **branch by row type**: token rows → join `librarian_evidence`, verify chunk ids exist; path-ref document rows → existing behavior unchanged |
| `wiki_forget::forget_entries_by_source_refs` (wiki_forget.rs:25) | exact-match JSON ref | exact-match token (callers build tokens) |
| commit-path dedupe/supersede by `source_ref` (the exact-match consumers the explorer flagged: proposal retraction and ref-keyed dedupe never match a mangled ref) | match JSON ref | match token |
| `get_chunk_ids_for_wiki_entry` (lib.rs:2528) | path-normalize the JSON blob, returns `[]` | join `librarian_evidence` chunk ids (currently dead for JSON rows — reviving it is the natural fix) |
| outbox payload (`push_entries_outbox`, commit.rs:938-960) | carries full JSON | unchanged — CT-owned, JSON is fine there. **Verified, not assumed**: implementer confirms no outbox drain path (including the engine's apply side) ever writes the payload's source_ref back into `llm_wiki_entries`; if one exists it joins the migration |
| TS/frontend readers of `source_ref` | unverified | implementer sweeps frontend for parse sites; any that JSON-parse the ref for librarian rows join the token migration |

### 2.4 Provenance enforcement (insert-time)

In the commit path: an inferred fact is written only if ≥1 evidence item has a
`chunk_id` that **exists in `chunks`**. Otherwise the fact is skipped and logged
(proposal id + reason) and counted in the run summary. Fail-closed: no unanchored
inferred facts enter the table. (Missing-chunk references from synthesis become visible
skips, not silent orphans — this also gives us the data to decide whether synthesis's
chunk selection itself needs a follow-up.)

### 2.5 Repair migration (the ~260 damaged rows)

1. **Census** (extends `warn_on_malformed_source_refs`): count rows matching the mangled
   shapes (`evidenceproposal_id%` / `evidencechunk_id%` prefixes, plus 255-char length)
   — the `{`-prefix query alone misses them.
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
  2. every inferred fact has a `librarian_evidence` row whose `evidence_json` parses
     and contains ≥1 `chunk_id` present in `chunks`;
  3. **engine-simulation pass**: run the GLOB selector + normalize + rewrite semantics
     over the table exactly as `setup()` does → **zero rows change**. Supplemental
     fast check — NOT the acceptance gate (see engine-in-the-loop gate below).
- **Engine-in-the-loop gate (the acceptance gate)**: a test harness copies a scratch
  brain.db seeded with CT-shaped rows, points node at the **actual installed engine**
  (`@equationalapplications/core-llm-wiki` as installed — NOT a re-implementation;
  with installed-vs-pinned version skew, only the real engine is authoritative), runs
  its `setup()`, and asserts zero source_ref changes to CT rows. Run on main pre-fix
  (marked `#[ignore]`, demonstrated via `cargo test -- --ignored`) it doubles as the
  real-repro proof that the shipped engine mangles JSON refs; post-fix it is the
  "bug is dead" gate.

Additional tests: repair-migration test (fixture with mangled-shape + orphan rows →
repaired/deleted/exported correctly, idempotent on re-run); provenance-skip test
(evidence referencing a missing chunk → no row, logged); `json_valid` CHECK rejection
test (mangled write to `librarian_evidence` fails loudly).

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
  [provenance gate: ≥1 existing chunk anchor, else skip+log]
  INSERT llm_wiki_entries (source_ref = "librarian-<hex entry-id hash>")
  INSERT librarian_evidence (evidence_json = full JSON)   ← json_valid CHECK
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
- Synthesis evidence with zero existing-chunk anchors → fact skipped, logged with
  proposal id, counted in the run summary.
- Repair migration: backup-before-mutate; orphan deletion only after successful export;
  idempotent re-runs.

## §6 Testing summary

See §2.6 — the **engine-in-the-loop gate** (real installed engine, scratch brain.db,
zero CT-row changes post-setup) is the acceptance gate; the E2E synthesis test,
engine-simulation pass, repair, provenance-skip, CHECK-rejection, and token-idempotence
property tests round it out. All tests run under `CURATED_BRAIN_DIR` redirect (the #178
live-brain guard panics otherwise).

## §7 Out of scope / open questions for Kurt

1. **Upstream engine guard**: file the core-llm-wiki issue (setup back-rewrite must
   exempt structured/JSON-parseable refs). Non-blocking for this PR.
2. **Version skew**: installed 6.0.1 vs pinned 7.1.0 — reinstall/bump as part of this
   work, or separate ops task? (Recommend separate; this PR must not depend on it.)
3. **Orphaned historical rows**: proposal is export-then-delete (§2.5.4). Alternative:
   keep them with NULL evidence. Recommend delete — they are ungrounded heal-bait.
4. **Post-landing librarian re-run**: one supervised re-run to validate at real LLM
   cost, per the issue's HOLD note — schedule after merge.
5. **Insert-time skip policy** (from GLM review): when synthesis emits dangling chunk
   refs, the spec's default is fail-closed skip+log. Alternatives: write-with-flag
   (unanchored rows visible but marked), or block-the-run. The audit suggests dangling
   refs are a large fraction of live output, so this choice shapes what the post-landing
   re-run produces. Default: skip+log; Kurt may prefer write-with-flag.
6. **Mock vs live for the requirement-3 test**: the E2E test mocks the generation
   provider (mockito). Whether mock-based satisfies issue #186's "live synthesis
   output" wording, or whether the supervised real re-run (item 4) is the true
   satisfaction of it, is Kurt's call. Default: mock for CI + supervised re-run for
   live proof.
7. `get_chunk_ids_for_wiki_entry` revival (§2.3) — confirm it has live callers worth
   reviving, vs. deprecating.
