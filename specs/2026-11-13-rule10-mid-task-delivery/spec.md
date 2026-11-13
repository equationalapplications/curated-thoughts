# `rule10-mid-task-delivery`: delete INTENT rule 10's limit on mid-task delivery

**Date:** 2026-11-13
**Status:** Draft — awaits the owner's approval (Step-3 review). OQ1 DECIDED (owner, 2026-11-13): the unverified-match prompt is wired into the `ct wisdom match` paths (§8). OQ2 DECIDED (owner, 2026-11-13): the turn-off setting ships in this PR (§7, §8). All open questions resolved. Rev 4: Fable spec-gate findings addressed (3 MAJOR: gate-bench freeze-tripwire disposition, no per-tier minimum on the zero-`user_stated` live brain, UI drain/list gating mechanism; 4 MINOR: nine sites, NULL-provenance rows, env overrides, numbering/paths). Rev 3: Opus spec verdict APPROVE WITH NITS; nits fixed (degraded-config resolves ON, golden-file rule sharpened, probe sampling read-only with mix recorded). Rev 2: addresses the Opus spec review (3 MAJOR / 4 MINOR, all adjudicated REAL and fixed: probe-set regeneration, configuration-safe rule-10 heading, test disposition corrected — `:3030` deleted, all seven parameter sites named, drain-file gates named, reversibility sentence, *Why* wording). Backed by the converged investigation of 2026-11-13 (Opus Tier-3 rounds 1-2 + Fable Tier-4; every finding verified against the tree at `d26654a3` and addressed).
**Issue:** #344. **Owner ruling:** 2026-11-12.

## 1. Problem

INTENT rule 10 limits mid-task delivery to human-tier facts: `user_stated` and `user_confirmed`. The owner ruled on 2026-11-12 that this limit goes. Human approval exists for accountability, not quality control. Nothing may rely on a person reviewing items one by one.

Rule 10 carries more than the limit. The amendment must settle each part, per the issue: the "recallable + provenance" sentence, the #265 exception, the confirmation queue and prompt, and the *Why* line.

## 2. Settled decisions (do not re-ask)

- The delivery limit goes. An agent may receive any fact the Librarian extracted.
- Who vouches for a fact never changes how it ranks or delivers. Labels stay.
- The confirmation queue and the recall prompt stay as accountability features. A config setting must make them possible to turn off; the owner intends to turn them off for his own brain.
- Rule 2 (a fact is human-trusted only if a person vouched for it) does not change.
- No feature re-processes facts already stored.

## 3. INTENT.md amendment

The amendment PR carries these edits to INTENT.md:

1. Delete the limit sentence ("Mid-task delivery admits only human-tier facts…") and the #265 exception sentence ("with the standing #265 exception: corrections of any provenance always flow"). The exception is redundant once the limit is gone; the code already runs no provenance filter on corrections (`find_corrections`, `wisdom_match.rs:1354+`).
2. Move the labels detail into rule 3. Rule 3's sentence "is live, recallable, and labeled with its provenance" (INTENT.md:122-124) gains the words "as per-hit `source_type` and `verified` labels". Rule 2 does not take the sentence: rule 2 is about earning the human tier, rule 3 is about labels.
3. Retitle rule 10. The heading "…and rarely delivered mid-task" contradicts the amendment. New heading: "Unconfirmed facts are recallable, labeled, and may be offered for confirmation". The heading stays true when the owner turns the confirmation surface off (§7), because "may be offered" holds in both configurations.
4. Rule 10's body then states only the queue and prompt mechanics.
5. Replace the *Why* line with: human approval records accountability; it does not gate delivery. Quality comes from the pipeline. Corrections and supersession are the safety net.
6. Add an amendment entry recording this ruling and its date, per the house convention.

## 4. Code change: remove the delivery aperture

File: `src-tauri/src/wisdom_match.rs`.

1. Set the Activity arm of the `provenance_filter` match (`:770-773`) to `None`.
2. Rewrite the comments at `:54-61` and `:602-607`. They state the human-tier aperture as current behavior.
3. Keep the `provenance_filter` parameter and its threading. The one-arm edit keeps every call site valid. Removing the parameter touches NINE sites — the parameter at `:528`, the construction at `:770-773`, the application block at `:608-614`, the three forwarding calls at `:917`, `:961`, `:985`, the v1-path caller at `:422-432` (passes `None`), and the two test callers at `:2095` and `:2147` — do it in a follow-up commit inside the same PR.
4. Do not touch `find_corrections` (`:1354+`). It has no provenance filter; it stays as it is.
5. Keep the `--max` cap (`:763`). It is a volume control, not a provenance control.

## 5. Floor re-tune (mandatory, same PR)

