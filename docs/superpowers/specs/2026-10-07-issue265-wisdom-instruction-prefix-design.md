# Wisdom-gate instruction prefix: both-side Qwen3 conditioning + scheme cutover (issue #265 follow-up)

**Date:** 2026-10-07 (rev 2 — post GLM-5.3 spec-tier FIX_FIRST round; all 8 required fixes applied)
**Status:** PROPOSED
**Branch:** `feat/issue265-query-prefix-test`
**PR:** #270
**Parent spec:** `docs/superpowers/specs/2026-10-06-issue265-wisdom-match-design.md`
**Investigation (Step 0, converged):**
`docs/superpowers/specs/2026-10-07-issue265-wisdom-instruction-prefix-investigation.md`
(GLM-5.3 non-flash CONVERGE after one fix round; Opus conclusion endorsed
after two rounds; artifacts in
`docs/benchmarks/2026-10-07-wisdom-gate-prefix-2x2/`).

## Problem

The wisdom gate catches only 44% of relevant messages at FP ≤ 0.05
(floor 0.70) because `qwen/qwen3-embedding-4b` is instruction-aware and
CT embeds raw text on both sides. The 2×2 + controls established:
both-side prefixing (cell E) reaches **hit@2 0.57 at floor 0.64**
(paired bootstrap Δhit@2 +0.16, 95% CI [+0.03, +0.35],
P(Δ>0)=0.97); query-only is unproven (A-vs-B ns); wording is not
detected to matter (D-vs-E ±4.4pp); the floor is
**byte-exact-prefix-defining**; the doc-side prefix is OFF-LABEL
(model card) — live-corpus validation is a merge blocker below.

## Decision

Adopt **cell E**: neutral canonical instruction, BOTH sides. Prefix
string (BYTE-EXACT, hard-coded; `\n` literal newline, no space after
`Query:`, direct concatenation with the text):

```
Instruct: Given a web search query, retrieve relevant passages that answer the query
Query:
```

New floor: **0.64** under the new scheme key. `wiki_search`/
`wiki_context` queries are prefixed with the SAME string (option (a));
option (b) — a second raw blob column — is REJECTED, so its dual-write
pricing residual is moot by decision.

## Scheme architecture (rev 2: WRITE and READ are separate, by design)

There is ONE `embedding_blob` column; every reader must agree on the
scheme. Two coordinated controls replace the single-switch design
(GLM spec-tier MAJOR-1: a single switch cannot both lock reads to the
old scheme and accumulate prefixed writes during the window):

- **WRITE scheme** — flips to `instr1` AT DEPLOY and stays there.
  Drives: doc-side prefix in `embed_text_for_entry`, the per-row
  `embed_scheme` stamp, and the write-time parity text function.
  During the window, prefixed rows accumulate while raw rows persist.
- **READ scheme tuple** — `{floor, query-prefix mode, gate SELECT
  filter, wiki_search SELECT filter}`. All four members are DERIVED
  from one stored value (below) and flip TOGETHER at cutover. No code
  path may express a tuple member independently.

