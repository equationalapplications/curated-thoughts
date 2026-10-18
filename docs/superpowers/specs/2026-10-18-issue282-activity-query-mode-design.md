# Activity query mode for the wisdom gate: paths and commands reach concept rules (issue #282)

**Date:** 2026-10-18 (rev 2 — rev 1 folded the step-0 Fable guidance memo and Kurt's
three rulings; rev 2 folds the Tier-2 GLM 5.3 review, all findings fixed).
**Status:** Draft — Tier 2 (GLM 5.3) passed rev 2 methodology pending Tier 3 (Opus
`--doc`) and the Tier-4 Fable gate; Kurt's explicit approval required before
implementation.
**Branch:** `spec/issue282-activity-query-mode`
**Priority:** High — the integrations' autonomous-work delivery (plugin 0.7.x) is live
and shipping the bare target to the message path until this lands.
**Issue:** equationalapplications/curated-thoughts#282
**Step-0 guidance (Kurt-authorized non-gate Fable dispatch, 2026-10-18, $4.20):**
equational-wiki vault `records/investigations/2026-10-18-issue282-fable-step0-guidance/memo.md`.
**Supremacy:** where the Fable memo disagrees with THIS spec, this spec is canonical
(the memo is advisory input; the ladder + Kurt own the calls). Where Kurt's three
2026-10-18 rulings disagree with the memo, the rulings win — they are folded below.

## Revision history (compressed)

- **rev 2 (this rev):** Tier-2 GLM 5.3 review returned Changes requested (5 Important,
  8 Minor — adjudicated all REAL, zero FP; full review
  `~/.local/state/tessera/issue282/tier2-glm-review.md`). Fixes folded: audit surface
  corrected to the STDOUT gate label + two-stage `floor=` token with per-arm label
  strings pinned byte-exactly (I-1); explicit-flag precedence vs the fallback made
  coherent via a per-arm fallback table (I-2); embed plan pinned to message-mode
  parity `RawAndSchemed` — stage-1 chunk leg on the RAW vector (I-3); composite
  `:activity` key form pinned `{scheme-key}:activity:two-stage:chunk-hop`,
  instr1-calibrated (I-4); aperture joints enumerated incl. rule (ii) opens,
  v1-merge complement, corrections-bypass, `user_confirmed` excluded (I-5); clap
  mechanism corrected to ONE required group (M-1); fallback test bullet
  de-contradicted (M-2); "fetch top-2, deliver top-1" (M-3); k shared, not
  recalibrated (M-4); free-text hint ban (M-5); `=` form as caller convention (M-6);
  empty-target pinned (M-7); stub `:activity` rows enumerated (M-8); letter measures
  (not guarantees) concept-from-path; issue body canonical over the memo for A6
  numbers (the memo's 0.596/0.644/0.583/0.898 do not match the issue's
  0.549/0.645/0.588/0.897 — nobody "fixes" the spec toward the memo).
- **rev 1:** initial draft. Contract = typed mutually-exclusive target
  flags on `ct wisdom match` (no new subcommand). Embeds = raw target under a new
  byte-exact activity instruction. Gate = one shared `:activity` floor suffix across
  all three target kinds (Kurt ruling 1); stage arm = two-stage chunk-hop when the
  kill-switch allows, v1 otherwise, each consulting its own `:activity` row.
  ONE fallback rule: missing activity floor ⇒ byte-identical message path on the
  same target text. Aperture = `user_stated` facts only while activity-active
  (Kurt ruling 3). Calibration = post-flip build, hit@1 vs the bare-target
  baseline, letter `act_hit1 ≥ base_hit1 && act_fp ≤ base_fp`. Batch form and
  fast path DEFERRED; deposit-time scope globs ROADMAP (Kurt ruling 2).

## 1. Problem