The `:activity` floor value was calibrated on positives drawn from `user_stated` facts only (`calibrate_two_stage.rs:1507`, `:1571-1587`). After the amendment, agent-tier facts compete against a floor derived from a human-only pool. This breaks the calibrated-cutoff guarantee of INTENT rule 7.

1. Remove or relax the positive-provenance hard error at `tools/src/bin/calibrate_two_stage.rs:1583-1591` (the whole harness lives in the `tools` crate, not under `src-tauri/`). Update the warning comment at `:1573`.
2. Set the two activity-arm arguments (`:1723`, `:1764`) to `None`.
3. **Regenerate the activity probe set with positives from every provenance** before re-running. Relaxing the gate alone changes nothing: today's probe file holds only `user_stated` positives, so a plain re-run re-derives the same human-only floors. **Sampling reality (rev 4, Fable MAJOR-2): the live brain has ZERO `user_stated` facts** (`docs/benchmarks/2026-10-18-wisdom-gate-activity-restricted-cosine.md:12-16,84-86`; `wisdom_match.rs:112-114`) — the 2026-10-18 run used a Kurt-approved relabeled scratch copy. Positives are therefore sampled READ-ONLY from the live population, which is almost entirely `librarian_inferred`; the record lists the mix; **NO per-tier minimum is imposed** — the aperture no longer discriminates by provenance and the floor must represent the population it gates. If the owner wants human-tier positives in the pool, the plan uses a relabeled scratch copy under the 2026-10-18 precedent (owner approval, live brain never written).
4. Re-run the calibration on the regenerated probe set.
5. Re-derive the floor for **the ONE registered `:activity` floor row** (`wisdom_match.rs:90-131` holds exactly one). Record the new benchmark under `docs/benchmarks/`. The benchmark record must list the provenance mix of its positives, so a future reader can tell the pool was mixed.
6. Rewrite or remove the now-dead predicate block at `:1244-1251` with its stale comment, in the follow-up commit.
7. Say in the PR that the harness passed `Some("user_stated")` alone, stricter than the production pair. This keeps the history readable.

## 6. Test changes

In `src-tauri/src/wisdom_match.rs` tests:

