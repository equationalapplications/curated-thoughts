# Two-stage retrieval for the wisdom gate: chunk-stage matching mapped to curated facts (issue #271)

**Date:** 2026-10-10 (rev 30 — Opus r28 REQUEST CHANGES resolved: stage-1 candidate SQL re-pinned to real columns (M1); non-forced no-op narrowed to `pending` + key-currency (M2); pre-V28 RO column-probe fallback (M3); m1–m8 folded). Full per-round changelog: **Revision history** block below.
**Status:** Draft
**Branch:** `spec/issue271-two-stage-retrieval`
**Priority:** High (merge-blocker-1 successor for PR #270; closes #265 when live matching works)
**Issue:** equationalapplications/curated-thoughts#271
**Investigation (Step 0):**
`docs/superpowers/specs/2026-10-09-issue271-two-stage-investigation.md` (v10).
**Supremacy (r24 M1):** where investigation v10 disagrees with THIS spec, this
spec is canonical. v10's §3.6 3-column `ct_reindex_pass_docs` signature is
superseded twice over — the table itself is deleted under Option A (M4), so
§9 step 1's read path no longer depends on pass-docs storage at all.

## Revision history (compressed)

- **r1–r8 (revs 1–10):** closed-set stage 1 designed; decision rules (i)/(ii);
  floor keys + TWO_STAGE_K; the ONE fallback rule (r9 M1); diff-swap
  transaction boundaries pinned; breaker writers/denominators/thresholds
  pinned; funnel refusal keyed to `last_indexed_hash` (rev 10, r8 B1);
  refusal-record design (rev 10-12).
- **r9–r16 (revs 11–18):** verify-scratch bootstrap + stamp; grant path
  (`bulk_reindex --model-swap`, GUI scratch step); pass-docs DDL pinned
  (r14 M2); lost-job recovery GUI/CLI split (r13/r14 M1-M2); failure retry +
  backoff; `pre_status` snapshot staging (r18-r19).
- **r17–r23 (revs 19–25):** snapshot membership pinned (r17 M1); CANONICAL
  5-arm dispatch predicate + claim-expiry split + `dispatch_hash`
  (r22-r23); per-doc `documents.embed_key` column (r21 B1, r22 M2/M3);
  sampled bootstrap backfill + stamp-model ≥90% gate (r23 M3 — the
  model component and gate are DELETED at r25-M3); rule-(ii)
  doc-level open (r1 m3, carried).
- **r24 → rev 26:** M1–M5 + m1–m8 resolved as dated in the header; Option A
  deletes the pass machinery and installs the coverage check (§6).
- **r25 → rev 27:** M1 — repair path for candidate docs no
  existing tool can reach: `ct reindex repair-embed-key --stale` re-embeds
  stored chunks from the DB, no file read (§6); M2 — refusal of a forced job
  on a `pending_reindex` row RESETS it to `indexed` (§8(c)); M3 — stamp is
  CHUNKER-ONLY, the model component + ≥90% rule are DELETED (§6, §8(c));
  M4 — rule (ii) pinned to no fact-level floor (§3); m1 audit fields
  `open`/`top` (§6); m2 COALESCE + check-before-embed + both-sides (§6);
  m3 migration V28 on the V24 pattern (§9); m4 `Err(IngestRefused)`
  signaling (§8(c)); m5 model-swap scope note (§4); 8 experiments recorded
  under EXPERIMENTS REQUESTED (r25).
- **r26 → rev 28:** M1 — never-dark restored by construction:
  source-less facts (`user_stated`/`user_confirmed`, no
  `curated_proposal_sources` hop) are scored via the v1 path IN the same
  call and merged into the two-stage list (§3); fix (a) empty-set fallback
  REJECTED; audit line restructured ONCE to seven fields, superseding the
  six-field pin (§6); M2 — coverage population = candidate docs with ≥1
  chunk (§6); M3 — non-forced pure rechunk = no-op success; refusal reset
  extended to BOTH statuses (§8(c)); M4 — `IngestRefused` downcast pinned
  at all FOUR callers (§8(c)); m1–m7 folded in (§6, §9 step 2, §3, §9
  step 1, §8(c), experiments); 5 experiments recorded under EXPERIMENTS
  REQUESTED (r26).
- **r27 → rev 29:** M1 — v1-merge population redefined as the
  STRUCTURAL COMPLEMENT of the stage-1-reachable set (anti-join, not
  provenance; four gap classes named; §3); M2 — funnel decision table
  rewritten as four rows on (forced?, chunker current?) (§8(c)); M3 —
  refusal reset + no-op restore `hash=last_indexed_hash`, reset pinned
  inside `ingest_file_virtual` (§8(c)); m1–m6 folded (header audit-field
  order fixed; kick caller unreachable; §10 reset predicate widened +
  hash; mixed-open audit semantics pinned; step-reference fix); 5
  experiments recorded under EXPERIMENTS REQUESTED (r27).
- **r28 → rev 30 (this rev):** M1 — stage-1 candidate SQL re-pinned to
  REAL columns: the evidence hash hop (`librarian_evidence` evidence_json
  `content_hash` → `chunks` → `doc_id`) ∪ the proposal chain
  (`curated_proposal_sources.doc_id`); the old
  `curated_proposal_sources.source_hash` column does not exist —
  `documents.hash` demoted to dedupe/row-identity only; gap class (3)
  restated (evidence-hash leg dangles, proposal chain cannot — CASCADE);
  r27-M3's hash restore kept as hygiene, membership argument withdrawn;
  M2 — non-forced no-op narrowed to `pending` rows only (`pending_reindex`
  keeps status+hash so the sweep's forced job still runs — the model-swap
  silent-drop hole) + key-currency condition (stale key ⇒ re-embed stored
  chunks in place); M3 — pre-V28/RO brains: `documents` column probe ⇒
  missing column = coverage 0 ⇒ v1 fallback, never an error; m1 coverage
  population harmonized to ≥1 stage-1-scorable chunk; m2 merge vector
  scheme-pinned (prefixed, floor per scheme, bit-parity with
  `wisdom_match_in_scheme`); m3 audit `top` = the evaluated rule's own
  decision variable; m4 `rule=none` narrowed to the coverage-below
  sub-case of the fourth cell; m5 stale "three-row" reference fixed;
  m6 verify-scratch live write guarded per-doc; m7 one-shot rule extended
  to the `last_indexed_hash` backfill; m8 header changelog collapsed;
  5 experiments recorded under EXPERIMENTS REQUESTED (r28).

## 1. Problem

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
token (`commit.rs:482-487`), not a broken pointer.

**Critical-path correction (r24 M1):** rev 25's claim that "no DDL and no
backfill are on the critical path for the read fix" is STALE and is
withdrawn. Since the per-doc `documents.embed_key` exclusion exists, the
read path DEPENDS on: (1) the `documents` DDL that adds `embed_key` and
`last_indexed_hash` (§9 step 1 carries it), (2) the verify-scratch bootstrap
that writes the stamp and performs the sampled `embed_key` backfill, and
(3) the coverage check reading `documents.embed_key`. §9 states the
dependencies exactly.

## 2. Approach

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
  watchdogs, nothing alerts. This is precisely why the coverage/fallback
  design below is mandatory (never-dark).
- **Model-swap grant/pass machinery (rev 6–rev 25; r24 M4)** — REJECTED
  under Kurt's Option A ruling (2026-10-10): per-doc `embed_key` plus a
  query-time coverage check answers "may the gate read two-stage?" directly
  from the data, without grant records, a pass-docs table, a completion
  evaluator, or pass-aware sweep dispatch — machinery that produced new
  MAJOR findings in each of the last ~12 review rounds. A model swap is
  just the existing forced re-embed tools (§6).

## 3. Design

The design of record is investigation v10 §3 (1)–(9) **as amended by this
spec** (supremacy statement, header); v10 §1 carries the
controller/Opus-verified code citations and live-brain measurements
underlying every choice below. Summary of the load-bearing points:

### 1. Stage 1 — closed-set chunk search (NEW SQL)

Top-k chunk search restricted to fact-bearing documents. The candidate doc
set is the UNION of two real edges, pinned by actual columns (r28-M1 — the
rev-26/28 phrasing "`documents.hash` matching
`curated_proposal_sources.source_hash`" joined a column that DOES NOT
EXIST: `curated_proposal_sources` is `(proposal_id, doc_id, role)` with
`doc_id REFERENCES documents(id) ON DELETE CASCADE` (`db/okf_ddl.rs:197-202`),
and `source_hash` exists only on `llm_wiki_entries` where it is NULL on
365/365 live rows — investigation v10 line 9):
1. the EVIDENCE HASH HOP: `librarian_evidence.evidence_json` entries'
   `content_hash` → `chunks.content_hash` → `chunks.doc_id` (the
   resolution `evidence_has_live_chunk` performs at `db/commit.rs:666-690`;
   v10's "406/406 hash refs live" counts these refs);
2. the PROPOSAL CHAIN: `librarian_evidence.proposal_id` →
   `curated_proposal_sources.doc_id` (the reverse join
   `wisdom_deposit.rs:183-193` already uses).
The union is deduped by `documents.hash` with a deterministic row choice
(the dedupe only picks the ROW IDENTITY — `documents.hash` is not a
membership predicate; it pre-writes via `enqueue_vault_event` and is
maintained by the diff-swap), filtered to live gate-eligible facts. On
the live brain this is 261 chunks / 47 docs under
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
  M4; r26 m3):** rule (i) uses the scheme-prefixed query embed
  (`query_text_for_scheme`, as the v1 gate does at
  `tools/src/queries.rs:788`) — never a raw-query-vs-instr1-blob
  comparison (PR #270 showed those cells' floors differ: 0.70 vs 0.64).
  **Embed count under each scheme (r26 m3, pinned):** when the read
  scheme is `raw`, `query_text_for_scheme` returns the query UNCHANGED
  (`src-tauri/src/embed_scheme.rs:149-150`), so the "second" embed would
  be an identical network call — the stage-1 raw query vector IS the
  fact-vector embed and is REUSED; raw does ONE embed per gated message.
  Only `instr1` embeds twice (raw stage-1 embed + prefixed fact-vector
  embed). The embed-skip paragraph below covers both; the per-message
  embed count and latency are recorded in the calibration artifact.
  **Embed-skip covers BOTH embeds (rev 6, Opus r4 m4; r25-M4 extends to
  rule (ii)):** the CLI
  embed-skip at `tools/src/queries.rs:791`, once it gains the
  both-keys-missing condition (§4), must skip the stage-1 raw embed
  when the TWO-STAGE floor is missing as well as the v1 embed when the
  v1 floor is missing — a v1 fallback then costs one embed, not two.
  **r25-M4 (pinned):** rule (ii) ALSO needs the scheme-prefixed
  fact-vector embed (raw: the same single embed, reused per r26 m3) —
  its entry scores are the rule-(i) restricted-set fact cosine —
  so this embed-skip paragraph covers rule (ii) exactly as it covers
  rule (i);
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
  **Rule-(ii) fact selection — NO fact-level floor (r25-M4, pinned;
  r26 m2 implementation pin):**
  the chunk-score floor is the ONLY abstention test under rule (ii); no
  per-fact floor is applied to the entries the opened doc carries.
  **r26 m2: rule (ii) therefore BYPASSES the `raw < floor` test at
  `wisdom_match.rs:379-381`** — the implementer CANNOT reuse
  `gated_entries` as-is for the rule-(ii) entry selection, because that
  loop drops `raw < floor` entries before the sort/truncate, which would
  silently apply the v1 floor and contradict r25-M4. The selection passes
  NO floor into the loop (e.g. a `floor = None` parameter) so the
  floor-drop branch is skipped while the weighted-cosine sort and
  truncate-to-`max` are reused.
  The
  facts are ranked by WEIGHTED fact cosine — the same weighted-cosine
  path `gated_entries` already uses (`wisdom_match.rs:391-400`), which
  requires the scheme-prefixed query embed (the raw cosine in
  that path compares a prefixed query vector against scheme-prefixed
  fact blobs; raw reuses the single stage-1 embed, r26 m3) — and
  truncated to `max`. The chunk score selects WHICH
  docs open; it is never reported as an entry score
  (`WisdomItem.score` is documented "Raw cosine for entries",
  `wisdom_match.rs:74-75`, and corrections carry `null`). Ordering and
  capping are `gated_entries`'s
  existing sort-by-weighted-cosine then truncate-to-`max`
  (`wisdom_match.rs:391-400`) — unchanged, minus the `:379-381`
  floor-drop (r26 m2). Injection volume per open is
  recorded in the calibration artifact (r24 E7).

**Source-less facts — the v1-merge (r26-M1, fix (b), PINNED; population
redefined STRUCTURALLY r27-M1):** the
candidate set is built only through the `curated_proposal_sources →
documents` hop, so gate-eligible entries with NO proposal-source edge —
today: the `user_stated`/`user_confirmed` provenances
(`wisdom_match.rs:44-48`;
3 such entries on the live brain) — are carried by NO candidate doc and
would never be scored under two-stage. An all-source-less brain would then
be permanently dark (empty set ⇒ "below floor" on every message) while v1
would have opened — a violation of the never-dark invariant. Fix, pinned:
whenever the gate runs two-stage, the gate-eligible facts in the
**STRUCTURAL COMPLEMENT of the stage-1-reachable set** are ALSO scored via
the v1 path in the SAME call — that is, gate-eligible entries NOT reachable
through any candidate doc that has ≥1 stage-1-scorable chunk (≥1 chunk
outside the skip-set classes). This complement is computed inside the same
stage-1 query (an anti-join over the candidate-chunk set), NOT by
provenance: provenance naming (`user_stated`/`user_confirmed`) is an
EXAMPLE of the complement, not its definition — four gap classes fall
between a provenance-defined merge and the true complement (r27-M1):
(1) zero-chunk docs' facts are sourced but unscored; (2) docs whose chunks
are ALL in skip-set classes; (3) evidence hash refs pointing at a
REMOVED OR REHASHED chunk with no proposal-chain edge (`content_hash`
matches nothing live — the CASCADE on `curated_proposal_sources.doc_id`
(`db/okf_ddl.rs:198-200`) means a deleted doc takes its edges with it, so
the proposal chain cannot dangle; the evidence-hash leg can — r28-M1
corrects this class); (4) other provenance
values — `immutable_document` is in `PROVENANCE_VOCAB`
(`wisdom_match.rs:47`) and `source_type` can be NULL — either kind can
lack an edge. A re-extraction to empty (§8(c)'s swap deletes all chunks,
facts stay live until heal) produces class (1) TODAY: a provenance merge
leaves those facts dark on a 100%-coverage brain. The complement's facts
are scored via the v1 path — raw cosine of the message
against `llm_wiki_entries.embedding_blob` (`gated_entries`' path, floor =
the current key's v1 floor). **The merge vector is SCHEME-PINNED
(r28-m2):** it is the SCHEME-PREFIXED query vector — the same vector
rule (i) scores facts with — NOT the raw stage-1 vector; under `instr1`
the blobs are prefixed, and scoring raw-against-prefixed would be the
cross-scheme comparison rev 3's M4 forbids. The floor is
`gate_floor(floor_key_for(key, scheme))` (the same floor rule (i)
resolves). Parity: for a complement fact, the merge's score must equal
`wisdom_match_in_scheme`'s score for the same fact, bit-identical (§10,
r28 E5). The results are MERGED with the
two-stage opens into ONE ranked list (weighted-cosine ordering, then
truncate to `max`). Never-dark is therefore restored BY CONSTRUCTION: the
gate can never be closed by an empty-or-dark candidate set while a v1
floor exists, and a `user_stated` fact can open on a mixed brain. Reviewer
fix (a) — an empty candidate set falls back to v1 wholesale — is
REJECTED: it is strictly weaker, abandoning two-stage entirely on
no-librarian brains instead of merging. Test rows: an all-`user_stated`
brain stays openable; a `user_stated` fact opens on a mixed brain; a
zero-chunk doc's facts open via the merge; an all-skip-class doc's facts
open via the merge; §10's unreachable-entries assertion asserts the
COMPLEMENT SIZE, not a provenance count (r27-M1).

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
the composite key `floor_key_for(model, scheme) + ":two-stage:" + rule`
to `gate_floor` so the existing
`gate_floor(gate_key) == expected.floor` assertion
(`wisdom_gate_bench.rs:107-111`) holds unchanged.
**Tie-break (GLM r1 minor-5):** if both rules pass the letter,
pick by higher hit@2, then lower FP, then rule (i) (simpler path).

**Empty closed set (GLM r1 minor-6; r26-M1 amends the consequence):**
with two-stage active and zero eligible fact-bearing docs (fresh or
emptied brain), the candidate set is empty and the gate is CLOSED only if
the v1-merge also yields nothing — with a v1 floor present the §3
v1-merge still scores source-less facts, so an empty candidate set no
longer implies a closed gate; when the merge too yields nothing (no
eligible entries at all), the gate is closed and the audit line is still
emitted — the empty set is "below floor," not an error. (Coverage over an
empty candidate set is vacuously
complete; the floor-missing rules of §4 still govern.)

## 4. Floor-missing semantics — ONE rule (r9 M1)

For each (model, scheme) key: when the two-stage floor is missing but the
v1 floor exists → **v1 fallback** (v1 embed, v1 floors, v1 label — can
open). When BOTH floors are missing → v1's uncalibrated behavior (gate
`uncalibrated`, `entries = Vec::new()`, embed skipped). Embed-skip applies
only when BOTH keys are missing. (rev 4, Opus r2 m4: this is a code
edit at `tools/src/queries.rs:791` too — the CLI embed-skip
`wm::gate_floor(&scheme_key).is_none()` checks only the v1 key; it
gains the both-keys-missing condition so the rule is implemented on the
CLI side as well as the Tauri side.)
**The fourth cell (Opus r1 m4):** two-stage floor PRESENT, v1 floor
missing ⇒ two-stage runs normally (it has its own floor). This is the one
state where a subsequent fallback is dark-adjacent, and it is INTENTIONAL:
an uncalibrated fallback must not guess a floor to open on. Accepted
because every deployed brain has v1 floors from the PR-#270 calibration;
the state arises only on a fresh/undercalibrated brain. Named test row in
§10.
Below-floor stage 1 ⇒ closed (no fallback).

**THE GUARD (Option A) — the same one rule governs vector staleness
(r24 M2 resolution):** before running two-stage, the gate computes
**coverage** — the fraction of the stage-1 CANDIDATE doc set (the ~47-doc
fact-bearing closed set, after the hash dedupe and the shared eligibility
predicate) whose `documents.embed_key` equals the current profile model
key. **Coverage below the pinned threshold (§6) ⇒ v1 fallback — the exact
same one-rule fallback as a missing floor, never dark while a v1 floor
exists.** This replaces BOTH of rev 25's guards: the stamp-model ≥90%
ratio (which was measured over ~291 chunk-bearing docs at stamp time —
the wrong population, r24 M2) and the grant/in-flight-pass gate
(deleted with the machinery, r24 M4). The verify-scratch sampled
backfill (r25-M1: from stored chunk text, regardless of file
existence or status) remains as the BOOTSTRAP that makes coverage
non-degenerate on day one; the coverage check is what enforces it on
every query thereafter. (r25-M3: the stamp-model ≥90% ratio itself no
longer exists — the stamp is chunker-only, §6.)

**Mixed-vector risk under Option A, stated plainly:** an out-of-pass
forced edit after a profile change writes new-model vectors for one doc
while others still carry old-model vectors. Under Option A this is
handled per-doc, not globally: the diff-swap re-embeds ALL of a doc's
chunks in one transaction (§8(a)), so a doc's vectors are single-model by
construction; the stage-1 `embed_key` predicate (§6) then simply EXCLUDES
any doc whose key is stale — the gate never scores a stale doc's vectors
against the wrong-model floor. A profile flip with no swap run at all
lowers coverage below the threshold ⇒ v1 fallback until the next forced
re-embed repairs it. No grant, no pass, no completion evaluator is needed
(r24 M4 reasoning, adopted).

**Scope note on model swaps (r25 m5, informational):** the two-stage
floors are per model key (`WISDOM_GATE_FLOORS`,
`wisdom_match.rs:53-67`, one entry per `<model-key>[:scheme]`), so
swapping to an UNCALIBRATED model means BOTH the two-stage floor and the
v1 floor are missing and the gate is uncalibrated regardless of coverage
— §4's both-keys-missing cell governs. (On such a swap the v1 fallback
also scores old-model fact blobs: a dimension mismatch is skipped at
`wisdom_match.rs:375`, and a same-dimension mismatch silently
cross-scores — exactly the staleness the per-doc `embed_key` predicate
excludes from the candidate set.) Coverage therefore matters at
BOOTSTRAP and for swaps between two CALIBRATED keys; this note is
explicitly recorded to preempt re-adding swap machinery.