During autonomous work (tool calls touching paths and commands), the integrations ask
`ct wisdom match` for wisdom relevant to what the agent is doing right now. The query
is a bare activity target — `pnpm test`, `src/db/migrations/0042_add_users.sql`,
`.env.local` — and the gate keys relevance to user-question-shaped prose. The measured
result (A6 experiment, 39 queries, issue body): **concept-from-path never opens**. The
rule "never edit an applied migration; add a new migration instead" scores 0.549–0.590
against every migration-shaped query while staying above the floor (0.64) when asked
as a question (0.897). Score ranges of relevant and near-miss targets overlap, so
lowering the message floor does not fix it and would raise the false-positive rate.
The integrations will not add query expansion on their side (by design — relevance is
CT's job), so CT must accept an activity-shaped query and still reach the concept rule.

## 2. Approach

Keep the #265 contract and machinery; add an **activity query mode** to the existing
`ct wisdom match` surface:

1. The caller states what the target IS (`--path=`, `--command=`, `--pattern=`)
   instead of sending prose.
2. The embed side treats the target as an activity document: raw target text under a
   NEW activity-specific query instruction, never the question instruction.
3. The gate gets its own floor suffix (`:activity`) and its own calibration, so the
   message floor and its probe set are never silently reused.
4. Until an activity floor row exists for the resolved key, activity mode is
   **provably not worse than today**: it falls back to the byte-identical message
   path on the same target text (the integrations already ship the bare target, so
   the fallback is exactly the behavior they see now).

## 3. Design

### 1. Contract surface (D1 — resolved: typed flags, no new subcommand)

On the existing `ct wisdom match` variant (`tools/src/bin/ct.rs`):

- Three new long flags: `--path=<target>`, `--command=<target>`, `--pattern=<target>`.
  Callers MUST use the `=` form (mirroring the `--exclude=<id>` pin) so targets
  beginning with `-` stay unambiguous — clap parses `--path X` identically, so this
  is a CALLER CONVENTION, not an enforceable pin (M-6). Targets may contain
  spaces and shell metacharacters and are passed through VERBATIM (no normalization,
  no quoting repair — literal preservation).
- Mutually exclusive, and each exclusive with the positional message: ONE clap
  `ArgGroup` named `activity_target` containing {the `text` positional, `--path`,
  `--command`, `--pattern`} with `required(true)` and `multiple(false)` — clap has
  no group-vs-arg conflict primitive, so the group OWNS the exactly-one rule (M-1).
  Mixing two target kinds, or a target with `-- text`, is a clap usage error — same
  exit discipline as today's `stage_sel` group (`--two-stage`/`--single-stage`).
  The group is what satisfies the arg-required contract when a flag is used;
  message mode is otherwise unchanged.
- `--help` advertises all three flags — this keeps the integrations'
  `--help` capability probe working (a NEW subcommand would break it; Fable D1).
- Everything else stays pinned: `--max` default 2 clamped 0..=10, `--exclude`
  repeatable with `parse_fact_id`, corrections always delivered as extra, JSON schema
  unchanged, **exit 0 on zero matches AND on an uncalibrated gate; errors propagate
  (exit 1)** — the #265 discipline, verbatim (`queries.rs:807-809`).
- Audit surface (I-1, pinned per arm): the gate label lives on STDOUT — the JSON
  `gate` field and the human `gate:` line (`print_wisdom_match`). The four-field
  `wisdom_gate_decision` stderr line (timestamp, read_scheme, open|closed, count) is
  UNCHANGED — it carries no gate label today and gains none. Pinned label strings,
  byte-exact:
  - v1 arm: `semantic-v1:{gate_key}:activity` — e.g.
    `semantic-v1:external:qwen/qwen3-embedding-4b:activity`. Graft point: the v1
    label is built from the UNSCHEMED `gate_key`, so `:activity` is appended at
    label construction, not folded into the key chain.
  - two-stage arm: `semantic-v2-two-stage:{scheme_key}` with the scheme key carrying
    the activity suffix — e.g.
    `semantic-v2-two-stage:external:qwen/qwen3-embedding-4b:instr1:activity:two-stage:chunk-hop`.
  The two-stage audit line's `floor=` token picks the composite up for free once the
  suffix is folded into the key chain in one place. NO new JSON field, NO `mode`
  field — the label is the audit surface (Fable D1). The activity target is never
  echoed to stderr beyond what the message path echoes of message text today.

### 2. What gets embedded (D2 — resolved: raw target, new instruction, message-mode embed plan)

- The embed input is the **raw target string only**, with no caller- or CT-side
  prose expansion: the A6 experiment measured prose expansion as the WORST shape
  (4/7 vs 6/7 for bare), and `ct_wisdom_activity.py` proves the caller can only
  supply the bare target. Component splits (`src`, `db`, `migrations` joined as a
  phrase) are a CALIBRATION GRID CELL, not the default (§7).
- New byte-exact constant in `embed_scheme.rs`, sibling to
  `QUERY_INSTRUCTION_PREFIX` (line 25):

  ```
  pub const ACTIVITY_QUERY_INSTRUCTION_PREFIX: &str =
      "Instruct: Given a code activity target (a file path, shell command, or search pattern), retrieve the coding rules and facts that apply to it\nQuery:";
  ```

  Application is direct concatenation (`format!("{prefix}{target}")`), same pin as
  the question prefix; applied ONLY under the `instr1` scheme and ONLY to the query
  side. Under `raw`, the bare target is embedded verbatim. The instruction text is
  PART OF THE CALIBRATED UNIT (§7): changing it invalidates the `:activity` floor
  row exactly like a model change, and the constant must never be "fixed"
  cosmetically (same rationale as `embed_scheme.rs:21-26`).
- `query_text_for_scheme` gains an activity branch (or its call site selects
  `ACTIVITY_QUERY_INSTRUCTION_PREFIX` under instr1). The 2000-char
  `truncate_text` clamp applies to the target as today; the prefix lands after
  truncation (same ordering as `queries.rs:824-831`).
- Embed plan = message-mode parity (I-3): under `instr1`,
  `TwoStageEmbedPlan::RawAndSchemed` — TWO embedder round-trips, exactly as
  message-mode two-stage does today: the RAW target vector drives stage-1 chunk
  scoring; the activity-prefixed vector drives the gate leg. Under `raw`, a single
  embed of the bare target. (The step-0 memo left the stage-1 vector undecided; this
  spec pins stage-1 = raw target, keeping the two-stage machinery's shape identical
  to message mode — the composite `:activity` floor is then calibrated for exactly
  this shape.) ONE target per invocation — no batch (§ Out of scope).

### 3. Gate arm (D3 — resolved: two-stage when available, its own floor key)

- Activity mode selects the two-stage chunk-hop arm EXPLICITLY (this spec is the
  explicit opt-in §5 of #271 requires) — subject to the same
  `embedder::single_stage_env_kill()` dominance rule and the same
  `WisdomStageMode` plumbing as flags today (`queries.rs:842-854`). Under
  `CURATED_WISDOM_SINGLE_STAGE=1` (or equivalent kill-switch), activity mode runs
  the v1 arm.
- Which stage-selection resolution wins when the caller ALSO passes
  `--two-stage`/`--single-stage` with an activity target: the explicit flag wins
  (parity with message mode; keeps the calibration harness able to walk both arms).
  Absent a flag, activity mode defaults to two-stage as pinned above.
- Floor keys (I-4, pinned byte-exact; these strings go into `WISDOM_GATE_FLOORS`
  and the resolver must agree):
  - v1 arm: `<scheme-key>:activity` — instr1:
    `external:qwen/qwen3-embedding-4b:instr1:activity`; raw twin (grid cell only):
    `external:qwen/qwen3-embedding-4b:activity`.
  - two-stage arm: the composite form `{scheme-key}:activity:two-stage:chunk-hop` —
    i.e. `:activity` folded into the scheme key BEFORE the composite suffix, yielding
    `external:qwen/qwen3-embedding-4b:instr1:activity:two-stage:chunk-hop`.
    (Suffix-at-end `…:two-stage:chunk-hop:activity` is FORBIDDEN — resolver and
    calibration table must never drift.)
  - Scheme of record: activity floors are calibrated under `instr1` (the activity
    instruction only exists there). No raw-scheme `:activity` composite is
    calibrated in this PR.
  - Same `WISDOM_GATE_FLOORS` table and exact-find-then-abstain lookup; the default
    row is NOT consulted for `:activity` keys (an activity query never matches on
    the message floor — that would silently re-create the uncalibrated-floor bug
    this mode exists to avoid). `TWO_STAGE_K` is SHARED with message mode and keyed
    on the unschemed profile key — k is NOT re-calibrated for activity mode (M-4).
- Deployment-lag honesty: the two-stage flip lives in main but the deployed 3.6.0
  dpkg predates it, and `queries.rs:852` pins `Auto => false`. All `:activity`
  calibration numbers MUST come from a post-flip build (the same build that ships
  this mode); the spec bans quoting message-mode probe numbers in any `:activity`
  row comment.

### 4. Floor-missing semantics — ONE rule (the never-worse fallback)

Precedence (I-2, making §3 and this section coherent): the explicit
`--two-stage`/`--single-stage` flag wins STAGE SELECTION; the fallback triggers when
the SELECTED arm's `:activity` row is ABSENT (uncalibrated). Per-arm table:

| Stage selection | Selected arm | Its `:activity` row | Behavior |
|---|---|---|---|
| none (default) | two-stage (kill off) | composite present | activity two-stage |
| none (default) | two-stage (kill off) | composite absent | MESSAGE FALLBACK |
| `--two-stage` | two-stage | composite present | activity two-stage |
| `--two-stage` | two-stage | composite absent | MESSAGE FALLBACK (audit line still prints — two-stage was requested; mirrors today's pre-calibration `--two-stage` degradation, r29-m4) |
| `--single-stage` | v1 | v1 row present | activity v1 |
| `--single-stage` | v1 | v1 row absent | MESSAGE FALLBACK |
| kill-switch + `--two-stage` | v1 (dominated) | v1 row present | activity v1 |
| kill-switch + `--two-stage` | v1 (dominated) | v1 row absent | MESSAGE FALLBACK |
| kill-switch, no flag | v1 | v1 row absent | MESSAGE FALLBACK |

(The v1-activity row is a grid/ablation cell — §7 — not shipped pre-calibration, so
the default path is fallback until the composite row lands. Calibration runs BYPASS
this table via the harness's explicit floor overrides / internal `wisdom_match_impl`
calls — the CLI precedence never blocks calibration.)

Fallback = the never-worse rule:

- When it fires, activity mode runs the **byte-identical message path** (v1 arm,
  message instruction, message floor for the resolved key) on the same target text —
  exactly what `ct wisdom match "src/db/migrations/0042_add_users.sql"` does today —
  and the gate label stays the plain message label (no `:activity` suffix: nothing
  activity-specific fired). No error, no warning line, exit 0.
- The `user_stated` aperture (§5) does NOT apply on the fallback: the fallback is
  today's behavior, all sources, by construction — "never worse" cuts both ways.
- The `--path`/`--command`/`--pattern` flags are still accepted (exit 0); callers
  may switch to the new mode before any floor row exists (the #271
  mechanism-harmless pattern).

### 5. Aperture — `user_stated` facts only (Kurt ruling 3, 2026-10-18)

While activity mode is ACTIVE (an `:activity` floor row resolved), the candidate
entries are filtered to provenance `user_stated` before scoring. Rationale
(Kurt): fail-closed, consistent with the integrations' philosophy — mid-task
delivery must protect the agent's signal-to-noise ratio, and a wrong fact pushed
mid-task is worse than a missed one. Widening the aperture to all sources requires
(a) calibration data showing activity FP stays at 0 across the labeled set, and
(b) Kurt's explicit sign-off — a one-line spec change, made THEN, not now.

Placement — the filter's five joints (I-5):
1. NOT inside the shared `entry_eligibility_where` fragment (it feeds v1
   `gated_entries`, stage-1 candidate SQL, AND the complement — touching it would
   change message mode). The aperture is applied at the FACT-LEVEL result sets of
   the activity-active path only.
2. Stage-1 candidate DOCS stay unfiltered — chunks are not facts; the aperture
   constrains delivery, not retrieval.
3. Chunk-hop rule (ii) opens (evidence-hash join, no fact-level floor): the opened
   ids pass through the aperture too — a non-`user_stated` fact opened by rule (ii)
   is dropped, not delivered.
4. The v1-merge complement is aperture-filtered like the primary results.
5. CORRECTIONS BYPASS the aperture as a stated RULE (not a test implication): the
   #265 corrections-always-delivered contract dominates ruling 3 — a superseding
   entry of any provenance still flows mid-task. And `user_confirmed` is EXCLUDED:
   the ruling says `user_stated` only.
Message mode and the fallback path are untouched.

### 6. Matching, ranking, corrections

- Scoring, dedupe, ordering, truncation to `--max`, and correction delivery are
  the existing machinery, unmodified (issue #265). Activity mode changes WHAT is
  embedded and WHICH floor/aperture applies — not the ranking rule.
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
- **Primary metric: hit@1** (not hit@2 — the integrations FETCH `--max 2` but
  DELIVER one fact per step, `ACTIVITY_MAX_PER_STEP = 1`, and the
  generic-vs-actionable failure mode is a TOP-1 failure; hit@2 is recorded, not
  gated). Baseline: the SAME build's message path on the SAME targets with the
  message floor (today's shipped behavior).
- **Letter (freeze before the run):** activity mode flips to its own floor row iff
  `act_hit1 ≥ base_hit1 && act_fp ≤ base_fp` — not worse on recall of the
  actionable rule, not worse on false opens — with FP counted per target over all
  negatives (any open on a negative is an FP). Floors are searched on the same
  grid discipline as #287 (`calibration-run/grid-*/` evidence dir, freeze snapshot
  committed, `replay_two_stage` un-ignored for the activity grid).
- **Honesty clause:** the letter measures NOT-WORSE-THAN-BASELINE; it does NOT
  guarantee the issue's headline concept-from-path case is solved — the migration
  example may still stay closed while the letter passes. The concept-from-path bet is
  MEASURED by the labeled set (it contains migration-shaped positives); if the grid
  cannot open them at any floor without FP, that result becomes evidence for the
  scope-globs roadmap, not a spec failure. Numbers canonical source: the ISSUE body
  (the step-0 memo's quoted scores differ slightly — the issue wins).
- **Grid cells (minimum):** floor sweep on the raw-target input; the
  component-split input variant; instruction ablation (generic question prefix vs
  activity prefix) to justify the new constant with data; k ablation only if
  stage-1 breadth looks binding.

## 8. Implementation order (within the ONE PR)

1. Clap surface: three flags + `activity_target` group + optional-text resolution
   (`ct.rs`), with usage-error tests.
2. `ACTIVITY_QUERY_INSTRUCTION_PREFIX` + scheme branch (`embed_scheme.rs`).
3. Activity floor key resolution (`:activity` suffix, no-default-rule) + the ONE
   fallback rule (§4) in `wisdom_match_cmd` (`queries.rs`).
4. `user_stated` aperture filter (§5) on the activity-active path.
5. Gate labels: `:activity` appended at v1 label construction; composite scheme key
   carries it on the two-stage arm (§3.1) — `wisdom_gate_decision` stderr untouched.
6. Calibration harness extension: `--query-kind` (message|path|command|pattern)
   on the existing calibration tools.
7. Tests (§ Testing), docs (`docs/wisdom-*.md` if the #265 doc exists), CHANGELOG.

## Error handling

- Clap usage errors (mixed targets, target + positional): clap's exit, same as
  `stage_sel` conflicts today. No new error kinds.
- Uncalibrated `:activity` key: the fallback (§4) — never an error, never
  `EXIT_NO_RESULTS` (the #265 exit discipline, unchanged).
- Embed failure, RO-brain open failure: propagate (exit 1) exactly as message mode
  does today.

## Testing

- `tools/tests/ct_wisdom_match.rs` additions:
  - group exclusivity matrix (path+command, path+pattern, command+pattern,
    each + `-- text`): usage error.
  - exit 0 ALWAYS on uncalibrated `:activity` (fallback fires); fallback output is
    byte-identical to the message-mode call on the same text — empty iff that call
    is empty (M-2; never "empty output" as an unconditional assertion).
  - empty target (`--path=`) behaves like today's empty-text short-circuit: closed
    gate, exit 0, no embed (M-7, pinned in the matrix).
  - `--help` advertises all three flags (integrations' capability probe).
  - `user_stated` filter: a `curated_proposal`-sourced fact that opens in message
    mode is withheld in activity mode (active path only).
  - corrections still delivered (union semantics unchanged) in activity mode.
  - stub-key rows (M-8; CLI integration tests resolve the PRODUCTION table, so the
    stub `:activity` rows live IN `WISDOM_GATE_FLOORS`, enumerated):
    `stub:constant8:activity` (v1 arm),
    `stub:constant8:instr1:activity` (v1 arm, instr1 twin), and
    `stub:constant8:instr1:activity:two-stage:chunk-hop` (the default arm) — so
    stub-backed tests and dev runs of the ACTIVE path do not silently fall back
    (the two-sided stub-floor lesson from `WISDOM_GATE_FLOORS`).
- One CLI test per JSON field: schema unchanged, label carries `:activity` only on
  the active path.

## Out of scope (Kurt's rulings + Fable D5)

- **Batch form** (several activity lines per call) — DEFERRED. The integrations
  rate-limit to one query per model step and measured p95 is 1.22 s against the
  3 s budget. Revisit when the budget binds.
- **Fast path** (sidecar wisdom_match tool / warm daemon) — DEFERRED, same data.
- **Deposit-time scope globs** (curator tags a fact with `src/db/migrations/**`)
  — **ROADMAP (Kurt ruling 2: 2026-10-18)**: the ultimate deterministic fix for
  connecting repo rules to code boundaries (e.g. migration immutability), too
  much scope creep for #282. Needs its own issue with a deposit-UX design.
- No free-text hint argument, EVER (M-5) — targets are typed kinds only; a `--hint`
  would reintroduce the injection surface the typed targets exist to avoid.
- Any ranking heuristic change (§6), any aperture widening (§5), any new
  subcommand (§3.1), any probe-set reuse from message mode (§7).

## Open questions for Kurt

*(None — all three 2026-10-18 questions are ruled; recorded under Rulings carried.
Remaining unknowns are EXPERIMENTS (§7), not decisions.)*

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
- Carried from #271/#287: merge commits only, no squash; letter frozen before the
  calibration run; opt-in mechanism ships before any flip-to-default.