1. `activity_aperture_delivers_user_stated_only_on_the_active_path` (`:2910`): invert. The agent-tier fact must now appear on the activity path.
2. `activity_admits_confirmed_and_stated` (`:2991`): rewrite. All provenances pass; the test pins the four-id delivery order. Out-of-vocabulary source types deliver with a NULL provenance label (the fixture's `f-np` seeds `source_type = "raw_note"`, which `provenance_for` maps to `None`, `wisdom_match.rs:2996`) — message mode already ships them this way, and the amended rule 3's "labeled" sentence is satisfied by the label being present, null where the source type is unknown.
3. `non_admitted_fact_never_shadows_admitted_below` (`:3030`): **delete in the main commit.** The test calls the public `two_stage_entries` (`:749`), which builds the filter itself from `QueryKind`; no caller can pass a filter. Post-amendment the high-ranker is admitted, the assertion `got == vec!["f-st-low"]` fails, and the property it pins — pre-truncate filtering — has no production exerciser left. Keeping it would test dead code through a direct call to the inner scan function (`:528`). The aperture-history spec (`#282` §5) remains the record of the property.
4. `activity_composite_attributes_chunk_hop_with_its_own_floor_value` (`:2794`): keep the floor-key attribution. Invert the `entries.is_empty()` assertion at `:2822-2825`: `f1` must now be delivered.
5. Add one exactly-once test: an agent-tier correction and an agent-tier ordinary fact both appear in one activity run. The `exclude ∪ corrections_absent_ids` skip set must deliver each id exactly once now that both lists hold agent-tier ids.
6. `tools/tests/ct_wisdom_match.rs:128` pins a provenance label on a delivered entry. It survives unchanged.
7. **The gate-bench freeze tripwire (rev 4, Fable MAJOR-1):** `src-tauri/tests/wisdom_gate_bench.rs:137-157` (`replay_activity_two_stage`) asserts `gate_floor == expected["floor"]` from the committed fixture `src-tauri/tests/fixtures/wisdom_gate_bench/external_qwen_qwen3-embedding-4b_activity_two-stage_restricted-cosine/expected.json`, and `activity_fp0_on_user_confirmed_kernel_brain` (`:194+`) replays all 196 frozen probes asserting FP == 0. The committed freeze snapshot is NOT regenerable from committed tooling (`docs/benchmarks/2026-10-18-wisdom-gate-activity-restricted-cosine.md:71-76`). Disposition: produce a NEW freeze snapshot (expected.json + vectors) from the regenerated probe set and commit it with the PR; the benchmark record states the tooling gap and the new provenance mix; if the re-derived floor is lower than 0.65, the FP-0 replay decides whether the floor stands.

## 7. New config setting: turn off the confirmation surface

No setting exists today. The only opt-out is a 7-day localStorage snooze (`UnverifiedPromptModal.tsx:26-27`), which is per-browser and undocumented in INTENT.

1. Add one config setting that turns off the whole confirmation surface: the queue UI and the recall prompt. One switch, not two.
2. Default: on. Absent key: on. The empty config keeps today's behavior.
3. When off, the backend must not write the unverified-match drain file. The gates sit at the three writer call sites — `tool_dispatch.rs:682`, `:1285`, `:1326` — plus the new `ct wisdom match` prompt site (§8 OQ1, decided: the prompt is wired in).
4. **The UI half has a mechanism (rev 4, Fable MAJOR-3):** `AppShell.tsx:487` mounts `UnverifiedPromptModal` unconditionally and the modal polls the drain command (`src/lib/tauri.ts:844`, backed by `queue_api.rs:148`) every 30 s; the frontend has NO command that returns a brain config key. The spec therefore gates the BACKEND commands, so the frontend needs no new config command: when the setting is off, the drain command returns an empty report and leaves the file untouched (`queue_api.rs:148` — stale lines already in `unverified-match.jsonl` are not drained and not rendered), and the queue-list command returns an empty list on the same reader. The modal and queue panel then render nothing without frontend changes.
5. **Process agreement (rev 4, Fable MINOR-6):** every process resolves the config through `brain_paths_for` (`retrieval/mod.rs:127-142`), which honors `CURATED_BRAIN_CONFIG` and `CURATED_BRAIN_DB`. The three processes agree when their overrides agree; a sidecar launched with an override reads that override's config, as today.
6. A malformed or unreadable config.json resolves to ON (today's behavior) in every process — the same degraded-load discipline the ingest keys follow (`config/mod.rs:872-877`: missing file is default; any other read error is degraded, never silently off). The setting gets its own small reader next to the ingest-policy loader; it does not ride on `IngestPolicy`, which is an ontology-and-tier type.
7. The setting is reversible. Turning it off leaves `.confirmations.jsonl` and every existing confirmation stamp untouched; rule 2's path to `user_confirmed` still exists when it is on, and turning it back on restores the queue with prior stamps intact.
8. Document the setting in INTENT rule 10's amended body, one sentence.
9. The exact name follows the existing config key style; the plan fixes it. The only opt-out today is the 7-day localStorage snooze in `src/components/queue/UnverifiedPromptModal.tsx:26-27`; it stays until this setting ships, then the plan may remove it.

## 8. Open questions for the owner

- **OQ1 — prompts on the `ct wisdom match` path — DECIDED (owner, 2026-11-13, option 1): wire the prompt in.** Today the never-worse fallback of `ct wisdom match` already delivers agent-tier facts with no prompt (`tools/src/queries.rs:842-913`; #282 spec line 579). The amendment makes unfiltered delivery the healthy path's behavior too. `emit_unverified_matches` is wired into the `ct wisdom match` paths as well, so the amended pipeline prompts everywhere the turn-off setting allows — one accountability surface, governed by one switch. (Option 2, a stated prompt-free exception, was rejected.)
- **OQ2 — is the turn-off setting in this PR or a follow-up? — DECIDED (owner, 2026-11-13, option 1): same PR.** The setting ships in this PR as its own commit, so rule 10 never states a setting that does not exist. (A follow-up PR was rejected.)

## 9. Benchmarks ("How changes are made" step 4)

The amendment is a recall and injection change, so all gates apply:

1. No superseded fact is ever injected.
2. LongMemEval hit rate.
3. Abstention precision.
4. Paraphrase tests, at least 200, across models.
5. Exactly-once delivery.
6. Cached output stays byte-identical to fresh output for the same inputs. The cached-equals-fresh property-test CODE stays unchanged. What changes: the two activity-path in-file test fixtures inverted in §6, and the gate-bench freeze snapshot named in §6.7 (a NEW snapshot from the regenerated probe set — the old one is not regenerated, it is replaced and its non-regenerability recorded). Regenerating a golden file used by a cached-equals-fresh test is forbidden — that would hide a real cache bug.

## 10. Out of scope

- Removing the `provenance_filter` parameter (follow-up commit in the same PR).
- curated-thoughts-integrations changes. CTI consumes whatever `ct wisdom match` returns and labels provenance per-hit; no source file assumes human-only injection (verified across TypeScript and Python sources).
- Any change to ranking, floors besides `:activity`, or the two-stage cell logic.