## 5. Two-stage active/off + labels

Two-stage activates on flip-to-default OR `--two-stage`; off-switches are
`CURATED_WISDOM_SINGLE_STAGE=1` and `--single-stage`. Active gate prints
the `semantic-v2-two-stage:{key}` stdout label. The pinned 5-field stderr
line stays byte-identical; the new `wisdom_two_stage_audit` stderr line is
separate from it so the harness prefix-filter is untouched. The audit
line's FULL field list is pinned once, at §6 (r24 m1).

## 6. Model swap + query-time coverage check (Option A core)

**The column (KEPT, r21 B1):** `documents` gains `embed_key TEXT NULL` —
the model key whose vectors the doc's current chunks carry. NULL or stale
⇒ the doc's chunks carry old-model (or unverified) vectors and the doc is
excluded from stage 1 until repaired.

**Key shape PINNED (r24 m2):** `embed_key = gate_model_key(profile, stub)`
(`wisdom_match.rs:107-124`) — the RAW key with NO scheme suffix — on BOTH
the write side and the read side. The stub argument is the live
`CURATED_EMBED_STUB` value: stub-backed tests and stub-backed dev runs
write/read `stub:constant8` consistently, so stub fixtures never see an
empty stage 1 from a key mismatch. Scheme-suffixed keys
(`gate_model_key_for_scheme`) are never written to or compared against
`embed_key`.

