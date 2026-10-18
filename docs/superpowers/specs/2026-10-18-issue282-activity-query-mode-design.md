# Activity query mode for the wisdom gate: paths and commands reach concept rules (issue #282)

**Date:** 2026-10-18 (rev 14 — FINAL of the review ladder: rev 1 step-0 Fable memo +
Kurt's rulings; rev 2 Tier-2 GLM 5.3; rev 3 Tier-3 Opus; rev 4-5 Tier-3 delta
convergence; rev 6-13 Tier-4 gate rounds 1-8; rev 14 folds the gate's two closing
nits).
**Status:** Tier-4 Fable gate: **Approved with nits at rev 13** (round 8: zero
blockers, zero majors; nits folded in rev 14 — no further review round needed, the
nits are evidence-citation fixes, not design). PRESENTED TO KURT for implementation
approval.
**Branch:** `spec/issue282-activity-query-mode`
**Priority:** High — the integrations' autonomous-work delivery (plugin 0.7.x) is live
and shipping the bare target to the message path until this lands.
**Issue:** equationalapplications/curated-thoughts#282
**Step-0 guidance (Kurt-authorized non-gate Fable dispatch, 2026-10-18, $4.20):**
equational-wiki vault `records/investigations/2026-10-18-issue282-fable-step0-guidance/memo.md`.
**Supremacy:** where the Fable memo disagrees with THIS spec, this spec is canonical
(the memo is advisory input; the ladder + Kurt own the calls). Where Kurt's three
2026-10-18 rulings disagree with the memo, the rulings win — they are folded below.
**Scheme-of-record correction (rev 3, Opus B1):** the LIVE brain reads scheme `raw` —
shipped baseline `external:qwen/qwen3-embedding-4b`, raw, floor 0.70 (benchmark doc
lines 16-17), production composite row
`external:qwen/qwen3-embedding-4b:two-stage:chunk-hop` = 0.65 (`wisdom_match.rs:79`),
k "measured under scheme raw". The A6 evidence in the issue was collected on an
`instr1`-scheme experimental brain (floor 0.64) — DIRECTIONAL only, a different scheme
from production. Both facts are recorded; neither invalidates the other; all
production floors in this spec are RAW.

## Revision history (compressed)

- **rev 14 (this rev):** Tier-4 re-gate round 8: **APPROVE WITH NITS** ($2.52; zero
  blockers, zero MAJORS; every rev-13 fold verified against the code, including
  `WisdomMatch`/`TwoStageOutcome` `PartialEq` derivability and the non-degenerate
  `!ran`-on-stub comparison). Nits folded: **m1** — the "twelve test files"
  pattern citation corrected (zero `tools/tests` files import
  `curated_thoughts_tools::queries`; twelve import `tauri_app_lib::` directly,
  five import `curated_thoughts_tools::`; this is the FIRST `queries` consumer,
  through the existing `pub mod queries`). **m2** — the helper SIGNATURE PIN
  extended with its INPUTS: `conn: &Connection`, the resolved key, `read_scheme`,
  `max`, `exclude`, `now_ms`, the two vectors, the stage selection, `json_mode`;
  `wisdom_match_cmd` stays the only place that opens the brain and reads the
  clock, so the equality test passes byte-identical inputs to both sides.
- **rev 13:** Tier-4 re-gate round 7 (REQUEST CHANGES, zero blockers;
  $2.23; full review `~/.local/state/tessera/issue282/tier4-delta7-review.md` —
  all rev-12 folds verified, the m1 proxy confirmed DISCRIMINATING). Folded:
  **M1** — the two aperture test bullets move to the `src-tauri` `mod tests` list
  as lib assertions (with the stub composite forcing cell 4: no non-`user_stated`
  entry in `run.result.entries`; a `user_stated` entry ranked below two filtered
  entries present with `max` < pre-filter count; corrections of any provenance
  present; `run.outcome.open` false when every opened fact is filtered); ONE CLI
  bullet kept — on the fallback, provenance is unfiltered (byte-identical to the
  reference, covered by the parity test). **M2** — §8 step 4 pins the helper
  signature: it RETURNS the `WisdomMatch` it printed plus the
  `Option<TwoStageOutcome>` for the audit line (print + return, not
  `Result<i32>`-only), so the helper test compares the returned `WisdomMatch`
  against the direct call (`wisdom_match_in_scheme` for the no-flag arm;
  `wisdom_match_two_stage_with_v1` for the `--two-stage` arm — which under the
  production table takes `!ran` on `stub:constant8`, a non-degenerate but
  exercisable result). **m1** — the helper test reaches the `pub` helper via
  `curated_thoughts_tools::queries` from a `tools/tests` file (the pattern twelve
  existing test files use). **m2** — the closing CLI line reworded: the CLI
  asserts the fallback's `gate` carries NO `:activity`; the active-path label is
  the lib test. **m3** — the new aperture parameter is NAMED `provenance_filter`
  (a second `Option` parameter beside the existing `id_filter`; all four call
  sites pass `None` except rule (ii)). **m4** — the proxy note: `gate ==
  GATE_UNCALIBRATED` alone proves `floor == None`; an un-threaded `QueryKind`
  would yield `semantic-v1:stub:constant8` via the stub v1 row.
- **rev 12:** Tier-4 re-gate round 6 (REQUEST CHANGES, zero blockers;
  $2.46; full review `~/.local/state/tessera/issue282/tier4-delta6-review.md` —
  four of five rev-11 folds verified faithful; the M1 fold fixed the CLI hole but
  reintroduced the same class on the lib side across a CRATE BOUNDARY: the
  composite stub is visible only inside `src-tauri`'s test build, while the embed
  step and the §8-step-4 helper live in `tools`, so "forced cell 4 + helper output
  equality + embed count" cannot share one test). Folded — **option 1, split by
  crate** (Fable-recommended, matches the rev-5 m4 helper design): the
  post-embed-degradation LIB test moves to `src-tauri/src/wisdom_match.rs`
  `mod tests` and asserts (a) `outcome.rule.is_none()` on the forced-then-degraded
  cell-4 exits and (b) the attempt's `TwoStageRun.result` carries the `None`-arm
  signature — `gate == GATE_UNCALIBRATED`, empty entries — which IS the
  "never enters `gated_entries`" observable (rev-12 m1 names the proxy). The
  tools-side HELPER test calls the helper with pre-computed vectors and asserts
  its output equals the direct message call on the same vectors — needing NO
  cell-4 projection. The embed-call COUNT assertion is DROPPED: a helper whose
  signature takes pre-computed vectors cannot embed, so there is nothing to
  count. §Testing's heading claim corrected: the attribution, active-path-label,
  and degradation tests are `src-tauri` `mod tests` additions, NOT
  `tools/tests/ct_wisdom_match.rs` (rev-12 M1). **m2** — the active-path label
  test is stated as the orchestrator's `result.gate` in a lib test (existing
  pattern `:2161`); the JSON `gate` claim is limited to the CLI-reachable
  fallback case.
- **rev 11:** Tier-4 re-gate round 5 (REQUEST CHANGES, zero blockers;
  $2.24; full review `~/.local/state/tessera/issue282/tier4-delta5-review.md` — all
  five rev-10 folds verified holding). Folded: **M1** — the post-embed-degradation
  TEST bullet promised CLI-observable stderr evidence that no build can produce
  (the composite stub lives in `#[cfg(test)] WISDOM_GATE_FLOORS_TEST`, compiled
  out of the `ct` binary; a lib test cannot capture `eprintln!`). Split and
  re-mechanismed: (a) LIB test asserts `outcome.rule.is_none()` on reachable
  post-embed exits, that the attempt's `TwoStageRun.result` is discarded (fallback
  output equals the direct message call on the same vectors) and that the reused
  vector was consumed (stub embed-call count); (b) the stderr pins (exactly-once
  warning, no audit line no-flag) are verified BY CONSTRUCTION — single print site
  in the helper (rev-5 m4) — plus the lib-level assertion that the attempt never
  enters `gated_entries`; the CLI-level stderr-count assertions are DROPPED (the
  env-gated-floor alternative was rejected: it would fight the deliberate
  `wisdom_match.rs:109-126` pin). **m1** — the `:717` exit is dropped from the
  post-embed claim (it cannot fire after a cell-4 projection — the same lookups
  that project cell 4 prove `ts_floor` present); it stays in the §4 row as a
  defensive catch, and key-drift is covered by the attribution test. **m2** —
  recorded deviation: in the no-flag + message-v1-floor-absent configuration an
  embedder failure exits 1 where the reference exits 0 — unreachable in production
  (the message row always exists) and under the stub. **m3** — `--max 0` joins the
  empty-target row (same embed short-circuit class; without it an active no-flag
  `--max 0` call would print an audit line + two-stage label with zero entries —
  a pointless divergence). **m4** — "call shapes undisturbed" corrected: all four
  `gated_entries` call sites change to pass `None`; only the rule-(ii) site passes
  the predicate.
