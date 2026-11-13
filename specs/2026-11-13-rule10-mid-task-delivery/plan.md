# rule10-mid-task-delivery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Delete INTENT rule 10's human-only limit on mid-task delivery, re-tune the calibrated `:activity` floor for the now-mixed provenance pool, and add the config switch that turns the whole confirmation surface off.

**Architecture:** One delivery-aperture arm flips to `None` in `wisdom_match.rs`; the calibrated floor value is re-derived from a regenerated mixed-provenance probe set (the key stays, the value changes); a new `confirmations.enabled` config key gates the three unverified-match writers, the app drain command, and the queue-list command; the unverified-match prompt is additionally wired into the `ct wisdom match` paths (OQ1) so one switch governs every surface.

**Tech Stack:** Rust (src-tauri workspace + tools crate), SQLite, Tauri 2 (React/TS frontend), `ct` CLI.

**Spec:** `specs/2026-11-13-rule10-mid-task-delivery/spec.md` (rev 4 + OQ rulings — the binding authority; the plan argues from it, and on conflict the spec wins).

## Global Constraints

- Branch: `feat/rule10-mid-task-delivery`; worktree `/mnt/kv-work/build/worktrees/curated-thoughts/rule10-delivery`. The controller pushes; implementers commit locally.
- Cargo is SERIAL machine-wide. Before ANY cargo command: `export CARGO_TARGET_DIR=/mnt/kv-work/build/worktrees/curated-thoughts/rule10-delivery/.cargo-shared-target` (ABSOLUTE). Pre-flight: `df -h /mnt/kv-work` ≥ 30G free. Never create a `target/` dir. `CARGO_BUILD_JOBS=2`.
- Every cargo gate redirects FULL output to a log file and judges the log, never the pipe exit code. `cargo check` gates include `--tests`.
- Spec-and-code in SEPARATE commits (AGENTS.md). Merge commits only at merge time (owner merges).
- INTENT.md is static (#322): the amendment entry lives in the spec status line and the PR body, NOT in a new INTENT log section.
- Rule 2 semantics untouched. No feature re-processes stored facts. The 2026-10-18 relabeled-scratch precedent applies wherever human-tier positives are wanted (owner-approved scratch copy only; live brain never written).
- Exactly-once delivery and cached-equals-fresh property tests must keep passing; only activity-path fixtures/goldens may change (§9.6).

---

### Task 1: INTENT.md amendment

**Files:**
- Modify: `INTENT.md:197-209` (rule 10), `INTENT.md:122-124` (rule 3).

**Interfaces:** Consumes: spec §3. Produces: the amended INTENT text all later tasks assume.

- [ ] **Step 1: Edit rule 10** — replace lines 197-209 with:

```markdown
### 10. Unconfirmed facts are recallable, labeled, and may be offered for confirmation

Every live fact is recallable on demand. The confirmation queue is the app's
list of facts the person has not yet confirmed; confirming stamps the fact,
it never asks the person to retype. When an unconfirmed fact matches during
an agent's recall, the app may offer the person a confirmation prompt. The
`confirmations.enabled` setting turns the whole confirmation surface off.

*Why:* human approval records accountability; it does not gate delivery.
Quality comes from the pipeline. Corrections and supersession are the
safety net.
```

- [ ] **Step 2: Fold the labels detail into rule 3** — the sentence "is live, recallable, and labeled with its provenance" (INTENT.md:122-124) becomes "is live, recallable, and labeled with its provenance, as per-hit `source_type` and `verified` labels".
- [ ] **Step 3: Self-check** — `grep -n "rarely delivered\|user_stated.*user_confirmed\|#265" INTENT.md` must return no rule-10 hits (rule-2 attestation prose may mention tiers). Verify the heading says "may be offered".
- [ ] **Step 4: Commit** — `git add INTENT.md && git commit -m "docs(intent): amend rule 10 — remove the mid-task delivery limit (issue #344, owner ruling 2026-11-12)"`.

### Task 2: Remove the delivery aperture

**Files:**
- Modify: `src-tauri/src/wisdom_match.rs:54-61` (doc comment), `:602-607` (arm comment), `:770-773` (the match).

**Interfaces:** Produces: `QueryKind::Activity` no longer filters; `provenance_filter` parameter threading (`:528`, `:917`, `:961`, `:985`) unchanged.

- [ ] **Step 1: Flip the arm** at `:770-773`:

```rust
// The delivery aperture is REMOVED (issue #344, INTENT rule 10 amended
// 2026-11-12/13): neither kind filters by provenance. The parameter stays
// threaded for the merge pass and future filters; its removal is a
// follow-up commit in this PR.
let provenance_filter: Option<&[&str]> = match kind {
    QueryKind::Message => None,
    QueryKind::Activity => None,
};
```

- [ ] **Step 2: Rewrite the comment at :54-61** — drop the sentence "…and, on the active path, the human-tier delivery aperture (`user_stated` + `user_confirmed`; §5 rev-15 of the #282 spec)"; keep the ACTIVITY floor-key sentence.
- [ ] **Step 3: Rewrite the comment at :602-607** — replace the aperture paragraph with:

```rust
// Issue #344: the delivery aperture was removed — every live provenance
// is admitted on the activity path. The filter application below survives
// for the parameter's remaining callers; both arms now pass `None`.
```

- [ ] **Step 4: Compile gate** — `cargo check -p curated-thoughts --tests 2>&1 | tee /tmp/t2-check.log` (expect test FAILURES listed, not compile errors).
- [ ] **Step 5: Commit** — `git commit -am "feat(wisdom): remove the activity delivery aperture (issue #344)"` — do NOT include test files yet (Task 3 owns them).

### Task 3: Test updates (wisdom_match)

**Files:**
- Modify: `src-tauri/src/wisdom_match.rs:2910-2990` (invert), `:2991-3029` (rewrite), `:3030-3073` (DELETE), `:2794-2830` (invert assertion).

**Interfaces:** Consumes: Task 2. Produces: green unit tests pinning unfiltered activity delivery.

- [ ] **Step 1: Invert `activity_aperture_delivers_user_stated_only_on_the_active_path` (:2910)** — rename to `activity_delivers_every_provenance_on_the_active_path`; assert `f1` (the `librarian_inferred` fact) IS delivered on the activity path and message mode is unchanged.
- [ ] **Step 2: Rewrite `activity_admits_confirmed_and_stated` (:2991)** — assert all four seeded provenances deliver, INCLUDING the `f-np` row (`source_type = "raw_note"` → `provenance_for` → `None`); pin the four-id delivery order; keep a comment: out-of-vocabulary source types deliver with a null label, as message mode already does.
- [ ] **Step 3: DELETE `non_admitted_fact_never_shadows_admitted_below` (:3030)** — per spec §6.3 (no caller can pass a filter; property has no production exerciser; `#282` §5 remains the record). Also delete its seed helpers IF now unused (`seed_aperture_entry` is shared with Task 3 Step 4 — check usage first).
- [ ] **Step 4: Invert the assertion in `activity_composite_attributes_chunk_hop_with_its_own_floor_value` (:2794)** — `entries.is_empty()` at `:2822-2825` becomes: `f1` must be delivered; keep the floor-key attribution assertions untouched.
- [ ] **Step 5: Add the exactly-once test** — one activity run where `find_corrections` returns an agent-tier correction AND an agent-tier ordinary fact matches: assert each id appears exactly once across the run (the `exclude ∪ corrections_absent_ids` skip set, `:774-776`).

```rust
#[test]
fn activity_exactly_once_with_agent_tier_correction_and_fact() {
    let conn = fixture();
    // seed an agent-tier fact + a superseded pair whose head is agent-tier
    // (mirror the seeding shape of activity_admits_confirmed_and_stated;
    // mark the ordinary fact as superseded-by the correction)
    // run two_stage_entries with QueryKind::Activity, exclude empty
    // assert: the correction id and the fact id each appear EXACTLY once
}
```

- [ ] **Step 6: Run the gate** — `cargo test -p curated-thoughts wisdom_match 2>&1 | tee /tmp/t3-tests.log` — expect PASS (fixture-level tests; no floors needed).
- [ ] **Step 7: Commit** — `git commit -am "test(wisdom): pin unfiltered activity delivery + agent-tier exactly-once (issue #344)"`.

### Task 4: Calibration harness + floor re-tune

**Files:**
- Modify: `tools/src/bin/calibrate_two_stage.rs:1573` (comment), `:1583-1591` (gate), `:1723`, `:1764` (args), `:1244-1251` (dead block — follow-up commit).
- Modify: `src-tauri/src/wisdom_match.rs:90-131` (`WISDOM_GATE_FLOORS` — the ONE `external:qwen/qwen3-embedding-4b:activity` row).
- Modify: `src-tauri/tests/wisdom_gate_bench.rs` fixture dir (NEW freeze snapshot).

**Interfaces:** Consumes: OPENROUTER_API_KEY in env; scratch-brain copy per the 2026-10-18 precedent. Produces: the new floor value all later gates assert.

- [ ] **Step 1: Relax the positive gate** — at `:1583-1591`, delete the `bail!` block; replace with a recorded note (no error) that positives may carry any provenance; update the `:1573` warning comment to describe the mixed pool.
- [ ] **Step 2: Set both activity-arm arguments to `None`** (`:1723`, `:1764`).
- [ ] **Step 3: Regenerate the probe set** from the live population (READ-ONLY sample; record the provenance mix; NO per-tier minimum — spec §5.3). Preconditions, STOP and report if unmet: `OPENROUTER_API_KEY` set; a ro scratch copy of the live brain exists (`~/ct-calibration-scratch/brain-dir/` per the 2026-10-18 record) — else create it per that record's procedure.
- [ ] **Step 4: Run the calibration** — `cargo run -p ct --bin calibrate_two_stage -- --query-kind activity …` (full args from the 2026-10-18 benchmark record `docs/benchmarks/2026-10-18-wisdom-gate-activity-restricted-cosine.md`), full output to a log.
- [ ] **Step 5: Write the new floor** — update the single `:activity` row in `WISDOM_GATE_FLOORS`; add a one-line comment: "Re-derived 2026-11-13 from a mixed-provenance pool (issue #344); prior value calibrated on user_stated-only positives."
- [ ] **Step 6: Produce the NEW freeze snapshot** — expected.json + vectors from the regenerated run, replacing the committed fixture under `src-tauri/tests/fixtures/wisdom_gate_bench/external_qwen_qwen3-embedding-4b_activity_two-stage_restricted-cosine/`; note in `expected.json`'s freeze_provenance that the old snapshot was not regenerable and this one is.
- [ ] **Step 7: FP-0 replay decides** — `cargo test -p curated-thoughts --test wisdom_gate_bench 2>&1 | tee /tmp/t4-bench.log`: `replay_activity_two_stage` must pass on the new snapshot; `activity_fp0_on_user_confirmed_kernel_brain` (196 frozen probes) must report FP == 0 at the new floor. If FP > 0, raise the floor to the FP-0 boundary and re-run.
- [ ] **Step 8: Update the benchmark record** — new file `docs/benchmarks/2026-11-13-wisdom-gate-activity-mixed-provenance.md`: provenance mix of positives, hit rate, FP, the floor delta (old 0.65 → new), the harness-divergence note (harness was `user_stated`-alone, stricter than production), and the tooling-gap note.
- [ ] **Step 9: Commit** — `git commit -am "feat(wisdom): re-derive the :activity floor from a mixed-provenance pool (issue #344)"`.

**IF Step 3 preconditions are unmet:** stop after Steps 1-2, commit those, and report the block — the floor flip (Steps 3-8) becomes the PR's single remaining task.

### Task 5: `confirmations.enabled` config setting

**Files:**
- Modify: `src-tauri/src/config/mod.rs` (new key + reader), `src-tauri/src/prompt_drop.rs:324` (`emit_unverified_matches` gate), `src-tauri/src/queue_api.rs:22` (`queue_list_cmd` gate), `:148` (drain gate), `src/components/queue/UnverifiedPromptModal.tsx` (no change needed — backend gates suffice).

**Interfaces:** Produces: `pub fn confirmation_enabled_for_db(db_path: Option<&str>) -> bool` in config/mod.rs; all confirmation surfaces read it.

- [ ] **Step 1: Add the key.** New top-level config section (serde):

```rust
/// Confirmation-surface switch (issue #344). Absent = true (today's
/// behavior). Malformed/unreadable config resolves to true — the same
/// degraded discipline as the ingest keys (config/mod.rs:872-877).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConfirmationsConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
}
fn default_true() -> bool { true }
```

with `#[serde(default)] pub confirmations: ConfirmationsConfig` on the root config struct; add `"confirmations"` to the known-keys list(s) mirroring `known_ingest_keys` (`:1320`) and the salvage path (`:1764-1796`).
- [ ] **Step 2: Add the reader** next to `ingest_policy_for_db` (`:878`):

```rust
/// True unless config.json explicitly sets confirmations.enabled=false.
/// Uses brain_paths_for (honors CURATED_BRAIN_CONFIG / CURATED_BRAIN_DB,
/// retrieval/mod.rs:127-142). Degraded/missing config => true.
pub fn confirmation_enabled_for_db(db_path: Option<&str>) -> bool
```

(the existing policy cache pattern is the model; a tiny cache is fine but not required — the readers are low-frequency).
- [ ] **Step 3: Gate the writers** — in `prompt_drop.rs`, at the top of `emit_unverified_matches` (defined `:324`; the three callers are `tool_dispatch.rs:682`, `:1285`, `:1326` and the NEW Task 6 site): when the flag is false, return the empty report immediately and touch NOTHING on disk.
- [ ] **Step 4: Gate the app commands** — `queue_list_cmd` (`queue_api.rs:22`) and the drain command (`:148`, `unverified_prompts_drain_cmd`) each return empty when the flag is false. The modal (`UnverifiedPromptModal.tsx:487` mount) then renders nothing with zero frontend changes. Do NOT delete stale lines in `unverified-match.jsonl`; leave the file untouched.
- [ ] **Step 5: Write the failing tests first (TDD)** — in `config/mod.rs` tests: absent key → true; `enabled: false` → false; malformed JSON file → true. In `prompt_drop` tests: flag off → no file written even with pending matches; flag on → writes as today. In `queue_api` tests (or the nearest integration level): flag off → `queue_list_cmd` returns empty, drain returns empty report.
- [ ] **Step 6: Run the gates** — `cargo test -p curated-thoughts config:: prompt_drop queue 2>&1 | tee /tmp/t5-tests.log` — PASS.
- [ ] **Step 7: Commit** — `git commit -am "feat(config): confirmations.enabled turns the whole confirmation surface off (issue #344)"`.

### Task 6: Wire the unverified-match prompt into `ct wisdom match`

**Files:**
- Modify: `src-tauri/src/wisdom_match.rs` (delivery completion), mirroring `tool_dispatch.rs:682`.

**Interfaces:** Consumes: Task 5's gated `emit_unverified_matches`. Produces: activity-path matches surface in the app's prompt drain.

- [ ] **Step 1: Study the pattern** — read `prompt_drop.rs:324` (signature, drain cap 3, write-only-for-non-human-hits) and `tool_dispatch.rs:682` (call shape after a recall delivery).
- [ ] **Step 2: Wire it** — after `gated_entries` delivery completes in the activity path (`:397` → `:519` area), call `emit_unverified_matches` with the delivered entries' provenance/tier info, mirroring the `:682` call shape. Message mode: mirror what `tool_dispatch` already does for message recalls (if message already emits via tool_dispatch, do NOT double-emit — the wisdom_match wiring must be reachable from BOTH the CLI (`cli_common::wisdom_match_cmd`, `tools/src/queries.rs:810+`) and MCP paths exactly once per recall; check tool_dispatch's call sites to avoid duplication — if tool_dispatch already emits for MCP recall, only the bare `ct` CLI path needs the new call).
- [ ] **Step 3: Test** — a `ct wisdom match` activity run with a deliverable agent-tier fact, flag on: `unverified-match.jsonl` gains an entry; flag off: it does not (Task 5's gate). Add to `tools/tests/ct_wisdom_match.rs`.
- [ ] **Step 4: Run** — `cargo test -p ct ct_wisdom_match 2>&1 | tee /tmp/t6-tests.log` — PASS.
- [ ] **Step 5: Commit** — `git commit -am "feat(wisdom): surface unverified matches from ct wisdom match (issue #344 OQ1)"`.

### Task 7: Fixture/golden sweep + full gates

**Files:** activity-path fixtures anywhere golden outputs pin human-only delivery; `tools/tests/ct_wisdom_match.rs:128` (label test — survives).

- [ ] **Step 1: Full test sweep** — `cargo test --workspace 2>&1 | tee /tmp/t7-all.log` (SERIAL; long). Every failure is either a fixture describing human-only delivery (update it, quoting this spec) or a REGRESSION (fix the code, never the test).
- [ ] **Step 2: Exactly-once + cached-equals-fresh property checks** — confirm both property suites pass UNCHANGED (spec §9.6).
- [ ] **Step 3: Cleanup commit** — delete `specs/2026-11-13-rule10-mid-task-delivery/plan.md` + `LEDGER.md`, flip the spec status line to `Implemented (2026-11-13, …)`, PR body cites the ruling dates. `git commit -m "chore(spec): rule10 implemented — drop transient scaffolding"`.

### Task 8: Benchmark gates (controller-run, may extend past the PR's code tasks)

The INTENT step-4 gates (LongMemEval hit rate, abstention precision, paraphrase ≥200 across models) run on scratch brains with the embedding provider. Precondition: `OPENROUTER_API_KEY` + scratch brain. If unavailable in-session, record `benchmark-pending` in the spec status line and PR body — the PR does NOT merge before these run.