**Writers:** the diff-swap transaction (§8(a)) sets `embed_key` to the
model key ACTUALLY used to embed, on EVERY full re-embed of the doc —
forced or unforced-full. Ordinary (non-swap) ingests write it too: the
ingest path knows its profile key, and a fresh ingest's chunks are by
definition current-key vectors. A doc whose embed FAILED keeps its prior
value — NULL stays NULL, stale stays stale (the swap transaction that
would have updated it never opened, §8(a)). Docs are never NULLed.

**Bootstrap backfill (KEPT; r22 M3, r23 m2; extended r25-M1):** `verify-scratch` samples
each chunk-bearing doc — first AND last chunk (one sample cannot catch a
doc with mixed-model vectors; under the diff-swap those cannot arise
within a doc, but the two-chunk sample costs nothing and keeps the
backfill honest) — re-embeds, and matches by **cosine ≥ 1−1e-3** (pinned
tolerance; exact float equality is over-brittle across embedder runs).
Backfill `embed_key` ONLY on docs whose samples match. The backfill
re-embeds from the STORED CHUNK TEXT in the DB — **file existence and
document status are irrelevant to it (r25-M1: the reviewer confirmed the
sample already embeds from DB text; this is now pinned explicitly, so a
missing source file or an `error`/`pending_reindex` status never blocks a
backfill)**. A sampled re-embed
that FAILS (e.g. a 401) leaves the doc NULL, is PRINTED in the
verify-scratch output per doc (artifact evidence), and is reflected in
the doc's absence from coverage — it counts as uncovered until repaired
(r25-M3: the ratio is evidence, not a gate). Scale: two re-embeds
per doc, one-time, ~291 live docs.

**The stamp is CHUNKER-ONLY (r25-M3):** verify-scratch writes the stamp
iff the chunker fingerprint it computes on the scratch rechunk is
current; the stamp has NO model component and there is NO ≥90% rule.
**Scratch/live split (r26 m6, pinned; per-doc guarded write r28-m6):**
verify-scratch rechunks on the
SCRATCH copy (the copy the comparison runs against), but the two writes —
the `embed_key` backfill and the chunker stamp — go to the LIVE DB; a
sample verdict is computed on scratch and applied live, never the reverse.
**The live write is GUARDED (r28-m6):** the sample verdict is computed on
scratch after minutes of embedding, so the live apply must not trust it
blindly — each doc's write is ONE IMMEDIATE transaction that re-checks
the doc's chunk-id set and current `embed_key` against the scratch
snapshot (the §8(a) swap-race rule) and SKIPS the doc on mismatch, same
rule as `repair-embed-key`.
Rev 26's model component and threshold are DELETED: under Option A
nothing read them (the funnel refuses on the CHUNKER component only,
§8(c), and the coverage SQL reads only `documents.embed_key`), so the
model component had no reader and one rev-26 test asserted a false
invariant. What does the work instead: the per-doc sampled backfill
(above) and the query-time coverage check (below), both of which
operate per doc. The backfill match ratio is still computed and
PRINTED by verify-scratch as artifact evidence; it gates no write.

**THE COVERAGE CHECK (Option A; r24 M2/M4 resolution; population r26-M2)
— replaces the grant/in-flight-pass guard of rev 25 §6 as the primary
mechanism:**

At gate time (whenever two-stage would run), the gate computes over the
stage-1 CANDIDATE doc set — **defined (r26-M2, pinned; population
harmonized with the complement r28-m1) as candidate docs
with AT LEAST ONE STAGE-1-SCORABLE CHUNK, i.e. ≥1 chunk outside the
skip-set classes** (hash-hop deduped, gate-eligible, BEFORE the
`embed_key` exclusion) — the SAME population the §3 v1-merge complement
is computed against, so an all-skip-class doc with a stale key can
neither force a pointless v1 fallback (it has no scorable vectors) nor
sit outside the coverage count while its facts sit outside the merge:

```sql
SELECT COUNT(*),
       COALESCE(SUM(CASE WHEN d.embed_key = :profile_key THEN 1 ELSE 0 END), 0)
  FROM <candidate-doc-set query>;
```

