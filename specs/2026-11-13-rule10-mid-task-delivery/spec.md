# `rule10-mid-task-delivery`: delete INTENT rule 10's limit on mid-task delivery

**Date:** 2026-11-13
**Status:** Draft — awaits the owner's approval (Step-3 review). Rev 2: addresses the Opus spec review (3 MAJOR / 4 MINOR, all adjudicated REAL and fixed: probe-set regeneration, configuration-safe rule-10 heading, test disposition corrected — `:3030` deleted, all seven parameter sites named, drain-file gates named, reversibility sentence, *Why* wording). Backed by the converged investigation of 2026-11-13 (Opus Tier-3 rounds 1-2 + Fable Tier-4; every finding verified against the tree at `d26654a3` and addressed).
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
3. Keep the `provenance_filter` parameter and its threading. The one-arm edit keeps every call site valid. Removing the parameter touches seven sites — the parameter at `:528`, the construction at `:770-773`, the application block at `:608-614`, and the three forwarding calls at `:917`, `:961`, `:985` — do it in a follow-up commit inside the same PR.
4. Do not touch `find_corrections` (`:1354+`). It has no provenance filter; it stays as it is.
5. Keep the `--max` cap (`:763`). It is a volume control, not a provenance control.

## 5. Floor re-tune (mandatory, same PR)

The `:activity` floor value was calibrated on positives drawn from `user_stated` facts only (`calibrate_two_stage.rs:1507`, `:1571-1587`). After the amendment, agent-tier facts compete against a floor derived from a human-only pool. This breaks the calibrated-cutoff guarantee of INTENT rule 7.

1. Remove or relax the positive-provenance hard error at `calibrate_two_stage.rs:1583-1591`. Update the warning comment at `:1573`.
2. Set the two activity-arm arguments (`:1723`, `:1764`) to `None`.
3. **Regenerate the activity probe set with positives from every provenance** before re-running. Relaxing the gate alone changes nothing: today's probe file holds only `user_stated` positives, so a plain re-run re-derives the same human-only floors.
4. Re-run the calibration on the regenerated probe set.
5. Re-derive the per-model `:activity` floors. Record the new benchmark under `docs/benchmarks/`. The benchmark record must list the provenance mix of its positives, so a future reader can tell the pool was mixed.
6. Rewrite or remove the now-dead predicate block at `:1244-1251` with its stale comment, in the follow-up commit.
7. Say in the PR that the harness passed `Some("user_stated")` alone, stricter than the production pair. This keeps the history readable.

## 6. Test changes

In `src-tauri/src/wisdom_match.rs` tests:

1. `activity_aperture_delivers_user_stated_only_on_the_active_path` (`:2910`): invert. The agent-tier fact must now appear on the activity path.
2. `activity_admits_confirmed_and_stated` (`:2991`): rewrite. All provenances pass; the test pins that delivery does not filter.
3. `non_admitted_fact_never_shadows_admitted_below` (`:3030`): **delete in the main commit.** The test calls the public `two_stage_entries` (`:749`), which builds the filter itself from `QueryKind`; no caller can pass a filter. Post-amendment the high-ranker is admitted, the assertion `got == vec!["f-st-low"]` fails, and the property it pins — pre-truncate filtering — has no production exerciser left. Keeping it would test dead code through a direct call to the inner scan function (`:528`). The aperture-history spec (`#282` §5) remains the record of the property.
4. `activity_composite_attributes_chunk_hop_with_its_own_floor_value` (`:2794`): keep the floor-key attribution. Invert the `entries.is_empty()` assertion at `:2822-2825`: `f1` must now be delivered.
5. Add one exactly-once test: an agent-tier correction and an agent-tier ordinary fact both appear in one activity run. The `exclude ∪ corrections_absent_ids` skip set must deliver each id exactly once now that both lists hold agent-tier ids.
6. `tools/tests/ct_wisdom_match.rs:128` pins a provenance label on a delivered entry. It survives unchanged.

## 7. New config setting: turn off the confirmation surface

No setting exists today. The only opt-out is a 7-day localStorage snooze (`UnverifiedPromptModal.tsx:26-27`), which is per-browser and undocumented in INTENT.

1. Add one config setting that turns off the whole confirmation surface: the queue UI and the recall prompt. One switch, not two.
2. Default: on. Absent key: on. The empty config keeps today's behavior.
3. When off, the backend must not write the unverified-match drain file. The gates sit at the three writer call sites — `tool_dispatch.rs:682`, `:1285`, `:1326` — plus any `ct wisdom match` site OQ1 option 1 adds. The sidecar and CLI processes read the same config.json the app reads: both crates resolve it from the brain directory (`ingest_policy_for_db`, `config/mod.rs:878`, via `brain_paths_for`), so one setting governs all processes.
4. The setting is reversible. Turning it off leaves `.confirmations.jsonl` and every existing confirmation stamp untouched; rule 2's path to `user_confirmed` still exists when it is on, and turning it back on restores the queue with prior stamps intact.
5. Document the setting in INTENT rule 10's amended body, one sentence.
6. The exact name follows the existing config key style; the plan fixes it.

## 8. Open questions for the owner

- **OQ1 — prompts on the `ct wisdom match` path.** Today the never-worse fallback of `ct wisdom match` already delivers agent-tier facts with no prompt (`tools/src/queries.rs:842-913`; #282 spec line 579). The amendment makes unfiltered delivery the healthy path's behavior too. Options:
  1. Wire `emit_unverified_matches` into the `ct wisdom match` paths, so the amended pipeline prompts everywhere the turn-off setting allows. (Recommended: one accountability surface, governed by one switch.)
  2. State explicitly that `ct wisdom match` never prompts, and leave the prompt to the MCP recall paths.
- **OQ2 — is the turn-off setting in this PR or a follow-up?** The issue lists it inside the amendment scope. Recommended: same PR, one commit, so rule 10 never states a setting that does not exist.

## 9. Benchmarks ("How changes are made" step 4)

The amendment is a recall and injection change, so all gates apply:

1. No superseded fact is ever injected.
2. LongMemEval hit rate.
3. Abstention precision.
4. Paraphrase tests, at least 200, across models.
5. Exactly-once delivery.
6. Cached output stays byte-identical to fresh output for the same inputs. The cached-equals-fresh property tests pass unchanged. The activity-path fixtures and golden outputs change; they describe the old human-only delivery.

## 10. Out of scope

- Removing the `provenance_filter` parameter (follow-up commit in the same PR).
- curated-thoughts-integrations changes. CTI consumes whatever `ct wisdom match` returns and labels provenance per-hit; no source file assumes human-only injection (verified across TypeScript and Python sources).
- Any change to ranking, floors besides `:activity`, or the two-stage cell logic.
