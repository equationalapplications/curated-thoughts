# Activity query mode for the wisdom gate: paths and commands reach concept rules (issue #282)

**Date:** 2026-10-18 (rev 4 — rev 1 folded the step-0 Fable memo + Kurt's rulings;
rev 2 the Tier-2 GLM 5.3 review; rev 3 the Tier-3 Opus review; rev 4 the Tier-3 delta
re-review: 1 blocker + 2 majors + 3 minors).
**Status:** Draft — in Tier-3 convergence loop; then the Tier-4 Fable gate; Kurt's
explicit approval required before implementation.
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

- **rev 4 (this rev):** Tier-3 delta re-review (REQUEST CHANGES; full review
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
  (cell 1); every degraded state (uncalibrated row, coverage <100%, probe miss,
  cells 2/3/4) falls back to the message-mode invocation with the SAME stage flags —
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
  - two-stage arm (the ONLY active arm — §3.3):
    `semantic-v2-two-stage:{scheme_key}` with the scheme key carrying the activity
    suffix — e.g.
    `semantic-v2-two-stage:external:qwen/qwen3-embedding-4b:activity:two-stage:chunk-hop`.
  - v1 arm: NOT an active arm in this PR (rev-4 M2′), so it has no activity label.
    If a future rev re-admits it, its label is `semantic-v1:{gate_key}:activity`,
    grafted at the `wisdom_match_impl` gate-key formatting site via `QueryKind`.
  The two-stage audit line's `floor=` token picks the composite up for free once the
  suffix is folded in at the §3.3 resolution sites. NO new JSON field, NO `mode`
  field — the label is the audit surface (Fable D1). The activity target is never
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
  (parity with message mode; the calibration harness needs to walk both arms).
  Absent a flag, an activity target prefers two-stage. Under rev 3 the arm matters
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
    shipped in `WISDOM_GATE_FLOORS` in this PR.
  - Same exact-find-then-abstain lookup in `gate_floor`; the message rows are NOT
    consulted for `:activity` keys (an activity query never opens on the message
    floor — that would silently re-create the uncalibrated-floor bug this mode
    exists to avoid).
- Key construction — resolution sites enumerated (rev-3 M3; a `QueryKind`
  {Message, Activity} parameter is threaded through all of them; `profile_key`
  itself is NEVER suffixed):
  1. the CLI cmd's floor resolution in `wisdom_match_cmd` (`tools/src/queries.rs`),
  2. `two_stage_pre_embed_check`,
  3. `wisdom_match_two_stage_with_v1` (moot in activity mode — the merge never
     runs, §4 — but the parameter is threaded for type-honesty),
  4. `match_with_resolved_floor`.
  The sites that must stay UNSUFFIXED: `two_stage::coverage` (both call sites) and
  `two_stage_k` — k is SHARED with message mode, keyed on the unschemed profile
  key, and NOT re-calibrated for activity mode (rev-2 M-4).
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
| composite `:activity` row ABSENT (would-be cell 1) | message call, same flags |
| coverage < 100% or probe MISS (composite present; cells 2/3) | message call, same flags |
| projected_cell != 4 for any other reason | message call, same flags |
| post-embed degradation (`candidate_set` → `None`, `run.outcome.rule` absent) | message call, same flags, REUSING `raw_vec` (under raw the vector is identical — no second embed) |
| kill-switch active | message call with the v1 message floor |
| empty target (`--path=`) | TODAY'S empty-text short-circuit wins FIRST — closed gate, exit 0, no embed, before key resolution and before fallback (rev-3 m4) |

Audit-line suppression (rev-4 B1′): the ABANDONED activity attempt prints NOTHING —
the activity code path emits no `print_two_stage_audit` before delegating, so the
only audit line on stderr is the one the fallback's message two-stage call prints.
Without this pin stderr would not be byte-identical in the `--two-stage` fallback
case.

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
2. The filter runs BEFORE truncation to `--max` — a provenance predicate passed
   into the entry-selection/scoring layer (or an equivalent pre-truncation filter
   in the scoring loop), so a top-N made of non-`user_stated` facts does not shadow
   a `user_stated` fact ranked below it. Filtering after truncation costs hit@1 for
   no FP benefit and is forbidden.
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
7. Audit pin (rev-3 M5): the two-stage audit `open=` and the JSON gate semantics
   are computed AFTER the aperture — `open` reflects what would actually be
   delivered (`TwoStageOutcome.open` keys on the final entries list), so a
   fully-filtered pass reports closed, not open-then-empty.
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

- **Precondition:** the post-#287 two-stage build (deployed, not just merged).
  Calibration on the single-stage deployed dpkg is a wasted round-trip.
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
  gated. Baseline (rev-4 M1′, pinned): `ct wisdom match --max 1 -- <target>` —
  no stage flag, which on current main is the v1 path (today's shipped behavior).
  The comparison is therefore mode-vs-mode (activity two-stage + aperture vs
  message v1), not floor-vs-floor; a secondary v1-arm activity ablation (harness
  floors, not shipped rows) records how much of the delta is the arm change.
- **Letter (freeze before the run):** activity mode flips to its own floor row iff
  `act_hit1 ≥ base_hit1 && act_fp ≤ base_fp` — not worse on recall of the
  actionable rule, not worse on false opens — with FP counted per target over all
  negatives (any open on a negative is an FP). Floors are searched on the same
  grid discipline as #287 (`calibration-run/grid-*/` evidence dir, freeze snapshot
  committed, `replay_two_stage` un-ignored for the activity grid).
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
2. `QueryKind` threading + key construction at the four resolution sites, with
   `profile_key` unsuffixed for coverage/k (`wisdom_match.rs`, `queries.rs`).
3. Composite stub row in `WISDOM_GATE_FLOORS_TEST` (rev-3 M4); NO v1 `:activity`
   row anywhere in production tables (rev-4 M2′).
4. Healthy-path-only activation (cell-4 predicate, §3.3) + the fallback rule (§4)
   in `wisdom_match_cmd`, including audit-line suppression and `raw_vec` reuse.
5. `user_stated` aperture pre-truncation filter (§5) on the activity-active path.
6. Gate label per §3.1 (composite scheme key carries `:activity`; no v1 activity
   label — v1 is not an active arm).
7. Calibration harness extension: `--query-kind` (message|path|command|pattern) +
   the instr1 instruction-ablation cell (harness-side prefix text).
8. Tests (§ Testing), docs, CHANGELOG.

## Error handling

- Clap usage errors (mixed targets, target + positional): clap's exit, same as
  `stage_sel` conflicts today. No new error kinds.
- Uncalibrated/unhealthy activity state: the fallback (§4) — never an error, never
  `EXIT_NO_RESULTS` (the #265 exit discipline, unchanged).
- Embed failure, RO-brain open failure: propagate (exit 1) exactly as message mode
  does today.

## Testing

- `tools/tests/ct_wisdom_match.rs` additions:
  - group exclusivity matrix (path+command, path+pattern, command+pattern,
    each + `-- text`): usage error.
  - fallback parity, per flags (rev-3 M6): `--path=X` output is byte-identical to
    `ct wisdom match -- X`; `--two-stage --path=X` is byte-identical to
    `ct wisdom match --two-stage -- X` (including the audit line); both exit 0;
    empty output iff the reference call is empty (rev-2 M-2 — never an
    unconditional "empty" assertion).
  - empty target (`--path=`): closed gate, exit 0, no embed, ahead of key
    resolution (rev-3 m4).
  - `--help` advertises all three flags (integrations' capability probe).
  - `user_stated` filter: a non-`user_stated` fact that opens in message mode is
    withheld in activity mode; a `user_stated` fact ranked below two filtered
    entries IS delivered (pre-truncation order, rev-3 M5); corrections of any
    provenance still delivered.
  - audit `open=`/top reflect post-aperture delivery (rev-3 M5).
  - the ACTIVE path (composite, cell 4) is exercised through lib tests / the
    harness's explicit floors — the composite stub lives in
    `WISDOM_GATE_FLOORS_TEST`, and CLI tests can and must see the fallback fire
    for an uncalibrated composite (rev-3 M4); there is NO v1-activity row or
    active-path test (rev-4 M2′).
  - post-embed degradation: `candidate_set` → `None` after a cell-4 projection →
    fallback fires, reuses the raw vector (no second embed), and the abandoned
    attempt leaves NO stderr output of its own (rev-4 B1′).
- One CLI test per JSON field: schema unchanged, `:activity` label only on the
  active path.

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