a cheap two-aggregate query over ~47 rows. **Pinned threshold:
coverage must be 100% — every candidate doc must carry the current
profile key — or the gate falls back to v1 (§4's one rule).**
**Zero-chunk candidate docs (r26-M2, pinned):** a doc with ZERO chunks
contributes no stage-1 vectors, so its `embed_key` is irrelevant to the
gate's scoring and it is NOT part of the coverage population — coverage
is computed over candidate docs with ≥1 chunk (the definition above),
never over all candidate docs. A vacuous `embed_key` write for zero-chunk
docs was considered and REJECTED (it writes a value nothing reads and
implies vectors the doc does not have). A census experiment
(`EXPERIMENTS REQUESTED (r26)`, E1) verifies the live brain has 0
zero-chunk candidate docs.
**Edge cases pinned (r25 m2):** `SUM` over an empty set is NULL, so the
covered count is pinned to `COALESCE(…, 0)` and COUNT 0 = the candidate
set is EMPTY = vacuously covered (the empty-closed-set rule of §3).
The check runs BEFORE any embed — a v1 fallback must never pay for a
wasted stage-1 raw embed (this keeps the one-embed-on-fallback promise,
§3's embed-skip paragraph, which by r25-M4 covers both rules). The check
is implemented on BOTH sides: the CLI (`tools/src/queries.rs:786-806`)
and the Tauri side, so the two implementations cannot disagree about
falling back. **Pre-V28 / read-only brains (r28-M3, pinned):** `open_ro`
→ `open_brain_readonly` (`retrieval/mod.rs:156-165`) is a bare
`SQLITE_OPEN_READ_ONLY` open with NO migration — a brain that has not
had a read-write open since the binary was upgraded has no
`documents.embed_key` (and possibly no `last_indexed_hash`) column, and
an unguarded coverage query would fail with "no such column", error the
whole `ct wisdom match` call (exit 1 on every message ⇒ dark, breaking
never-dark). Therefore BOTH sides pin a `documents` COLUMN PROBE first
(the `ddl_compat::existing_columns` pattern, `db/ddl_compat.rs:128`, the
same probe style `wisdom_match.rs:162-186` uses for V24/V27 skew): a
missing `embed_key` or `last_indexed_hash` column counts as COVERAGE 0
⇒ the §4 v1 fallback, with `stale_embed_key=<candidate count>` on the
audit line. The gate never errors on schema skew; it degrades to v1.

Why 100% and not a lower fraction: (1) the set is tiny (~47 docs), so
the check is cheap at any threshold and strictness costs nothing; (2)
the acceptance letter's paired run must measure the REAL candidate set —
a 90%-coverage two-stage arm silently drops 4–5 fact-bearing docs
(r24 M2's exact failure scenario), and "not worse" on a skewed subset
is exactly the artifact M2 forbids; (3) under Option A there is no
scheduler driving coverage up — no pass, no grant — so the only forces
that raise coverage are forced re-embeds (user-initiated) and ordinary
ingests. A sub-100% threshold would let the gate run two-stage forever
on a partially-stale set in a deployment where nobody re-embeds; 100%
makes "stale docs exist ⇒ v1" the invariant, which is also trivially
testable and trivially explainable. The failure direction is safe:
coverage below 100% ⇒ v1 fallback, which can still open (never dark
while a v1 floor exists).

**Repair paths (r25-M1 revised):** a model swap = the
EXISTING forced re-embed tools, unchanged: `ct ingest` (unconditionally
forced, `tools/src/cmds.rs:172-178`), `bulk_reindex`
(`tools/src/bin/bulk_reindex.rs:128-130`, forced
`ingest_document_with_vault_root`), `queue_full_reindex(force_rechunk:
true)` (`lib.rs:2320-2324`), GUI reembed (`run_wiki_reembed`,
`lib.rs:2762+`, stages `pending_reindex` and the sweep re-enqueues
forced). Every forced pass runs the §8(a) diff-swap, which re-embeds
ALL chunks and writes `embed_key` in the swap transaction — coverage
rises to 100% as the work completes, and the gate moves to two-stage on
its own at the next query. NO new flags, NO grants, NO pass tracking.

**Repair path for UNREACHABLE docs (r25-M1, NEW — the four tools above
cannot reach every candidate doc):** all four repair paths select only
`tier = 'user_doc' AND status = 'indexed'` rows
(`list_indexed_user_doc_paths`, `db/queries.rs:38-41`) and skip any
file that no longer exists (`lib.rs:2349-2351`, `lib.rs:2795-2797`,
`bulk_reindex.rs:125-128`; `ct ingest` walks the vault, so a deleted
file is invisible to it) — and `bulk_reindex` aborts the whole pass on
the first ingest error (`bulk_reindex.rs:129-130`). The candidate set
by design filters on neither status nor file existence, and V22
deliberately keeps class-2 phantom path rows
(`connection.rs:3046`), so hash-deduped duplicates are possible. A
candidate doc whose source file is gone, whose status is not
`indexed`, whose tier is not `user_doc`, or whose bootstrap sample
re-embed failed (a 401) would otherwise hold coverage below 100%
FOREVER with no tool able to reach it. Therefore a per-doc repair
command is added:

> **`ct reindex repair-embed-key --stale`** — re-embeds the STORED
> CHUNKS of every stale/NULL-key CANDIDATE doc (the stage-1 candidate
> set, ~47 docs) DIRECTLY FROM THE DB, with NO file read and NO
> status/tier precondition, writing `embed_key` on success. This is
> the repair path for exactly the docs the four tools above cannot
> reach; it requires no stamp and no file to exist. Run it after a
> failed bootstrap sample or whenever the audit line shows
> `stale_embed_key ≥ 1`.

(Reviewer fix (b) — defining coverage only over docs a tool can reach —
was REJECTED: it reintroduces the silent-set-shrink that was r24 M2.)
Test: a candidate doc with a missing file and a stale `embed_key` has
this documented way back to 100% coverage (§10).

**Unforced revival — and the M5 resolution:** rev 25 added an
`embed_key = current key` requirement to the unforced short-circuit at
`pipeline/mod.rs:692` (rev-25 m1). That requirement is REMOVED. The
short-circuit stays exactly as today: unchanged hash + `indexed` +
non-forced ⇒ `Ok(())`. Rationale: the query-time coverage check makes
per-doc exclusion at INGEST time unnecessary — a stale doc is excluded
from stage 1 at QUERY time (it fails the `embed_key` predicate) and is
repaired by the next forced re-embed. Keeping rev 25's m1 requirement
would (a) break the "same decision table for forced and non-forced
paths" invariant, (b) send every unchanged-doc deposit kick
(`wisdom_deposit.rs:379-388`) and every unchanged re-pend through the
§8(c) decision table as a pure rechunk — refused whenever the stamp's
chunker fingerprint is stale or missing (with no verify-scratch bootstrap
run yet), writing a
refusal record + stderr line on EVERY kick — **this rationale is STALE
(r27 m2): refusal records are DELETED (§8(c)) and the kick calls ingest
with `force=false` (`wisdom_deposit.rs:384`), so a kick can never be
refused under the r26-M3 rules**, and (c) make
`wisdom_deposit`'s kick caller record `chunked` and then run the
librarian on a doc whose ingest was refused. The `pipeline/mod.rs:692`
early return is therefore restored, unmodified, and the §4/§6 fallback
is the ONLY staleness response. Self-healing claim, corrected (r24
M5): a stale doc is revived by ANY forced re-embed that covers it (a
model swap via the existing tools, or a per-doc forced re-embed); there
is no watcher/kick revival path under Option A — that is accepted, and
is why the pinned threshold is enforced at query time rather than
trusted to ingest-time revival.

**Audit line (r24 m1; r25 m1 swapped two fields; r26-M1 + r26 m4
RESTRUCTURE the line ONCE — this supersedes the six-field pin):**
`wisdom_two_stage_audit` carries EXACTLY SEVEN fields, in this order:
`rule=<i|ii|none> open=<0|1> top=<score> v1merged=<n> floor=<key>
k=<k> stale_embed_key=<n>` — where `stale_embed_key` counts
gate-eligible candidate-set docs (≥1 chunk, r26-M2) with NULL/stale
`embed_key` (i.e. 47 − covered count on the live brain today), `<n>`
counts vs the FULL candidate set (so a v1 fallback under low coverage
still reports HOW stale the set is — the number is actionable after the
fact, r24 M2), and `v1merged=<n>` counts facts admitted by the §3
v1-merge (facts in the structural complement scored via the v1 path in
the same call; 0
whenever the merge admits nothing). **Field semantics pinned (r26 m4;
mixed opens pinned r27 m5):**
`rule=none` is RESERVED for opens that no two-stage rule produced — the
§4 no-librarian merge-only v1 open, any §4 fallback/uncalibrated open,
and the coverage-below-100% sub-case of §4's fourth cell (r28-m4:
at 100% coverage the fourth cell's two-stage floor is PRESENT and
two-stage runs normally, so its opens are labeled `rule=i`/`rule=ii`
like any other two-stage open; only the coverage-below sub-case falls
back with no two-stage rule evaluated, and that case has no open to
label); a two-stage gate that CLOSES below its own floor
reports `rule=i` or `rule=ii` with `open=0`, never `rule=none`.
**Mixed opens (r27 m5, pinned; `top` defined per rule r28-m3):** when the
two-stage arm closes below
its floor but the v1-merge opens on complement facts, the line reports
`rule` = the two-stage rule that was EVALUATED, `open=1` = the FINAL
gate state (the merged list opened the gate), and `top` = **that rule's
own decision variable** (r28-m3): rule (i) opens on the restricted-set
FACT cosine, so `top` = the best restricted fact cosine; rule (ii)
opens on the member-chunk score, so `top` = the best member-chunk
score — NOT always the stage-1 chunk score (that would make rule-(i)
decisions unreconstructable from the audit line, since rule (i)'s
`open` is the final merged state while its decision variable is a fact
cosine). With per-rule `top`, the harness computes the two-stage arm's
FP from `open`/`top` without
v1-path opens contaminating the arm's score, and a v1-merged-only open
is derivable as `v1merged>0 ∧ two-stage-closed`. Hit/FP
remain harness-derived: the calibration harness derives hit and FP from
the probe labels plus the pinned `open`/`top` fields; the gate itself
cannot compute them (r25 m1, unchanged). The field COUNT stays pinned at
seven. The pinned 5-field wisdom stderr line is untouched. The line-shape
test asserts ALL SEVEN fields, including the new `v1merged` (§10).

## 7. Floors + acceptance letter

Floors are derived from the paired live calibration; **flip-to-default is
conditional on the acceptance letter passing in the same paired run**
(live coverage recorded — under Option A there is no `model_guard`;
the artifact records the candidate-set coverage fraction at run time,
which must be 100% for the two-stage arm to have run at all).

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
under the not-worse-than-baseline bar). **Same-probe overfit + held-out
split (r24 m7):** with 40 relevant probes, hit@2 moves in steps of
0.025 and the baseline is 1–2 hits, so a grid search over
(rule × floor × k) on the same set makes "not worse" close to
guaranteed. Kurt's letter stands as pinned; the artifact ADDITIONALLY
records the grid size and a stratified 75/75 held-out split scored over
five random splits — INFORMATIONAL only, no pass/fail hangs on it
(r24 E5).

## 8. Destructive-pass safety (pre-existing data-loss paths surfaced by the review ladder; fixed in this PR)

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
  under the new key).
  The mechanics: `texts` at `src-tauri/src/pipeline/mod.rs:741` already
  covers every chunk; unchanged-hash chunks KEEP their `chunk_id` and
  their `embeddings` row is UPDATEd in place with the new-model vector
  (never a bare INSERT — no unique constraint on `chunk_id`,
  duplicates would double-return in `semantic_search`); changed-hash
  chunks are inserted (new `(doc_id, content_hash)` identity — an
  UPDATE cannot represent them); removed chunks are deleted. Because a
  forced pass re-embeds ALL chunks, mixed-model vectors cannot arise
  within a doc. **`embed_key` is written HERE — in the swap
  transaction, to the key actually used (Option A: this is the column's
  primary writer; no pass-docs row exists anymore).**
  An UNFORCED content edit may skip re-embedding its unchanged chunks
  only when **the doc's own `documents.embed_key` equals the
  profile's current key (rev 24, Opus r22 MAJOR-2: the rev-5 rule
  keyed the skip on the GLOBAL stamp — a doc whose file returns
  after a missing stretch re-embeds only its changed chunks under
  model B while unchanged chunks keep model-A vectors; the
  per-doc key makes the skip self-limiting — NULL or stale
  `embed_key` ⇒ FULL re-embed of the doc, which then stamps it
  current. Test: missing file returns, unrelated swap completes in
  between, unforced edit ⇒ doc's vectors end up single-model, and
  stage 1 never scored it before the edit).**
  Empty-hash rows are treated as REMOVED (the
  `idx_chunks_doc_hash` partial index can't match them).
  **Per-doc unforced-edit skip rule, exactly (KEPT from rev 24):** the
  skip condition is `documents.embed_key == gate_model_key(profile,
  stub)` evaluated at swap time; it is a PER-DOC test, never a stamp
  test, never a coverage test. A doc failing it gets the full
  re-embed — which is also what raises coverage (§6).
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
  opened, so status/hash/chunks/`embed_key` are all the old ones and
  nothing was lost; the failure is logged (rev 9, Opus r7 MINOR-1: the
  doc is NOT moved to `error`, which the sweep skips — an indexed doc
  with intact chunks must stay reachable for the retry pass). The
  embed-failure test asserts the post-failure doc status AND hash for
  both cases, so a hash-matching rerun cannot short-circuit at the
  unchanged-hash check (`:692`). The stage-1 doc set adds NO
  `documents.status` filter beyond the shared eligibility predicate
  **plus the `documents.embed_key` exclusion (rev 23, Opus r21
  BLOCKER-1: stage 1 requires `d.embed_key` = the active model key, so
  docs still on model-A vectors — file-missing, never-swapped,
  post-bootstrap-unverified — are excluded; this is the ONE point
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
  upsert + delete-removed + insert-new + `embed_key` + mark indexed (no
  write lock across the network call; 5s busy timeout).
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
  `breaker_L_regrade`. **Regrade batch semantics (rev 5, Opus r3 m4;
  rev 7, Opus r5 MINOR-1):** `|doomed|` counts the rows the
  CLASSIFICATION flagged as doomed (the subset of `unanchored=1 AND
  deleted_at IS NULL` flagged rows that FAIL `evidence_has_live_chunk`
  at `:173-200` — the others are re-anchored, not deleted), not the raw
  flagged population. The breaker is ALL-OR-NOTHING — `|doomed|` >
  remaining budget refuses the WHOLE batch (never a partial delete); the
  in-transaction recount RE-RUNS the classification (it needs the
  per-row `evidence_has_live_chunk` verdicts, not a SQL COUNT),
  **compares against the pre-transaction `doomed` ID SET** (rev 8, Opus
  r6 m3 — the id set always exists) and **ABORTS on any mismatch**
  rather than deleting a different set — matching the swap-race rule.
  **Re-anchor writes commit regardless of the breaker verdict (rev 9,
  Opus r7 MINOR-3):** the pre-transaction classification pass's
  re-anchor UPDATEs (`UPDATE librarian_evidence SET unanchored = 0`,
  `:175-178`) are already committed when the breaker decides — benign
  (re-anchoring is a recovery action), NOT rolled back; the
  in-transaction recount may perform further re-anchors (do not make it
  read-only — it must agree with what the purge would delete).
  **Budget window (rev 5, Opus r3 M5 — a 24h auto-rollover only bounds
  TRANSIENT faults; for the persistent faults this breaker targets,
  rolling epochs with shrinking L would let each epoch spend a full
  budget and feed rows to the 7-day prune):** an epoch that TRIPS does
  not roll over — it stays tripped (spent stays at threshold) until an
  operator resolves the cause and resets the breaker keys explicitly.
  Healthy epochs roll over on the 24h cadence (start ts in
  `llm_wiki_meta`, baseline L re-snapshotted at rollover inside the
  IMMEDIATE transaction). **Residual risk, stated honestly:** with
  nothing alerting, a tripped breaker means heal silently does nothing
  indefinitely — that is the safe failure direction (no deletions at
  all) versus mass deletion; the 7-day prune only touches rows
  soft-deleted BEFORE the trip, so a tripped epoch cannot feed it.
  **Reset surface (rev 6, Opus r4 M3):** the reset is a NEW flag,
  `ct heal --reset-breaker`, deleting the epoch timestamp, spent
  counter, and both `breaker_L_*` keys in one IMMEDIATE transaction.
  **`--allow-bulk` is also NEW** (rev 6, Opus r4 M3 — no such flag
  exists in the code today; a NEW flag on `ct heal` and
  `ct evidence regrade` that skips the breaker for one run, the only
  sanctioned way to exceed the budget when an operator has verified the
  deletion is intended). While tripped, heal and regrade print one
  stderr line EVERY run — in a no-alerts deployment that is the only
  trip signal. (Under Option A these breaker keys are the only
  `llm_wiki_meta` state this spec adds; there is no guard/pass/grant
  key purge anymore. Regrade's migration-context path is V20-gated and
  dead on the live brain — the covered paths are the manual
  `ct evidence regrade` command and pre-V20 replicas via
  `skipped_destructive=true`, which holds V21+ on an upgrading brain —
  the safe direction.)
- **(c) Stamp enforced inside the funnel.** `ingest_file_virtual` itself
  refuses `force_rechunk=true` when the fingerprint stamp is
  stale. **Every** forced-rechunk path funnels through
  `ingest_file_virtual`, so one check gates every entry point (r9 M3):
  `ct ingest` (UNCONDITIONALLY forced — `tools/src/cmds.rs:172-178`
  hard-codes `force=true`; there is no `--force` flag in `Cmd::Ingest`),
  `tools/src/bin/bulk_reindex.rs:129`, `queue_full_reindex`
  (`src-tauri/src/lib.rs:2324`), and the automatic forced producers —
 the watchdog sweep re-enqueues `pending_reindex` as a *forced* rechunk
 (`src-tauri/src/pipeline/watchdog/sweep.rs:134-139` — the actual
 re-enqueue decision; r26 m7 corrects the drifted `:128-136`;
 `:73-82` is the status constant's doc comment) and
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
  inside the swap transaction (a new column on `documents`). A **pure
  rechunk** = `last_indexed_hash == the file's current hash AND the doc
  has ≥1 chunk`. A pure rechunk is refused whenever the stamp's
  CHUNKER component is not verified current. **Forced vs non-forced
  (r26-M3, pinned):** a pure rechunk arriving on a FORCED job (the only
  kind today's forced producers send for unchanged bytes) is REFUSED —
  that is the destructive-write protection. A pure rechunk on a
  NON-FORCED job is NOT refused at all: the chunks are by definition
  the current bytes (nothing destructive to protect), so the job is a
  NO-OP SUCCESS — it marks the row `indexed` and returns `Ok(())`,
  exactly like the `pipeline/mod.rs:692` short-circuit it sits beside.
  Refusals therefore arise only on the forced path, and every refusal
  site below is a forced caller. A
  watcher/`queue.rs` edit has `last_indexed_hash` = the OLD hash ⇒
  mismatch ⇒ normal diff-swap path — indexed with the new content,
  never refused. Unchanged files match and are refused (nothing to
  gain). Zero-chunk docs bypass (nothing to lose). Unforced edits also
  skip re-embedding unchanged chunks only when **the doc's own
  `documents.embed_key` equals the profile's current key (rev 24, Opus
  r22 MAJOR-2 — per-doc, not the stamp; NULL/stale ⇒ full re-embed)**
  (unchanged rule). **Refusal scope under Option A (r24 M5 disposition;
  r25-M3: the stamp is CHUNKER-ONLY):** the refusal keys on the stamp's
  CHUNKER component ONLY — no model component exists to omit, so the
  r24-M5 churn concern (refusing every unchanged-doc kick right after
  upgrade) is moot by construction. Chunker-component staleness behaves
  exactly as before.
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
  staged from `indexed` with unchanged bytes, so they ARE backfilled
  — i.e. the backfill WHERE clause is
  `WHERE status IN ('indexed','pending_reindex')` (rev 12, Opus r10
  m1 — the clause is the rule).
  `last_indexed_hash` is also written **inside
  `mark_document_indexed`** — which gains a hash parameter,
  `mark_document_indexed(conn, doc_id, indexed_hash)` (rev 13, Opus
  r11 m1: today it takes `(conn, doc_id)` only, `queries.rs:91-103`;
  writing `documents.hash` would inherit the existing race —
  `enqueue_vault_event` can overwrite the row with a newer hash
  between the upsert of H1 and the mark, recording H2 while holding
  H1's chunks, so the next forced run "pure-rechunks" and refuses the
  H2 edit forever). `indexed_hash` = the hash of the bytes actually
  chunked and embedded; it is the only writer (rev 12, Opus r10 m3),
  which also fixes the empty-re-extraction branch: an existing doc
  re-extracting to empty runs the swap transaction with every chunk
  treated as removed — upsert + delete-all + `last_indexed_hash` +
  mark indexed — so the hash no longer stays stale and later runs hit
  the `:692` early return. `error` and `orphaned` rows deliberately
  stay NULL: the guard is inert for them (treated as an edit) —
  accepted residual, noted in investigation §4.
  **M3 disposition (one line, so the reviewer sees it):** r24 M3's
  arm-4/arm-5 reset race lived entirely in the pass-docs dispatch
  machinery, which Option A DELETES wholesale — no pass rows, no 5-arm
  predicate, no reset, no race; the sweep returns to its pre-spec
  legacy backstop behavior and needs no pass-aware changes.
  Tests: post-upgrade fixture, stamp missing, forced rechunk of a
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
  pass/fail rule):** `ct reindex verify-scratch` rechunks a scratch
  copy of the live DB under the current binary + profile — **CHUNK-ONLY
  scope PINNED (r24 m5): it does NOT re-embed the scratch corpus; the
  only embeds it performs are the §6 sampled backfill embeds
  (first+last chunk per chunk-bearing doc, two per doc) — the hash and
  hop-resolvability checks need chunking, not embeddings** — then
  then **writes the stamp ONLY IF every live `librarian_evidence` hash
  ref still resolves on the scratch rechunk (baseline: 406/406) AND
  every gate-eligible entry stays hop-resolvable (baseline: 365/365)
  — CHUNKER-ONLY (r25-M3: the rev-26 model component — sampled
  backfill ratio ≥90% — is DELETED; the ratio is still computed and
  PRINTED per doc in the verify-scratch output as artifact evidence
  and is reflected per doc in the coverage count, but it gates no
  write)**.
  Stated plainly: **a chunker-version bump FAILS this check** (hashes
  rehash → evidence refs orphan) until the text-match remap follow-up
  lands — the correct response to that refusal is to NOT rechunk, not
  to override. The refusal override remains "re-run verify-scratch",
  which now can only succeed if the rechunk is actually lossless.
  **Refusal records DELETED (Option A; rationale CORRECTED r25-M2 —
  rev 26's reason was factually wrong):** rev 10–12's refusal-record
  table (keyed to `(doc_id, documents.hash at refusal, stamp
  fingerprint)`) existed to stop re-sweep loops over refused
  `pending_reindex` rows. Rev 26 claimed those rows "existed only as
  pass staging, which Option A deletes" — FALSE: **`pending_reindex`
  rows are created by TODAY's code, with no pass machinery involved —
  `queue_full_reindex(force_rechunk=true)` stages them when the
  channel is full (`lib.rs:2378-2391`) and `run_wiki_reembed` does the
  same (`lib.rs:2815-2825`); the sweep then re-enqueues them as forced
  rechunks (`sweep.rs:134-139`; r26 m7 corrects the drifted
  `:133-137`) — and the spec's own backfill clause
  (`WHERE status IN ('indexed','pending_reindex')`, above) already
  assumed they exist.** With the funnel refusal now applying only to
  genuine unchanged-file forced rechunks (a
  no-op-by-definition job), there is no loop to suppress and no record
  is written.
  **Refusal row-state PINNED (r25-M2; reset widened to BOTH statuses
  r26-M3; hash restored r27-M3):** a refused forced rechunk leaves the doc's
  status untouched **EXCEPT one reset**: when the refused row's status
  is `pending` OR `pending_reindex`, the refusal path RESETS it to
  `indexed` and RESTORES the pre-written hash, with a conditional
  `UPDATE documents SET status='indexed',
  hash=last_indexed_hash WHERE status IN
  ('pending','pending_reindex') AND id=? AND last_indexed_hash IS NOT
  NULL` — the data is intact (a refusal
  changes nothing), so the reset is sound, and without it the row
  would sit `pending_reindex` forever: the sweep claim is kept while
  the row stays sweepable (`retain_sweepable`, `sweep.rs:62-64`), each
  restart re-enqueues and refuses it again, and
  `list_indexed_user_doc_paths` (`db/queries.rs:38-41`) excludes it
  from every later reindex pass. **The hash restore (r27-M3; rationale
  corrected r28-M1):** `enqueue_vault_event` (`db/queue.rs:168-175`) writes the
  new hash BEFORE ingest, so a pure-rechunk refusal fires exactly when
  `documents.hash` holds a stale pre-written value (enqueue read H2,
  the file was reverted to H1, the worker read H1); resetting only the
  status would leave the row `indexed` with `hash=H2` while its chunks
  hold H1 — the doc then risks falling out of hash-hop bookkeeping: the
  row's `hash` no longer matches its chunks, so the r27-M3 r24-M2-style
  "drops out of the hash-hop candidate set" argument was OVERTURNED at
  r28-M1 — with the real hop (evidence `content_hash` → `chunks` →
  `doc_id` ∪ proposal `doc_id`), `documents.hash` is NOT a membership
  predicate, only the dedupe/row-identity key — but the restore is kept
  as HYGIENE: it keeps `:692`'s unchanged-hash check and the dedupe
  consistent with the on-disk bytes and prevents the stale pre-write
  from masking a future content change. Coverage still reads 100% in
  either case (the doc's membership is unaffected). The reset is
  specified INSIDE `ingest_file_virtual` (r27 m3) — all refusal paths
  funnel through it, so a forced `ct ingest` refusal on a `pending`
  row is covered without any worker-side reset; the pipeline worker
  only logs. (`pending` joined the reset at r26-M3
  for the same reason `pending_reindex` needed it at r25-M2 — a refused
  row left `pending` is equally sweep-stuck; the reset remains a single
  conditional UPDATE, one statement.) An `indexed` row, by contrast, stays
  `indexed` — nothing changes. **r24 m3 disposition (carried, now on
  corrected grounds):** no refusal-record table, no re-sweep loop.
  The replacement tests in §10 assert BOTH cases: a refused forced
  rechunk on an `indexed` row leaves it `indexed` (pending counter at
  baseline, no refusal record anywhere), and a refused forced job on a
  `pending_reindex` row resets it to `indexed` and it is not
  re-swept. **r26-M3: the reset also fires on a `pending` row —
  edit-then-revert with the stamp missing on a `pending` row leaves it
  `indexed` after the refusal**. §10 also carries the reviewer's fixture:
  GUI reembed
  queued with a full channel and a missing stamp ⇒ every deferred row
  ends `indexed` and outside the sweep claim set. **r26-M3 non-forced
  fixture (narrowed r28-M2): a NON-FORCED pure rechunk (unchanged hash +
  ≥1 chunk + stale stamp) on a `pending` ROW is a no-op SUCCESS — row
  marked `indexed`, `hash` restored to
  `last_indexed_hash` (r27-M3), `Ok(())`, no refusal
  record, no stderr refusal line; on a `pending_reindex` row the no-op
  does NOT fire (status+hash untouched, row stays sweepable for the
  sweep's forced job); only a FORCED pure rechunk is
  refused.** **r26-M4 refusal-storm rows (both new caller sites): a
  `bulk_reindex` pass over a set containing refused paths does NOT abort
  — the refusal is counted as a SKIP, the pass continues, and the linker
  still runs over the processed entity_ids; a pipeline-worker refusal
  produces exactly ONE informational stderr line, ZERO
  `write_error_log` entries, and the row reset has already been applied
  inside `ingest_file_virtual` (r27 m3 — the worker logs only).**

  **Funnel decision table (Option A — FOUR rows keyed on
  (forced?, chunker current?); the rev-9 four-row
  table died with the grant; r25-M3: the stamp is CHUNKER-ONLY, so the
  rev-26 "stamp read = (chunker component, model component)" phrasing
  is withdrawn — the stamp read is the CHUNKER fingerprint vs current
  chunker, and the MODEL component is not read anywhere; r27-M2: the
  table is rewritten so it agrees with r26-M3 — the two paths do NOT
  behave identically):** the table is evaluated AFTER the existing
  non-forced short-circuit (`pipeline/mod.rs:691-694` — unchanged +
  `indexed` + non-forced ⇒ `Ok(())` as today, so `wisdom_deposit` kicks
  of unchanged docs return early and never touch the stamp check, no
  refusal churn on the normal path; rev-25 m1's `embed_key` leg on
  this short-circuit is REMOVED, §6):
  - FORCED + chunker current ⇒ **pass** (the stamp carries no model
    component — r25-M3 — and
    the coverage check owns model staleness; a forced job re-embeds all
    chunks and writes `embed_key` itself, §8(a));
  - FORCED + chunker stale or missing ⇒ **refuse** (the
    bootstrap/override is `ct reindex verify-scratch`);
  - NON-FORCED + chunker current ⇒ **no-op success, `pending` ROWS ONLY**
    (r27-M2: pinned so
    behavior never depends on the stamp — a non-forced pure rechunk is
    a no-op by definition whether or not the stamp is current; the row
    is marked `indexed` with `hash=last_indexed_hash` restored, r27-M3).
    **`pending_reindex` rows are EXCLUDED from this no-op (r28-M2):** a
    `pending_reindex` row was staged by `queue_full_reindex`/`run_wiki_reembed`
    precisely BECAUSE its re-embed must be forced — `sweep.rs:76-81`
    documents the hazard ("re-enqueueing them as a plain ingest would
    let `ingest_file`'s unchanged-hash check short-circuit and silently
    drop the chunk-strategy or embedding-model upgrade"). A non-forced
    job on a `pending_reindex` pure rechunk returns `Ok(())` with STATUS
    AND HASH UNTOUCHED — the row stays sweepable so the sweep's forced
    job still runs. (Failure this prevents: model swap A→B, channel
    full, D staged `pending_reindex`, a wisdom-deposit kick ingests D
    non-forced before the sweep; an unconditional no-op would mark D
    `indexed` with model-A vectors and key — out of the sweepable set,
    below coverage forever, nothing alerts.)
    **Key-currency condition (r28-M2):** additionally, the no-op only
    fires when the doc's `embed_key` already equals
    `gate_model_key(profile, stub)`; if the key is stale/NULL, the
    non-forced job RE-EMBEDS THE STORED CHUNKS IN PLACE (the
    `repair-embed-key` mechanics — no rechunk, no file read) and updates
    the key, so a non-forced touch repairs instead of ignoring model
    drift;
  - NON-FORCED + chunker stale or missing ⇒ **no-op success, same
    `pending`-only + key-currency conditions** (r26-M3, narrowed r28-M2).
  Zero-chunk docs bypass the whole table (nothing to lose); content
  edits bypass it (diff-swap).
  **Refusal signaling (r25 m4; ALL FOUR callers pinned r26-M4):** the
  forced path signals a
  refusal with a TYPED ERROR, `Err(IngestRefused)` — not `Ok(())`.
  Every caller downcasts it; a refusal is never a generic `Err`:
  1. `ct ingest` downcasts it in `cmds.rs` and treats it as success (exit
     0, §8(c)'s loop accounting below);
  2. `wisdom_deposit`'s kick
     (`wisdom_deposit.rs:379-388`) treats `IngestRefused` as
     success and still records `chunked` — an unchanged file's chunks are
     valid, so the downstream state is identical. **UNREACHABLE today
     (r27 m2):** the kick calls ingest with `force=false`
     (`wisdom_deposit.rs:384`), and only FORCED pure rechunks are
     refused, so this caller can never receive `IngestRefused` under
     the r26-M3 rules; keep a DEFENSIVE downcast placed BEFORE the
     `map_err(|e| anyhow!(...))` at `:387` (after it the error is a
     string and no downcast can succeed), and treat this item as
     belt-and-suspenders rather than a live path;
  3. **`bulk_reindex` (r26-M4):** a refusal is counted as a SKIP for
     that path — one informational stderr line, no `?` propagation, the
     pass CONTINUES to the remaining paths, and the linker loop still
     runs over the processed entity_ids (today `bulk_reindex.rs:129-141`
     aborts the whole pass on the first `Err` via `?`; the downcast
     replaces that abort for `IngestRefused` only — genuine errors still
     abort);
  4. **the pipeline worker (r26-M4):** the worker's generic `Err` arm
     (`pipeline/mod.rs:257-261`) writes "ingest error" to stderr AND
     `write_error_log` — an `IngestRefused` must NEVER reach that arm
     un-downcast. The worker downcasts FIRST: on `IngestRefused` it logs
     ONE informational stderr line (NOT `write_error_log` — a refusal is
     not an error) and then continues the loop — the row-state reset
     itself happens inside `ingest_file_virtual` (r27 m3, above), not in
     the worker, so `ct ingest` refusals get identical reset behavior.
  This is a SINGLE typed
  error, not an enum: §9's "no `IngestOutcome` enum" language stands.
  **`ct ingest` loop accounting (rev 6, Opus r4 m1):** the refusal
  outcome is treated as a SKIP in the per-file loop at
  `tools/src/cmds.rs:180` — no `failed += 1`, no non-zero exit, and
  the file still counts in the linker's entity set (an unchanged file
  has valid chunks; a refused rechunk changes nothing).
  **"Fresh brain" defined (rev 5 Opus r3 m6; rev 7 Opus r5 MAJOR-2 — a
  COUNT(*)=0 check cannot implement "no rows EVER": `librarian_evidence`
  has no high-water mark (`schema.rs:393-399`, `entry_id TEXT PRIMARY
  KEY`, no AUTOINCREMENT) and rows leave via regrade/prune/clear hard
  deletes):** a persistent marker `librarian_evidence_seen` in
  `llm_wiki_meta`, set by an `AFTER INSERT` trigger on
  `librarian_evidence` and backfilled by the new ungated migration for
  any brain that has rows today.
  **The marker is explicitly KEPT by the clear transaction** (NOT in
  any purge list): a cleared brain has CARRIED librarian facts — the
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

## 9. Implementation order (within the ONE PR)

**Dependencies stated exactly (r24 M1 fix):** under Option A the read path
(step 1) depends on (1) the `documents` DDL adding `embed_key` and
`last_indexed_hash`, (2) the verify-scratch bootstrap run (stamp + sampled
`embed_key` backfill), and (3) the coverage check reading
`documents.embed_key`. The old
"no DDL on the critical path" claim is withdrawn (§1). The DDL IS on the
critical path. With pass-docs deleted, step 1 does NOT depend on: any
`ct_reindex_pass_docs` DDL, any grant/sweep/refusal-record machinery, any
frontend pass-progress fields, or `IngestOutcome` threading (the
short-circuit is restored as-is, §6).

1. Read path + its DDL: the ungated idempotent migration adds
   `documents.embed_key` and `documents.last_indexed_hash` — **ordering
   PINNED (r24 m6): the new columns are added by a migration that runs
   AFTER every versioned migration (the V15 rebuild of `documents` copies
   an explicit column list, `schema.rs:315-345` at
   `db/connection.rs:300-303`; columns added before it would be dropped by
   the rebuild) — i.e. after V18 in the migration chain, same slot as the
   trigger migration — plus a pre-V15 fixture test asserting both columns
   exist after open.** **Migration mechanics PINNED (r25 m3): the new
   migration is versioned V28 — the next free number: the chain currently
   caps at V27, `apply_v27_embed_scheme`, which stamps 27 gated on V22
   (`db/connection.rs:1003-1017`; the test at `:3377` asserts
   22→27 all stamp) — and follows the V24 pattern exactly: unlocked
   column-existence pre-check, re-inspect UNDER `BEGIN IMMEDIATE`, stamp
   last (the V21 non-idempotent ALTER precedent and its fix are at
   `db/connection.rs:902-948`). Its STAMP is gated on V22 having stamped,
   like V23–V27, so a rootless open cannot mask a deferred V22.**
   **The V28 backfill is ONE-SHOT (r26 m5, pinned; extended to BOTH
   backfills r28-m7):** if V28 carries an
   `embed_key` backfill statement (e.g. seeding the key for docs whose
   chunks provably carry the current model's vectors) OR the
   `last_indexed_hash` backfill
   (`WHERE status IN ('indexed','pending_reindex')`), EACH executes ONLY
   in the migration call that adds its column, inside the same
   transaction — the V27 precedent
   (`db/connection.rs:973-980`: the `embed_scheme` UPDATE runs only in
   the `ALTER TABLE` call, because `embed_scheme` is unindexed and an
   ungated UPDATE would full-scan `llm_wiki_entries` and take the write
   lock on EVERY open forever). Repeating either backfill on every open
   is FORBIDDEN: re-running the `last_indexed_hash` backfill per open
   would re-stamp `pending_reindex` rows created after the upgrade —
   rows whose staged hash may itself have been race-written — bringing
   back the r11-m1 hazard; the migration's work is guarded by the
   column-existence check, never re-run. Then: stage-1 search + hop + decision rules +
   labels/fallbacks + the coverage check, opt-in (`--two-stage`). This
   step is testable against a live-brain fixture ONLY after step 2's
   bootstrap has run on that fixture (dependency: coverage must be
   computable and the stamp present, else every test falls back to v1 and
   exercises nothing).
2. Safety + bootstrap: diff-swap (§8(a), including `embed_key` and
   `last_indexed_hash` writes in the swap transaction and the
   `mark_document_indexed(conn, doc_id, indexed_hash)` hash param) +
   funnel stamp + FOUR-row decision table (§8(c)) + cross-process
   breaker + `ct heal --reset-breaker` / `--allow-bulk` flags + the
   `librarian_evidence_seen` marker (trigger + migration backfill) +
   `ct reindex verify-scratch` (bootstrap command: stamp write, sampled
   backfill, chunk-only scope)
   **+ `ct reindex repair-embed-key --stale` (r26 m1: the §6 r25-M1
   repair command joins the step 2 build list — it is part of the same
   ONE PR, not a follow-up)**. NO pass-docs storage, NO grants, NO
   refusal-record table, NO frontend work, NO `IngestOutcome` enum (no
   caller branches on a refusal outcome anymore — `ct ingest`'s loop
   treats the refusal as a skip internally, §8(c)). Note
   `db/queries.rs` (`mark_document_indexed` `:91-103`,
   `list_indexed_user_doc_paths` `:38-41`) is distinct from
   `tools/src/queries.rs` (embed-skip `:791`, query_text_for_scheme
   `:788`); full paths used throughout.
   **repair-embed-key write shape PINNED (r26 m1):** the repair
   command's write is ONE IMMEDIATE transaction covering, together, (a)
   the in-place embeddings UPDATE (the doc's chunks' vectors, from
   stored chunk text) and (b) `documents.embed_key =
   gate_model_key(profile, stub-as-used)` — and it RE-CHECKS the doc's
   chunk-id set UNDER THE LOCK (the §8(a) swap-race rule: if the chunk
   set changed between the read and the transaction, retry the re-embed
   rather than writing vectors for stale chunk ids). No partial write
   ever lands: vectors without the key, or the key without matching
   vectors.
3. Paired live calibration = the acceptance gate; pick rule + floor;
   flip-to-default if the letter passes.
   **3a (r22 MAJOR-4) runs BEFORE step 3's calibration:
   `ct reindex verify-scratch` on the live brain — writes the
   CHUNKER-ONLY funnel stamp AND performs the sampled backfill of
   `documents.embed_key` (r25-M3: no model component exists; r25-M1:
   the backfill reads stored chunk text, so file existence and doc
   status never block it).**
   A stamp-less brain still writes `embed_key` on every ordinary
   ingest — coverage reads 0% only for docs never re-ingested since
   the upgrade (r25-M3 corrects rev 26's blanket "coverage reads 0%").
   Without step 3a the calibration arms run on an
   unverified vector population (a mixed corpus
   would be measured as if homogeneous) and with unbackfilled
   `embed_key` columns. **If coverage is below 100% after 3a, repair
   first (r25-M1): `ct reindex repair-embed-key --stale` re-embeds the
   stored chunks of any stale/NULL-key candidate docs directly from
   the DB — the repair path for docs no other tool can reach.** Verify
   step 3a's own output —
   stamp present, backfill counts printed, candidate-set coverage = 100%
   — before starting the paired run.
4. Provenance backfill only if the audit shows hop gaps (expected:
   unnecessary — 365/365 live).

## Error handling

- Missing two-stage floor → v1 fallback (never uncalibrated while a v1
  floor exists); both missing → v1 uncalibrated semantics.
- Coverage below 100% → v1 fallback (never dark — ops crons are DECLINED;
  nothing else alerts). Same one-rule matrix as the floor case.
- Two-stage floor present + v1 floor missing + coverage below threshold →
  uncalibrated no-open (the sole dark-adjacent cell — intentional, see
  §4; fresh/undercalibrated brains only).
- Below-floor stage 1 → gate closed, audit line still emitted.
- Rechunk scratch-verification failure → refuse the live rechunk.
- Breaker refusal → destructive pass aborts with
  `skipped_destructive=true` semantics (migration context: V21+ hold).

## Testing

- Stage-1 SQL: closed-set membership vs live-gate-eligible filter parity
  (incl. the >1024 excludes parity guard); deterministic row choice under
  `documents.hash` dedupe.
- Hop: 365/365 resolvability on the live brain as a fixture-backed test;
  the §3 v1-merge's complement size is asserted (structurally computed —
  expected 3 on the live brain, all `user_stated`/`user_confirmed`;
  r27-M1 replaces the r8-m4 provenance-count assertion with a
  complement-size assertion so the test tracks the actual merge
  population).
- Fallback matrix, one test per branch of the §4 single rule:
  two-stage-floor-only-missing / both-missing / below-floor /
  kill-switches / **coverage-check branches (Option A): coverage = 100%
  ⇒ two-stage runs; one stale doc in the candidate set ⇒ v1 fallback +
  `stale_embed_key=1` on the audit line; ALL candidate `embed_key` NULL
  (stamp present, no backfill) ⇒ v1 fallback, never closed; empty
  candidate set with the v1-merge admitting nothing ⇒ closed (vacuous
  coverage, §3) with audit line
  emitted** / fourth cell: two-stage floor present + v1 floor missing +
  coverage below threshold ⇒ uncalibrated no-open (asserted
  dark-adjacent BY DESIGN).
- **r26-M1 v1-merge tests:** an ALL-`user_stated` brain (no candidate
  docs at all, v1 floor present) STAYS OPENABLE — the v1-merge scores the
  complement facts and the gate opens when one clears the v1 floor,
  audit `v1merged ≥ 1`; a `user_stated` fact OPENS on a MIXED brain (it
  appears in the merged list alongside two-stage opens, one ranked list,
  truncate to `max` respected). **r27-M1 complement tests:** a
  ZERO-CHUNK doc's facts (sourced, but no stage-1-scorable chunk) open
  via the merge; an ALL-SKIP-CLASS doc's facts open via the merge —
  both are in the structural complement by definition, and a
  provenance-based implementation would miss them. **r28-m2 parity
  test:** under `instr1`, a complement fact's merge score equals
  `wisdom_match_in_scheme`'s score for it, bit-identical.
- **r26-M2 zero-chunk test:** a candidate doc with 0 chunks is excluded
  from the coverage population — its NULL/stale `embed_key` neither
  lowers coverage nor appears in `stale_embed_key` (§6); population
  harmonized r28-m1: an ALL-SKIP-CLASS doc is likewise excluded (≥1
  stage-1-scorable chunk is the test on both sides).
- **r28-M2 test:** a `pending_reindex` row + a non-forced kick ⇒ the
  row STAYS `pending_reindex` (status+hash untouched), remains
  sweepable, the sweep's forced job runs, and `embed_key` ends CURRENT
  — the no-op must not fire on it. A non-forced touch on a `pending`
  row with a STALE `embed_key` re-embeds stored chunks in place and
  updates the key (key-currency condition).
- **r28-M3 test:** a pre-V28 fixture brain (no `embed_key` column)
  opened READ-ONLY with `--two-stage` ⇒ v1 label, exit 0,
  `stale_embed_key=<candidate count>` — never a "no such column" error.
- Labels: pinned 5-field stderr line byte-identical under both paths;
  `wisdom_two_stage_audit` line shape asserts ALL SEVEN pinned fields
  (§6) in pinned order — `rule`, `open`, `top`, `v1merged`, `floor`,
  `k`, `stale_embed_key` (r26-M1 restructure supersedes the six-field
  pin; r25 m1: `hit@2_open`/`fp_open` are gone; the
  harness derives hit/FP from probe labels plus the `open`/`top`
  runtime facts) incl. `stale_embed_key=<n>` (r24 m1); `rule=none`
  never on a two-stage close (r26 m4).
- Diff-swap: unchanged-hash preservation; changed-hash in-place UPDATE
  (no duplicate embeddings — `semantic_search` returns the chunk once);
  empty-hash rows removed; mid-document-insertion behavior (documents the
  rehash cascade — follow-up remap is explicitly out of scope); embed-
  failure mid-run leaves prior chunks, status, hash, AND `embed_key`
  intact (the 401 scenario); per-doc skip rule — missing file returns,
  unrelated swap in between, unforced edit ⇒ single-model vectors, stage
  1 never scored the doc before the edit (r22 M2).
- Breaker: threshold math max(⌈0.05·L⌉,10) on both denominators;
  cross-process budget via `llm_wiki_meta` (two connections, shared
  refusal); ALL THREE heal writers covered (scheduler/`ct heal` via
  `heal_invalid_sources_conn`, GUI button via
  `heal_lost_librarian_inferred` — including its IMMEDIATE-transaction +
  conditional-UPDATE fix — and regrade's single pre-delete check on
  `|doomed|`).
- Stamp/backfill: missing/stale chunker fingerprint refused inside
  `ingest_file_virtual` via each of the three manual callers PLUS the
  forced producers (watchdog sweep re-enqueue and `rechunk_for_reembed`);
  fresh-brain bypass requires `librarian_evidence_seen` ABSENT (r6 m5);
  `ct reindex verify-scratch` on a chunker bump FAILS and writes no
  stamp; verify-scratch passes only when evidence refs + hop
  resolvability hold; sampled backfill — first+last chunk, cosine
  ≥ 1−1e-3, failed sample leaves NULL and is PRINTED in the
  verify-scratch artifact with its per-doc verdict, reflected per doc
  in the coverage count (r25-M3 replaces the rev-26 "counts against the
  ratio" test — the model-component test is dead: there is no model
  component and no ≥90% rule);
  bootstrap-only brain (stamp + backfill, no swap ever run) has a
  NON-empty stage-1 set; a new file ingested after bootstrap IS scored
  by stage 1; file-missing doc (stale `embed_key`) is never scored by
  stage 1; **r25-M1 repair test — a candidate doc with a MISSING FILE
  and a stale `embed_key` reaches 100% coverage via
  `ct reindex repair-embed-key --stale`, which re-embeds its stored
  chunks from the DB with no file read**; **refusal row-state, BOTH
  cases (r25-M2): a refused unchanged-file forced rechunk on an
  `indexed` row leaves it `indexed`, pending counter at baseline, and
  NO refusal record; a refused forced job on a `pending` or
  `pending_reindex` row
  is reset to `indexed` WITH `hash=last_indexed_hash` restored via the
  conditional
  `UPDATE … SET status='indexed', hash=last_indexed_hash
  WHERE status IN ('pending','pending_reindex') AND id=? AND
  last_indexed_hash IS NOT NULL` and is not
  re-swept; the edit-then-revert fixture (r27-M3) additionally asserts
  the doc is still IN the stage-1 candidate set afterwards**; **reviewer fixture (r25-M2): GUI reembed queued with a
  full channel and a missing stamp ⇒ every deferred row returns to
  `indexed` and leaves the sweep claim set**; `ct ingest --yes` of a
  NEW file succeeds with no stamp present; watcher Modify event +
  stamp missing ⇒ doc indexed with the NEW content (r8 B1).
- Coverage check: covered/uncovered COUNT aggregation over a fixture
  candidate set; `embed_key = gate_model_key(profile, stub)` round-trip
  (stub set ⇒ `stub:`-prefixed key written AND matched — r24 m2);
  scheme-suffixed key never written or matched; a forced re-embed of
  the stale doc raises coverage to 100% ⇒ two-stage resumes;
  **r25 m2: `COALESCE(…,0)` pinned — the SQL's covered count over an
  empty candidate set returns 0 with COUNT 0 (vacuously covered), not
  NULL; the coverage query runs BEFORE any embed in the gate path**.
- Audit line: seven fields, order, and `stale_embed_key` arithmetic
  (47-doc fixture with 2 stale ⇒ `stale_embed_key=2`); `v1merged` present
  and an integer, `=0` when the v1-merge admits nothing (r26-M1);
  `rule=none` never appears on a two-stage close (r26 m4).
- Acceptance: paired live calibration on the 150-probe real-traffic set,
  both arms same run, letter numbers as pinned by Kurt; artifact records
  grid size + held-out splits (informational, r24 m7/E5) and the
  candidate-set coverage fraction; flip-to-default lands in this PR only
  on a passing letter.

## EXPERIMENTS REQUESTED (r25)

The r25 reviewer requested eight experiments. They are carried here
VERBATIM IN MEANING, compacted, as requests to run during implementation
and calibration — they are NOT new acceptance gates; the acceptance
letter (§7) stays exactly as Kurt pinned it.

1. **Repairability census of the candidate set** (before implementation;
   read-only SQL on a scratch copy). For the ~47 candidate docs, count
   each of: `status != 'indexed'`, `tier != 'user_doc'`, source file
   missing on disk, more than one `documents` row per `hash` (phantoms),
   and whether the dedupe's chosen row is the reachable one. Expected:
   every count is 0. Bad result: any non-zero count is a doc that would
   keep the gate on v1 permanently (r25-M1) — the repair path has to
   work first.
2. **Re-embed self-cosine distribution.** Re-embed 50 random stored
   chunks twice under the live profile (OpenRouter qwen3-embedding-4b);
   record min and p1 of cosine(stored, fresh) and cosine(fresh₁,
   fresh₂). Expected: min ≥ 0.9995. Bad result: values in 0.995–0.999
   (backend routing/quantization) would make the 1−1e-3 tolerance reject
   valid docs and coverage would never reach 100%.
3. **Dry run of bootstrap + coverage on a scratch brain.** Run
   `ct reindex verify-scratch`; print per-doc sample verdicts and final
   candidate coverage. **This experiment is PRINT-ONLY (r26 m6): it
   writes NOTHING to either DB — the scratch copy is discarded and the
   live DB is not touched; the real bootstrap (stamp + backfill to the
   live DB) is the §9 step 3a run, not this dry run.** Expected: 47/47.
   Bad result: below 100% with no
   repair path available (see 1; the r25-M1 repair command is the fix).
4. **Stuck-`pending_reindex` fixture** (r25-M2). One-slot channel,
   missing stamp, `run_wiki_reembed`; assert the deferred rows' final
   status (must return to `indexed`) and that they are not stuck in the
   sweep claim set. Bad result: rows still `pending_reindex` after the
   job is refused.
5. **Rule (ii) per-open injection volume** during calibration. Record
   entries per open under each floor-semantics reading (r25-M4). Signal:
   if the pinned no-fact-floor reading injects a median of ≥5 facts per
   open, rule (ii) behaves very differently from v1 even when "not
   worse" passes.
6. **Stage-1 SQL latency.** Time stage-1 plus coverage on the live brain
   (261 chunks; time the full ~291-doc scan as a worst case) and the
   fact-vector embed's p50/p95 per gated message (r26 m3: this is a
   SECOND embed only under `instr1`; under `raw` the stage-1 vector is
   reused and there is no second embed). Bad result: more than
   300 ms added to the gate path.
7. **Calibration robustness** (informational; §7's held-out splits).
   Besides the five held-out splits, report hit@2 with a bootstrap 95%
   CI per arm. With a baseline of 1–2 hits out of 40, fully overlapping
   CIs mean "not worse" carries almost no information — fine under
   Kurt's letter, but the artifact must say so.
8. **Live-path control.** Rerun PR #270's Cell A probe set through the
   two-stage arm with `--two-stage` while coverage is forced below 100%
   (NULL out one doc on a scratch copy). Assert the stdout label falls
   back to v1, `stale_embed_key=1` appears, and the v1 numbers match the
   baseline arm — the fallback must be exactly the v1 path.

## EXPERIMENTS REQUESTED (r26)

The r26 reviewer requested five experiments. Same status as the r25
list: requests to run during implementation and calibration, NOT new
acceptance gates; the acceptance letter (§7) stays exactly as Kurt
pinned it.

1. **Expanded census of the candidate set** (read-only SQL on a scratch
   copy; extends the r25 E1 census). Count, in addition to the r25 E1
   columns: (a) ZERO-CHUNK candidate docs (r26-M2's coverage population
   excludes them; expected 0); (b) gate-eligible entries that are
   `user_stated` or `user_confirmed` — the source-less facts the §3
   v1-merge exists to carry (expected 3 on the live brain); (c)
   `documents` rows with `status = 'pending'` AND
   `hash = last_indexed_hash` — rows the §8(c) refusal reset (r26-M3)
   would have left stuck pre-fix (expected 0). Bad result on any:
   (a)/(c) non-zero means live data already hits the fixed edges — run
   the repair/reset paths first; (b) ≠3 means the v1-merge test counts
   change, not the design.
2. **No-librarian brain fixture.** A brain with
   `librarian_evidence_seen` ABSENT and no candidate docs: the gate
   opens as v1 would (the §4 no-librarian branch) and reports
   `rule=none` (r26 m4) — the v1-merge and the branch must be
   indistinguishable from today's v1 behavior except for the audit
   line.
3. **Edit-then-revert watcher fixture** (companion to the §10 M3
   non-forced fixture). Edit a file, let the watcher enqueue, revert it
   before the worker picks it up, with the stamp missing: the resulting
   pure rechunk is a NO-OP SUCCESS (r26-M3) — row `indexed`, `Ok(())`,
   no refusal, no `write_error_log` line; under the pre-fix behavior
   this edit was refused on a `pending` row and stayed sweep-stuck.
4. **Refusal storm** (r26-M4 acceptance for the two new caller pins).
   Force-refuse N paths (missing stamp, unchanged bytes) through (a)
   `bulk_reindex` and (b) the pipeline worker. Assert: `write_error_log`
   count = 0 after the storm (the worker logged informational lines
   only); `bulk_reindex` processed every non-refused path AND ran the
   linker over the processed entity_ids despite the refusals.
5. **Raw-scheme embed count** (folds into the r25 E6 latency
   experiment). Assert the per-gated-message embed count: ONE embed
   under `Scheme::Raw` (the stage-1 vector IS the fact vector —
   `query_text_for_scheme` returns the query unchanged,
   `src-tauri/src/embed_scheme.rs:149-150`; r26 m3), TWO under `instr1`.
   The r25 E6 "second embed" p50/p95 measurement applies to instr1 only.

## EXPERIMENTS REQUESTED (r27)

Same status as the r25/r26 lists: requests, not acceptance gates; the
acceptance letter (§7) stays exactly as Kurt pinned it.

1. **Complement census** (r27-M1; read-only SQL on a scratch copy).
   Count gate-eligible entries NOT reachable via any candidate doc with
   ≥1 non-skip-class chunk, grouped by `source_type`. Expected: exactly
   3, all `user_stated`/`user_confirmed`. Bad: >3, or any
   `librarian_inferred`/`immutable_document`/NULL — that means the
   provenance-defined merge would leave live facts dark today (the
   structural complement is the fix; this census measures the gap it
   closes).
2. **Hash-drift census** (r27-M3; premise corrected r28-M1). On a
   scratch copy, count `documents`
   rows where `hash` ≠ sha256 of the file on disk, split by status.
   Expected: 0 outside `pending`. Bad: non-zero `indexed` drift — those
   docs' rows are inconsistent with the dedupe key (hygiene signal; with
   the real hop, membership is unaffected — see r28-M1).
3. **Edit-then-revert candidate-membership fixture** (r27-M3; extends
   r26 E3; membership premise retained — the doc's facts are hop-resolvable
   via the evidence/proposal edges regardless of the `hash` restore).
   After the no-op, assert BOTH `documents.hash =
   last_indexed_hash` AND that the doc is in the stage-1 candidate set.
4. **Merge attribution during calibration.** Report how many
   two-stage-arm opens and hits came only from `v1merged`. Bad: a large
   share — then "not worse" is measuring v1 on complement facts, not
   two-stage retrieval.
5. **Mixed-brain ranking check.** A fixture with one `user_stated` fact
   and one rule-(ii) doc opening several facts at `max=2`. Assert the
   merged list is ordered by weighted cosine across both sources, and
   that rule-(ii) facts (no floor) can't systematically push out
   v1-merged facts that cleared the v1 floor. Record how often the
   displacement happens in calibration.

## EXPERIMENTS REQUESTED (r28)

Same status as the r25/r26/r27 lists: requests, not acceptance gates; the
acceptance letter (§7) stays exactly as Kurt pinned it.

1. **Real hop SQL dry run** (r28-M1). On a scratch copy, run the
   corrected candidate SQL (evidence `content_hash` → chunks ∪ proposal
   `doc_id`). Expected: 47 docs, 261 chunks, 62 hash-hop chunks —
   matching investigation v10 line 17. Bad: any other number means the
   v10 measurements came from a query the spec didn't describe;
   re-measure before calibrating.
2. **`pending_reindex` + kick fixture** (r28-M2). One-slot channel,
   stamp current, profile changed to stub key B, `queue_full_reindex(true)`,
   then a non-forced `ingest_document_virtual` on a deferred row.
   Expected: the row stays `pending_reindex`, the sweep's forced job
   runs, `embed_key = B`. Bad: the row is `indexed` with key A (the
   regression this fix prevents).
3. **Pre-V28 RO gate** (r28-M3). A V27 fixture brain, `ct wisdom match
   --two-stage` without ever opening RW. Expected: exit 0, v1 label,
   audit `stale_embed_key = candidate count`. Bad: exit 1.
4. **All-skip-class census** (r28-m1). Count candidate docs whose
   chunks are all skip-class, split by `embed_key` state. Expected: 0.
   Non-zero with a stale key means the gate currently falls back for no
   scoring reason.
5. **Instr1 merge parity** (r28-m2). Same probe under instr1: the
   v1-merge score for a `user_stated` fact must equal
   `wisdom_match_in_scheme`'s score for it, bit-identical. A mismatch
   means the merge is scoring raw against instr1 blobs.

## Out of scope

- Text-match chunk remap (follow-up work, own test list entry).
- Provenance backfill unless the hop audit shows gaps.
- PR #270 re-litigation — ships as-is per Kurt's ruling.
- Pass/grant machinery of every kind (Option A): no pass-docs table, no
  grants, no completion evaluator, no pass-aware sweep predicates, no
  epoch guard, no refusal records, no pass-progress UI fields — a
  model swap is the existing forced re-embed tools (§6).
- Pre-existing issues to file separately (handoff open item 4): bundle
  export drops `superseded_by`/`valid_to`; CWD-relative `exists()` skip in
  `bulk_reindex`/`queue_full_reindex`; reconcile CWD-dependence of
  duplicate path-shape rows; chunker-drift warning (any chunker-order
  change silently orphans ALL evidence hashes — `chunk_hash.rs:5-11`).

## Open questions for Kurt

*(None. Acceptance pinned 2026-10-09: pass = not worse than single-stage
in the same paired run; absolute performance deferred until the issue
backlog clears. Coverage threshold pinned at 100% in rev 26 under the
Option A ruling; revisit only if the paired run shows legitimate
coverage churn — a one-line spec change.)*

## Rulings carried

ONE PR total · ops crons BOTH DECLINED (no watchdogs; nothing alerts —
this is WHY the never-dark fallback is mandatory) · injection stays at the
curated layer · pinned 5-field stderr line untouched · PR #270 ships
as-is · curated-thoughts merges: regular merge commits only, no squash ·
**Option A — SIMPLIFY (Kurt, 2026-10-10): keep `documents.embed_key` +
diff-swap + breaker + funnel stamp; delete grant/pass-docs/model-guard
machinery; query-time coverage check in its place (§6).**
