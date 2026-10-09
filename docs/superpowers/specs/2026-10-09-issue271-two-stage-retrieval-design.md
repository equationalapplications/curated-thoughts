# Two-stage retrieval for the wisdom gate: chunk-stage matching mapped to curated facts (issue #271)

**Date:** 2026-10-09 (rev 5 — Opus spec-tier r3 REQUEST CHANGES resolved:
B1 scratch-check pass/fail rule defined; M1 edited-file refusal scope;
M2 new-doc upsert stays pre-embed; M3 two-stage bench coverage; M4
instr1 rule-(i) query vector; M5 tripped-epoch persistence; m1-m6)
**Status:** Draft
**Branch:** `spec/issue271-two-stage-retrieval`
**Priority:** High (merge-blocker-1 successor for PR #270; closes #265 when live matching works)
**Issue:** equationalapplications/curated-thoughts#271
**Investigation (Step 0, design of record):**
`docs/superpowers/specs/2026-10-09-issue271-two-stage-investigation.md` (v10 —
GLM r1 FIX-FIRST + 9 rounds of real-Opus review, iterations v1→v10; Opus r9:
0 blockers, 4 majors + 4 minors, all resolved in v10). This spec cites the
investigation by section; it does not restate its evidence chain.

## Problem

PR #270's live paired calibration (150 real-message probes, scratch copy of
the live brain, 368 gate-eligible entries) showed the wisdom gate's
fact-layer matching does not transfer from fixtures to live traffic:

- Cell A (raw, floor 0.70): FP 0.036, gate opened on **2/40** relevant probes.
- Cell E (instr1 both-side, floor 0.64): FP 0.018, gate opened on **1/40**.
- No floor in the 0.50–0.72 sweep yields usable hit@2 at FP ≤ 0.05 on live
  traffic; relevant vs irrelevant top-1 medians are indistinguishable.

Root cause: the gate scores incoming messages against
**librarian-synthesized one-line fact cards**
(`llm_wiki_entries.embedding_blob`), which have dropped the specifics that
would match a real message. The immutable-source chunk embeddings already
exist in the same brain (`chunks`/`embeddings`); the gate does not use them.

Kurt's direction (2026-10-07, on the issue): two-stage retrieval — semantic
search the immutable-source chunk embeddings first, map the best-matching
source files to the curated facts derived from them, and gate on those
facts. Injection stays at the curated layer.

The issue's named blocker — the empty fact→source provenance mapping — is
**superseded by the investigation**: the fact→source edge already exists
relationally
(`librarian_evidence → curated_proposal_sources → documents ← chunks.doc_id`;
reverse join `src-tauri/src/wisdom_deposit.rs:183-193`); live coverage is
365/365 entries hop-resolvable, 406/406 evidence-hash refs live.
`llm_wiki_source_ref_index` has 0 rows because nothing writes it on the live
path (only a test fixture does); `source_ref` is a deliberate idempotent
token (`commit.rs:482-487`), not a broken pointer. **No DDL and no backfill
are on the critical path for the read fix.**

## Approach

Add a chunk-stage (stage 1) search restricted to the immutable-source
documents that carry gate-eligible curated facts, then apply calibrated
decision rules that hop from matching chunks to facts. The existing
fact-card scoring remains as the v1 path and as a mandated fallback.
Everything ships in ONE PR (Oct-4 ruling), with flip-to-default conditional
on the acceptance letter below.