- **rev 10:** Tier-4 re-gate round 4 (REQUEST CHANGES, zero blockers;
  $2.40; full review `~/.local/state/tessera/issue282/tier4-delta4-review.md` —
  all six rev-9 folds verified holding). Folded: **M1** — the degradation row's
  mechanism was `--two-stage`-only wording; applied to the NO-FLAG call (the
  integrations' exact shape) it would print an audit line and a two-stage label
  where the pinned reference (`ct wisdom match -- X`, the v1 path) prints neither.
  Fixed: the helper re-runs the MESSAGE PATH THE STAGE FLAGS SELECT — no flag →
  message v1 (`wisdom_match_in_scheme`, message key, `raw_vec` reused as the
  schemed vector; the helper's v1 arm takes the vector as given, never
  re-evaluates the embed-skip, never calls `embed_one`; safe even with the message
  floor absent — `wisdom_match_impl` returns no entries for `floor = None`
  regardless of vector); `--two-stage` → the message two-stage orchestrator on the
  reused vectors. The `gated_entries` pin is SCOPED: never entered BY THE ACTIVITY
  ATTEMPT; the fallback's message call enters it exactly as the reference does.
  **m1** — the `:activity` suffix applies in BOTH arms of
  `match_with_resolved_floor` (the temporal-scheme-`None` arm resolves
  `gate_floor(gate_key)` unsuffixed today); `QueryKind::Activity` never resolves a
  floor at site 4, regardless of the coverage probe. **m2** — the aperture
  mechanism NAMED: a filter parameter on `gated_entries`, defaulting to no filter
  (message mode and the merge untouched; the `:363`/`:818`/`:884` call shapes
  undisturbed). **m3** — the `None`-arm citation corrected to `:374` (the `match
  floor` head is `:358`). **m4** — the degradation test covers ALL THREE
  `rule: None` exits, with the in-core coverage re-probe made reproducible by
  inserting a stale candidate row between the precheck and the run.
- **rev 9:** Tier-4 re-gate round 3 (REQUEST CHANGES, zero blockers; $2.73;
  full review `~/.local/state/tessera/issue282/tier4-delta3-review.md` — all six
  rev-8 folds plus every standing code claim verified in a table). Folded:
  **M1** — the `!ran` question is answered, option (b) PINNED: `QueryKind::Activity`
  is passed into the orchestrator's `!ran` `wisdom_match_in_scheme` call, so site 4
  resolves `gate_floor("…:activity")` → `None`, `wisdom_match_impl` takes its
  `None` arm, and `gated_entries` is NEVER entered inside the activity attempt —
  no v1 scan, no per-row stderr warnings, no wasted scan. Pin added to §4 and
  §8 step 4: "no v1 scan runs inside the activity attempt; `gated_entries` is not
  entered on any activity fallback path"; the malformed-wiki-row case joins the
  §Testing degradation bullet as the reason the pin matters. **m1** — the
  post-aperture `top` claim is scoped to rule (ii) (the shipping and only activity
  cell); a future rule-(i) activity row must compute `top` from pre-aperture
  scores. **m2** — the degradation row covers ANY `rule: None` exit after the
  embed, including the in-core coverage re-probe. **m3** — "replay_two_stage
  un-ignored" reworded: a replay test FOR THE ACTIVITY COMPOSITE KEY, parallel to
  `replay_two_stage`, with its own freeze dir. **m4** — the zero-target clap
  error joins the test matrix. **m5** — §3.3 site 1 notes the CLI floor site is
  never reached with the activity key (v1 embed-skip only).
- **rev 8:** Tier-4 re-gate round 2 (REQUEST CHANGES, zero blockers; $2.74;
  full review `~/.local/state/tessera/issue282/tier4-delta2-review.md` — every rev-7
  fold verified holding, plus clap/provenance/stub claims independently confirmed).
  Folded: **M1** — §3.3 item 3 was NOT moot: `wisdom_match_two_stage_with_v1`
  resolves `v1_floor` (the merge's only input) AND builds the ran-path label —
  threading `QueryKind` there is precisely what forces `v1_floor = None` and yields
  the pinned label; resolving `v1_floor` from the message key is FORBIDDEN (it
  would run the merge on the message floor inside the activity attempt). Also
  pinned: in the post-embed-degradation fallback the activity attempt's own
  `TwoStageRun.result` (the orchestrator's `!ran` v1 pass on the UNSUFFIXED key) is
  DISCARDED, never printed — the helper re-runs the message two-stage orchestrator
  on the reused vectors. **m1** — cell numbering under the activity key: with no
  v1 `:activity` row, `v1_floor_present` is always false, so a missing composite
  projects CELL 2 (probe miss / cov<100% project cell 3); cell 1 is unreachable in
  activity mode. **m2** — the harness call site is the existing rule-(i) arm call,
  not an "activity cell" (none exists yet). **m3** — the aperture predicate enters
  the RULE ARM's scoring BEFORE any arm-internal `truncate(max)` (the combined-list
  truncation is too late for rule (i)); it reads `WisdomItem.provenance`. **m4** —
  `--query-kind` changes probe-set labeling/filtering and the per-kind breakdown
  ONLY — never per-kind floors (Kurt's one-floor ruling). **m5** — the grid-only
  instr1 composite key order pinned: `model:instr1:activity:two-stage:chunk-hop`.
- **rev 7:** Tier-4 re-gate round 1 (REQUEST CHANGES, ZERO blockers; $2.38;
  full review `~/.local/state/tessera/issue282/tier4-delta-review.md` — every rev-6
  fold verified holding against the code, item by item). Folded: **M1** — the
  empty-target TESTING bullet still carried the retracted rev-5 wording; replaced
  with byte-identical-to-reference (the two tests for `--two-stage --path=` could
  not both pass). **M2** — the named harness has NO hit@1 metric (`ArmMetrics`
  carries only `hit2`; `letter_verdict` gates on hit2; best-cell sorts by hit2;
  the artifact letter string is hard-coded hit2): `--query-kind` now also adds
  `hit1` to `ArmMetrics`, switches `letter_verdict` AND best-cell ordering to
  hit1 for activity cells, and writes the activity letter string into the
  artifact (hit2 kept as a recorded column; message-mode runs unchanged).
  **M3** — §8 steps 2 and 6 still pointed at the pre-rev-6 constructions (four
  sites; composite-key label) — updated to five sites and the v1-shaped label.
  **m1** — `Some(v1_floor)` is passed at THREE harness sites (`two_stage_entries`
  `:1512`, `chunk_hop_arm` `:1533`/`:1572`) — the `v1_floor = None` rule binds
  all three; the harness's direct `two_stage_entries` call site (`:1507`) is named
  in step 7 (the `QueryKind` signature change touches it). **m2** — history line
  aligned ("implies all three"). **m3** — §3.1 parenthetical un-conflated:
  `scheme_key` is `floor_key_for(profile_key, scheme)`; the LABEL is prefix + that
  key. **m4** — no "closed gate"/cheap phrasing for the empty two-stage case
  anywhere (the reference call still runs the precheck and probe — that IS
  today's behavior).
- **rev 6:** Tier-4 Fable gate (REQUEST CHANGES at rev 5; $3.46, 36 turns;
  full review `~/.local/state/tessera/issue282/tier4-fable-review.md`; all prior-tier
  fixes verified holding). Folded: **B1** — a FIFTH floor-key resolution site: the
  rule-attribution block inside `two_stage_entries` recomputes
  `scheme_key = floor_key_for(profile_key, scheme)` and picks the rule whose composite
  table value EQUALS the passed floor; an activity floor calibrated to anything but
  the message value would attribute NOTHING → default RestrictedCosine on a
  chunk-hop-calibrated floor, on the only active path. Fix: `two_stage_entries`
  (attribution block) is resolution site 5, `QueryKind` threaded; lib test with a
  stub activity value DIFFERENT from the message stub (0.40 vs 0.45) asserting
  `outcome.floor_key_used` byte-exactly `…:activity:two-stage:chunk-hop` and
  `rule == ChunkHop`; §3.1's "picks the composite up for free" dropped. **M1** —
  the pinned two-stage gate label was WRONG: the ran-path label is
  `{TWO_STAGE_LABEL_PREFIX}{scheme_key}` with the v1-shaped scheme key — NO composite
  segment. Pinned label corrected to
  `semantic-v2-two-stage:external:qwen/qwen3-embedding-4b:activity`; the composite
  key appears ONLY in the audit `floor=` token. **M2** — empty-target row reworded:
  immediate delegation to the message call with the same flags, NO activity
  precheck; that call's own empty-text handling applies (the two-stage path checks
  emptiness AFTER the pre-embed check today, so "before key resolution" was false).
  **M3** — harness fidelity: activity calibration cells run with `v1_floor = None`
  (no merge — matching production), the harness scoring applies the SAME
  `user_stated` pre-truncation aperture, and positives are drawn from `user_stated`
  facts (gated hit@1 reported on that subset; the aperture's cost reported
  separately); `--query-kind` implies all three (rev-7 m2 alignment). **m2** — audit `open` is post-aperture;
  `top` stays the rule's own decision variable (chunk score under rule ii — the
  aperture cannot move it). **m3** — `two_stage::coverage` has THREE unsuffixed
  call sites (`:727` inside `two_stage_entries`, `:1073`, `:1160`). **m4** — the
  fallback-parity test passes the target as ONE argv element after `--`. **m1/m5** —
  header rev corrected; the `--max 1` claim stays controller-verified (integrations
  repo outside the gate's context).
- **rev 5:** Tier-3 delta #2: **APPROVE WITH NITS** ($0.50; the rev-4
  fixes verified against the code — cell-4 projection `:1110`, probe-miss cells
  `:1090-1094`, `outcome.rule` guard `:1183`, `schemed = raw.clone()` under raw
  `queries.rs:910-911`, v1-merge dead without a v1 row `:1150`). Folded:
  **M1″** — §7 precondition rewritten to "a build containing the #287 floor rows
  AND this PR"; the harness selects arms explicitly, so no "two-stage deploy" wait.
  **m1** — rev-3 history entry carries an inline numbering-correction pointer.
  **m2** — §4 table gains the explicit `--single-stage`/kill-switch row (no
  activity attempt); §3.3 drops the wrong harness reason for the CLI flag.
  **m3** — `raw_vec` reuse pinned as RAW-only (a future instr1 row must re-embed).
  **m4** — implementation step names the helper refactor (message paths pulled
  out, printing in one place) so the byte-identical-stderr test has a single
  print site. **m5** — the expected no-flag stderr difference (fallback: no audit
  line; active: audit line) is pinned and both CLI cases are tested.
- **rev 4:** Tier-3 delta re-review (REQUEST CHANGES; full review
  `~/.local/state/tessera/issue282/tier3-delta-review.md`, $0.57). Fixes:
  **B1′** — the healthy cell is CELL 4, not cell 1 (cell 1 is "ts floor missing →
  v1 fallback", `wisdom_match.rs:715/:1163`; cell 4 is "ts present + cov 100% →
  two-stage runs", `:741/:1110/:1167`). Active predicate pinned code-anchored:
  BEFORE embedding `pre.projected_cell == 4`; AFTER the run
  `run.outcome.rule.is_some()` (because `candidate_set` returning `None` — schema
  skew after the coverage probe, `:742-751` — can degrade the run post-embed); the
  after-run fallback REUSES `raw_vec` (under raw the vector is identical — no
  second embed); the abandoned activity attempt prints NOTHING (the audit line the
  fallback's message two-stage call prints is the only one on stderr), keeping
  stderr byte-identical. **M1′** — false premise removed: `Auto` is STILL v1 in
  main (`queries.rs:852`; #287 flipped the chunk-hop floor row, NOT the stage
  default); the integrations get v1 today; the letter baseline is PINNED to
  `ct wisdom match --max 1 -- X` (no stage flag = v1); recorded that the default
  activity path therefore changes BOTH arm and floor (activity two-stage + aperture
  vs message v1) — a mode-vs-mode comparison, stated as such. **M2′** — the v1
  arm is dropped as an ACTIVE path: no v1 `:activity` production row, no v1
  activity label, no v1 active-path test; v1 `:activity` survives only as the
  §7 harness ablation cell; re-admitting it requires a spec rev with its own
  never-worse rule. **m1** — "folded into the key chain in one place" reworded to
  the QueryKind threading. **m2** — the `--max 1` claim was verified by the
  controller directly (grep of `ct_wisdom_live.py`: argv at line 125 is
  `[ct_path, "wisdom", "match", "--json", "--max", str(max_n)]`; the delivery doc
  line 20 says "start `ct wisdom match --max 1`"), now cited in §3.1.
  **m3** — framing sentence added (§2): under raw, activity mode IS message
  two-stage with a different composite floor + the aperture; the separate
  typed-query floor is the whole bet; the honesty clause covers its failure.
- **rev 3:** Tier-3 Opus review (REQUEST CHANGES; full review
  `~/.local/state/tessera/issue282/tier3-opus-review.md`, $0.89). Fixes:
  **B1** — scheme of record is RAW (see header block): activity floor keys are
  raw-keyed (`external:qwen/qwen3-embedding-4b:activity`,
  `external:qwen/qwen3-embedding-4b:activity:two-stage:chunk-hop`); instr1 activity
  rows demote to grid cells; the activity instruction constant is NOT landed in this
  PR (it moves to the calibration harness — §2.1, a recorded deviation from issue
  request 2). **B2** — activity mode occupies ONLY the healthy two-stage path
  (cell 1; numbering corrected in rev 4: cell 4); every degraded state (uncalibrated
  row, coverage <100%, probe miss, the other cells) falls back to the message-mode
  invocation with the SAME stage flags —
  the never-worse guarantee now holds by construction. **M1/m1** — honest statement
  that under chunk-hop the open/close decision keys on the RAW vector and CHUNK
  scores; any query-side instruction only reorders already-open facts and feeds the
  v1-merge; the instructed-passages asymmetry is recorded; stage-1-prefixed vector =
  optional exploratory cell (new embed plan, NOT required for the letter). **M2** —
  mooted by B2: the v1-merge never runs in activity mode; recorded as a deliberate
  deviation from message-mode shape parity. **M3** — floor-key resolution sites
  enumerated; a `QueryKind` parameter is threaded; `profile_key` stays unsuffixed
  for coverage and k. **M4** — the stub composite row lives in
  `WISDOM_GATE_FLOORS_TEST`, NOT the production table (preserves the deliberate pin
  at `wisdom_match.rs:109-126`); only the v1 stub `:activity` row lands in
  production. **M5** — aperture filters BEFORE truncation; audit `open`/`top` are
  post-aperture values. **M6** — fallback redefined per arm as "the message-mode
  invocation with the same stage flags"; the parity test reference changes
  accordingly. **m2** — code sites pinned by symbol, not line number. **m3** —
  `required = true` removal and the Match doc-comment update are stated. **m4** —
  empty-target short-circuit precedes key resolution and fallback. **m5** — the
  integrations pass `--max 1` (`ct_wisdom_live.py:125`; delivery doc "start
  `ct wisdom match --max 1`") — hit@1 is both the fetch and the delivery depth.
- **rev 2:** Tier-2 GLM 5.3 review (Changes requested — 5 Important, 8 Minor, all
  REAL, zero FP; full review `~/.local/state/tessera/issue282/tier2-glm-review.md`).
  Audit surface moved to the STDOUT gate label with per-arm strings pinned;
  explicit-flag precedence + fallback table; embed plan stated as message-mode
  parity; composite key form pinned; aperture joints enumerated; clap mechanism
  corrected to ONE required group; fetch/deliver depth corrected (rev 3 then fixed
  the depth itself to 1); k shared; free-text hint ban; `=` form as caller
  convention; empty-target pinned; stub rows enumerated (placement corrected by
  rev-3 M4); honesty clause; issue body canonical over the memo for A6 numbers (the
  memo's 0.596/0.644/0.583/0.898 do not match the issue's 0.549/0.645/0.588/0.897 —
  nobody "fixes" the spec toward the memo).
- **rev 1:** initial draft — typed flags on `ct wisdom match` (no new subcommand),
  raw target embeds, own floor suffix + calibration, never-worse fallback, aperture.

## 1. Problem

During autonomous work (tool calls touching paths and commands), the integrations ask
`ct wisdom match` for wisdom relevant to what the agent is doing right now. The query
is a bare activity target — `pnpm test`, `src/db/migrations/0042_add_users.sql`,
`.env.local` — and the gate keys relevance to user-question-shaped prose. The measured
result (A6 experiment, 39 queries, issue body): **concept-from-path never opens**. The
rule "never edit an applied migration; add a new migration instead" scores 0.549–0.590
against every migration-shaped query while staying above the floor (0.64 on the
experimental instr1 brain; the live raw gate is 0.70) when asked as a question
(0.897). Score ranges of relevant and near-miss targets overlap, so lowering the
message floor does not fix it and would raise the false-positive rate. The
integrations will not add query expansion on their side (by design — relevance is
CT's job), so CT must accept an activity-shaped query and still reach the concept rule.

## 2. Approach

Keep the #265 contract and machinery; add an **activity query mode** to the existing
`ct wisdom match` surface:

1. The caller states what the target IS (`--path=`, `--command=`, `--pattern=`)
   instead of sending prose.
2. On the raw scheme of record, the embed input is the bare target — BYTE-IDENTICAL
   to what message mode embeds for the same text. What changes is the FLOOR KEY the
   gate consults (an activity-keyed, activity-calibrated row) and the delivery
   aperture — NOT the embedding. There is no query-side instruction under raw
   (instr1 prefixes do not exist on this scheme), and no new instruction constant is
   landed in this PR (§2.1).
3. The gate gets its own floor rows and its own calibration, so the message floor and
   its probe set are never silently reused.
4. Until an activity floor row resolves AND the two-stage path is fully healthy, the
   behavior is **provably not worse than today**: the message-mode invocation with
   the same stage flags (§4) — exactly what the integrations see now.

Framing (rev-4 m3): under the raw scheme of record, an activity-active call IS a
message two-stage call with (a) a different composite floor row and (b) the
aperture — embeds byte-identical, stage-1 gating keyed only on chunk scores. The
separate typed-query floor is therefore THE WHOLE BET of this issue; §1's evidence
that "lowering the message floor does not fix it" is exactly why the floor must be
its own calibrated row, and the §7 honesty clause covers the case where the bet
fails.

### 2.1. The activity instruction is calibration-cell material (rev 3: B1, M1, m1)

The issue asks for activity mode to have "its own instruction" (request 2). Under the
raw scheme of record there IS no instruction on either side, so request 2 cannot ship
in production without an instr1 cutover — out of scope for this PR. Recorded
deviation, with the evidence that softens it: under the chunk-hop arm, the open/close
decision keys on the RAW query vector and CHUNK scores (stage-1 uses the raw vector;
a `…:two-stage:chunk-hop` key "abstains on a CHUNK score … never on raw cosine"; rule
(ii) scores facts with the schemed vector at NO fact-level floor). So a query-side
activity instruction could only REORDER already-open facts and feed the v1-merge —
it does not move the open/close needle that defines this issue. The draft activity
instruction text from rev 1 ("Instruct: Given a code activity target (a file path,
shell command, or search pattern), retrieve the coding rules and facts that apply to
it\nQuery:") moves into the calibration HARNESS as the instr1 ABLATION CELL input,
together with the recorded asymmetry that under instr1 BOTH sides carry
`QUERY_INSTRUCTION_PREFIX` ("BOTH sides", cell E) — a code-target query instruction
would pair against web-search-instructed passages, contra the step-0 memo's "query
side only" claim. If that cell ever shows a real open-rate or ordering win, an
instr1 cutover (with its own message-mode recalibration) is the follow-up that
carries it; the constant lands THEN, with its calibration, never "fixed"
cosmetically.

## 3. Design

### 1. Contract surface (D1 — resolved: typed flags, no new subcommand)

On the existing `ct wisdom match` variant (the `Match` arm of the wisdom subcommand
in `tools/src/bin/ct.rs`):

- Three new long flags: `--path=<target>`, `--command=<target>`, `--pattern=<target>`.
  Callers MUST use the `=` form (mirroring the `--exclude=<id>` pin) so targets
  beginning with `-` stay unambiguous — clap parses `--path X` identically, so this
  is a CALLER CONVENTION, not an enforceable pin (rev-2 M-6). Targets may contain
  spaces and shell metacharacters and are passed through VERBATIM (no normalization,
  no quoting repair — literal preservation).
- Mutually exclusive, and each exclusive with the positional message: ONE clap
  `ArgGroup` named `activity_target` containing {the `text` positional, `--path`,
  `--command`, `--pattern`} with `required(true)` and `multiple(false)` — clap has
  no group-vs-arg conflict primitive, so the group OWNS the exactly-one rule
  (rev-2 M-1). Knock-on edits, stated (rev-3 m3): the `text` positional's per-arg
  `required = true` is REMOVED (the group satisfies the arg-required contract), and
  the Match doc comment ("The text goes after `--`") is updated to describe both
  forms so the `--help` capability probe the integrations run stays truthful.
- `--help` advertises all three flags — a NEW subcommand would break that probe
  (Fable D1).
- Everything else stays pinned: `--max` default 2 clamped 0..=10, `--exclude`
  repeatable with `parse_fact_id`, corrections always delivered as extra, JSON schema
  unchanged, **exit 0 on zero matches AND on any fallback; errors propagate (exit
  1)** — the #265 discipline (the exit site in `wisdom_match_cmd`,
  `tools/src/queries.rs`). Note the integrations call with `--max 1` (rev-4 m2, controller-verified:
  `ct_wisdom_live.py:125` builds argv
  `[ct_path, "wisdom", "match", "--json", "--max", str(max_n)]` and the delivery
  doc line 20 says "start `ct wisdom match --max 1`") — fetch depth 1, deliver
  depth 1.
- Audit surface (rev-2 I-1, pinned per arm): the gate label lives on STDOUT — the
  JSON `gate` field and the human `gate:` line (`print_wisdom_match`). The
  four-field `wisdom_gate_decision` stderr line (timestamp, read_scheme,
  open|closed, count) is UNCHANGED — it carries no gate label today and gains none.
  Pinned label strings, byte-exact:
  - two-stage arm (the ONLY active arm — §3.3): the label is
    `{TWO_STAGE_LABEL_PREFIX}{scheme_key}` where `scheme_key` =
    `floor_key_for(profile_key, scheme)` (the v1-shaped key, NO composite
    segment; the prefix lives in the label construction, not the key), with the
    `:activity` suffix folded in via `QueryKind`: pinned byte-exact as
    `semantic-v2-two-stage:external:qwen/qwen3-embedding-4b:activity` (rev-6 M1;
    the rev-3..5 string with `:two-stage:chunk-hop` appended was WRONG — appending
    the composite at the label site would change the MESSAGE two-stage label too,
    a #271 contract break).
  - v1 arm: NOT an active arm in this PR (rev-4 M2′), so it has no activity label.
    If a future rev re-admits it, its label is `semantic-v1:{gate_key}:activity`,
    grafted at the `wisdom_match_impl` gate-key formatting site via `QueryKind`.
  The audit line's `floor=` token prints `floor_key_used` from the §3.3 site-5
  attribution fix — "for free" was FALSE and is dropped (rev-6 B1). NO new JSON
  field, NO `mode` field — the label is the audit surface (Fable D1). The activity target is never
  echoed to stderr beyond what the message path echoes of message text today.

### 2. What gets embedded (D2 — resolved: bare target, byte parity with message mode)

- Under the raw scheme of record, activity mode embeds the SAME text message mode
  embeds for the same input: the raw target, no prefix, either leg. Embed plan =
  message-mode parity exactly (the `RawAndSchemed` two-leg plan on two-stage; single
  raw embed on v1) — with scheme `raw` the schemed leg is identity, so there are no
  instruction asymmetries in production at all (rev-3 B1/M1 fix; the instruction
  material lives in §2.1's ablation cell).
- Component splits (`src`, `db`, `migrations` joined as a phrase) are a CALIBRATION
  GRID CELL, not the default (§7): the A6 experiment measured prose expansion as the
  WORST shape (4/7 vs 6/7 for bare), and `ct_wisdom_activity.py` proves the caller
  can only supply the bare target.
- The 2000-char `truncate_text` clamp applies to the target as today.
- ONE target per invocation — no batch (§ Out of scope).

### 3. Gate arm (D3 — resolved: healthy two-stage only, its own floor keys)

- Activity mode is ACTIVE only on the fully healthy two-stage path — CELL 4 of the
  #271 §4 decision table (rev-4 B1′: cell 1 is "ts floor missing → v1 fallback";
  cell 4 is "ts present + cov 100% → two-stage runs"). Code-anchored predicate:
  - BEFORE embedding: the `two_stage_pre_embed_check` projection returns
    `projected_cell == 4` (composite `:activity` row resolved, coverage 100%,
    probe HIT).
  - AFTER the run: `run.outcome.rule.is_some()` — because `candidate_set` can
    return `None` (schema skew after the coverage probe) and degrade the run
    AFTER the embed has happened.
  Any other state — uncalibrated row, coverage below 100%, probe miss, cells 1/2/3,
  or a post-embed degradation — is a FALLBACK (§4). The never-worse guarantee must
  hold in every state the core can reach, and the only states where
  activity-specific floors and the aperture are calibrated are healthy-path states.
- Stage selection: the explicit `--two-stage`/`--single-stage` flag wins SELECTION
  (parity with message mode). Absent a flag, an activity target prefers two-stage. Under rev 3 the arm matters
  mainly for which message-mode invocation the fallback mirrors (§4): a
  `--two-stage` fallback therefore runs the message two-stage path on its raw
  composite and prints the audit line — byte-identical to
  `ct wisdom match --two-stage -- <target>` (exactly the r29-m4 pin: a
  two-stage-requested path always prints the audit line, including when it
  degrades).
- Floor keys (rev-3 B1, raw-keyed; these strings go into `WISDOM_GATE_FLOORS` and
  the resolver must agree):
  - v1 arm: NO `:activity` row ships (rev-4 M2′ — v1 activity is never an active
    path, §3.3/§4; the key `external:qwen/qwen3-embedding-4b:activity` exists only
    as a §7 harness ablation cell).
  - two-stage arm (composite): 
    `external:qwen/qwen3-embedding-4b:activity:two-stage:chunk-hop` — the
    `:activity` suffix is folded into the scheme key BEFORE the composite suffix;
    the suffix-at-end form `…:two-stage:chunk-hop:activity` is FORBIDDEN (resolver
    and calibration table must never drift).
  - instr1 `:activity` rows (both arms) are GRID CELLS ONLY (§2.1, §7) — never
    shipped in `WISDOM_GATE_FLOORS` in this PR; the grid-only composite key order
    follows the same rule as production (rev-8 m5):
    `external:qwen/qwen3-embedding-4b:instr1:activity:two-stage:chunk-hop`
    (`floor_key_for` output, then `:activity`, then the composite suffix).
  - Same exact-find-then-abstain lookup in `gate_floor`; the message rows are NOT
    consulted for `:activity` keys (an activity query never opens on the message
    floor — that would silently re-create the uncalibrated-floor bug this mode
    exists to avoid).
- Key construction — resolution sites enumerated (rev-3 M3; a `QueryKind`
  {Message, Activity} parameter is threaded through all of them; `profile_key`
  itself is NEVER suffixed):
  1. the CLI cmd's floor resolution in `wisdom_match_cmd` (`tools/src/queries.rs`;
     NOTE rev-9 m5: that resolved key is consumed only by the v1 embed-skip check —
     on the activity path this site is never reached with the activity key, so
     nobody threads `QueryKind` into the v1 branch's skip check),
  2. `two_stage_pre_embed_check`,
  3. `wisdom_match_two_stage_with_v1` — the OPPOSITE of moot (rev-8 M1): it
     resolves `v1_floor` via `gate_floor(&scheme_key)` (the merge's only input)
     and builds the ran-path label from the same `scheme_key`. Threading
     `QueryKind` HERE is what makes `v1_floor = None` (no v1 `:activity` row →
     the merge is dead) and yields the pinned `:activity` label. Resolving
     `v1_floor` from the message key at this site is FORBIDDEN — it would run
     the v1-merge on the message floor inside the activity attempt,
  4. `match_with_resolved_floor` — in BOTH arms (rev-10 m1): the schemed arm and
     the temporal-scheme-`None` arm (which resolves `gate_floor(gate_key)`
     unsuffixed today via the pre-V27 degradation); `QueryKind::Activity` NEVER
     resolves a floor at this site, whatever the probe said,
  5. the RULE-ATTRIBUTION block inside `two_stage_entries` (rev-6 B1): it
     recomputes `scheme_key = floor_key_for(profile_key, scheme)` and picks the
     rule whose composite table value EQUALS the passed `ts_floor` — without
     `QueryKind` there, a calibrated activity floor (≠ the message value)
     attributes NOTHING, the tie-break default RestrictedCosine runs against a
     chunk-hop-calibrated floor, and `floor_key_used` (what the audit `floor=`
     token prints) degrades to `…:two-stage:unattributed`.
  The sites that must stay UNSUFFIXED: `two_stage::coverage` — ALL THREE call
  sites (inside `two_stage_entries`, in `two_stage_pre_embed_check`, and in
  `wisdom_match_two_stage_with_v1`) — and `two_stage_k`: k is SHARED with message
  mode, keyed on the unschemed profile key, and NOT re-calibrated for activity
  mode (rev-2 M-4; rev-6 m3).
- Stage-default honesty (rev-4 M1′): `WisdomStageMode::Auto => false` IN MAIN —
  #287 flipped the chunk-hop floor row, NOT the stage default; with no stage flag
  the integrations run plain v1 today. All `:activity` calibration numbers MUST
  come from a build containing the #287 floor rows AND this PR's code; the spec
  bans quoting message-mode probe numbers in any `:activity` row comment. Because
  the no-flag activity path prefers two-stage while the no-flag message path is
  v1, the default-path comparison changes BOTH arm and floor — the letter (§7)
  pins the baseline and states the comparison as mode-vs-mode, not
  floor-vs-floor.

### 4. Fallback semantics — the never-worse rule (rev-3 B2/M6)

**Fallback = the message-mode invocation with the SAME stage flags, on the same
target text, byte-identical** (stdout JSON, human lines, stderr, audit lines, exit
code). Per-state table:

| State | Behavior |
|---|---|
| composite `:activity` row ABSENT (projects CELL 2 under the activity key — `v1_floor_present` is always false there, so `(false, false)`; probe miss / cov<100% project CELL 3; CELL 1 is unreachable in activity mode) | message call, same flags |
| coverage < 100% or probe MISS (composite present; cells 2/3) | message call, same flags |
| projected_cell != 4 for any other reason | message call, same flags |
| post-embed degradation (ANY `rule: None` exit after the embed — `candidate_set` → `None`, the in-core coverage re-probe dropping below 100%, or `ts_floor` absent — all caught by `run.outcome.rule.is_some()`; the `ts_floor` exit is a defensive catch only, unreachable after a cell-4 projection, rev-11 m1) | message call, same flags, REUSING `raw_vec` (under raw the vector is identical — no second embed). MECHANISM PINNED (rev-9 M1, option b): `QueryKind::Activity` is passed into the orchestrator's `!ran` `wisdom_match_in_scheme` call, so site 4 resolves `gate_floor("…:activity")` → `None`, `wisdom_match_impl` takes its `None` arm (`:374`; the `match floor` head is `:358`), and `gated_entries` is NEVER entered inside the activity attempt — no v1 scan, no per-row "skipping unreadable wiki row" stderr warnings, no wasted scan. The activity attempt's own `TwoStageRun.result` is DISCARDED, never printed. The helper then re-runs the MESSAGE PATH THE STAGE FLAGS SELECT, on the reused vectors (rev-10 M1): NO FLAG → the message v1 call (`wisdom_match_in_scheme`, message key, `raw_vec` reused as the schemed vector — identical under raw; the helper's v1 arm takes the vector as given, never re-evaluates the embed-skip check, never calls `embed_one`; safe even when the message v1 floor is absent — `wisdom_match_impl` returns no entries for `floor = None` regardless of vector, matching the reference's empty-vector call); `--two-stage` → the message two-stage orchestrator on the reused raw/schemed vectors. `gated_entries` is never entered BY THE ACTIVITY ATTEMPT; the fallback's message call enters it exactly as the reference call does. RAW-ONLY shortcut: if a future rev ships instr1 activity rows, the fallback must RE-EMBED `query_text` — the reuse argument does not survive a scheme change |
| `--single-stage` selected, or kill-switch active | message call, same flags — NO activity attempt at all (the v1 arm is not an active path, §3.3; no `two_stage_pre_embed_check` is reached, so no cell logic applies) |
| empty target (`--path=`) OR `--max 0` | immediate delegation to the message call with the same flags — NO activity precheck; that call's own embed short-circuit applies (no embed on either arm; `--max 0` is the same class on both paths), exit 0 (rev-6 M2: on the two-stage path the empty check runs AFTER the pre-embed check today, so "before key resolution" was false; rev-11 m3 adds `--max 0` — without it an active no-flag `--max 0` call would print an audit line + two-stage label for zero deliverable entries) |

Audit-line suppression (rev-4 B1′): the ABANDONED activity attempt prints NOTHING —
the activity code path emits no `print_two_stage_audit` before delegating, so the
only audit line on stderr is the one the fallback's message two-stage call prints.
Without this pin stderr would not be byte-identical in the `--two-stage` fallback
case. Expected NO-FLAG asymmetry (rev-5 m5): the no-flag fallback (`-- X`) is the
v1 path and prints NO audit line, while a no-flag ACTIVE activity call (two-stage)
DOES print one — that difference is the mode-vs-mode comparison of §7, not a parity
break; the CLI tests pin BOTH cases.

- The fallback reference is per flags: `--path=X` alone falls back to
  `ct wisdom match -- X`; `--two-stage --path=X` falls back to
  `ct wisdom match --two-stage -- X` (message two-stage on the raw composite, audit
  line prints). This fixes the rev-2 contradiction where the table said "v1 arm"
  but the `--two-stage` row implied the two-stage message path (rev-3 M6).
- The `user_stated` aperture (§5) does NOT apply on the fallback: the fallback is
  today's behavior, all sources, by construction — "never worse" cuts both ways.
- The flags are always ACCEPTED (exit 0); callers may switch to the new mode before
  any floor row exists (the #271 mechanism-harmless pattern). Until calibration
  lands, every activity call is a fallback — and the fallback is behaviorally
  identical to today's shipped behavior.
- The v1-merge never runs in activity mode (rev-3 M2): the merge gates the
  complement on the message row (the unsuffixed scheme key), scoring the activity
  key against the message floor is forbidden (§3.3), and an activity-keyed merge
  floor is uncalibrated. Rather than silently disable the merge inside a running
  two-stage pass, activity mode simply never leaves cell 4 — a deliberate, recorded
  DEVIATION from message-mode shape parity. Extending activity mode to degraded
  cells with its own merge floor is future work, only if the calibration data shows
  the healthy path is not enough.

### 5. Aperture — `user_stated` facts only (Kurt ruling 3, 2026-10-18)

While activity mode is ACTIVE (healthy two-stage path, composite row resolved), the
delivered entries are filtered to provenance `user_stated`. Rationale (Kurt):
fail-closed, consistent with the integrations' philosophy — mid-task delivery must
protect the agent's signal-to-noise ratio, and a wrong fact pushed mid-task is worse
than a missed one. Widening the aperture to all sources requires (a) calibration
data showing activity FP stays at 0 across the labeled set, and (b) Kurt's explicit
sign-off — a one-line spec change, made THEN, not now.

Placement (rev-2 I-5 + rev-3 M5):
1. NOT inside the shared `entry_eligibility_where` fragment (it feeds v1
   `gated_entries`, stage-1 candidate SQL, AND the complement — touching it would
   change message mode).
2. The filter runs BEFORE truncation to `--max` — specifically (rev-8 m3, with
   the mechanism NAMED by rev-10 m2) as a NEW `Option` PARAMETER on
   `gated_entries` NAMED `provenance_filter` (rev-13 m3 — a SECOND parameter
   beside the existing `id_filter: Option<&[String]>`; Rust has no default
   arguments, so ALL FOUR call sites change to pass `None` —
   `:363`/`:818`/`:861`/`:884`; only the rule-(ii) site passes the predicate),
   so the predicate sits inside
   the arm's scoring BEFORE its arm-internal `truncate(max)`, reading the
   `WisdomItem.provenance` field. Filtering at the combined-list truncation is
   TOO LATE for rule (i) (its arm has already truncated), and filtering after any
   truncation costs hit@1 for no FP benefit — forbidden. A top-N made of
   non-`user_stated` facts must not shadow a `user_stated` fact ranked below it.
3. Stage-1 candidate DOCS stay unfiltered — chunks are not facts; the aperture
   constrains delivery, not retrieval.
4. Chunk-hop rule (ii) opens (evidence-hash join, no fact-level floor): the opened
   ids pass through the aperture too — a non-`user_stated` fact opened by rule (ii)
   is dropped, not delivered.
5. The v1-merge complement is moot in activity mode (the merge never runs, §4);
   recorded so a future cell-extension inherits the joint.
6. CORRECTIONS BYPASS the aperture as a stated RULE (not a test implication): the
   #265 corrections-always-delivered contract dominates ruling 3 — a superseding
   entry of any provenance still flows mid-task. And `user_confirmed` is EXCLUDED:
   the ruling says `user_stated` only.
7. Audit pin (rev-3 M5 + rev-6 m2): the two-stage audit `open=` is computed AFTER
   the aperture — `open` reflects what would actually be delivered
   (`TwoStageOutcome.open` keys on the final entries list), so a fully-filtered
   pass reports closed, not open-then-empty. `top` is NOT post-aperture under
   RULE (ii) — the shipping and only activity cell — where it stays the rule's
   own decision variable (the member-CHUNK score), which the aperture cannot
   move; only `open` changes. (rev-9 m1: rule (i)'s `top` folds from the
   post-`gated_entries` scored list, so a future rule-(i) activity row — none
   ships — must compute `top` from pre-aperture scores.)
Message mode and the fallback path are untouched.

### 6. Matching, ranking, corrections

- Scoring, dedupe, ordering, truncation to `--max`, and correction delivery are
  the existing machinery, unmodified (issue #265). Activity mode changes WHICH
  floor key and aperture apply — not the ranking rule.
- "Prefer actionable rules over command-echoing entries" (issue request 3) is NOT
  addressed by ranking logic in this spec; it is a calibration OUTCOME — the
  labeled set includes the pnpm-test pair (generic vs actionable) and the letter
  (§7) counts hit@1, which punishes the generic-entry win. If hit@1 passes only
  with a ranking tweak, that tweak comes back as a spec rev with evidence.

## 7. Calibration plan + acceptance letter (D4 — resolved)

- **Precondition (rev-5 M1″):** a build containing the #287 floor rows AND this
  PR (§3.3 stage-default honesty). The harness selects arms explicitly, so the
  deployed dpkg's stage default is irrelevant to calibration; waiting for a
  "two-stage deploy" would block forever, since this PR never produces one.
- **Harness fidelity (rev-6 M3):** activity calibration cells MUST measure the
  shape production ships — `v1_floor = None` (the merge never runs in activity
  mode, §4; the harness's default passes `Some(v1_floor)`, which would inflate
  hit@1 with merged complement facts production never delivers), the SAME
  `user_stated` pre-truncation aperture applied inside the harness scoring, and
  positives drawn from `user_stated` facts only — the gated hit@1 is reported on
  that subset, with the aperture's cost reported separately (a non-`user_stated`
  positive reachable by baseline v1 but never by activity mode must not read as
  "the floor bet failed"). `--query-kind` (step 7) implies ALL of this.
- **Labeled set:** positives GENERATED from the stored facts (a proposal model
  drafts plausible activity targets per fact — path, command, and pattern shapes;
  Opus or Kurt adjudicates), because 7 adjudicated message-mode probes must not be
  reused and hand-crafting 30 targets is the expensive half. Negatives are FREE:
  `git ls-files` paths from repos with no related fact, the `ct` subcommand table,
  near-miss siblings (`edit src/db/schema.ts`, `run pnpm build`, `read
  .github/workflows/ci.yml` — the A6 near-miss set). Targets: **≥30 adjudicated
  relevant targets, ≥150 negatives, covering all three kinds**; every fact used for
  positives stays fixed while targets vary (the Fable lesson: per-fact confounds
  hid the message-mode bottleneck). One brain, one model, recorded per the
  `calibrate_wisdom_gate` discipline (explicit `--profile`; OPENROUTER_API_KEY from
  `~/.brain/config.json` — the 2026-10-18 401 footgun).
- **Primary metric: hit@1** — the integrations fetch `--max 1` and deliver one fact
  per step (rev-3 m5), so hit@1 IS both the fetch and the delivery depth, and the
  generic-vs-actionable failure mode is a TOP-1 failure; hit@2 is recorded, not
  gated. HARNESS REQUIREMENT (rev-7 M2): the named harness today has NO hit@1 —
  `ArmMetrics` carries only `hit2`, `letter_verdict` gates on hit2, best-cell
  selection sorts by `two_stage.hit2`, and the artifact's letter string is
  hard-coded hit2. `--query-kind` therefore ALSO (a) adds `hit1` to `ArmMetrics`,
  (b) switches `letter_verdict` and best-cell ordering to hit1 for activity cells
  (a floor that wins on hit@2 but loses on hit@1 must not be selected), and
  (c) writes the activity letter string (`act_hit1 ≥ base_hit1 && act_fp ≤
  base_fp`) into the artifact, with hit2 kept as a recorded column. Message-mode
  runs keep hit2 unchanged. Baseline (rev-4 M1′, pinned): `ct wisdom match --max 1 -- <target>` —
  no stage flag, which on current main is the v1 path (today's shipped behavior).
  The comparison is therefore mode-vs-mode (activity two-stage + aperture vs
  message v1), not floor-vs-floor; a secondary v1-arm activity ablation (harness
  floors, not shipped rows) records how much of the delta is the arm change.
- **Letter (freeze before the run):** activity mode flips to its own floor row iff
  `act_hit1 ≥ base_hit1 && act_fp ≤ base_fp` — not worse on recall of the
  actionable rule, not worse on false opens — with FP counted per target over all
  negatives (any open on a negative is an FP). Floors are searched on the same
  grid discipline as #287 (`calibration-run/grid-*/` evidence dir, freeze snapshot
  committed, and a replay test FOR THE ACTIVITY COMPOSITE KEY, parallel to
  `replay_two_stage`, with its own freeze dir — rev-9 m3; the existing test is
  keyed to the single message `TWO_STAGE_GATE_KEY`).
- **Honesty clause:** the letter measures NOT-WORSE-THAN-BASELINE; it does NOT
  guarantee the issue's headline concept-from-path case is solved — the migration
  example may still stay closed while the letter passes. The concept-from-path bet
  is MEASURED by the labeled set (it contains migration-shaped positives); if the
  grid cannot open them at any floor without FP, that result becomes evidence for
  the scope-globs roadmap, not a spec failure. Numbers canonical source: the ISSUE
  body (the step-0 memo's quoted scores differ slightly — the issue wins).
- **Grid cells (minimum):** composite-floor sweep on the raw two-stage arm (the
  shipping cell); the v1 `:activity` arm as HARNESS-ONLY ablation (explicit
  floors; never a shipped row — rev-4 M2′); the instr1 activity
  instruction ablation (§2.1 — harness-side prefix text; also records the
  instructed-passages asymmetry); component-split input variant; an OPTIONAL
  exploratory cell with the stage-1 vector prefixed (requires a new embed plan —
  not required for the letter, only if the ablation shows instruction sensitivity).

## 8. Implementation order (within the ONE PR)

1. Clap surface: three flags + `activity_target` group + `text` `required` removal +
   doc-comment update (`ct.rs`), with usage-error tests.
2. `QueryKind` threading + key construction at the FIVE resolution sites (§3.3 —
   including the attribution block in `two_stage_entries`), with `profile_key`
   unsuffixed for coverage/k (`wisdom_match.rs`, `queries.rs`).
3. Composite stub row in `WISDOM_GATE_FLOORS_TEST` (rev-3 M4); NO v1 `:activity`
   row anywhere in production tables (rev-4 M2′).
4. Healthy-path-only activation (cell-4 predicate, §3.3) + the fallback rule (§4),
   including audit-line suppression, `raw_vec` reuse, and the rev-9/rev-10 M1
   mechanism (`QueryKind::Activity` through the `!ran` call — `gated_entries`
   never entered BY THE ACTIVITY ATTEMPT; the fallback's message call enters it
   exactly as the reference does, and routes by the stage flags: no flag →
   message v1, `--two-stage` → message two-stage). NAMED PREREQUISITE
   (rev-5 m4): the fallback is inline in `wisdom_match_cmd` today — embeds and
   prints in the same place — so this step FIRST extracts the message v1 and
   two-stage paths into a helper that takes pre-computed vectors and owns ALL
   printing; byte-identical stderr then has exactly one print site to test.
   SIGNATURE PIN (rev-13 M2, inputs added rev-14 m2): the helper takes
   `conn: &Connection`, the resolved key, `read_scheme`, `max`, `exclude`,
   `now_ms`, the two vectors, the stage selection and `json_mode` —
   `wisdom_match_cmd` stays the ONLY place that opens the brain and reads the
   clock — and RETURNS the `WisdomMatch` it printed plus the
   `Option<TwoStageOutcome>` for the audit line: print + return, NOT a bare
   `Result<i32>` (a unit test cannot read `println!` back; the helper test
   passes byte-identical inputs to helper and direct call and compares the
   returned `WisdomMatch` against the direct call:
   `wisdom_match_in_scheme` for the no-flag arm, `wisdom_match_two_stage_with_v1`
   for the `--two-stage` arm — which under the production table takes `!ran` on
   `stub:constant8`, a non-degenerate but exercisable result).
5. `user_stated` aperture pre-truncation filter (§5) on the activity-active path.
6. Gate label per §3.1 (v1-shaped scheme key + `:activity`, NO composite segment;
   no v1 activity label — v1 is not an active arm).
7. Calibration harness extension: `--query-kind` (message|path|command|pattern) —
   implying (rev-6 M3) `v1_floor = None` at ALL THREE harness pass-sites
   (`two_stage_entries` `:1512` AND `chunk_hop_arm` `:1533`/`:1572`; the shipping
   cell is chunk-hop, so `chunk_hop_arm` binds first), (rev-6 M3) the
   `user_stated` aperture in scoring with `user_stated`-only positives, and
   (rev-7 M2) the hit@1 metric/letter/ordering changes. KIND SEMANTICS (rev-8 m4):
   the kind changes probe-set labeling/filtering and the per-kind breakdown ONLY —
   never the floor key (Kurt's one-floor ruling: all three kinds resolve the same
   `:activity` key). Plus the instr1 instruction-ablation cell (harness-side
   prefix text). NAMED CALL SITE (rev-8 m2): the `QueryKind` signature change to
   `two_stage_entries` touches the harness's EXISTING rule-(i) arm call inside
   its per-probe loop — no activity cell exists in the harness yet.
8. Tests (§ Testing), docs, CHANGELOG.

## Error handling

- Clap usage errors (mixed targets, target + positional): clap's exit, same as
  `stage_sel` conflicts today. No new error kinds.
- Uncalibrated/unhealthy activity state: the fallback (§4) — never an error, never
  `EXIT_NO_RESULTS` (the #265 exit discipline, unchanged).
- Embed failure, RO-brain open failure: propagate (exit 1) exactly as message mode
  does today. RECORDED DEVIATION (rev-11 m2): in the no-flag configuration with
  the message v1 floor ABSENT, the reference skips its embed and exits 0 while an
  activity call that projected cell 4 embeds first and would exit 1 on an embedder
  failure — unreachable in production (the message v1 row always exists) and under
  the stub; accepted, not papered over.

## Testing

- Test placement (rev-12 M1): the CLI-surface tests below live in
  `tools/tests/ct_wisdom_match.rs`; the attribution test, the active-path label
  test, and the post-embed-degradation test are `src-tauri/src/wisdom_match.rs`
  `mod tests` additions (crate boundary — see the split-by-crate bullets).
  `tools/tests/ct_wisdom_match.rs` additions:
  - group exclusivity matrix (path+command, path+pattern, command+pattern,
    each + `-- text`): usage error.
  - fallback parity, per flags (rev-3 M6): `--path=X` output is byte-identical to
    `ct wisdom match -- X`; `--two-stage --path=X` is byte-identical to
    `ct wisdom match --two-stage -- X` (including the audit line); both exit 0;
    empty output iff the reference call is empty (rev-2 M-2 — never an
    unconditional "empty" assertion). The reference passes the target as ONE argv
    element after `--` (rev-6 m4: the positional is `Vec<String>` joined with a
    space; a multi-space target must not produce a false parity failure).
  - empty target (`--path=` / `--command=` / `--pattern=`): byte-identical to the
    reference message call with the same flags and `-- ""`; no embed on either arm
    (the v1 arm checks emptiness at its own early site; the two-stage arm's
    reference still runs the pre-embed check + coverage probe and prints its audit
    line — that IS today's behavior, and the spec never describes the empty
    two-stage case as cheap or "closed gate"); exit 0 (rev-7 M1, superseding the
    rev-3 m4 wording rev-6 M2 retracted).
  - `--help` advertises all three flags (integrations' capability probe).
  - on the FALLBACK, provenance is unfiltered — byte-identical to the reference
    (already covered by the parity test; rev-13 M1 keeps exactly this one CLI
    aperture bullet).
  - the ACTIVE path (composite, cell 4) is exercised through lib tests / the
    harness's explicit floors — the composite stub lives in
    `WISDOM_GATE_FLOORS_TEST`, and CLI tests can and must see the fallback fire
    for an uncalibrated composite (rev-3 M4); there is NO v1-activity row or
    active-path test (rev-4 M2′).
  - attribution test (rev-6 B1): with a stub activity composite value DIFFERENT
    from the message stub (e.g. 0.40 vs 0.45), the active path's
    `outcome.floor_key_used` is byte-exactly
    `stub:constant8:activity:two-stage:chunk-hop` and `rule == ChunkHop` —
    attribution exercised, not coincidental.
  - label test (rev-6 M1, restated rev-12 m2): a LIB test asserts the
    orchestrator's `result.gate` on the active path is exactly
    `semantic-v2-two-stage:{scheme_key}:activity`-shaped (prefix + scheme key +
    `:activity`, NO composite segment; existing pattern at
    `wisdom_match.rs:2161`); the MESSAGE two-stage label is unchanged. The JSON
    `gate` claim is limited to the CLI-reachable fallback case.
  - SPLIT BY CRATE (rev-12 M1 — the composite stub is visible only inside
    `src-tauri`'s test build; the helper lives in `tools`; one test cannot make
    all assertions):
    - `src-tauri/src/wisdom_match.rs` `mod tests` (NOT
      `tools/tests/ct_wisdom_match.rs`): with the stub composite forcing a cell-4
      projection then degraded (`candidate_set` → `None`; the in-core coverage
      re-probe, made reproducible by inserting a stale candidate row between
      precheck and run), assert `outcome.rule.is_none()` AND that the attempt's
      `TwoStageRun.result` carries the `None`-arm signature —
      `gate == GATE_UNCALIBRATED`, empty entries — which is the named proxy for
      "the attempt never enters `gated_entries`" (rev-12 m1: with a non-empty
      vector and `max > 0`, that signature is reachable ONLY through the `None`
      arm that skips `gated_entries`; alone it proves `floor == None` — an
      un-threaded `QueryKind` would yield `semantic-v1:stub:constant8` via the
      stub v1 row, so the proxy is discriminating, rev-13 m4).
    - APERTURE lib assertions (rev-13 M1 — the aperture is reachable only on the
      active path, which the `ct` binary can never enter), with the stub
      composite forcing cell 4: `run.result.entries` contains NO
      non-`user_stated` entry; a `user_stated` entry ranked below two filtered
      entries IS present with `max` smaller than the pre-filter count
      (pre-truncation order, rev-3 M5); corrections of any provenance are
      present; `run.outcome.open` is false when every opened fact is filtered.
    - `tools` HELPER test (rev-13 m1, citation corrected rev-14): reached via the
      `pub` helper from `curated_thoughts_tools::queries` in a `tools/tests` file
      — the lib-import pattern `tools/tests` already uses (twelve files import
      `tauri_app_lib::` directly, five import `curated_thoughts_tools::`; none
      imports `queries` yet, so this is the first `queries` consumer, through the
      existing `pub mod queries`): call it with pre-computed vectors and
      compare the RETURNED `WisdomMatch` against the direct call on the same
      vectors — no cell-4 projection needed. The embed-call COUNT assertion is
      dropped (a helper whose signature takes pre-computed vectors cannot embed —
      nothing to count).
  - stderr pins, BY CONSTRUCTION (rev-11 M1): exactly-once per-row warning and
    no-flag no-audit-line hold because the helper (rev-5 m4) is the SINGLE print
    site and the attempt has none — verified by the lib assertion above plus the
    helper's structure, not by CLI output capture.
  - zero-target case (`ct wisdom match` with no target at all): clap's
    group-required error, message pinned (rev-9 m4).
- One CLI test per JSON field: schema unchanged; the CLI asserts the fallback's
  `gate` carries NO `:activity` — the active-path label is the lib test
  (rev-13 m2).

## Out of scope (Kurt's rulings + Fable D5)

- **Batch form** (several activity lines per call) — DEFERRED. The integrations
  rate-limit to one query per model step and measured p95 is 1.22 s against the
  3 s budget. Revisit when the budget binds.
- **Fast path** (sidecar wisdom_match tool / warm daemon) — DEFERRED, same data.
- **Deposit-time scope globs** (curator tags a fact with `src/db/migrations/**`)
  — **ROADMAP (Kurt ruling 2: 2026-10-18)**: the ultimate deterministic fix for
  connecting repo rules to code boundaries (e.g. migration immutability), too
  much scope creep for #282. Needs its own issue with a deposit-UX design.
- **instr1 cutover for activity instructions** (§2.1) — only if the ablation cell
  earns it; carries its own message-mode recalibration.
- No free-text hint argument, EVER (rev-2 M-5) — targets are typed kinds only; a
  `--hint` would reintroduce the injection surface the typed targets exist to avoid.
- Any ranking heuristic change (§6), any aperture widening (§5), any new
  subcommand (§3.1), any probe-set reuse from message mode (§7).

## Open questions for Kurt

*(None — all three 2026-10-18 questions are ruled; recorded under Rulings carried.
The rev-3 scheme-of-record deviation (§2.1: instruction demoted to a calibration
cell; production = raw byte-parity embeds) is evidence-driven, not a Kurt question —
it is flagged here so he sees it at approval time. Remaining unknowns are EXPERIMENTS
(§7), not decisions.)*

## Rulings carried

- **One floor** (Kurt, 2026-10-18): a single shared `:activity` floor across
  path/command/pattern kinds; split per-kind only if the first calibration cell
  shows the score distributions diverge wildly.
- **Scope globs on the roadmap** (Kurt, 2026-10-18): deterministic glob matching
  of facts to code boundaries is the endgame fix; not in #282.
- **`user_stated`-only aperture at first** (Kurt, 2026-10-18): fail-closed,
  protect mid-task signal-to-noise; widen only on robust calibration evidence
  that FP stays at zero, with Kurt's sign-off.
- **Spec first, then implement** (Kurt, standing instruction for #282).
- **Own the PR** (Kurt, 2026-10-18): Tessera runs the ladder and the PR
  end-to-end; Kurt's spec approval remains the implementation gate.
- Carried from #271/#287: merge commits only, no squash; letter frozen before the
  calibration run; opt-in mechanism ships before any flip-to-default.
