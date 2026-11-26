# Dispositions — #381 plan review, Opus r1 (REQUEST CHANGES → all folded)

Repo: curated-thoughts, branch `fix/381-confirmations-hook-restart`.
Reviewed head: `1efcf8a1` (plan.md). Folded head: this commit.
Reviewer verdict r1: REQUEST CHANGES — 1 MAJOR, 3 MINOR. Cost $0.55 / 16
turns (cap $2.00). Controller spot-checked every load-bearing cite in the
verdict against the tree before folding: harness builds `PipelineWorker`
directly with `None` root and installs its own counting hook
(`mod.rs:1858–1875`) CONFIRMED; `tx`/`join` pub (`mod.rs:1367–1368`)
CONFIRMED; "once per switch epoch" doc comment (`mod.rs:1386–1387`)
CONFIRMED; no fixture-conventions doc CONFIRMED; cadence test
(`mod.rs:1914`) CONFIRMED.

| id | finding | disposition |
|----|---------|-------------|
| M1 | Plan tests 3–4 cannot pin the central install (harness bypasses start_pipeline; production closure unobservable from a drain test) | ADOPTED. Task 2 redesigned per the reviewer's fix: tests build a real rooted `start_pipeline` handle, lock `handle.stated_confirmations`, call the installed hook directly on a connection to the same DB, assert `Ok` + DB state. Design rule + fixture + cleanup requirements written into Task 2. Cadence not re-pinned (already pinned by `drain_hook_fires_on_every_drain_crossing`). |
| m1 | Plan silently drops spec test 3 (unrooted negative) | ADOPTED as explicit mapping: plan test 2 (slot `None`) covers spec Test 3's intent, mapping stated in the test text with a comment requirement. |
| m2 | `docs/test-fixture-conventions` does not exist | ADOPTED. Fixture cite replaced with the actual harness pattern (`tempfile::TempDir` + `temp_env::with_vars` + `CURATED_EMBED_STUB`, `mod.rs:1839–1849`). |
| m3 | Stale rationale/docs: "Run 2 (spec §2 S5)" comment dies with the deleted block; `install_stated_confirmations` doc comment says "once per switch epoch"; redundant `.clone()` | ADOPTED. Task 1 now: move the comment to the new install site, update the doc comment, drop the `.clone()`. |

Revision history: r1 folded at this commit (plan.md Task 1 + Task 2 rewritten).