Rejected alternatives (from the investigation's review ladder):

- **Populate `llm_wiki_source_ref_index` / backfill `source_ref`** —
  unnecessary on the critical path: the relational hop already resolves
  365/365. Backfill stays conditional on the audit showing gaps
  (implementation order step 4).
- **Search ALL chunks (unrestricted stage 1)** — pollutes the gate with
  documents bearing no curated facts; the closed set is issue #271's stated
  direction (flagged to Kurt in the investigation, no objection).
- **Gate directly on chunks (skip the fact hop)** — violates Kurt's
  constraint that the wisdom layer stays curated; injection is unchanged.
- **Ops watchdog crons** — DECLINED (Kurt ruling, 2026-10-09): no
  watchdogs, nothing alerts. This is precisely why the guard-failure
  fallback below is mandatory (never-dark).

## Design

The design of record is investigation v10 §3 (1)–(9); its §1 carries the
controller/Opus-verified code citations and live-brain measurements
underlying every choice below. Summary of the load-bearing points:

### 1. Stage 1 — closed-set chunk search (NEW SQL)

Top-k chunk search restricted to fact-bearing documents: the candidate doc
set is (hash hop PRIMARY: `documents.hash` matching
`curated_proposal_sources.source_hash` ∪ proposal chain) deduped by
`documents.hash` with a deterministic row choice, filtered to live
gate-eligible facts. On the live brain this is 261 chunks / 47 docs under
rule (i), 62 hash-hop-hit chunks under rule (ii) (chunk-level membership).
Query embed is raw (no prefix). k starts at 8, swept with the floor.
Skip-aware: chunks from skip-set classes are excluded. Stage 1 needs NEW
SQL — `semantic_search` is a full scan with no gate filter
(`tool_dispatch.rs:371-378` RECALL_CHUNKS_SQL_BASE covers wiki_search, not
the gate).

### 2. Shared eligibility predicate

One predicate function (NULL-only title/body test, supersession/validity,
scheme filter, skip-in-Rust set) shared by `gated_entries` (refactored onto
it) and the stage-1 doc-set query, so stage 1 and stage 2 can never
disagree about which facts are eligible. Parity guard for >1024 excludes.

### 3. Decision rules (calibrated, picked from data)

- (i) restricted-set fact-cosine floor — score the facts of matched docs
  with the existing raw cosine, open on a floor calibrated against the
  261-chunk restricted set. **Query vector under instr1 (rev 5, Opus r3
  M4):** rule (i) uses a SECOND embed of the query with the scheme
  prefix (`query_text_for_scheme`, as the v1 gate does at
  `tools/src/queries.rs:788`) — never a raw-query-vs-instr1-blob
  comparison (PR #270 showed those cells' floors differ: 0.70 vs 0.64).
  The extra embed is once per gated message, only when the gate is
  two-stage-active; its latency is recorded in the calibration artifact;
- (ii) chunk-score floor with chunk-level membership — open when the
  message's top chunk score clears a floor AND the matched chunk is in
  the hash-hop-hit set (chunk-level membership per v10 §3.1 — the 62
  hash-hop-hit chunks), then the gate opens on the facts the matched
  chunk's doc carries. (GLM r1 MAJOR-3: membership is a property of the
  CHUNK, not of the doc — the earlier wording was circular.
  **Opus r1 m3, intended scope:** the gate opens on ALL facts of the
  chunk's doc, not just facts whose evidence hashes point at that chunk
  — this follows Kurt's direction ("map the best-matching source FILES
  to the curated facts") and v10's doc→facts stage-2 hop; the audit
  line's FP therefore measures gate-OPEN decisions, not per-fact
  injection.)

The paired live calibration (acceptance, below) picks the rule and the
floor. **Floor storage + key shape (rev 5, Opus r3 m2):** two-stage
floors live in `WISDOM_GATE_FLOORS` (`src-tauri/src/wisdom_match.rs:53`)
under keys `embed_scheme::floor_key_for(model, scheme) + ":two-stage:" +
rule` (rule ∈ `restricted-cosine` | `chunk-hop`) — matching the existing
key shapes (raw keys carry no scheme suffix; instr1 appends `:instr1`).
**k is NOT a floor key** (rev 5, Opus r3 M3: `WISDOM_GATE_FLOORS` is
`&[(&str, f32)]` — an integer k would be matchable by `gate_floor`);
k lives in its own constant `TWO_STAGE_K: &[(&str, usize)]` beside the
floors, default 8.
**Bench coverage (rev 5, Opus r3 M3 — the rev-4 claim was wrong):
`src-tauri/tests/wisdom_gate_bench.rs` has hard-coded per-key tests
(`:22-44`) and the uncovered-key assertion runs only when a freeze dir
lacks `expected.json` (both existing fixture dirs are populated), so a
new two-stage key would pass every existing test with no snapshot.**
Required: a `replay_two_stage` test function + freeze directory for the
winning key, and a real per-key coverage assertion — every non-stub key
in `WISDOM_GATE_FLOORS` must map to a freeze dir that exists.
**Tie-break (GLM r1 minor-5):** if both rules pass the letter,
pick by higher hit@2, then lower FP, then rule (i) (simpler path).

**Empty closed set (GLM r1 minor-6):** with two-stage active and zero
eligible fact-bearing docs (fresh or emptied brain), the gate is closed
and the audit line is still emitted — the empty set is "below floor,"
not an error.

### 4. Floor-missing semantics — ONE rule (r9 M1)

For each (model, scheme) key: when the two-stage floor is missing but the
v1 floor exists → **v1 fallback** (v1 embed, v1 floors, v1 label — can
open). When BOTH floors are missing → v1's uncalibrated behavior (gate
`uncalibrated`, `entries = Vec::new()`, embed skipped). Embed-skip applies
only when BOTH keys are missing. (rev 4, Opus r2 m4: this is a code
edit at `tools/src/queries.rs:791` too — the CLI embed-skip
`wm::gate_floor(&scheme_key).is_none()` checks only the v1 key; it
gains the both-keys-missing condition so the rule is implemented on the
CLI side as well as the Tauri side.) Guard failure (absent/mismatch/mixed
model) ⇒ v1 fallback — the gate is never dark **while a v1 floor exists**.
**The fourth cell (Opus r1 m4):** two-stage floor PRESENT, v1 floor
missing ⇒ two-stage runs normally (it has its own floor); but a guard
FAILURE in that state falls back to a v1 that is itself uncalibrated —
gate `uncalibrated`, no open, embed skipped. This is the one state where
the fallback is dark-adjacent, and it is INTENTIONAL: an uncalibrated
fallback must not guess a floor to open on. Accepted because every
deployed brain has v1 floors from the PR-#270 calibration; the state
arrows only on a fresh/undercalibrated brain. Named test row below.
Below-floor stage 1 ⇒ closed (no fallback).