**Storage of the read scheme:** `meta` table (KV), key
`wisdom_active_scheme`, values `raw` | `instr1`, default `raw`.
Cutover = one atomic `UPDATE` executed by an admin command
(§Migration). Unknown value or a scheme with no registered floor →
**hard error, fail-closed** (no fallback to another scheme's floor).

**Floor key representation (pinned; GLM r2 + spec-tier MAJOR-3):**
`external:qwen/qwen3-embedding-4b:instr1`, floor `0.64`, added to
`WISDOM_GATE_FLOORS` alongside (not replacing) the existing raw key.
`gate_model_key` gains the scheme suffix when the read scheme is
`instr1`.

## Scope decisions

1. **One blob column, all readers share the scheme** — no gate-only
   prefix, no helper-based protection (Opus r2 MAJOR-1).
2. **`embed_scheme` column (V+1 migration, pinned NULL semantics —
   spec-tier MAJOR-2):** `TEXT NOT NULL DEFAULT 'raw'`; the migration
   BACKFILLS `'raw'` explicitly on every existing row, so the NULL
   class never exists and `!=`/`=` filters are plain two-valued SQL.
   `embedding_blob IS NULL` rows (never embedded) are scheme-agnostic:
   they are invisible to both readers and DO NOT block cutover; the
   normal sweep embeds them under the WRITE scheme and stamps
   `instr1`.
3. **Filters:** gate SELECT (`wisdom_match.rs:233-235`) and a NEW
   filter in the `wiki_graph` scoring query (`wiki_graph.rs:301`,
   none today) each add `AND embed_scheme = <read scheme>`.
   Mixed-scheme scoring is impossible by construction.
4. **Parity rewiring:** write-time parity (`commit.rs:1534-1538,
   1721-1725`) is COMPARISON-ONLY and compares against the WRITE-scheme
   text function (the same function that produced the embedded text);
   no path may rewrite a vector+stamp non-atomically — a plan-level
   assertion must confirm every blob-writing path (new entry, edit,
   sweep, any parity rewrite) stamps from the WRITE constant (GLM N2).
   Edits during the window re-embed under `instr1` implicitly via
   `embed_text_for_entry`. During the window parity follows WRITE
   while the gate follows READ — that split is the point of the
   two-control design.
5. **Deploy skew (footnote, stated):** CT's sidecar is the single
   writer; an old binary during sidecar restart could stamp `raw`
   into an `instr1`-world — bounded by the restart window, detected by
   the cutover pre-condition count, corrected by the sweep.

## Migration window semantics (all mechanisms pinned)

1. **Sweep gains a scheme-filtered mode:** re-embed rows WHERE
   `embedding_blob IS NOT NULL AND embed_scheme != 'instr1'`
   (two-valued; no NULL class exists). Per-row stamping makes crash
   resume trivial (crash between embed and stamp = one wasted API
   call; the row still reads its old scheme).
2. **During the window:** READ scheme stays `raw` (floor 0.70, raw
   queries, raw filter). Accepted degradation, bound: NEW entries
   (written `instr1`) are invisible to BOTH the gate and
   `wiki_search` until cutover. Window duration bound: one full-brain
   re-embed ≈ N_entries × ~0.6 s — hours, not days, for a personal
   brain (N ≈ 10²–10³); the `ct wisdom scheme` command reports the
   remaining raw count.
3. **Cutover:** admin command `ct wisdom scheme activate instr1`
   (spec-tier MAJOR-4: actuation is a DB meta UPDATE, not a rebuild):
   - Precondition check: raw non-null-blob count = 0; refuses
     otherwise and prints the count.
   - Effect: atomic meta flip → floor 0.64, prefixed queries at both
     call sites, `instr1` filters at both SELECTs.
   - Executor: Tessera (or Kurt manually); the command is idempotent.
4. **Rollback after cutover (mechanism + cost, stated):** flip meta
   back to `raw` — but raw vectors are gone (in-place rewrite), so
   rollback costs a FULL re-embed back to raw plus a second degraded
   window (route is deterministic, so fidelity is preserved). Rollback
   is therefore a deliberate, owner-executed procedure, not an
   instant revert; the tripwire below is what triggers it.
   - **WRITE side of rollback:** the WRITE scheme must flip back to
     `raw` BEFORE any raw re-embed runs — the re-embed must stamp and
     text-function under `raw` (via `doc_text_for_entry`), never keep
     writing `instr1` while raw vectors accumulate. Order is: flip
     READ meta to `raw`, flip WRITE to `raw`, then sweep raw. A
     re-embed under a stale `instr1` WRITE would re-mix schemes and
     reproduce the exact corruption rollback exists to undo.
   - **Mechanism gap (follow-up, not this PR):** `WRITE_SCHEME` is
     currently a compile-time constant
     (`src-tauri/src/embed_scheme.rs`, `pub const WRITE_SCHEME`), so
     the WRITE flip cannot be actuated at runtime — every writer
     (deposit, commit, sweep, parity) reads the constant. Rollback
     today therefore requires a code change + rebuild between the
     meta flip and the re-embed, and the tripwire cron can only
     perform the READ half. Follow-up: make the WRITE scheme a
     second stored control (or derive it from the same meta row with
     a separate write-path resolution) so the owner/cron rollback
     procedure is a runtime operation like cutover.

## Merge blockers (all four restated in testable form)

1. **Live paired calibration (pre-sweep, scratch-brain copy — pinned
   sequencing, spec-tier blocker-1 gap):** raw blobs still exist
   there, so both cells are computable. Run cell A (raw, floor 0.70)
   vs cell E (floor 0.64) on the SAME labelled live probes.
   **Labelling (spec-level, not plan-level):** probes = 150 real
   messages sampled from recent session logs; GLM-5.3 pre-labels
   relevant/irrelevant; KURT spot-validates ≥ 30 (owner: Tessera
   prepares, Kurt validates). **Pass only if E hit@2 ≥ A hit@2
   (paired) AND E FP ≤ 0.05, using the E prefix string only**
   (GLM N3 wording fix). Fail → no merge.
2. **Migration completeness:** every mechanism in §Migration
   implemented AND covered by tests — including the coupling and
   fail-closed tests below. No prose-only answers.
3. **Monitoring tripwire (form pinned NOW; number lands from
   blocker 1):** CT gains a local gate-decision log line (one line per
   `wisdom match` call: timestamp, scheme, open/closed, n_results) —
   this instrumentation is IN SCOPE for this PR (uncosted
   instrumentation was the reviewer's objection). Tripwire FORM:
   rolling 24 h open-rate < 0.5 × baseline → cron-owned check alerts
   Kurt AND flips `wisdom_active_scheme` back to `raw` (fail-safe
   direction: gate abstains more, never bluffs). Baseline definition:
   the calibration-run (blocker 1) cell-E open-rate is the baseline
   UNTIL 7 post-cutover days accrue, after which the rolling 7-day
   post-cutover figure takes over (GLM N1: a pre-cutover run cannot
   record a post-cutover window). Roles harmonized (GLM N1): the cron
   flips the meta (cheap; the gate then abstains), KURT owns any
   restoration re-embed. Rollback owner: Tessera cron + Kurt.
4. **Provider-drift canary (operationalized):** 32 fixed canary texts
   frozen at calibration time (committed with the fixtures). Weekly
   cron re-embeds them via OpenRouter and compares against frozen
   canary vectors (frozen vectors computed under the `instr1` text
   function — prefixed — so like compares with like; GLM N4): FAIL if
   mean cosine < 0.995 or any single cosine <
   0.98 (tolerance justified: the observed `input_type`-class
   perturbation is ~0.9999; genuine provider-side model drift moves
   vectors far more). Failure action: alert Kurt + the gate FAILS
   CLOSED (scheme lookup treats the canary-failed scheme as unknown)
   until recalibration. This canary is the only guard on the floor's
   validity — the frozen-vector bench never re-embeds.

## Implementation outline (branch tasks; Rust tasks SERIAL)

1. V+1 migration: `embed_scheme TEXT NOT NULL DEFAULT 'raw'` +
   backfill; `meta` key `wisdom_active_scheme` = `raw`.
2. Scheme module: byte-exact prefix constant, WRITE scheme constant
   (`instr1`), read-scheme resolution from meta (fail-closed),
   floor-key derivation.
3. `embed_text_for_entry` + both query call sites apply prefixes per
   WRITE/READ respectively; both SELECTs filter; parity rewiring.
4. Scheme-filtered sweep mode + `ct wisdom scheme` admin command
   (status / activate with precondition).
5. `WISDOM_GATE_FLOORS` new key, floor 0.64; recalibrate + commit new
   `expected.json` + `vectors.json.gz` together (calibrator
   `--query-prefix`/`--doc-prefix` flags — already prototyped on this
   branch — land committed).
6. Tests (non-gated, always run):
   - (a) production prefix constant == the BYTE-EXACT literal above
     (hard-coded in the test, `Query:` no trailing space, direct
     concatenation) AND == snapshot `query_prefix`/`doc_prefix` keys
     (spec-tier §5: snapshot-parity alone is insufficient — a
     re-stamped snapshot must not launder a constant change).
   - (b) mechanical scheme-filter test: seed a DB with rows stamped
     `raw` and `instr1`; under read=`instr1` assert raw rows are
     absent from gate AND wiki_search scored candidates (and vice
     versa).
   - (c) parity uses the same text function as the write path.
   - (d) COUPLING test (the worst-failure-mode test): for each
     expressible read scheme, assert floor lookup, SELECT filter
     value, and query-prefix mode all derive from the same stored
     value — run the gate over the dual-stamped fixture under both
     schemes and assert (raw → 0.70/raw rows/unprefixed) and
     (instr1 → 0.64/instr1 rows/prefixed); plus unknown scheme → hard
     error.
     - (e) reader inventory (GLM optional hardening, accepted): a test
     that greps the source for `embedding_blob` SELECT sites and
     asserts the only scoring readers are the two filtered ones
     (gate + wiki_graph) — a hypothetical third reader cannot
     silently mix schemes.
7. Local wisdom-gate bench run, output pasted in the PR (CI cannot run
   slow-tests — investigation M3); optional CI slow-tests job,
   non-blocking.
8. Latency re-measure after cutover on the ThinkPad (p95 ≤ 1.5 s
   recipe, PR266 closure).

## Non-goals

- No probe-set expansion in this PR (tracked follow-up; the tripwire
  is the interim FP guard).
- No change to `recall_chunks`/`semantic_search` (separate
  `embeddings` table — verified unaffected).
- No retry/re-rank logic — the floor still abstains per rule 7.
