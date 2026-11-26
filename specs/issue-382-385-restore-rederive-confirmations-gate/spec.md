# Spec: moved-root restore rederive + confirmations-leg idle-cost gate — issues #382 + #385

**Status:** Reviewed — owner approved 2026-11-26 (Q1 = 1, Q2 = 2, Discord msg 1575566029889867837) · **Branch:** `fix/382-385-restore-rederive-confirmations-gate`
**Lane:** PR-2 of the 2026-11-26 three-lane ruling (decision log D1) — one PR, one spec dir, three sections (A = #382, B = #385 m1, C = #385 m4).
**Recon:** every pin below verified against `211c1e79` (recon report + controller re-check); line cites refer to that base.

## 1. Problem

### A — #382: restoring a stamped backup at a MOVED root leaves old-root keys

`switch_vault`'s restore arm (`lib.rs:2261-2310`) installs the backup as a
whole-file swap (`commit_staged_backup`, single rename at `:2283`), aligns the
config to the new path (`set_vault_path` before the reopen, `:2304-2309`), and
returns — it never sweeps and never rederives. The sweep + "Run 1" rederive
exist only in the in-place arm (`lib.rs:2318-2333`, gate `if swept` at
`:2343-2346`).

The backup is a whole-file copy (`lib.rs:1030-1056`), so it carries the
`stated_ledger_rederived` stamp — which suppresses the stamp-gated open-time
rederive (`connection.rs:905-960`). Net: after a restore at a moved root, the
stated-ledger derived tables keep keys built from the OLD canonical root:

- `stated_attestations` keys are `virtual_path_key(vault_root.join(document_path))`
  (`stated_ledger.rs:399-410`), where `virtual_path_key` is
  `std::path::absolute` — root-embedded, non-canonicalizing (`queue.rs:39-43`);
- deposit attribution keys likewise (`stated_ledger.rs:503`, `:798`), and the
  provenance join `da.document_path = d.path` (`queue.rs:307`) silently misses
  because post-move `documents.path` rows use the new root;
- the backup records NO capture-time root anywhere (the stamp is the bare
  string `'1'` in `llm_wiki_meta`), so nothing can even detect the move today.

Normal same-root restores are unaffected (the backup's own keys are correct).

### B — #385 m1: full wiki scan per idle crossing once the ledger is non-empty

`apply_stored_confirmations` (`stated_ledger.rs:453`) early-returns only on an
EMPTY ledger (`:457-461`). With ≥1 confirmation on disk, every 1→0 drain
crossing reads the ledger and then loads + hashes every live
`llm_wiki_entries` title+body (`apply_confirmations_by_hash`,
`stated_ledger.rs:309-331`).

Mechanism (pinned): after every completed job, `pipeline/mod.rs:414` sets
`probe_next` when pending == 0 (also true for uncounted `PipelineJob::ingest`
jobs, `mod.rs:48-53`); the next recv becomes
`recv_timeout(DRAIN_PROBE = 250ms)` (`mod.rs:103`) and on timeout the
installed hook runs `apply_stored_confirmations` (`mod.rs:211-261`). Since
#384 (merge `3a9bc2a2`) the hook is installed for EVERY rooted start
(`mod.rs:1456-1471`), not just post-switch — the whole class pays the scan on
every crossing. Attribution correction vs the issue text: `lib.rs:2619` is
`queue_full_reindex`, not the watcher; the watcher/sweep path uses
`ingest_counted` (`watchdog/sweep.rs:138`) — but both trigger the same hook,
so the fix is at the hook, not at any one producer.

There is no existing cheap gate: the pending counter cannot discriminate, no
entries-written counter exists, and there is no last-fire bookkeeping in the
worker or handle.

### C — #385 m4: test hygiene in the #381 test surface

1. Duplicated ledger-seeding test block at `pipeline/mod.rs:2045-2056` and
   `:2076-2087` (tests `installed_hook_stamps_confirmation` `:2040` and
   `drain_fires_installed_hook_end_to_end` `:2071`).
2. Zero-byte / blank-line ledgers are not pinned through
   `apply_stored_confirmations`; only the missing-file case is pinned
   (`stated_ledger.rs:1128-1133`). Both cases share the same `read_lines`
   branch (`:127-146`: missing → `[]`, blank lines skipped).
3. `install_stated_confirmations` (`pipeline/mod.rs:1389`) is `pub` with no
   caller outside its module since #384 (sole caller `:1468`; no test/bench
   callers — repo-wide grep).

## 2. Proposed change

### A — restore re-derives unconditionally (recommended; Q1)

**Structural retry (T4 M2):** the rederive stamp is cleared from the STAGED
backup before the install rename (`restore_sync::clear_staged_rederive_stamp`,
called from the switch's restore arm before `commit_staged_backup`).
Whichever open comes next — the happy-path reopen, a failed-step recovery
reopen (failures after the install rename keep the installed backup and
align the config), or the startup reconciliation after a crash mid-install —
finds no stamp and rederives from the root the config names. A failed clear
fails the switch before the point of no return, with the outgoing database
still in place.

At restore completion — after `commit_staged_backup` publishes the captured
backup and the config is aligned to the new root, before/at the reopen (the
plan's Task 1 ordering is the authority; "before/at the reopen" here means
the same window) — run the same one-shot rederive the switch path already
uses:
`rebuild_rederive(conn, canonical_new_root)` (pub, one tx, idempotent,
`stated_ledger.rs:356`; canonicalizes its own arg `:365`; Run 1 precedent at
`lib.rs:2346`). Because the backup records no capture-time root, no reliable
moved-detection exists — and unconditional rederive is correct for the
same-root case too (it is a no-op-equivalent re-key of already-correct keys,
at a cost the switch path already pays on every in-place switch).

- Precedent pins: stamp delete/query already exist in the repair flow
  (`connection.rs:4529`, `:4550`); the unstamp seam `sweep_and_unstamp`
  exists in `db/switch_pin_tests.rs:74-84` (stamp DELETE at `:81`), and the
  restore-order pin test `:130-166` already documents that production restore
  does NOT sweep — it gets extended, not replaced.
- **Failure story (differs from Run 1 on purpose):** Run 1 declares rederive
  failure permanent (the stamp gate blocks re-runs, `lib.rs:2348-2364`). For
  restore, delete the stamp on failure so the next open-time gate
  (`connection.rs:905-960`) retries — a restored brain that failed one
  rederive must not be frozen with stale keys.
- Alternative rejected: (a2) write the capture-time canonical root into the
  backup at snapshot time and rederive only on mismatch — adds a meta write
  to the capture flow and a detection branch whose only payoff is skipping a
  rederive the restore path can afford; (b) document moved-restore as
  unsupported — leaves silent stale-key residue behind, which the issue
  exists to remove.

### B — entries-written + stat dual gate on the confirmations hook (ruling Q2=2)

Gate the hook leg on a **dual condition**: the ledger stat memo **OR** an
entries-written dirty flag. A stat memo alone is WRONG: new facts ingested
across drain waves change `llm_wiki_entries` while
`<root>/stated/.confirmations.jsonl` — built as
`root.join(STATED_DIR).join(CONFIRMATIONS_LEDGER)` (`stated_ledger.rs:25`,
`:455`; `STATED_DIR` is a directory constant, so the bare file name alone is
NOT the path) —
stays unchanged, and `apply_confirmations_by_hash` must re-scan so the
newly-ingested entries can match confirmed hashes (owner ruling, Discord
msg 1575566029889867837, 2026-11-26).

- **Ledger stat memo:** `(mtime_nanos, len)` of the ledger file, captured in
  the hook's state. The hook closure already captures `root`
  (`mod.rs:1469`); the memo lives next to it. The confirmations slot is
  `Arc<Mutex<Option<ConfirmationsHook>>>` (`mod.rs:1426-1453`) and
  `respawn_worker` reuses the slot, so state survives worker respawns
  (unlike worker fields). **Capture order is stat-before-read:** the stat
  compared/stored is taken BEFORE the apply's read, and on success the
  memo stores that pre-read stat. A ledger append that lands at or after
  the stat (during or after the apply) then changes the file's stat and
  forces a rescan on the next fire. Statting after the read instead would
  record a not-yet-applied append as already seen (unsafe direction).
- **Entries-written dirty flag:** a flag in the same shared state, set by the
  job-completion path when the completed job WROTE entries (ingest /
  rechunk / any variant that inserts or changes `llm_wiki_entries` rows —
  the implementer verifies the exact variant set from the tree and STOPs if
  it is ambiguous; pure-read jobs never set it). The flag is CONSUMED at the
  start of an apply (swapped to false before the apply runs) and re-armed if
  the apply does not succeed — error or panic, since the drain probe runs
  the hook under `catch_unwind` and retries the leg — so the next fire
  retries regardless of which trigger fired; the stat memo likewise never
  advances past a failed apply. A panicking hook poisons the slot mutex, so
  all three slot locks (probe, fire, install) recover with `into_inner`
  (`lock_confirmations_slot`): without that recovery the one panic the
  `catch_unwind` leg survives would read "hook not installed" forever and
  the retry above would never run. **Writer invariant (multi-generation):** the
  watchdog can supersede a wedged worker (`mod.rs:1373-1378`), so two
  dirty-setters can transiently coexist — the superseded worker's job-tail
  `mark_dirty` (the worker's job-tail call) is unconditional and
  epoch-blind, and a
  set-then-clear loss against an in-flight apply WAS possible while the
  clear ran after the apply. Consume-before-apply closes that race by
  program order: a worker's entry writes precede its `mark_dirty`, so
  against a fire that consumed the flag before its apply, either the
  apply's SELECT sees the writes or the mark lands after the consume and
  survives to the next fire.
  Regression tests:
  `mark_during_apply_survives_the_clear`,
  `panicking_apply_with_unchanged_ledger_rearms_dirty`,
  `panicking_hook_is_reinvoked_on_the_next_crossing`.
- **Fire path unchanged** (probe cadence `mod.rs:211-261`, `DRAIN_PROBE`
  250ms, panic-safety, drain semantics). Hook leg:
  - first fire (no memo): always applies;
  - otherwise: skip read + scan only when the stat is unchanged AND the
    dirty flag is clear; else apply exactly as today (the existing
    empty-ledger early return `:457-461` stays as the inner fast path), then
    store the PRE-read stat into the memo — the stat is captured before the
    read, so an append landing at/after the capture resolves toward
    "changed stat next fire" (the safe direction).
- Alternative rejected: stat memo alone (breaks the multi-wave ingest case,
  per the ruling); leave-as-is (the #381 spec accepted the cost once; #384
  widened who pays it to every rooted session).

### C — hygiene (mechanical, no owner question)

1. Extract `seed_confirmation_ledger(root, title, body)` and use it at both
   duplicated sites (`pipeline/mod.rs:2045-2056`, `:2076-2087`).
2. New pins through `apply_stored_confirmations`: zero-byte ledger file and
   blank-lines-only ledger file → `Ok(0)`, no panic — and no wiki scan, which
   is delivered by the empty-ledger early return that precedes the
   transaction and the walk (`stated_ledger.rs`, `apply_stored_confirmations`).
   The pins live in `stated_ledger.rs`'s test module (the no-scan guarantee's
   home); `pipeline/mod.rs` additionally pins the hook seam: an installed
   hook over an empty ledger stamps nothing.
3. `install_stated_confirmations` `pub` → private (`fn`); sole caller
   `:1468` is in-module; repo-wide grep shows no test/bench callers.

## 3. Files touched

1. `src-tauri/src/lib.rs` — restore arm: rederive call + unstamp-on-failure;
   staged-backup stamp clear before the install rename (§2-A structural
   retry).
2. `src-tauri/src/db/restore_sync.rs` — `clear_staged_rederive_stamp` (the
   switch call site + test; no change to the install rename itself).
3. `src-tauri/src/pipeline/mod.rs` — hook memo gate; seed helper; visibility
   narrowing; B-section unit tests.
4. `src-tauri/src/db/switch_pin_tests.rs` — moved-root restore pin; rederive
   failure → unstamp → open-retries pin; same-root restore pin.
5. `src-tauri/src/db/stated_ledger.rs` — zero-byte/blank-line pins (test
   module only; no prod change expected).

## 4. Test plan

1. **Moved-root restore pin:** brain at root A, ≥1 attestation stamped;
   snapshot backup; restore at root B; assert `stated_attestations` keys are
   re-derived under B (and deposit attribution rows re-keyed) — extends
   `switch_pin_tests.rs:130-166`.
2. **Same-root restore pin:** restore at the same root; assert keys unchanged
   and rederive ran (log/report parity, not key churn).
3. **Failure-unstamp pin:** inject rederive failure at restore; assert stamp
   absent; reopen → open-time gate rederives (`connection.rs:905-960`).
4. **Dual-gate units:** hook with unchanged ledger and no entries-written
   flag → scan skipped; entries-written flag set (worker job completed) or
   ledger append → scan runs, memo/flag updated per §2-B; first fire → scan
   runs.
5. **Hygiene pins:** zero-byte + blank-line ledgers → `Ok(0)`; helper dedupe
   compiles both tests; `install_stated_confirmations` no longer visible
   outside the module (compile-enforced).
6. Full suite: `cargo test` (src-tauri) + clippy; serial-Rust rule (single
   workstream; heavy-build wrapper with kv-work exports).

## 5. Risks / trade-offs

- **A:** every restore pays one rederive tx (same cost class as an in-place
  switch's Run 1 — already accepted there). Restores are rare, user-invoked.
- **B:** mtime granularity varies by filesystem; `mtime_nanos` + len
  combined make false-skip windows negligible and self-healing (next real
  change re-arms). Hook semantics (when it fires) are untouched.
- **A+B do not interact:** B gates the read of the CURRENT root's ledger;
  A guarantees the derived tables match the CURRENT root before any hook
  fire matters.

## 6. Decisions

1. **Q1 (owner, RULED 2026-11-26): #382 shape = (a1).** Unconditional
   rederive at restore + unstamp-on-failure (next open retries), per §2-A.
2. **Q2 (owner, RULED 2026-11-26): #385 m1 gate = dual gate.** Ledger stat
   memo AND entries-written dirty flag (consumed at apply start, re-armed
   on error or panic; the memo never advances past a failed apply),
   per §2-B — a stat memo alone was explicitly rejected by the owner: it
   would miss newly-ingested facts across drain waves while the
   confirmations file is unchanged.
3. C section is mechanical — no ruling needed.
4. PR topology per D1: ONE PR for all three sections (Single PR delivery;
   lifecycle cleanup last LIFECYCLE commit before the ready flip).
