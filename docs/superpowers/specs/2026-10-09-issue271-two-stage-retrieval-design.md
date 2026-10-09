# Two-stage retrieval for the wisdom gate: chunk-stage matching mapped to curated facts (issue #271)

**Date:** 2026-10-09 (rev 22 — Opus spec-tier r20 REQUEST CHANGES resolved:
M1 stage-1 exclusion made durable — per-doc `embed_key` written in the
swap tx, stage 1 requires embed_key = current key (the open-pass-row
`skipped` predicate stopped matching the moment the pass completed);
M2 claim-expiry split from dispatch — `sweepable_path_set` keeps
`retrying` rows regardless of attempts, only completed/failed/skipped
release claims (the attempts-filter dropped attempt-3 claims one tick
after dispatch, so the flip caught the row mid-flight a tick later);
M3 re-stage always sets `pending_reindex`, `pre_status` column carries
the restore memory (rev 21's "restores pre_status" self-contradicted
rev 19 and could hang a pass silently on a status='error' doc no sweep
query selects); m1 ONE swap IN-list stated (with `failed` included the
swap may overwrite a watchdog `failed` — sentence corrected); m2
`pre_status` pinned for EVERY snapshotted row + terminal-`failed`
writers named as the restorers; m3 refused-`pending_reindex` restore
routes through `pre_status` when an open-pass row exists; nits:
`orphaned` is a dead documents.status (synthetic-fixture note added),
§8(a) no-status-filter sentence updated for the embed_key exclusion)
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
  two-stage-active; its latency is recorded in the calibration artifact.
  **Embed-skip covers BOTH embeds (rev 6, Opus r4 m4):** the CLI
  embed-skip at `tools/src/queries.rs:791`, once it gains the
  both-keys-missing condition (§4), must skip the stage-1 raw embed
  when the TWO-STAGE floor is missing as well as the v1 embed when the
  v1 floor is missing — a v1 fallback then costs one embed, not two;
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
**Comment + replay updates (rev 7, Opus r5 MINOR-4):** the
`WISDOM_GATE_FLOORS` doc comment (`wisdom_match.rs:50-52`, currently
"Abstention floor on RAW cosine") is updated — a `chunk-hop` floor
abstains on a chunk score, not raw cosine; `replay_two_stage` passes
the composite key `floor_key_for(model, scheme) + ":two-stage:" +
rule` to `gate_floor` so the existing
`gate_floor(gate_key) == expected.floor` assertion
(`wisdom_gate_bench.rs:107-111`) holds unchanged.
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
CLI side as well as the Tauri side.) **Guard-failure "mixed" defined
(rev 7, Opus r5 MINOR-3):** the guard checks the STAMP's model key
against the profile's current key — mismatch ⇒ v1 fallback. Stated
plainly: an out-of-pass forced edit after a profile change (an edit
bypasses the stamp check by design and writes new-model vectors) is
what produces a genuinely mixed-vector doc; the guard's mismatch
fallback covers the GATE (it never reads with the wrong-scheme floor);
the mixed vectors themselves are inherent to no-model-stamping on
chunks (investigation §2) and are repaired by the next model-swap
pass, which re-embeds every chunk. Guard failure (absent/mismatch/mixed
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
`ct_reindex_pass_docs` rows (r9 m2 purge list). **ONE swap IN-list (rev 22, Opus r20 MINOR-1 — the rev-20 epoch-guard
sentence and the rev-21 §6 list stated the IN-list two different ways;
with `failed` included, the rev-20 "neither writer can overwrite the
other in either order" claim was false):** the swap tx's pass-docs
completion write is
`WHERE outcome IN ('snapshotted','retrying','failed')` — a non-sweep
ingest (watcher edit, `ct ingest` against the GUI brain) of a
failed/attempts<3 doc marks the row `completed` and never
costs a redundant sweep re-embed (r19 MINOR-1). Stated plainly: **the
swap MAY overwrite a watchdog-written `failed`** (the doc's vectors
really are new-model at that point — the correct outcome); the
conditionality still protects `completed`/`skipped` in both orders,
which is what the rev-20 race fix needed. The DDL also gains
`pre_status` (see snapshot staging below).
**Snapshot staging per status (rev 20, Opus r18 MAJOR-1; revised
rev 21, Opus r19 MAJOR-1 — rev 20's "error/orphaned ⇒ skipped"
RECREATED the mixed-vector state: a skipped doc keeps its model-A
chunks, stage 1 has no status filter so it scores them against
model-B queries, completion counts skipped as done ⇒ false
model_guard=ok; and the "natural retry later" is poisoned by the
unforced-edit rule, which would re-embed only changed chunks —
the unchanged A chunks stay forever):** `ct_reindex_pass_docs`
gains a **`pre_status`** column, **written for EVERY snapshotted row
(rev 22, Opus r20 MINOR-2 — rows "left as-is" had no stated value;
restoring from NULL would violate the CHECK constraint on
`documents.status`, `schema.rs:38-40`)**: at snapshot time
`pre_status = status` — no exceptions. At snapshot time — `indexed` ⇒
staged `pending_reindex`; `pending` ⇒ left as-is (already
sweepable); chunk-bearing `pending_reindex` (not quarantined) ⇒
left as-is; **`error` / `orphaned` with chunks ⇒ STAGED
`pending_reindex` like indexed docs** —
on refusal or exhausted
failure the doc's REAL status is restored from `pre_status` (the
r18 objection to staging them disappears once the row remembers
it). **The restoring writers are NAMED (rev 22, Opus r20 MINOR-2):
every writer that sets a row terminally `failed` with
`attempts = 3` performs the `pre_status` restore in the SAME
statement — the worker's third `Err` write, the sweep's
exhausted-flip, and the quarantine transaction.** Rows "left
as-is" get `pre_status` = their unchanged status; if such a row
is later flipped exhausted, the restore writes back the same
status — a no-op by construction, never NULL.
**Precedence pinned (rev 21, Opus r19 MINOR-3):** the
rev-19 quarantine rule wins — a chunk-bearing doc that is BOTH
`error` AND `quarantined_at`-set is snapshotted
`failed`/`attempts=3` (not skipped); **its re-stage always sets
doc status `pending_reindex` with `outcome='snapshotted'`,
`attempts=0` — `pre_status` is NOT written back to
`documents.status` at re-stage time; it is read only when the row
ends refused or exhausted (rev 22, Opus r20 MAJOR-3: rev 21's
"re-stage restores `pre_status`" contradicted the rev-19 re-stage
target — status `error` is selected by NO sweep query
(`sweep.rs:85`, `:105`) and shows in no failed counter, so the
pass would hang silently with the grant open and the gate on v1).**
**File-missing docs (snapshot-time `skipped`) stay skipped.
Stage-1 exclusion made DURABLE (rev 22, Opus r20 MAJOR-1 —
rev 21's "stage 1 EXCLUDES docs whose open-pass row is `skipped`"
held only while the pass was open, but §4/§8(c) keep the gate on
v1 fallback for the whole open pass, so stage 1 never ran then;
after completion the stamp refreshes to model B, the predicate
stops matching, and stage 1 scored the file-missing doc's model-A
chunks — the exact mixed-vector state rev 21 claimed to prevent;
a file deleted after the snapshot hits the same state via the
worker's `NotFound` ⇒ `skipped`):** the pass-docs DDL carries a
per-doc **`embed_key`**, written by the same writers that write the
swap tx's pass-docs completion rows (and set at snapshot for
pre-existing rows, keyed to the pass's target key); stage 1
requires **`embed_key = current profile model key`** — a doc
re-embedded (or snapshotted-verified) under model B passes; a doc
whose last pass row is `skipped`, or which has never completed a
swap under the current key, FAILS the predicate and is excluded.
The exclusion therefore outlasts the pass and clears on the doc's
next successful swap. Test (replaces the rev-20 test): "`error`
doc with chunks, A→B pass completes ⇒ stage 1 never scores an A
vector for that doc" — the test queries AFTER completion, not
during the pass.
**Snapshot membership PINNED (rev 19, Opus r17 MAJOR-1 — the spec
never said which docs are snapshotted; if the snapshot reuses
`list_indexed_user_doc_paths` (`db/queries.rs:38-41`,
`tier='user_doc' AND status='indexed'`), docs that are `pending`
(queue.rs pre-written a watcher edit) or `pending_reindex`
(quarantined from an earlier stall) are missed — they keep
model-A chunks, the pass completes, `model_guard=ok` fires, the
stamp refreshes to B, and stage 1 (no status filter) then compares
B queries against A vectors — the exact mixed-vector state the
guard exists to prevent, silently):** the snapshot is
**every document with at least one chunk** —
`WHERE EXISTS (SELECT 1 FROM chunks c WHERE c.doc_id = d.id)` —
NO status or tier filter. At snapshot time, chunk-bearing docs
with `quarantined_at` set get the rev-18 treatment immediately
(`outcome='failed'`, `attempts=3`) so they show in
"N failed — retry" instead of hiding. Test: quarantined
`pending_reindex` doc with chunks at snapshot time ⇒ completion
BLOCKED, shown as failed; after re-stage (clears quarantine +
strikes) ⇒ `model_guard=ok`.
**Snapshot membership — every doc with ≥1 chunk, `orphaned`
INCLUDED (rev 22, Opus r20 NIT: `orphaned` is a dead
`documents.status` — its only writer is the `wiki_pages`
updater at `src-tauri/src/db/okf_migration.rs:299`; no code
path sets a `documents` row to `orphaned`, so "error/orphaned
⇒ staged" is written for completeness with a synthetic
fixture only. The EXISTS predicate captures it if a fixture
ever produces one; nothing else in the mechanism treats it
differently from `error`.)**
**Pass-docs schema — PINNED DDL (rev 16, Opus r14 M2; made the sole
signature rev 18, Opus r16 MINOR-5; extended rev 22, Opus r20
MAJOR-1/MINOR-2):**
`ct_reindex_pass_docs(pass_id, doc_id, embed_key, outcome, attempts,
last_attempt_at, pre_status)` with `outcome ∈ {snapshotted, retrying,
completed, failed, skipped}`. Snapshot rows and outcome rows are THE
SAME TABLE (a row starts `snapshotted` and transitions). **Completion = every
row of the pass has `outcome ∈ {completed, skipped}`** — a `failed`
row BLOCKS completion (test: one `failed` row + everything else
completed ⇒ BLOCKED; the false-ok where a 401'd doc's failure row
satisfied the old "no row missing" check is impossible under this
definition).

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
  keep unchanged-hash chunks, and **every chunk is re-embedded on a
  forced pass** (rev 6, Opus r4 M2 — without this rule a model swap
  would be a no-op: no hash changes on a model swap, so "keep
  unchanged" would re-embed nothing while still marking the doc indexed
  under the new key, faking `model_guard=ok` over old-model vectors).
  The mechanics: `texts` at `src-tauri/src/pipeline/mod.rs:741` already
  covers every chunk; unchanged-hash chunks KEEP their `chunk_id` and
  their `embeddings` row is UPDATEd in place with the new-model vector
  (never a bare INSERT — no unique constraint on `chunk_id`,
  duplicates would double-return in `semantic_search`); changed-hash
  chunks are inserted (new `(doc_id, content_hash)` identity — an
  UPDATE cannot represent them); removed chunks are deleted. Because a
  forced pass re-embeds ALL chunks, mixed-model vectors cannot arise.
  An UNFORCED content edit may skip re-embedding its unchanged chunks
  only when the stamp's model key equals the profile's current key.
  Empty-hash rows are treated as REMOVED (the
  `idx_chunks_doc_hash` partial index can't match them).
  **Transaction boundaries pinned (Opus r1 m2; rev 5, Opus r3 M2):**
  `upsert_document` (`src-tauri/src/pipeline/mod.rs:708` — writes the
  new hash and `pending` status before the embed) moves INSIDE the swap
  transaction **for docs that already have a row**; for NEW docs the
  pre-embed `upsert` (pending status) STAYS where it is — there are no
  chunks to lose, and moving it would leave a failed embed with no
  `doc_id` for `mark_document_error` (`mod.rs:749`) and no row for the
  sweep's `pending` retry (`watchdog/sweep.rs:84-85`) to pick up.
  On embed failure: a NEW doc's row is `mark_document_error`ed (row
  exists as pending→error); an EXISTING doc keeps its prior state
  ENTIRELY — the swap transaction (which contains the upsert) never
  opened, so status/hash/chunks are all the old ones and nothing was
  lost; the failure is logged (rev 9, Opus r7 MINOR-1: the doc is NOT
  moved to `error`, which the sweep and every model-swap doc-lister
  skip — an indexed doc with intact chunks must stay reachable for
  the retry pass). The embed-failure test asserts the post-failure
  doc status AND hash for both cases, so a hash-matching rerun cannot
  short-circuit at the unchanged-hash
  check (`:692`). The stage-1 doc set adds NO `documents.status`
  filter beyond the shared eligibility predicate **plus the per-doc
  `embed_key` exclusion (rev 22, Opus r20 MAJOR-1/nit: stage 1
  requires the doc's current pass-docs `embed_key` to equal the
  active model key, so docs still on model-A vectors — file-missing
  `skipped`, never-swapped — are excluded; this is the ONE point
  where stage 1 intentionally differs from `gated_entries`, which
  has no vector-generation guarantee to enforce)** — facts stay live
  and gate-eligible regardless of their source doc's index status
  (a doc in `error` still has valid chunks until its next swap).
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
  shared budget** (rev 6, Opus r4 m3 — one shared spend counter, two
  thresholds: `remaining = own_threshold − shared_spent`, where
  own_threshold is computed from the writer's OWN L; the epoch is
  marked tripped when ANY writer refuses, and the tripping writer's
  refusal line names which threshold was hit); spending
  against the one shared counter; rev 5, Opus r3 m1: each L is
  snapshotted under its OWN key — `breaker_L_heal` /
  `breaker_L_regrade` — and both keys are in the clear-transaction
  purge). **Regrade batch semantics (rev 5, Opus r3 m4; rev 7, Opus r5
  MINOR-1):** `|doomed|` counts the rows the CLASSIFICATION flagged as
  doomed (the subset of `unanchored=1 AND deleted_at IS NULL` flagged
  rows that FAIL `evidence_has_live_chunk` at `:173-200` — the others
  are re-anchored, not deleted), not the raw flagged population. The
  breaker is ALL-OR-NOTHING — `|doomed|` > remaining budget refuses the
  WHOLE batch (never a partial delete); the in-transaction recount
  RE-RUNS the classification (it needs the per-row
  `evidence_has_live_chunk` verdicts, not a SQL COUNT), **compares
  against the pre-transaction `doomed` ID SET** (rev 8, Opus r6 m3 —
  the export exists only on the backed path,
  `evidence_regrade.rs:214-219`; the id set always exists and is a
  superset of the exported set on the backed path) and **ABORTS on
  any mismatch** rather than deleting a different
  set — matching the swap-race rule. **Re-anchor writes commit
  regardless of the breaker verdict (rev 9, Opus r7 MINOR-3):** the
  pre-transaction classification pass's re-anchor UPDATEs
  (`UPDATE librarian_evidence SET unanchored = 0`, `:175-178`) are
  already committed when the breaker decides — benign (re-anchoring
  is a recovery action), NOT rolled back; the in-transaction recount
  may perform further re-anchors (do not make it read-only — it must
  agree with what the purge would delete). **Budget window (rev 5, Opus r3 M5 — a
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
  epoch cannot feed it. **Reset surface (rev 6, Opus r4 M3):** the
  reset is a NEW flag, `ct heal --reset-breaker`, deleting the epoch
  timestamp, spent counter, and both `breaker_L_*` keys in one IMMEDIATE
  transaction. **`--allow-bulk` is also NEW** (rev 6, Opus r4 M3 — no
  such flag exists in the code today; a NEW flag on `ct heal` and
  `ct evidence regrade` that skips the breaker for one run, the only
  sanctioned way to exceed the budget when an operator has verified the
  deletion is intended). While tripped, heal and regrade print one
  stderr line EVERY run — in a no-alerts deployment that is the only
  trip signal. Those keys (epoch-start ts, spent counter,
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
  **Refusal scope (rev 5, Opus r3 M1; rev 7 r5 M1; rev 8 r6 M1;
  rev 10, Opus r8 B1 — the documents.hash comparison was BROKEN for
  watcher edits: `enqueue_vault_event` (`db/queue.rs:168-175`) writes
  the NEW file hash into `documents.hash` BEFORE ingest, so by the
  worker every watcher-staged edit matched its (old) chunks and looked
  like a pure rechunk — the edit would be silently refused and the
  gate would keep scoring stale chunks):** the stamp check compares
  against **`last_indexed_hash`** — a per-doc value written ONLY
  inside the swap transaction (option (a) of the review; a new column
  or `llm_wiki_meta` key). A **pure rechunk** = `last_indexed_hash ==
  the file's current hash AND the doc has ≥1 chunk`. A pure rechunk is
  refused whenever the stamp's CHUNKER component is not verified
  current, forced or not. A watcher/`queue.rs` edit has
  `last_indexed_hash` = the OLD hash ⇒ mismatch ⇒ normal diff-swap
  path — indexed with the new content, never refused. Unchanged files
  match and are refused (nothing to gain). Zero-chunk docs bypass
  (nothing to lose). Unforced edits also skip re-embedding unchanged
  chunks only when the stamp's model key is current (unchanged rule).
  **Backfill (rev 11, Opus r9 MAJOR-1 — without it every pre-upgrade
  doc has `last_indexed_hash = NULL`, nothing counts as a pure
  rechunk, and a bumped-chunker bulk pass would rehash all 291 live
  docs through the diff-swap path with verify-scratch never
  consulted — the stamp guard inert exactly where it matters):**
  `last_indexed_hash` is added by the new ungated idempotent
  migration, backfilled as
  `last_indexed_hash = hash WHERE status = 'indexed'` ONLY —
  `pending` rows may already carry queue.rs's pre-written NEW hash
  (backfilling them would recreate B1); `pending_reindex` rows were
  staged from `indexed` with unchanged bytes, so they ARE backfilled.
  `last_indexed_hash` is also written **inside
  `mark_document_indexed`** — which gains a hash parameter,
  `mark_document_indexed(conn, doc_id, indexed_hash)` (rev 13, Opus
  r11 m1: today it takes `(conn, doc_id)` only, `queries.rs:91-103`;
  writing `documents.hash` would inherit the existing race —
  `enqueue_vault_event` can overwrite the row with a newer hash
  between the upsert of H1 and the mark, recording H2 while holding
  H1's chunks, so the next forced run "pure-rechunks" and refuses the
  H2 edit forever). `indexed_hash` = the hash of the bytes actually
  chunked and embedded. (rev 12, Opus r10 m3: it
  is the only writer, which also fixes the empty-re-extraction branch:
  an existing doc re-extracting to empty runs the swap transaction
  with every chunk treated as removed — upsert + delete-all +
  `last_indexed_hash` + mark indexed — so the hash no longer stays
  stale and later runs hit the `:692` early return). **Epoch guard
  pinned (rev 19, Opus r17 MINOR-3 — without it a stage-stalled
  worker that later unwedges would open the IMMEDIATE swap tx,
  commit the swap, and write `completed`, overriding the
  watchdog's rev-18 `failed`/attempts=3 AND racing the replacement
  worker):** the heartbeat-epoch check happens IMMEDIATELY before
  `BEGIN IMMEDIATE` (last line of defense — the existing
  `hb.enter(Stage::Committing)` check at `:752-756` stays); the
  superseded case returns a DISTINCT `IngestOutcome` variant (not
  `Ok(())`). **The swap tx's pass-docs write is ALSO conditional —
  `WHERE outcome IN ('snapshotted','retrying')` (rev 20, Opus r18
  MINOR-1: the rev-19 point-in-time epoch check is check-then-act;
  the watchdog can bump the epoch during the 5s busy timeout inside
  `BEGIN IMMEDIATE` — with both writes conditional, neither writer
  can overwrite the other in either order).** Test: bump the epoch
  during Embedding ⇒ no pass-docs
  write, no chunk changes).
  Backfill wording
  is `WHERE status IN ('indexed','pending_reindex')` (rev 12, Opus r10
  m1 — the rev-11 "indexed ONLY ... pending_reindex ARE backfilled"
  pairing was self-contradictory; the WHERE clause is the rule).
  `error` and `orphaned` rows deliberately stay NULL: the guard is
  inert for them (treated as an edit) — accepted residual, noted in
  investigation §4.
  Test: post-upgrade fixture, stamp missing, forced rechunk of a
  pre-existing indexed doc ⇒ REFUSED.
  Test: existing doc re-extracts to empty ⇒ swap-tx delete-all +
  `last_indexed_hash` written + indexed (rev 12, Opus r10 m3).
  Test: watcher Modify event + stamp missing ⇒ doc ends up indexed
  with the new content.
  **Scope honesty (rev 6, Opus r4 m2):** under a stale CHUNKER stamp an
  EDITED file still rechunks with the new chunker (the refusal keys on
  hash equality, and an edit breaks equality) — rehashing that doc's
  chunks and orphaning its evidence refs. That is no worse than
  today's behavior, but the stamp's chunker-bump protection therefore
  covers UNCHANGED files only; investigation §4's limitation carries
  this note.
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
  **Grant path (rev 6 — NEW tooling marked as new; Opus r4 M4):** a
  model-key grant record in `llm_wiki_meta`, **keyed to the target
  model key** and **invalidated when the stamp refreshes to that key**
  (a leftover A→B grant cannot authorize a later B→C swap). Writers —
  both are NEW surfaces, listed in §9 step 2 as build items: **NEW
  `bulk_reindex --model-swap <key>`** (today's `bulk_reindex.rs:21-64`
  accepts only `--dry-run`/`--limit`/path filter; the flag's semantics:
  write the grant for the SUPPLIED target model key — the key is the
  flag argument, resolved against the profile's provider registry —
  then run the forced re-embed pass) and **GUI `run_wiki_reembed`
  gains a scratch-verification step that REUSES the `ct reindex
  verify-scratch` code path** (today it has none, `lib.rs:2762-2831`)
  — both write the grant only after verification succeeds.
 **`--model-swap <key>`/profile agreement (rev 13, Opus r11 m2):**
 grant time REFUSES unless `<key>` equals the profile's current
 model key; the swap transaction writes a pass-docs row only when
 the model it embedded with equals `target_key` — a completed pass
 under a different model can never record a false `model_guard=ok`
 (the guard's stamp-vs-profile fallback stays the safe backstop).
 **`ct ingest` is NOT a grant writer** (routine ingest, not a model
 swap). Model-key-only swaps need NO scratch check for correctness —
  the diff-swap keeps all `content_hash`es — the scratch check on these
  paths is belt-and-suspenders and identical on CLI and GUI.
  **Funnel decision table (rev 9, Opus r7 MAJOR-2 — the full table; the
  rev-8 three-case rule left "chunker current, model key stale, no
  grant" undefined):** stamp read = (chunker component, model-key
  component) vs current profile; FORCED and NON-FORCED paths behave
  identically:
  - chunker current, model current ⇒ **pass**.
  - chunker current, model stale, grant for the target key present ⇒
    **pass**. The stamp is NOT refreshed and the grant NOT consumed
    per job — both happen only when the pass COMPLETES with
    `model_guard=ok` (rev 10, Opus r8 M3: a mid-pass refresh would
    report model B while ~290 docs still carry model-A vectors — the
    §4 guard would then pass on wrong scores with no fallback, and
    the unforced-edit skip rule would go true mid-pass). Until
    completion, the gate guard treats an open grant or an in-flight
    pass as a model mismatch ⇒ v1 fallback. **Completion evaluator
    (rev 12, Opus r10 M1 — nothing in the code signals completion
    back to a supervisor, `sweep.rs:55-57`; without a pinned
    evaluator a GUI model swap would leave the grant open forever,
    the gate on v1 or uncalibrated, with no signal):** ONE
    idempotent `evaluate_pass_completion(conn)` is pinned, run (1)
    at the end of `bulk_reindex --model-swap` (synchronous), (2)
    inside every successful swap transaction (cheap check: no row of
    the open pass has `outcome NOT IN ('completed','skipped')` —
    rev 17, Opus r15 MINOR-3 wording; the snapshotted doc always has
    a row under the same-table model), and (3) from
    the sweep tick as backstop. The swap transaction reads the open
    `(pass_id, target_key)` from `llm_wiki_meta` — with no `pass_id`
    on `PipelineJob`, this DB read is how a pass-docs row gets
    written; stated explicitly. **Lost-job recovery (rev 13, Opus r11
    M1; redesigned rev 14, Opus r12 M1-M2 — rev 13's
    "mark-every-row-pending_reindex" needed the GUI sweep, which
    doesn't exist on the CLI-only deployment (the sweep runs only
    inside the Tauri watchdog, `watchdog/mod.rs:452-482`), and made
    the sweep double-dispatch GUI passes (sweep claims cover only
    sweep-sent paths, `sweep.rs:126-142`; forced passes re-embed ALL
    chunks, so a second dispatch = double embed spend)):**
    - **GUI paths (`run_wiki_reembed`, `queue_full_reindex` model
      swap):** stage every snapshotted row `pending_reindex` and DO
      NOT `try_send` — the sweep is the ONLY dispatcher for a model-
      swap pass (no double dispatch, no claims interplay). UI counts
      include `pending_reindex` in pending (or report pass progress
      from pass-docs), so the counts don't read ~0/0 mid-pass (r12
      m4; rev 15's "or" is RESOLVED — see the M3 wiring below: pass
      progress from pass-docs, `count_pending_documents` untouched).
      The r16 MINOR-1 note: pass rows are outside the drain input by
      status — no exclusion code is needed; the regression test stays
      as a guard.
      **Failed-job retry (rev 15, Opus r13 M1; mechanics pinned
      rev 16, Opus r14 M1-M3 — rev 15's claim-release created double
      dispatch (a `failed` marker released the claim while attempt 2
      was in flight, so any embed slower than the 60s
      `SWEEP_INTERVAL` was re-sent every tick) and left backoff /
      3-retries unenforced):** the pass-docs row carries
      `(outcome, attempts, last_attempt_at)`; outcomes ∈
      {snapshotted, retrying, completed, failed, skipped}. On embed
      failure the worker writes `failed`; when the sweep re-dispatches
      a failed row it flips the marker to non-terminal `retrying` in
      the SAME connection BEFORE `try_send` — only `completed`,
      `failed`, and `skipped` release claims, and a `retrying` row is
      claim-protected, so no double dispatch. `list_sweepable_pending` (ONLY
there) joins pass-docs and filters
`outcome='failed' AND attempts < 3 AND last_attempt_at + backoff
<= unixepoch()` — backoff and the 3-retry cap are enforced by
the query itself. **Claim expiry is split from dispatch (rev 22,
Opus r20 MAJOR-2 — `sweepable_path_set` previously "applied the
same filter", so a row the moment it went `retrying`/attempts=3
left the sweepable set; `retain_sweepable` (`sweep.rs:126`) then
dropped its claim while attempt 3 was still in flight, and the
rev-21 claims-filtered flip caught the row one tick later — any
attempt-3 embed slower than ~60–120 s still got a false failure,
the r19 scenario delayed by one tick; and if the user re-staged
on the false "N failed — retry", the doc was dispatched again
while attempt 3 ran: a double embed and two racing swaps):**
`sweepable_path_set` (the claim-expiry set) keeps `retrying`
rows REGARDLESS of `attempts` — only
`completed`/`failed`/`skipped` release a claim. The dispatch
filter (attempts-capped) and the claim-expiry filter are now
stated as two different predicates; the exhausted-flip remains
the only path that turns a claimed in-flight row `failed`. Test
(replaces the rev-21 flip test): an attempt-3 embed spanning
three sweep ticks is never flipped and never double-dispatched. **The exhausted-flip SKIPS
      in-flight rows (rev 21, Opus r19 MAJOR-2 — rev 20's UPDATE ran
      at every sweep start, but a row is `retrying`/attempts=3 from
      the moment attempt 3 is dispatched; any embed slower than the
      60s sweep interval got flipped to `failed` mid-flight, the
      swap's IN-list then matched 0 rows, and the doc showed a false
      "1 failed — retry" after SUCCEEDING):** at the start of
      `sweep()`, before `retain_sweepable`: select candidates
      (`outcome='retrying' AND attempts>=3 AND pass_id=:open`),
      FILTER THEM AGAINST `InFlightClaims` in Rust, then
      `UPDATE ... SET outcome='failed' WHERE id IN (...)` by id —
      claimed rows are never flipped; after a respawn `claims.clear()`
      makes every abandoned row eligible (the intended case), and the
      epoch guard already stops an abandoned worker from committing.
      (r19 nit: the original justification cited `&Connection` =
      read-only, which is wrong — `Connection::execute` takes `&self`;
      the real reason for an explicit statement is that the helpers
      are SELECTs and the flip is a write.) **Full sweep predicate (rev 17,
      Opus r15 M2 — rev 16's failed-only filter would have prevented
      passes from EVER starting (`snapshotted` rows excluded) and
      never re-dispatched a `retrying` row after a restart (claims
      cleared, `sweep.rs:48`), hanging the pass with the grant open):**
      the sweep queries LEFT JOIN pass-docs scoped to the OPEN
      `(pass_id, target_key)` (leftover rows from earlier passes never
      filter later sweeps) and dispatch a row iff:
      `pd.doc_id IS NULL OR pd.outcome IN ('snapshotted','retrying')
       OR (pd.outcome='failed' AND pd.attempts < 3
           AND pd.last_attempt_at + :backoff <= unixepoch())`
      — non-pass rows (`pd.doc_id IS NULL`) keep the sweep's existing
      backstop behavior unchanged. The `snapshotted`/`retrying` arms
      are ALSO attempts-capped: dispatch requires
      `pd.attempts < 3` on every arm (rev 19, Opus r17 MINOR-2 — a
      `retrying` row at `attempts=3` caught by a restart/respawn
      would otherwise be dispatched a 4th time, contradicting the
      3-total cap; a `retrying` row at `attempts=3` found by the
      sweep is treated as exhausted-failed: the query flips it to
      `failed`). `attempts += 1` ONLY after
      `try_send` returns `Ok` (a `QueueFull` does not burn an attempt
      — `sweep.rs:145-147` releases the claim and breaks); the
      worker's failure write sets `last_attempt_at` (backoff is
      measured from the FAILURE, not the dispatch). "3" means 3 TOTAL
      attempts (1 initial + 2 retries) — predicate `attempts < 3`
      matches. Backoff = named constant `PASS_RETRY_BACKOFF_SECS`
      (300). Test: restart with a `retrying` row ⇒ re-dispatched once
      ⇒ completes. After 3 auto-retries the UI
      pass-progress line shows "N failed — retry" with a re-stage
      action (re-staging resets attempts). This is the GUI twin of the
      CLI resume: no restart needed. Tests: slow retry spanning two
      ticks ⇒ ONE dispatch; GUI pass, one 401, key fixed, no restart
      ⇒ `model_guard=ok`; persistent 401 ⇒ "N failed — retry",
      auto-retries stop at 3.
    - **CLI path (`bulk_reindex --model-swap`):** synchronous — it
      cannot lose jobs, so it does NOT pre-stage. Its work list
      iterates the pass-docs snapshot table, never
      `list_indexed_user_doc_paths` (which filters
      `status='indexed'` and would go blind mid-pass — `queries.rs:
      38-41`; writers iterate the snapshot, never re-list by status,
      r12 m2). A re-run with the same key RESUMES the open pass:
      iterates snapshot rows lacking a completed row under
      `target_key`, prints how many remain, runs
      `evaluate_pass_completion` at the end.
    - File-missing docs are recorded as skipped AT SNAPSHOT TIME (a
      pass-docs row with a `skipped` marker), so
      `evaluate_pass_completion(conn)` decides from the database
      alone; **skipped rows are NOT staged `pending_reindex`** (rev
      15, Opus r13 m1 — staging them would leave a permanent
      `pending_reindex` row whose `fs::read` fails at
      `pipeline/mod.rs:687` before any status write, re-sent on every
      restart). Their stage-1 exclusion is carried by `embed_key`
      (rev 22, Opus r20 MAJOR-1), not by this row's `skipped`
      outcome — a `skipped` row never transitions, so the outcome
      alone would freeze the exclusion until the pass-docs rows are
      purged, and would never have applied had stage 1 run
      post-completion.
    Tests: CLI-only, one 401 mid-pass, re-run ⇒ `model_guard=ok`
    (r12 M1's scenario); GUI pass ⇒ exactly ONE embed call per doc
    (r12 M2); worker respawn mid-GUI-pass ⇒ completes; missing file
    - skipped-recorded ⇒ `model_guard=ok`.
    **M3 (r14) wiring:** leave
    `count_pending_documents` UNTOUCHED — it is the drain
    watchdog's canonical counter (`watchdog/mod.rs:288-291` feeds
    `DrainTracker::observe`; adding `pending_reindex` would trip
    `DrainStall` every window during a pass and set health to
    `Stalled` permanently — a false stall). Pass rows are outside
    the drain input BY STATUS — no exclusion code is needed or
    written; the regression test (exhausted-retry row does not trip
    a drain stall) stays as a guard only.
  - chunker current, model stale, NO grant ⇒ **refuse** — EXCEPT the
    sole sanctioned model-swap paths: `bulk_reindex --model-swap` and
    `run_wiki_reembed`, which write the grant first (after scratch
    verification). `queue_full_reindex(force=true)` — the code's own
    documented "embedding model changes" path (`lib.rs:2320-2322`) —
    is ADDED as a third grant writer, reusing the verify-scratch code
    path like `run_wiki_reembed`; without this the GUI's
    queue-full-reindex after a model change would refuse every job
    with nothing shown. With the grant present, its jobs pass like any
    other granted re-embed. **Its scratch verification runs ASYNC, not
    on the IPC thread** (rev 10, Opus r8 m2; rev 13, Opus r11 m3 — the
    verification does NOT go through the single pipeline channel, where
    it would block ingest for a full scratch re-embed: it runs on its
    own blocking task): `queue_full_reindex` enqueues the verification
    and returns immediately — its `Ok(usize)` return keeps today's
    meaning (docs queued for THIS reindex; the model-swap variant
    returns after staging), the verification task itself performs the
    scratch re-embed, writes the grant, and THEN the staging transaction
    marks all snapshotted rows `pending_reindex` (per the lost-job
    recovery above), from where the existing sweep drives the pass; the
    UI shows a "verifying scratch before re-embed" status line while it
    runs, and on a failed verification (e.g. a chunker-strategy change —
    `lib.rs:2320-2322` documents force=true for those too) a visible
    "refused: chunker changed — run `ct reindex verify-scratch`" state,
    not a pending counter that just drops to zero. **Variant trigger +
    return (rev 13, Opus r11 m3; r12 m3):** the model-swap variant runs
    when `force_rechunk == true` AND the stored stamp's model key
    differs from the profile's current key (a plain chunker change with
    matching model key is NOT a model swap and fails verification per
    the chunker rule). For this variant the command's `Ok(usize)`
    returns `0` (nothing queued yet — verification is async); progress
    is shown through the status line and pass-docs progress.
    **Quarantine interplay (rev 18, Opus r16 MAJOR-1 — a worker that
    HANGS mid-stage never reaches the Err/panic branches, so no
    `failed` marker is written; the watchdog replaces the worker and
    records a strike (`watchdog/mod.rs:354-452`),
    `QUARANTINE_THRESHOLD = 2` (`recovery.rs:10`), and both sweep
    queries filter `quarantined_at IS NULL` (`sweep.rs:85, :105`) —
    two hangs on one large doc = quarantined + pass row stuck
    `retrying` = pass incomplete forever, grant open, gate on v1, no
    signal):** when the watchdog quarantines a path that has a row
    in the open pass, it writes `outcome='failed'` with
    `attempts = 3` (exhausted) — the doc then appears in
    "N failed — retry". **Conditional write (rev 20, Opus r18
    MAJOR-2 — a Summarizing/Linking stall AFTER the swap committed
    would otherwise overwrite a `completed` row with `failed`,
    blocking completion despite every chunk carrying new-model
    vectors, and re-staging would re-embed + re-run the same
    stalling summary — a loop):** the pass-docs UPDATE is
    conditional — `... AND outcome IN ('snapshotted','retrying',
    'failed')` — it never touches `completed`/`skipped`. (The
    rev-16 "optionally gated on stall stage ≤ Committing" is
    DELETED — optional behavior doesn't belong in a spec, and the
    IN-list makes it redundant, r19 nit.) Test: Summarizing
    stalls twice on a completed pass doc ⇒ row stays `completed`,
    pass completes. **Atomicity + keying (rev 19, Opus r17
    MINOR-1 — a crash between `quarantine()` and the pass-docs
    write would leave the doc quarantined with its row stuck
    `retrying`, the exact hang rev 18 closes):** the quarantine
    UPDATE and the pass-docs `failed`/`attempts=3` UPDATE run in
    ONE transaction; `doc_id` resolved via
    `documents.path = ?` + the open `(pass_id, target_key)` read
    from `llm_wiki_meta` (the watchdog only knows the path).
    **Re-stage target pinned:** `outcome='snapshotted'`,
    `attempts=0`, doc status `pending_reindex` (leaving `failed`
    would make retry timing depend on a stale `last_attempt_at`).
    The re-stage action ALSO clears
    `quarantined_at` and the strikes for that path (otherwise
    re-staging could never succeed). Test: two stage-stall respawns
    on one pass doc ⇒ row shown as failed ⇒ re-stage recovers it ⇒
    pass completes.
  - chunker stale or missing (any model state) ⇒ **refuse** (the
    bootstrap/override is `ct reindex verify-scratch`).
  Zero-chunk docs bypass the whole table (nothing to lose); content
  edits bypass it (diff-swap). **Ordering (rev 11, Opus r9 MINOR-3):
  the table is evaluated AFTER the existing non-forced short-circuit**
  (`pipeline/mod.rs:691-694` — unchanged + `indexed` + non-forced ⇒
  `Ok(())` as today, so `wisdom_deposit` kicks of unchanged docs
  return early and never touch the stamp check, no refusal-record
  churn on the normal path).
  `PipelineJob` is NOT extended (no
  pass_id field — it would be dropped
  by the sweep anyway); grant + stamp live in the DB, so they survive
  sweep re-enqueue and channel-overflow deferral.
  **Refused-job disposition (rev 4, Opus r2 M3; rev 5-6, Opus r3 m3 /
  r4 M1):** a refused forced job returns a DISTINCT outcome from
  `Ok(())` (so the worker at `pipeline/mod.rs:233-255` skips
  `generate_summary` and the linkers — an LLM call plus re-linking on a
  doc that didn't change) and writes the doc's status back by rule:
  **a refused doc with an open-pass row restores from the row's
  `pre_status` (rev 22, Opus r20 MINOR-3 — the old blanket
  "`pending_reindex` maps to `indexed`" was safe only when staging
  came from `indexed`; rev 21 stages `error`/`orphaned` docs
  `pending_reindex` too); a refused doc with NO open-pass row and
  pre-job status `pending_reindex` maps to `indexed`** (the staging
  guard `WHERE path = ?1 AND status = 'indexed'` at
  `src-tauri/src/lib.rs:2815-2821` proves the pre-staging status was
  `indexed` — that staging path only ever fires on `indexed` rows),
  **any other pre-job status is restored as it was** (never
  blindly `indexed` — that would overwrite a prior `error`). One stderr
  stderr line; NOT counted as a strike; never quarantined. This kills the
  re-sweep loop: the row no longer reads `pending_reindex`, so
  `list_sweepable_pending` cannot pick it up again.
  **Disposition for refused rows (rev 4, Opus r2 M3; rev 5-6 r3 m3 /
  r4 M1; rev 9-11, Opus r7-r9 — rev 9's pre_staging_status markers
  were replaced in rev 10 by a per-doc refusal record; rev 11 pins
  the record's LIFETIME after r9 MAJOR-2 showed it could outlive its
  cause and block a later real edit):** a refused row records a
  **refusal record keyed to `(doc_id, documents.hash at refusal, stamp
  fingerprint)`** — the hash leg is compared against
  `documents.hash` (which `queue.rs:172` updates on edits; the sweep
  has no fresh file hash to compare), so a later content change
  breaks the match ⇒ the next sweep dispatches the edit normally (no
  silent loss); a stamp refresh changes the fingerprint ⇒ exemption
  lifted (pending rows that need a retry resume after bootstrap).
  **Storage (rev 12, Opus r10 m2):** a NEW TABLE in the ungated
  migration (not per-doc meta keys — both sweep queries need it as a
  join filter). **Fingerprint = the stamp stored in `llm_wiki_meta`**
  (the sweep has only `conn`, no profile), compared NULL-safely
  (`IS`/`'<absent>'` sentinel) — a refusal recorded while the stamp
  is missing must still suppress re-dispatch until the fingerprint
  changes. The record is DELETED inside the swap transaction (job
  succeeded)
  and in `delete_document`, and is in the clear-transaction purge
  list — no leaks (the r8 M1-M2 criticism, now fully answered). The
  filter is applied in BOTH `list_sweepable_pending` and
  `sweepable_path_set` (claims must expire in step). Status handling
  on refusal: open-pass row ⇒ restore from its `pre_status`
  (rev 22, Opus r20 MINOR-3 — the staging guard only proves
  pre-staging state for NON-pass staging; pass-staged `error`/
  `orphaned` docs must come back as `error`/`orphaned`); no row +
  `pending_reindex` ⇒ `indexed` (staging guard proves pre-staging
  state), ANY other status restores as it
  was (never blindly `indexed` — that would hide error/orphaned
  state, per r7 M1's queue.rs/connection.rs/okf_migration writers).
  No loop forms within a process even without the record
  (`InFlightClaims.retain_sweepable`, `sweep.rs:62-64`); the record
  covers worker respawn/restart.
  **Pending counter: NO change needed (rev 11, Opus r9 MINOR-1 — the
  rev-10 decrement idea would DOUBLE-decrement: the post-job
  decrement at `pipeline/mod.rs:271-294` already runs for every
  counted job regardless of outcome, even after a panic). Test: the
  counter returns to baseline after a refused job.**
   **Citation correction (rev 10, Opus r8 M4):** the
   `connection.rs:248-257` tier_working re-pend is inside
   `if version < 5` (`connection.rs:235-239`) — **pre-V5 only, dead
   on V27**; rev 9's "LIVE on V27" claim was wrong. The r7-M1
   conclusion (status string is not a safe restore key) still holds
   via `queue.rs:175` and `okf_migration.rs:308`; the tier_working
   test is labeled a pre-V5 fixture case. **Reachability note (rev
   10, Opus r8 m3):** `okf_migration.rs:308` runs only in the V7
   one-shot conversion (`run_okf_migration`, skipped once complete) —
   not reachable on a V27 brain post-migration; listed for
   restore-path completeness only.
  **`ct ingest` loop accounting (rev 6, Opus r4 m1):** the refusal
  outcome is treated as a SKIP in the per-file loop at
  `tools/src/cmds.rs:180` — no `failed += 1`, no non-zero exit, and the
  file still counts in the linker's entity set (an unchanged file has
  valid chunks; a refused rechunk changes nothing).
  **UI counts during a pass (rev 15, Opus r13 m2; corrected rev 17,
  Opus r15 M1 — this paragraph previously said count_pending_documents
  "gains pending_reindex", which contradicted rev 16's M3 and would
  false-trip DrainStall every window; also cited the wrong file):
  `get_indexing_status` (`src-tauri/src/lib.rs:2312-2318`) reports
  pass progress FROM THE PASS-DOCS TABLE (completed/skipped/failed
  counts + total). `count_pending_documents` (`src-tauri/src/db/
  queries.rs:168-174`) is UNTOUCHED — it is the drain watchdog's
  canonical counter; `count_indexed_documents` is also untouched (its
  dip is real — those docs are mid-pass). Pass rows are outside the
  drain input BY STATUS (`pending_reindex` is not in the counter's
  WHERE clause and never will be — r15 m1: the rev-16 "watchdog
  excludes exhausted rows" clause is dropped as a no-op; the
  regression test stays as a guard).
  Test: mid-pass `get_indexing_status` shows non-zero progress.
  **Two open passes (rev 15, Opus r13 m4):** a second grant/snapshot
  is REFUSED while a pass is open, unless it targets the SAME key —
  same-key re-entry resumes (as `bulk_reindex` already does); the
  refusal names the open pass. Test: second pass, different key ⇒
  refused naming the open pass; second pass, same key ⇒ resumes.
  **`bulk_reindex --model-swap` loop semantics (rev 9, Opus r7
  MINOR-2):** per-doc embed failure is COUNTED, the loop CONTINUES
  (remaining docs still re-embed — one 401 on doc k of 291 must not
  strand docs k+1.. on the old model), and the process exits non-zero.
  Test: one embed failure mid-pass ⇒ completion BLOCKED, remaining
  docs re-embedded, non-zero exit.
  Tests (rev 4/5 additions): post-upgrade live brain, stamp missing →
  `ct ingest --yes` succeeds for BOTH a new file AND an EDITED existing
  file (rev 5, Opus r3 M1); `ct reindex verify-scratch` on a chunker
  bump FAILS and writes no stamp (rev 5 B1: the check's pass/fail rule
  is the test); verify-scratch passes only when evidence refs + hop
  resolvability hold; `bulk_reindex --model-swap` → `model_guard=ok`
  reachable; refused `pending_reindex` row maps back to `indexed` (per
  the staging-guard rule) and does not reappear on the next sweep,
  and the worker skips summary/linkers;
  chunker bump + reembed ⇒ refused; grant for key B does not authorize
  swap to C; new doc + embed failure ⇒ row exists as pending/error
  (rev 5, Opus r3 M2); swap aborts on concurrent hash change (rev 5
  m5); fresh-brain bypass requires `librarian_evidence_seen` ABSENT
  (rev 8, Opus r6 m5 — reworded from the row-count phrasing); refused
  `pending_reindex` row does NOT reappear on the next `sweep()` call
  (rev 6, Opus r4 M1 — seeds the row and asserts); unchanged file in a
  folder ingest is skipped without failing the run and still counts in
  the linker's entity set (rev 6, Opus r4 m1); after a model-swap pass
  EVERY `embeddings` row of the doc carries the new-model vector
  (rev 6, Opus r4 M2); breaker trip → stderr line on every heal/regrade
  run → `ct heal --reset-breaker` clears it → heal proceeds (rev 6,
  Opus r4 M3); `--allow-bulk` bypasses the breaker for one run (rev 6
 M3); v1-fallback gate costs exactly one embed (rev 6, Opus r4 m4);
 new file, embed fails, key fixed, `ct ingest --yes` with no stamp ⇒
 doc ENDS UP INDEXED (rev 7, Opus r5 MAJOR-1 — the error-row retry
 must not be refused); bug-emptied brain (marker present, zero live
 facts) does not bypass the stamp guard (rev 7 MAJOR-2); regrade
 recount mismatch aborts the purge (rev 7 MINOR-1); out-of-pass edit
 after a profile change ⇒ gate falls back to v1 (mixed vectors, rev 7
 MINOR-3); GUI non-forced overflow → chunker bump → sweep ⇒ NO chunk
 hash changes (rev 8, Opus r6 M1); pre-V18 fixture opens cleanly with
 the trigger migration (rev 8, Opus r6 m4); chunker current + model
 stale + NO grant ⇒ REFUSED — the table's empty cell (rev 9, Opus r7
 MAJOR-2), and `grant for B` does not authorize a B→C swap in the
 same cell; pure-rechunk embed failure on an EXISTING doc ⇒ status
 stays `indexed`, old chunks intact (rev 9, Opus r7 MINOR-1); refused
 `pending` row does not re-dispatch after worker restart while its
 refusal record matches, and dispatches normally once its hash or
 the stamp changes (rev 10-12 refusal-record design, supersedes the
 rev-9 marker wording); bulk_reindex
 --model-swap: one embed failure mid-pass ⇒ BLOCKED + rest re-embed +
 non-zero exit (rev 9 MINOR-2); regrade breaker refusal leaves
 re-anchor writes committed (rev 9 MINOR-3); watcher Modify event +
 stamp missing ⇒ doc indexed with the NEW content (rev 10, Opus r8
 B1 — the queue.rs hash pre-write case); refused row's exemption
 lifts automatically after a fresh stamp bootstrap (rev 10, Opus r8
 M1); gate guard falls back to v1 while a grant is open or a pass
 is in flight (rev 10, Opus r8 M3).
  **Fingerprint =
  (live-DB identity, chunker version, model key)** — the document-set
  hash is DROPPED (r9 M4: content changes are already lossless under
  the diff-swap; only chunker/model changes can mass-rehash). Scratch
  verification failing ⇒ the live rechunk refuses (override: re-run
  the scratch check; no `--allow-bulk` for rechunk — that override
  stays heal/regrade-only). **"Fresh brain" defined (rev 5 Opus r3 m6;
  rev 7 Opus r5 MAJOR-2 — a COUNT(*)=0 check cannot implement "no rows
  EVER": `librarian_evidence` has no high-water mark
  (`schema.rs:393-399`, `entry_id TEXT PRIMARY KEY`, no AUTOINCREMENT)
  and rows leave via regrade/prune/clear hard deletes):** a persistent
  marker `librarian_evidence_seen` in `llm_wiki_meta`, set by an
  `AFTER INSERT` trigger on `librarian_evidence` and backfilled by the
  new ungated migration for any brain that has rows today.
  **The marker is explicitly KEPT by the clear transaction** (NOT in
  the purge list): a cleared brain has CARRIED librarian facts — the
  guard stays armed; "fresh" means the marker is absent, which after
  clear requires deleting the brain file.
  **Trigger placement (rev 8, Opus r6 m4):** the `librarian_evidence_
  seen` trigger + backfill are part of the NEW ungated migration and
  run AFTER the V18 DDL (SQLite rejects `CREATE TRIGGER … ON
  librarian_evidence` if the table doesn't exist yet — a pre-V18
  replica/bundle upgrade would break); the trigger body uses
  `INSERT OR IGNORE INTO llm_wiki_meta` (idempotent). Test: opening a
  pre-V18 fixture succeeds and leaves the marker armed on any brain
  that has rows. `restore_in_progress` stamping remains [PROPOSED].

