# Dispositions — #381 T4 Fable gate r1 (APPROVE WITH CHANGES → all folded)

Repo: curated-thoughts, branch `fix/381-confirmations-hook-restart`.
Range reviewed: `6f6d311e..e16e927f`. Verdict: APPROVE WITH CHANGES.
Cost $1.64 / 13 turns (cap $4.00). Reviewer could not run cargo
(read-only contract) — the ledger's gate numbers stand as
controller-executed evidence (recorded in LEDGER.md at `e16e927f`).

| id | finding | disposition |
|----|---------|-------------|
| M1 | No test pins that the WORKER fires the installed hook; deleting `.with_confirmations_slot(...)` survives all four tests — exactly the #381 class. Spec Test 2's drain-crossing shape was substituted by the plan without a spec amendment. | ADOPTED (mechanism/test-surface → fold, no owner call needed): add one end-to-end drain test on a real rooted `start_pipeline` handle — seed ledger + fact, send one counted ingest job via `handle.tx`, poll `source_type_of` until `user_confirmed` (bounded timeout), `shut_down`. Direct-call tests stay as fast unit pins. Spec §Test plan amended to record both layers. |
| m1 | Empty/missing ledger still opens a transaction and hashes every wiki entry per crossing; the diff widens that exposure to every rooted session. | ADOPTED: early `return Ok(0)` when `confirmations.is_empty()` in `apply_stored_confirmations` (`stated_ledger.rs`), with a unit pin; spec "Files touched" amended. |
| m2 | Lifecycle close-out (Status: Implemented; delete plan.md/LEDGER.md/dispositions in a cleanup commit) not done at reviewed head. | ADOPTED as a pre-ready-flip checklist item (lane is intentionally still in flight). Scheduled: status flip + cleanup commit immediately before the PR leaves draft. |
| m3 | Comment/doc nits: moved comment says "after the switch-time rebuild" but the site now also serves startup/recovery; garbled doc comment at mod.rs:1944–1945; unrooted test runs inside `with_rooted_fixture` (misleading); plan list numbering 4→6 (moot post-deletion). | ADOPTED the three code-side nits in the fix wave (comment wording per the reviewer's suggested shape, garbled doc comment fixed, unrooted test given its own minimal fixture). Plan numbering needs no action (plan deleted at close-out). |

Fix wave: same implementer session (d417c937), commits land on the lane
branch; controller re-runs Task 3 gates; T4 delta re-gate follows with
these dispositions as `--delta` anchor.
