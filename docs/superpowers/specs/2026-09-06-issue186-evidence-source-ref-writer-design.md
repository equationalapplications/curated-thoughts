# Spec: Issue #186 — evidence source_ref mangling: structural fix, provenance enforcement, data repair

**Date:** 2026-09-06
**Status:** Implemented (2026-09-06, branch `spec/issue186-source-ref-writer`, PR #188)
**Branch:** `spec/issue186-source-ref-writer`
**Priority:** P1 — librarian re-runs are on HOLD until this lands; blocks the evidence-provenance backlog item and the wiki rebuild.
**Baseline:** `main` @ `bc2a283` (v2.5.1)
**Issue:** equationalapplications/curated-thoughts#186 ("Fixes #186" on the implementation PR)
**Evidence base:** explore-first report (this session, 68 tool calls, read-only):
`docs/superpowers/references/2026-09-06-issue186-explore-report.md` — all file:line anchors below are from that report and
must be re-verified by the implementer against the checkout at implementation time.

---

## §1 Problem (verified current state)

### 1.1 Root cause — CONFIRMED, with a correction to the issue's hypothesis

The Rust commit path writes **valid JSON** into `llm_wiki_entries.source_ref`
(`db/commit.rs:900` `evidence_json_with_hashes` → INSERT at `commit.rs:917-936`).
The destruction happens **afterwards, in the JS engine**:

- core-llm-wiki's `setup()` runs an **unconditional legacy-ref back-rewrite**:
  `findRowsForSourceRefMigration()` (7.1.0 dist/index.js:1454) selects every row whose
  `source_ref` triggers any of **five predicates** — `TRIM(source_ref) != source_ref`,
  `INSTR(source_ref,'/')>0`, `INSTR(source_ref,'\')>0`,
  `INSTR(source_ref,CHAR(0))>0`, or GLOB `'*[^-A-Za-z0-9._ ]*'` — **every JSON blob
  qualifies** — and rewrites each through `normalizeSourceRef` (7.1.0 dist:4082)
  unconditionally inside `setup()` (7.1.0 dist:7782-7791).
- This runs on **every app launch** (`src/main.tsx:32`) and **every outbox-worker
  start/stop transition** (`src/lib/wiki.ts:332-352`), over the **shared brain.db**
  (`wiki_exec`/`wiki_run` → `src-tauri/src/lib.rs:2190/2207`).
- **Engine version ground truth (Kurt, Sep 6)**: the pin is **7.1.0** (#183,
  `2bf1c18`; V17 gates opening pre-7.1 DBs). The mangler is intact in the 7.1.0 dist
  at the anchors above — **the current pin itself mangles**. The committed explore
  report's anchors were taken against 6.0.1.
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

**Migration placement (Opus review, Sep 6)**: this is a **numbered ladder step — V18**.
The repo has `db/migration.rs` / `db/okf_migration.rs`; `tests/okf_migration.rs:222`
currently pins `assert_eq!(max_version, 17)` and moves to 18 with this change. The
repair pass (§2.5) is a **one-shot repair inside the V18 step** — not a recurring
startup pass — which is what makes §2.5.6's idempotence requirement natural: a numbered
migration runs once per DB, and re-running against an already-repaired DB must be a
no-op.

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

For `librarian_inferred` rows: `source_ref := "librarian-" + <hex digest>`, where
`<hex digest>` is **exactly the first 32 lowercase hex characters of the SHA-256 of
the entry id (normative — no 'e.g.'; this exact shape is what the census regex
`^librarian-[0-9a-f]{32}$` keys on)** — **derived by hash, not by
slicing the proposal id**, so no assumption about id format. The token is
**per-entry unique**: the old JSON refs could differ between facts of one proposal
(different evidence subsets), so a per-proposal token would silently change
dedupe/supersede collision semantics — the hash-of-entry-id scheme preserves
distinctness. Charset-legal, well under 255, `normalizeSourceRef(token) === token`.
Document-sourced rows keep their existing refs: after the engine's first setup pass any
legacy path-like ref is already at a normalizer fixed point (that rewrite is the
migration working as intended — **implementer must verify** no CT write path introduces
new non-fixed-point path refs). **The engine's migration selector is a five-predicate
OR, not just the GLOB** (Opus review, Sep 6 — implementer reads it from the installed
engine's dist, not from this spec): `TRIM(source_ref) != source_ref` OR
`INSTR(source_ref,'/')>0` OR `INSTR(source_ref,'\')>0` OR `INSTR(source_ref,CHAR(0))>0`
OR the GLOB `'*[^-A-Za-z0-9._ ]*'`. The token survives all five — but note the charset
permits **space**, so a ref with leading/trailing whitespace passes GLOB and is still
selected by the TRIM predicate; the token-idempotence property test must assert against
the full predicate set, not the GLOB alone. The setup rewrite becomes a no-op for CT
rows.

### 2.3 Consumer migration

Consumers that currently parse JSON out of `source_ref` move to `librarian_evidence`
(joined by `entry_id`); keying/matching consumers move to the token:

| Consumer | Today | After |
| --- | --- | --- |
| `source_docs_from_ref` (entities.rs:201-244) | parse JSON from ref | join `librarian_evidence` |
| `source_ref_is_still_grounded` (commit.rs:294) | parse JSON, find chunks | **branch by row type**: token rows → join `librarian_evidence`, verify chunk ids exist; path-ref document rows → existing behavior unchanged. **Phase-1 carve-out (Round-2 review, Sep 6): token rows with `unanchored=1` are treated as still-grounded while the flag is set** — otherwise `heal_invalid_sources` (lib.rs:412) soft-deletes them and `prune_old_librarian_inferred` hard-deletes them 7 days later, destroying the drop-rate data the Phase-1 policy exists to measure. The Phase-2 re-grade (§2.4) is the ONLY path that purges still-unanchored rows, and it does so deliberately, after export. **Missing-evidence-row stance (Opus review, Sep 6): a token row whose `librarian_evidence` row is absent ⇒ treat as still-grounded + loud warn** — same defensive posture as the existing parse-error/DB-error branches; §2.1 deliberately does not rely on FK CASCADE, so missing-evidence windows are a *when*, not an *if*. Pinned as the **seventh D-test** alongside the existing six |
| `wiki_forget::forget_entries_by_source_refs` (wiki_forget.rs:25) | exact-match JSON ref | exact-match token. **Token routing (CodeRabbit/Kurt, Sep 6): the token is derived from the entry id, so a caller holding only a `proposal_id` cannot construct it directly.** Retraction must first query `librarian_evidence` by `proposal_id` to resolve the target `entry_id`s, then derive the tokens (hash of entry id) and pass those to the exact-match forget. Implementer sweeps all existing retraction callers for this two-step shape |
| commit-path dedupe/supersede by `source_ref` | match JSON ref | match token (Round-2 review: dedupe in this codebase keys on normalized body, not source_ref — this row is precautionary; implementer verifies which consumers actually exact-match the ref) |
| `get_chunk_ids_for_wiki_entry` (lib.rs:2528) | path-normalize the JSON blob, returns `[]` | join `librarian_evidence` chunk ids — **hard requirement, see §7.7**, plus the TS `rootChunkId`→`entryId` rename and the `rowid = ?1 OR id = ?1` fix. Not optional: a live consumer chain reaches it via `wikiGraphAdapter.ts:24`, and today's `[]` silently degrades into a wrong-namespace `getImpactRadius` call |
| outbox payload (`push_entries_outbox`, commit.rs:938-960) | carries full JSON | unchanged — CT-owned, JSON is fine there. **Verified, not assumed**: implementer confirms no outbox drain path (including the engine's apply side) ever writes the payload's source_ref back into `llm_wiki_entries`; if one exists it joins the migration |
| **bundle export** (`bundle_io.rs:49` SELECTs source_ref into the export) | carries full JSON | export also includes the row's `librarian_evidence` row (bundle export becomes brain-complete for librarian facts: entries + evidence + chunks + proposals), and the token replaces JSON in the entries columns (Round-2 review: bundle_io/bundle_apply is a second write path into `llm_wiki_entries` the spec must cover) |
| **bundle apply** (`bundle_apply.rs:358-405` INSERTs facts with the bundle's source_ref) | inserts JSON ref | inserts the token + its evidence row together. Token-without-evidence grounding follows the **single source of truth in the `source_ref_is_still_grounded` row above** (still-grounded + loud warn + census warning; never auto-purged) |
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
   it asks "is this the new shape" rather than "does it look mangled", so detection
   stays correct regardless of the mangled blobs' internal layout. The observed mangled
   shapes are documented in §2.5.4, where they drive **recovery**, not detection).
   Within
   `source_type = 'librarian_inferred'` the length-255 signal is now safe as
   corroboration **because no legitimate post-fix ref is 255 chars — the token is
   short**; that is the whole justification for the census scoping rule. Census and ALL
   subsequent mutation are explicitly restricted to
   `source_type = 'librarian_inferred'` (CodeRabbit/Kurt, Sep 6): a legitimate
   document-sourced `source_ref` can itself hit the 255-char cap (long vault paths
   normalize to exactly 255), so shape/length heuristics alone could classify valid
   document entries as damaged and delete good data. Every census query, repair UPDATE,
   and orphan DELETE in this migration carries the `source_type = 'librarian_inferred'`
   predicate — no exceptions. **NULL refs**: the census explicitly requires
   `source_ref IS NOT NULL` — a NULL `source_ref` on a `librarian_inferred` row is
   pre-existing engine-era data (the engine's own writes), is untouched by this
   migration, and is counted in the census output as `null_ref_count` for visibility
   only (no repair, no delete — NULL is outside the central invariant's scope, which
   permits NULL).
2. **Backup**: export affected rows (+ their re-derived evidence) to
   `<brain>/repair-export-186/` before any mutation.
3. **Valid-JSON-first branch** (Kurt, Sep 6): if a damaged-scope row's `source_ref`
   still **parses as valid JSON**, migrate it **verbatim** into
   `librarian_evidence.evidence_json` and rewrite the ref to
   the token. **If the valid JSON lacks a `proposal_id` field → export and delete
   directly** (same treatment as orphans) — valid JSON is never routed through the
   mangled-text extraction rule, which only applies to unparseable refs. Only rows
   whose ref does **not** parse proceed to re-derivation. This
   preserves exact original evidence wherever it survived, and shrinks the
   re-derivation set to the truly mangled rows.
4. **Re-derive**: for each still-mangled row whose owning proposal can be resolved,
   rebuild `evidence_json` from `curated_proposal_items.evidence`, insert into
   `librarian_evidence`, rewrite `source_ref` to the token.

   **Key order — CORRECTED (Opus review, Sep 6; supersedes the Round-2 assumption).**
   `evidence_json_with_hashes` does **not** emit `proposal_id` first. `serde_json::Map`
   is a `BTreeMap` — keys serialize **alphabetically** — unless the `preserve_order`
   feature is active, and it is **not** active for this crate's runtime graph:
   `cargo tree` resolves two distinct feature sets for `serde_json v1.0.151`, and the
   only `preserve_order` activation traces to `tree-sitter` under
   `[build-dependencies]`, which `resolver = "2"` (`Cargo.toml:3`) does **not** unify
   into the normal dependency graph. Therefore `evidence` sorts before `proposal_id`
   and **every mangled blob begins `evidence`**. The live DB confirms exactly the two
   shapes this predicts (explore report lines 33 and 220-222) — a `proposal_id`-first
   writer would have produced only one. Recovery splits on those two shapes:

   **4a. Outbox-first (preferred, try before any head-parsing).** Check whether local
   `wiki_outbox` rows from the Sep 3–5 waves still carry the **pristine, untruncated**
   JSON payload for the damaged entries (§2.3: the outbox payload is CT-owned and was
   never mangled). Where one survives, use it directly as `evidence_json` — it is the
   exact original, byte-for-byte, and needs no reconstruction at all. Implementer
   establishes retention first: local outbox rows are deleted on drain, so treat
   survival as unlikely — but it is free to check and strictly better than any
   reconstruction below.

   **4b. `evidenceproposal_id%` rows — `proposal_id` intact.** These are the
   **empty-evidence** serializations: `{"evidence":[],"proposal_id":"prop_…"}` mangles
   to `evidenceproposal_idprop_<24hex>`, short enough that the 255-char cap never
   truncated it. Extract directly: strip the leading literal `evidenceproposal_id` and
   take the remainder to end-of-string. Validate: non-empty, and must match an existing
   `curated_proposals.id` (`prop_` + 24 hex, per `generate_llm_id`,
   `commit.rs:256-260`). Note these rows carried **no evidence** to begin with, so they
   land in the unanchored/orphan class on re-derivation regardless.

   **4c. `evidencechunk_id%` rows — `proposal_id` destroyed, recover via
   `content_hash`.** These are the non-empty serializations. `proposal_id` sorted last,
   sat at the tail of the blob, and was **truncated away** by `.slice(0, 255)`: it is
   not present in the ref, and any rule that looks for it here fails on every such row
   — which, per explore report line 33, is the bulk of the 260. What *does* survive in
   the truncated head is the first evidence item's `chunk_id` and `content_hash`.
   Resolve the proposal through those: strip to the literal `content_hash` key token,
   take the following hex run (prefer `content_hash` over `chunk_id` — the hash is
   stable across re-chunks, while `chunk_id` is a legacy rowid and may be absent), then
   query `curated_proposal_items.evidence` for the item carrying that hash and read its
   owning `curated_proposals.id`. If the hash resolves into **more than one** proposal,
   prefer the proposal whose `created_at` is nearest the entry's and record the
   ambiguity in the repair report.

   **Fallback-to-delete** applies only after 4a, 4b and 4c have all failed: a row whose
   proposal cannot be resolved by any path — or whose proposal resolves but whose
   `curated_proposal_items` rows are all gone — is exported and deleted (same treatment
   as orphans). Never a stuck class, never a silent mis-attribution.

   The entry→item mapping was destroyed by the mangling, so the **reconstruction rule
   is: attach the proposal's FULL item evidence to each of its surviving entries** (all
   items of the proposal, not a per-entry subset) — except for 4a rows, which restore
   their exact original payload verbatim. A rebuilt blob may therefore not byte-equal
   the original per-entry blob — acceptable: grounding verification re-checks chunk
   existence at repair time, and superseding is per-entry.
5. **Orphans** (proposal or all anchor chunks gone — the 8/8 + 16/61 + 24/69 classes):
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
   **Verifiable backup invariant (Round-3 review, Sep 6)** — the table-level
   check above cannot prove a *partial import* complete: an import may carry
   non-empty `chunks`/`documents` while omitting the specific chunk a valid
   inferred fact anchors to. That distinction is not decidable from the
   database alone (legitimate chunk deletion and never-imported are
   indistinguishable), so `import_pending` stays the documented fallback for a
   future import pipeline. What **is** decidable at migration time — and is
   therefore enforced — is the backup invariant: the migration compares the
   export count against the repair census and refuses to enter the destructive
   phase unless `exported == census.damaged`, i.e. every doomed row is provably
   on disk in `repair-export-186/` before any deletion. A mismatch skips the
   repair (fail-safe, same handling as brain-incomplete) with a loud WARN; the
   data survives damaged rather than dies unbacked.
6. **Idempotent**: re-running the migration is a no-op (token rows and existing
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
  3. **engine-simulation pass**: run the **full five-predicate selector** (§2.2:
     `TRIM != self` OR `INSTR '/'` OR `INSTR(source_ref,'\')` OR `INSTR CHAR(0)` OR GLOB —
     not the GLOB alone) + normalize + rewrite semantics over the table exactly as
     `setup()` does → **zero rows change**. Supplemental
     fast check — NOT the acceptance gate (see engine-in-the-loop gate below).
- **Engine-in-the-loop gate (the acceptance gate)**: a test harness copies a scratch
  brain.db seeded with CT-shaped rows, points node at the **actual installed engine**
  (`@equationalapplications/core-llm-wiki` as installed — NOT a re-implementation;
  with installed-vs-pinned version skew, only the real engine is authoritative), runs
  its `setup()`, and asserts zero source_ref changes to CT rows. **The gate must read
  and record the active engine version** (`pnpm ls --json
  @equationalapplications/core-llm-wiki` — the `node -e` package.json read fails due
  to the package's exports map) in its output/assertions, so drift between installed
  and pinned versions is visible in every test run, never silent (Kurt, Sep 6). Run on main pre-fix
  (marked `#[ignore]`, demonstrated via `cargo test -- --ignored`) it doubles as the
  real-repro proof that the shipped engine mangles JSON refs; post-fix it is the
  "bug is dead" gate. **CI runs it on every push** (review round 5, finding 6): the
  "Engine source_ref acceptance gate" step in `ci.yml` executes the `#[ignore]`d test
  with `--ignored` against the pnpm-installed engine, so the gate the spec designates
  as the CI acceptance gate actually runs in CI instead of only on demand.

Additional tests: repair-migration test (fixture with mangled-shape + orphan rows →
repaired/deleted/exported correctly, idempotent on re-run; **fixtures must cover both
§2.5.4 recovery paths — an `evidenceproposal_id%` row recovered by direct id extraction
and an `evidencechunk_id%` row recovered by `content_hash` lookup — plus a row that
resolves by neither and must be exported-and-deleted**; plus a
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
  every existing install keeps mangling until the engine ships a fix. **The current
  pin itself mangles**: core-llm-wiki **7.1.0** (the checkout's installed version —
  the earlier 6.0.1-vs-7.1.0 skew was resolved by #183 / `2bf1c18`, which also gates
  pre-7.1 DB opens via V17; the mangler is verifiably intact in 7.1.0's dist at the
  `setup()` rewrite) still rewrites JSON refs unconditionally (Opus review, Sep 6 —
  re-verified against the checkout). CT's data must be engine-proof regardless of
  upstream timing. Filed upstream, non-blocking (§7).
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
  full five-predicate selector (§2.2) matches nothing CT wrote → rewrite is a no-op
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
  brain-complete schema assertion** (§2.5.5) and only after successful export;
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
2. **Version skew** *(RESOLVED twice over)*: the 6.0.1-vs-7.1.0 skew Kurt ruled a
   separate ops task (Sep 6) is **already resolved** — #183 (`2bf1c18`) landed; the
   checkout now resolves to core-llm-wiki 7.1.0 matching the pin, and V17 gates
   pre-7.1 DB opens (Opus review, Sep 6). The re-grounded fact: **7.1.0 itself still
   mangles** — the fix in this spec remains required. The engine-version recording
   requirement in the engine-in-the-loop gate (§2.6) stays: good practice against
   future drift.
3. **Orphaned historical rows** *(RESOLVED)*: export-then-delete, gated by the
   brain-complete schema assertion (§2.5.5).
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
7. **Deep-review round 5 amendments** *(Sep 6, post-implementation on PR #188)* — six
   behavior changes from the 10-finding review wave, all landed in the PR branch:
   - **Manual facts carry NULL** (finding 1): `wisdom.rs` no longer writes the JSON
     sentinel `{"proposal_id":null,"evidence":[]}` — a live mangleable ref this same
     PR wired new MCP/UI writers into. NULL is the "no provenance" value for
     `user_stated` rows; the V18 step normalizes pre-existing sentinel and
     mangled (`proposal_idnullevidence`) user_stated rows to NULL.
   - **One token-shape predicate** (finding 2): `bundle_apply`'s local
     `is_ascii_hexdigit` copy (uppercase-tolerant) is replaced by delegation to the
     canonical `commit::is_librarian_source_ref_token` (`^librarian-[0-9a-f]{32}$`).
   - **No silent provenance loss at the bundle boundary** (findings 3+10): salvage
     uses the single strict helper `commit::proposal_id_from_evidence_json`
     (non-null, non-empty string only). Unsalvageable refs and blobs without a
     usable `proposal_id` are preserved **verbatim in `ImportResult.warnings`**
     instead of silently dropped, and no `proposal_id = ''` evidence row is ever
     written. The V18 repair's two hand-copied extractions use the same helper.
   - **V18 fail-safe posture** (finding 4): an error inside the file-backed repair
     arm (full disk, unwritable `repair-export-186/`, mid-repair SQL fault) no
     longer aborts `migrate()`/`AppDb::open` — the destructive phase is skipped
     with a loud WARN, damaged data survives as-is, and the version stamp still
     lands. Availability outranks auto-retry; manual idempotent
     `run_evidence_repair` is the documented recovery path.
   - **Atomic, replicated repair deletes** (finding 5): the repair's delete arm is
     one transaction (evidence delete + entry delete + edge purge) and pushes one
     `OutboxOperation::Delete` per removed entry — same shape as `wiki_forget`
     (#132 class), so prisma-outbox replicas converge on the erasure.
   - **Errors propagate on export; readers stay aligned** (findings 8+9):
     `evidence_json_for_entry` propagates SQLite errors (the bundle-export path
     halts rather than writing bare tokens with no evidence; UI read paths
     degrade defensively), and `chunk_ids_for_entry` mirrors
     `evidence_has_live_chunk` exactly — hash anchors match with no entity-id
     filter and chunk_id rowid items are accepted as fallback anchors.
7. **`get_chunk_ids_for_wiki_entry` revival** *(RESOLVED — revive; hard requirement,
   Opus review, Sep 6)*: **not** deprecated, and not left to implementer discretion.
   It has a live consumer chain: `lib.rs:3651` (registered command) →
   `src/lib/tauri.ts:526` → `src/lib/wikiGraphAdapter.ts:24` (`resolveRootChunkIds`) →
   `tauriGraphAdapter.getNeighbors`, with coverage in
   `src/__tests__/impact-radius.test.ts`. Its current `[]` return is **not** benign:
   at `wikiGraphAdapter.ts:29-31` an empty result falls through to
   `getImpactRadius(rootChunkId, …)`, passing an **entry id** into a parameter the
   graph treats as a **chunk id** — a wrong-namespace query returning plausible
   garbage, not a no-op. Three required changes:
   - **Revive the lookup**: replace the `normalize_path_argument_to_vault_relative` →
     `safe_vault_path` routing with a join against `librarian_evidence` for token rows
     (§2.3); document-sourced path rows keep their existing behavior.
   - **Rename in TS**: `rootChunkId` → `entryId` in `src/lib/wikiGraphAdapter.ts` and
     `src/lib/tauri.ts` (`getChunkIdsForWikiEntry`'s first parameter is an entry id —
     the Rust signature at `lib.rs:2529` types it `entry_id`). The current name is what
     makes the wrong-namespace fallback read as reasonable code.
   - **Fix the SQL**: `WHERE rowid = ?1 OR id = ?1` (`lib.rs:2547`) binds an `i64`
     against a TEXT `id` (`fact_<hex>`), so the `id` half can never match. Either drop
     it and match `rowid` only, or take the id as a string and match `id` — a recorded,
     deliberate choice, not dead weight left in place.