### 9. Implementation order (within the ONE PR)

1. Read path: stage-1 search + hop + decision rules + labels/fallbacks,
   opt-in (`--two-stage`).
2. Safety: diff-swap + funnel stamp + cross-process breaker + pass-doc
   storage + clear-transaction purge. **NEW tooling in this step (rev 6,
   Opus r4 M4; rev 7 Opus r5 MINOR-2):** `ct reindex verify-scratch`
   (bootstrap command), `bulk_reindex --model-swap <key>` (grant
   writer), the GUI reembed scratch step (reuses verify-scratch's code
   `ct heal --reset-breaker`, the NEW `--allow-bulk` flags on
   `ct heal` / `ct evidence regrade`, the `librarian_evidence_seen`
   marker (trigger + migration backfill), the `ct_reindex_pass_docs`
   outcome/attempts DDL + refusal-record table (both in the ungated
   migration), a named **`IngestOutcome`**
   enum** returned by the ingestion funnel so every caller can branch on
   refusal — refusal-as-skip handling is added at the **pipeline
   worker** (`src-tauri/src/pipeline/mod.rs:231`, the
   `match ingest_file(..)` branch point that executes
   `queue_full_reindex`, sweep, and `rechunk_for_reembed` jobs — those
   only `try_send` and never see the outcome), **`bulk_reindex`**
   (synchronous), and `ct ingest`. **Frontend work is IN scope
   (rev 16, Opus r14 MINOR-1; sites corrected rev 17, Opus r15 m2):**
   `IndexingStatus` (`lib.rs:2305-2309`; consumers `StatusBar.tsx:63`
   AND `ReviewMode.tsx:84`; mirrored in `src/lib/tauri.ts:156`;
   DEFAULT STATE in `useIndexingStatus.ts:7`) gains pass-
   progress fields — the new fields are OPTIONAL in the TypeScript
   type, so existing mocks stay valid; the seven files with
   `{indexed, pending}` mocks (`StatusBar.test.tsx` ×2,
   `EntityList.test.tsx`, `BrainMode.test.tsx` ×3,
   `AppShell.dragdrop.test.tsx`, `ReviewMode.test.tsx` ×2,
   `test-setup.ts`) need NO changes (that is the point of optional
   fields); pass-progress UI added in `StatusBar.tsx` only. §9 step 2
   includes the TS type update. **Pass-docs `failed`/`skipped` markers
   (MINOR-2, refined rev 17, Opus r15 m3):** written at the WORKER's
   `Err` branch (`mod.rs:257`) and panic path (`:266`) — GATED on
   "the open pass has a row for this doc" (the worker reads the open
   `(pass_id, target_key)` from `llm_wiki_meta` exactly as the swap
   transaction does; an ordinary forced job writes no marker). The
   panic path runs OUTSIDE the `catch_unwind` closure on a FRESH
   short-lived connection WITH ITS OWN `busy_timeout` — reason
   corrected per r16 MINOR-4: the worker's `Connection` is a local
   owned value (`pipeline/mod.rs:175`), not behind a `Mutex`, so
   there is no lock to poison; unwinding already rolls back any open
   transaction. The fresh connection is for cleanliness (the old one
   may be mid-statement), not necessity. `fs::read` (`:687`), `extract_text`
   (`:703`), and post-snapshot deletion all surface as worker `Err`s
   and are covered; `NotFound` ⇒ `skipped` so deletion completes the
   pass. MINOR-3: `db/queries.rs` (`src-tauri/src/db/queries.rs`
   — mark_document_indexed `:91-103`, list_indexed_user_doc_paths
   `:38-41`, count queries `:160-174`) is distinct from
   `tools/src/queries.rs` (embed-skip `:791`, query_text_for_scheme
   `:788`); full paths used throughout.
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
  **superseded job on an EXISTING doc: status stays `indexed`, no
  pass-docs row under the new key ⇒ BLOCKED** (rev 8, Opus r6 m2 —
  under rev 5's swap-tx upsert an existing-doc superseded return no
  longer leaves `pending`, so the test is written against that
  observable; a separate new-doc case covers `pending`) — SPLIT into
  GUI and CLI cases (rev 15, Opus r13 m3 — the rev-14 single bullet
  self-contradicted): **GUI** rows are `pending_reindex` from
  snapshot; assert BLOCKED with NO sweep tick, then "sweep tick +
  worker drain ⇒ pass completes" (one `sweep()` call only `try_send`s
  — completion needs the worker to drain and the swap tx to evaluate).
  **CLI** rows stay `indexed` (bulk_reindex is synchronous, no
  staging) and assert BLOCKED before the run's own final
  `evaluate_pass_completion`. file-missing ⇒ counted in
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