### 5. Two-stage active/off + labels

Two-stage activates on flip-to-default OR `--two-stage`; off-switches are
`CURATED_WISDOM_SINGLE_STAGE=1` and `--single-stage`. Active gate prints
the `semantic-v2-two-stage:{key}` stdout label. The pinned 5-field stderr
line stays byte-identical; the new `wisdom_two_stage_audit` stderr line is
separate (5 pinned fields) so the harness prefix-filter is untouched.

### 6. Model guard completion + purge completeness

Pass-id snapshot with `ct_reindex_pass_docs(pass_id, doc_id, embed_key)`
(+ migration, ungated idempotent DDL following the V23/V26 pattern).
A snapshotted doc not indexed under the new key (including superseded-job
`pending`) blocks completion; file-missing counts as
`model_guard_skipped_docs`. The `clear` transaction additionally deletes
the guard/pass/stamp meta keys, the breaker baseline/state keys, and all
`ct_reindex_pass_docs` rows (r9 m2 purge list).

### 7. Floors + acceptance letter

Floors are derived from the paired live calibration; **flip-to-default is
conditional on the acceptance letter passing in the same paired run**
(live `model_guard=ok` recorded).

**Acceptance letter — PINNED by Kurt (2026-10-09):** on the 150-probe
paired live set, same paired run for both arms — **two-stage passes if it
is not worse than single-stage**: hit@2 ≥ single-stage hit@2 AND
FP ≤ single-stage FP. No absolute performance floor (the earlier
PROPOSED ≥ 0.30 hit@2 bar is dropped). Passing flips the default;
absolute performance tuning is deferred until the issue backlog is
cleared (Kurt's call: throughput now, performance later).
**Baseline arm pinned (Opus r1 M3):** "single-stage" = the CURRENTLY
SHIPPED default configuration, named by its model/scheme key and floor
in the calibration artifact before the run — no post-hoc arm picking
(Cell A raw/0.70 and Cell E instr1/0.64 differ on both hit@2 and FP;
the comparison is against whichever is live at calibration time).
The calibration artifact also records that rule/floor/k selection and
scoring happen on the same 150-probe set (known limitation, accepted
under the not-worse-than-baseline bar).

### 8. Destructive-pass safety (pre-existing data-loss paths surfaced by the review ladder; fixed in this PR)

The ladder surfaced three PRE-EXISTING data-loss paths. All three fixes
are in scope:

- **(a) Force-rechunk brain-wipe → diff-swap.** Today
  `ingest_file_virtual` deletes a doc's chunks BEFORE the network embed
  (`pipeline/mod.rs:695`, `:748-749`): one mid-run 401 (we know the stale
  `.bashrc` key 401s) leaves the doc chunkless; scheduled heal + 7-day
  prune then mass-deletes its facts. Fix: diff-swap by `content_hash` —
  keep unchanged-hash chunks, re-embed changed ones in place (UPDATE of
  `embeddings`, never a bare INSERT — no unique constraint on `chunk_id`,
  duplicates would double-return in `semantic_search`), delete only
  removed chunks. Empty-hash rows are treated as REMOVED (the
  `idx_chunks_doc_hash` partial index can't match them).
  **Transaction boundaries pinned (Opus r1 m2; rev 5, Opus r3 M2):**
  `upsert_document` (`src-tauri/src/pipeline/mod.rs:708` — writes the
  new hash and `pending` status before the embed) moves INSIDE the swap
  transaction **for docs that already have a row**; for NEW docs the
  pre-embed `upsert` (pending status) STAYS where it is — there are no
  chunks to lose, and moving it would leave a failed embed with no
  `doc_id` for `mark_document_error` (`mod.rs:749`) and no row for the
  sweep's `pending` retry (`watchdog/sweep.rs:84-85`) to pick up.
  On embed failure the doc is `mark_document_error`ed — the
  embed-failure test asserts the expected post-failure doc status AND
  hash (for new docs: the row exists as pending/error), so a
  hash-matching rerun cannot short-circuit at the unchanged-hash
  check (`:692`).
  **Swap race (rev 5, Opus r3 m5):** because the existing-chunk-hash
  read happens pre-embed, OUTSIDE any transaction, a concurrent writer
  (GUI worker + `ct ingest`) can change hashes between the read and the
  swap — the swap transaction RE-READS the chunk hashes under its
  IMMEDIATE lock and ABORTS (clean error, job retryable) on any
  mismatch with the pre-embed read, rather than corrupting state or
  tripping the `(doc_id, content_hash)` UNIQUE constraint.
  Read/chunk/embed
  happen OUTSIDE any transaction; ONE short IMMEDIATE transaction does
  upsert + delete-removed + insert-new + mark indexed (no write lock
  across the network call; 5s busy timeout).
  **Scope narrowed (r9 M2):** the diff-swap is lossless where hashes are
  unchanged (same-chunker force-rechunk, model swaps). Position-shifting
  edits and chunker-version bumps rehash downstream chunks; those chunks
  are deleted+reinserted and `curated_relationships` cascades — this is
  TODAY's pre-existing behavior of the full swap, made explicit in the
  investigation's §4 limitations. A text-match remap (keep `chunk_id`,
  UPDATE position/`content_hash`, record old→new hash remap for evidence)
  is follow-up work and appears in the test list as a mid-document-
  insertion test only.
- **(b) Bulk-deletion circuit breaker (cross-process).** (rev 3, Opus
  r1 M1-M2: writers, transactions, denominators, and the file path are
  pinned precisely.) **Three heal writers, named:** scheduler and
  `ct heal` via `heal_invalid_sources_conn` (`src-tauri/src/db/heal.rs:46`,
  per-row IMMEDIATE transaction at `:80`) and the GUI **"Heal Database"
  button** via `heal_lost_librarian_inferred` (`src-tauri/src/lib.rs:2403`,
  breaker site `:2440`) — the button path is NOT the scheduler (rev 2's
  citation mislabeled it). **Transaction fix required on the button
  path:** `heal_lost_librarian_inferred` currently uses
  `unchecked_transaction()` (DEFERRED, `lib.rs:2444`) with an UPDATE
  lacking a `deleted_at IS NULL AND source_ref = ?` guard — it moves to
  `TransactionBehavior::Immediate` with the conditional UPDATE (matching
  `heal_invalid_sources_conn`), so the read-then-increment budget can
  neither race nor hit an upgrade `SQLITE_BUSY`; this also stops it
  double-counting already-soft-deleted rows. (rev 4, Opus r2 m1: the
  grounding check `source_ref_is_still_grounded` — currently run
  BEFORE the transaction at `lib.rs:2440` — moves INSIDE the IMMEDIATE
  transaction, as `heal.rs:85` already does; the function keeps its
  `&Connection` signature and uses
  `Transaction::new_unchecked(conn, TransactionBehavior::Immediate)`
  rather than rippling `&mut Connection` through its callers. Citation
  tightening, Opus r2 m5: the grounding check is at `:2440`, the
  soft-delete at `:2445-2448`.) **Regrade:** the file is
  `src-tauri/src/db/evidence_regrade.rs` (not `pipeline/`); its
  classification loop (`:173-200`) has no per-row write transaction —
  the breaker is therefore a **single pre-delete check** on
  `|doomed|` (the `unanchored=1 AND deleted_at IS NULL` population,
  `:160-165`) against the threshold. (rev 4, Opus r2 m2: the purge
  transaction itself is `unchecked_transaction()` at
  `evidence_regrade.rs:134`, DEFERRED — it moves to IMMEDIATE so the
  pre-delete count and budget read serialize against concurrent heal
  writers, consistent with the heal-writer fix above.)
  **Denominators pinned:** heal L = heal's own selection
  (`source_type='librarian_inferred' AND source_ref IS NOT NULL`); regrade
  L (rev 4, Opus r2 M2 — written as SQL, no "same as heal" ambiguity;
  different table than heal's): `SELECT COUNT(*) FROM librarian_evidence
  le JOIN llm_wiki_entries e ON e.id = le.entry_id WHERE e.deleted_at IS
  NULL`. Threshold **max(⌈0.05·L⌉, 10)** for all. Spent budget
  is kept in `llm_wiki_meta`, incremented inside each per-row IMMEDIATE
  transaction — scheduler, `ct heal`, GUI button share one budget, no
  TOCTOU across processes; **regrade's hard-deletes spend the SAME
  shared budget** (threshold computed with regrade's own L; spending
  against the one shared counter; rev 5, Opus r3 m1: each L is
  snapshotted under its OWN key — `breaker_L_heal` /
  `breaker_L_regrade` — and both keys are in the clear-transaction
  purge). **Regrade batch semantics (rev 5, Opus r3 m4):** the breaker
  is ALL-OR-NOTHING — `|doomed|` > remaining budget refuses the WHOLE
  batch (never a partial delete); `doomed` is RECOUNTED inside the
  IMMEDIATE purge transaction so the count the breaker judged is the
  count that would be deleted. **Budget window (rev 5, Opus r3 M5 — a
  24h auto-rollover only bounds TRANSIENT faults; for the persistent
  faults this breaker targets, rolling epochs with shrinking L would
  let each epoch spend a full budget and feed rows to the 7-day
  prune):** an epoch that TRIPS does not roll over — it stays tripped
  (spent stays at threshold) until an operator resolves the cause and
  resets the breaker keys explicitly. Healthy epochs roll over on the
  24h cadence (start ts in `llm_wiki_meta`, baseline L re-snapshotted
  at rollover inside the IMMEDIATE transaction). **Residual risk,
  stated honestly:** with nothing alerting, a tripped breaker means
  heal silently does nothing indefinitely — that is the safe failure
  direction (no deletions at all) versus mass deletion; the 7-day
  prune only touches rows soft-deleted BEFORE the trip, so a tripped
  epoch cannot feed it. Those keys (epoch-start ts, spent counter,
  `breaker_L_heal`, `breaker_L_regrade`) are the "breaker
  baseline/state keys" of the clear-transaction purge.
  Regrade's migration-context path is V20-gated
  and dead on the live brain (the covered paths are the manual
  `ct evidence regrade` command and pre-V20 replicas via
  `skipped_destructive=true`, which holds V21+ on an upgrading brain —
  the safe direction).
- **(c) Stamp enforced inside the funnel.** `ingest_file_virtual` itself
  refuses `force_rechunk=true` when the fingerprint stamp is
  stale. **Every** forced-rechunk path funnels through
  `ingest_file_virtual`, so one check gates every entry point (r9 M3):
  `ct ingest` (UNCONDITIONALLY forced — `tools/src/cmds.rs:172-178`
  hard-codes `force=true`; there is no `--force` flag in `Cmd::Ingest`),
  `tools/src/bin/bulk_reindex.rs:129`, `queue_full_reindex`
  (`src-tauri/src/lib.rs:2324`), and the automatic forced producers —
  the watchdog sweep re-enqueues `pending_reindex` as a *forced* rechunk
  (`src-tauri/src/pipeline/watchdog/sweep.rs:128-136` — the actual
  re-enqueue; `:73-82` is the status constant's doc comment) and
  `rechunk_for_reembed` is `force:true`
  (`src-tauri/src/pipeline/mod.rs:72-74`).
  **Refusal scope (rev 5, Opus r3 M1 — rev 4's "docs with chunks" scope
  still refused every EDITED file, since `ct ingest` is always forced
  and an edited indexed file has chunks):** the stamp check refuses only
  a **pure rechunk** — `force && the doc's stored hash equals the file's
  current hash` (nothing changed but we're about to rehash everything).
  A content edit produces a hash mismatch ⇒ it takes the normal
  diff-swap path, no stamp check involved. New docs (no row) ⇒ no
  check. So routine `ct ingest --yes` never blocks on the stamp, stale
  or missing — only a no-op rechunk of unchanged content does, and that
  is exactly the operation with nothing to gain and everything to lose.
  **Bootstrap (rev 5, Opus r3 B1 — the check now has a concrete
  pass/fail rule; rev 4's `verify-scratch` had none, making the override
  itself the bypass):** `ct reindex verify-scratch` rechunks a scratch
  copy of the live DB under the current binary + profile, then
  **writes the stamp ONLY IF every live `librarian_evidence` hash ref
  still resolves on the scratch rechunk (baseline: 406/406) AND every
  gate-eligible entry stays hop-resolvable (baseline: 365/365)**.
  Stated plainly: **a chunker-version bump FAILS this check** (hashes
  rehash → evidence refs orphan) until the text-match remap follow-up
  lands — the correct response to that refusal is to NOT rechunk, not
  to override. The refusal override remains "re-run verify-scratch",
  which now can only succeed if the rechunk is actually lossless.
  **Grant path (rev 4 — keyed, verified, expiring; Opus r2 M4):** a
  model-key grant record in `llm_wiki_meta`, **keyed to the target
  model key** and **invalidated when the stamp refreshes to that key**
  (a leftover A→B grant cannot authorize a later B→C swap). Writers:
  `bulk_reindex --model-swap` (CLI, the model-swap tool) and
  `run_wiki_reembed` (GUI) — both AFTER their scratch verification flow;
  **`ct ingest` is NOT a grant writer** (routine ingest, not a model
  swap). Model-key-only swaps need NO scratch check for correctness —
  the diff-swap keeps all `content_hash`es — the scratch check on these
  paths is belt-and-suspenders and identical on CLI and GUI. The funnel
  check in `ingest_file_virtual` reads: stamp fresh ⇒ pass; stamp stale
  only in the model-key component AND a grant for the target key
  present ⇒ pass (stamp refreshed as a side effect, grant consumed);
  chunker component stale ⇒ refuse.
  `PipelineJob` is NOT extended (no pass_id field — it would be dropped
  by the sweep anyway); grant + stamp live in the DB, so they survive
  sweep re-enqueue and channel-overflow deferral.
  **Refused-job disposition (rev 4, Opus r2 M3; rev 5, Opus r3 m3):** a
  refused forced job returns a DISTINCT outcome from `Ok(())` (so the
  worker at `pipeline/mod.rs:233-255` skips `generate_summary` and the
  linkers — an LLM call plus re-linking on a doc that didn't change),
  resets its `documents` row to its PRIOR status (`indexed` if that's
  what it was — never blindly `indexed`, which would overwrite a prior
  `error`; the pre-job status is captured before the attempt), logs one
  stderr line, and does NOT count as a strike: a refused
  `pending_reindex` row is never re-swept into a loop and never
  quarantined.
  Tests (rev 4/5 additions): post-upgrade live brain, stamp missing →
  `ct ingest --yes` succeeds for BOTH a new file AND an EDITED existing
  file (rev 5, Opus r3 M1); `ct reindex verify-scratch` on a chunker
  bump FAILS and writes no stamp (rev 5 B1: the check's pass/fail rule
  is the test); verify-scratch passes only when evidence refs + hop
  resolvability hold; `bulk_reindex --model-swap` → `model_guard=ok`
  reachable; refused `pending_reindex` row keeps its prior status, is
  not re-swept or quarantined, and the worker skips summary/linkers;
  chunker bump + reembed ⇒ refused; grant for key B does not authorize
  swap to C; new doc + embed failure ⇒ row exists as pending/error
  (rev 5, Opus r3 M2); swap aborts on concurrent hash change (rev 5
  m5); fresh-brain bypass requires zero rows EVER (rev 5 m6).
  **Fingerprint =
  (live-DB identity, chunker version, model key)** — the document-set
  hash is DROPPED (r9 M4: content changes are already lossless under
  the diff-swap; only chunker/model changes can mass-rehash). Scratch
  verification failing ⇒ the live rechunk refuses (override: re-run
  the scratch check; no `--allow-bulk` for rechunk — that override
  stays heal/regrade-only). **"Fresh brain" defined (rev 5, Opus r3
  m6):** a brain with NO rows EVER in `librarian_evidence` (not
  "zero live facts" — a brain emptied by a bug must NOT qualify and
  silently skip the guard); test: emptied-but-not-fresh brain does not
  bypass. `restore_in_progress` stamping remains [PROPOSED].

### 9. Implementation order (within the ONE PR)

1. Read path: stage-1 search + hop + decision rules + labels/fallbacks,
   opt-in (`--two-stage`).
2. Safety: diff-swap + funnel stamp + cross-process breaker + pass-doc
   storage + clear-transaction purge.
3. Paired live calibration = the acceptance gate; pick rule + floor;
   flip-to-default if the letter passes.
4. Provenance backfill only if the audit shows hop gaps (expected:
   unnecessary — 365/365 live).

## Error handling

- Missing two-stage floor → v1 fallback (never uncalibrated while a v1
  floor exists); both missing → v1 uncalibrated semantics.
- Two-stage floor present + v1 floor missing + guard failure →
  uncalibrated no-open (the sole dark-adjacent cell — intentional,
  see §4; fresh/undercalibrated brains only).
- Guard failure (absent/mismatch/mixed) → v1 fallback (never dark — ops
  crons are DECLINED; nothing else alerts).
- Below-floor stage 1 → gate closed, audit line still emitted.
- Rechunk scratch-verification failure → refuse the live rechunk.
- Breaker refusal → destructive pass aborts with
  `skipped_destructive=true` semantics (migration context: V21+ hold).

## Testing

- Stage-1 SQL: closed-set membership vs live-gate-eligible filter parity
  (incl. the >1024 excludes parity guard); deterministic row choice under
  `documents.hash` dedupe.
- Hop: 365/365 resolvability on the live brain as a fixture-backed test;
  3 non-librarian gate-eligible entries are unreachable under the closed
  set — named by live query before implementation (r8 m4), asserted here.
- Fallback matrix: two-stage-floor-only-missing / both-missing /
  guard-failure / below-floor / kill-switches — one test per branch of the
  §4 single rule; PLUS the fourth cell (Opus r1 m4): two-stage floor
  present + v1 floor missing + guard failure ⇒ uncalibrated no-open
  (asserted dark-adjacent BY DESIGN).
- Labels: pinned 5-field stderr line byte-identical under both paths;
  `wisdom_two_stage_audit` line shape.
- Diff-swap: unchanged-hash preservation; changed-hash in-place UPDATE
  (no duplicate embeddings — `semantic_search` returns the chunk once);
  empty-hash rows removed; mid-document-insertion behavior (documents the
  rehash cascade — follow-up remap is explicitly out of scope); embed-
  failure mid-run leaves prior chunks intact (the 401 scenario).
- Breaker: threshold math max(⌈0.05·L⌉,10) on both denominators;
  cross-process budget via `llm_wiki_meta` (two connections, shared
  refusal); ALL THREE heal writers covered (scheduler/`ct heal` via
  `heal_invalid_sources_conn`, GUI button via
  `heal_lost_librarian_inferred` — including its IMMEDIATE-transaction +
  conditional-UPDATE fix — and regrade's single pre-delete check on
  `|doomed|`).
- Stamp: missing/stale fingerprint refused inside `ingest_file_virtual`
  via each of the three manual callers PLUS the forced producers
  (watchdog sweep re-enqueue and `rechunk_for_reembed`);
  fresh-brain bypass; purge deletes guard/pass/stamp/breaker keys +
  pass-doc rows.
- Stamp grant path (rev 4 mechanism): stamp-missing brain →
  `ct reindex verify-scratch` bootstraps the stamp →
  `bulk_reindex --model-swap` writes the keyed grant → forced re-embed
  pass completes → `model_guard=ok` (the flip dependency must be
  reachable); GUI channel-overflow → sweep re-enqueue → passes on the
  DB-stored grant; chunker-version change + reembed ⇒ refused; grant
  for key B does not authorize swap to C; refused `pending_reindex`
  row is reset to `indexed` and not re-swept/quarantined; `ct ingest
  --yes` of a NEW file succeeds with no stamp present.
- Guard completion branches (GLM r1 MAJOR-4), one test each:
  snapshotted doc not indexed under the new key ⇒ completion BLOCKED;
  superseded-job `pending` ⇒ BLOCKED; file-missing ⇒ counted in
  `model_guard_skipped_docs`, completion proceeds.
- Acceptance: paired live calibration on the 150-probe real-traffic set,
  both arms same run, letter numbers as pinned by Kurt; flip-to-default
  lands in this PR only on a passing letter.

## Out of scope

- Text-match chunk remap (follow-up work, own test list entry).
- Provenance backfill unless the hop audit shows gaps.
- PR #270 re-litigation — ships as-is per Kurt's ruling.
- Pre-existing issues to file separately (handoff open item 4): bundle
  export drops `superseded_by`/`valid_to`; CWD-relative `exists()` skip in
  `bulk_reindex`/`queue_full_reindex`; reconcile CWD-dependence of
  duplicate path-shape rows; chunker-drift warning (any chunker-order
  change silently orphans ALL evidence hashes — `chunk_hash.rs:5-11`).

## Open questions for Kurt

*(None. Acceptance pinned 2026-10-09: pass = not worse than single-stage
in the same paired run; absolute performance deferred until the issue
backlog clears. Opus reset spent on this spec, not a v10 re-review —
investigation v10 stands as the design of record.)*

## Rulings carried

ONE PR total · ops crons BOTH DECLINED (no watchdogs; nothing alerts —
this is WHY the never-dark fallback is mandatory) · injection stays at the
curated layer · pinned 5-field stderr line untouched · PR #270 ships
as-is · curated-thoughts merges: regular merge commits only, no squash.
