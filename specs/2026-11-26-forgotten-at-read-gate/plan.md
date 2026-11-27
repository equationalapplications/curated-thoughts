# forgotten-at-read-gate Implementation Plan (#378, PR #395)

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development
> to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** a forgotten event suppresses its data at read time. Event readers drop
forgotten events. Entry readers hide an entry only when it has backing events and
zero of them are live. Stated rows and structural containers are never hidden.

**Architecture:** one shared SQL predicate. The event arm is a direct
`forgotten_at IS NULL` column predicate. The entry arm is a survival-guard
anti-join plus carve-outs, interpolated by `entry_eligibility_where` and by a
small helper at each bypass site. A partial index serves the anti-join. All
fragments are shape-conditional: a pre-v19 database compiles without them.

**Tech stack:** Rust (`src-tauri` crate), SQLite via rusqlite, existing test harness.

**Spec:** `specs/2026-11-26-forgotten-at-read-gate/spec.md` — the binding authority.
On conflict, the spec wins. OQ5 keeps the spec default: the `source_type` carve-out
ships in SQL; the attestation join stays documented as the owner's optional widening.

## Global constraints

- Branch: `spec/378-forgotten-at-read-gate`. Worktree:
  `/mnt/kv-work/build/worktrees/curated-thoughts/ct-395-impl`. The controller pushes.
- Task 0 (before Task 1): run `git merge origin/main` in the worktree. Re-verify the
  plan's anchors after the merge; the merge may shift lines. The spec's anchors came
  from `bbe15dc2`; minor drift is already known (`rank_wiki_entries` now at
  `tool_dispatch.rs:434`, `dispatch_curated_get_wiki_entry` at `:730`,
  `wiki_search_in_scheme` at `wiki_graph.rs:366`).
- Cargo is SERIAL machine-wide. Build location authority: repo `AGENTS.md` wins.
  Export `CARGO_TARGET_DIR=/home/kv-thinkpad-t420-ubuntu/code/github/equationalapplications/curated-thoughts/.cargo-shared-target`
  (ABSOLUTE, primary checkout, 13G warm cache). Pre-flight: `df -h /` ≥ 30G.
  Export `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0` on every cargo run.
- Wrap every full-suite gate in `~/.hermes/bin/heavy-build` (`heavy-build -- <cmd>`;
  script files, not inline pipes; `--need-mb 2000-3000`). Targeted module tests may
  run bare. Run suites from `src-tauri/` with `PATH=$HOME/.cargo/bin:$PATH`.
- CI-exact gates before every push: `cargo test --features test-utils,mcp-server -- --test-threads=1`
  and `cargo clippy --all-targets --features test-utils -- -D warnings`.
- Never run `cargo fmt` write mode. Never rustfmt a file you did not author.
- The closeout gate is the FULL unfiltered suite. Per-task filtered runs do not
  execute other modules' structural guard tests.
- Every task is RED-first: the failing test runs and its output is captured to a
  receipt file under `.superpowers/sdd/issue-378/` (gitignored) before the fix.
- Spec changes and code changes land in separate commits. `plan.md` and `LEDGER.md`
  are transient; the cleanup commit deletes them.
- The gate is read-only at query time. No reader writes `forgotten_at`. No reaper,
  no backfill, no vault-file edits (INTENT rule 9).

## Rejected alternatives

- We rejected post-fetch filtering because it starves top-k (spec D3).
- We rejected an entry-level `forgotten_at` column because the engine owns the
  forget verb and CT is read-side only (spec E1).
- We rejected "any forgotten event hides the entry" because `related_entry_id` is
  many-to-one and one forget must not delete a shared rule (spec E4, ruling #2).
- We rejected per-tool duplicated filters because ruling #1 requires one shared
  predicate (spec D1).

---

### Task 0: sync the branch with origin/main

- [ ] `git fetch origin && git merge origin/main` in the worktree (merge commit is fine).
- [ ] Re-run the anchor greps for every `file:line` this plan cites. Fix drifted
      anchors in the task briefs before any dispatch.

### Task 1: shape probe field + partial index mirror (spec D2)

**Files:**
- Modify: `src-tauri/src/wisdom_match.rs` — `Temporal` probe struct (near `:294`)
  and `temporal_columns` (near `:320`): add a `has_forgotten_at` (name at implementer's
  discretion, matching the `has_superseded_by` pattern) field probed from the
  `llm_wiki_events` shape.
- Modify: `src-tauri/src/db/okf_ddl.rs` (near `:387`) — add the partial index
  `llm_wiki_events_related_forgotten_idx ON llm_wiki_events(related_entry_id)
  WHERE forgotten_at IS NOT NULL AND related_entry_id IS NOT NULL`, guarded on both
  columns existing, mirroring the `forgotten_idx` pattern.
- Modify: `src-tauri/src/db/connection.rs` (near `:8192`) — add the index to the
  schema-guard expectation list, guarded the same way.

- [ ] **Step 1 (RED):** write the failing tests — index present when the columns
      exist, absent otherwise (mirror the `connection.rs:8192` pattern); probe field
      true on a v19 shape, false on a pre-v19 shape. Capture outputs to receipts.
- [ ] **Step 2:** run the filtered suite (`-- db::connection` plus the probe test) — FAIL.
- [ ] **Step 3:** implement the probe field and the index mirror.
- [ ] **Step 4:** run again — PASS. Run clippy on touched targets.
- [ ] **Step 5:** commit — `feat(db): forgotten-event anti-join index mirror + shape probe (issue #378)`.

### Task 2: survival-guard arm in the shared predicate (spec D1.2)

**Files:**
- Modify: `src-tauri/src/wisdom_match.rs` — `entry_eligibility_where` (near `:577`).
  When the shape has `forgotten_at`, the fragment gains the carve-out block and the
  survival-guard block from spec D1.2, verbatim in logic: never hide
  `source_type = 'user_stated'`; never hide a structural container
  (`e.id = e.entity_id` OR `e.id LIKE 'tier%'` OR `e.entity_id LIKE 'tier%'`);
  hidden iff `EXISTS` a backing event AND `NOT EXISTS` a live one
  (`related_entry_id = e.id`, `forgotten_at IS NULL` for live).
- Test: `src-tauri/src/wisdom_match.rs` tests (shared-fragment parity trap).

- [ ] **Step 1 (RED):** seed fixtures per spec test plan 1–3: (a) parity — an entry
      with one fully-forgotten event disappears from `gated_entries`,
      `eligible_entry_ids`, and `candidate_set`, and reappears when `forgotten_at`
      clears; (b) survival — one entry, two events, forget one: still visible in all
      three; forget both: hidden; re-add a live event: visible again; (c) carve-outs —
      a `user_stated` row and each container-arm row stay visible with all events
      forgotten. Capture RED receipts.
- [ ] **Step 2:** run — FAIL.
- [ ] **Step 3:** implement the fragment.
- [ ] **Step 4:** run — PASS. Degraded-shape check: same fixtures on a pre-v19
      shape compile and match today's behavior (spec test plan 4).
- [ ] **Step 5:** commit — `feat(wisdom_match): forgotten-facts survival guard in the shared entry predicate (issue #378)`.

### Task 3: close the bypass sites (spec D1.3)

**Files:**
- Add: a small helper (name at implementer's discretion, e.g.
  `entry_not_forgotten_where(conn)`) that returns the identical arm text, shape-probed.
- Modify: `src-tauri/src/tool_dispatch.rs` — `rank_wiki_entries` (near `:434`) and
  `dispatch_curated_get_wiki_entry` (near `:730`): interpolate the shared arm.
- Modify: `src-tauri/src/wiki_graph.rs` — `wiki_search_in_scheme` (near `:366`) and
  the entry-existence probes (near `:469`, `:1145`, `:2227`): interpolate the arm.
- Test: parity tests covering all six sites.

- [ ] **Step 1 (RED):** extend the parity trap: a fully-forgotten entry is absent
      from `rank_wiki_entries`, `curated_get_wiki_entry`, `wiki_search_in_scheme`,
      and never re-enters via `wiki_context` / `wiki_traverse_graph`; a
      survival-guard entry (mixed events) stays reachable at every site. Capture RED.
- [ ] **Step 2:** run — FAIL.
- [ ] **Step 3:** implement. One definition only — every site calls the helper or
      the shared fragment. No duplicated SQL text.
- [ ] **Step 4:** run — PASS.
- [ ] **Step 5:** commit — `feat(read-paths): route all entry read paths through the forgotten gate (issue #378)`.

### Task 4: event readers drop forgotten events (spec D1.1)

**Files:**
- Modify: `src-tauri/src/db/entities.rs` — `load_events` (near `:652`): add
  `AND forgotten_at IS NULL` to the WHERE, shape-conditional.
- Modify: `src-tauri/src/db/events.rs` — the `llm_wiki_events` arm of `list_events`
  (near `:64-82`): same arm, shape-conditional.
- Audit, do not change: `src-tauri/src/db/queue.rs:2179` (internal replay scan —
  must still see forgotten events). State the audit result in the LEDGER.

- [ ] **Step 1 (RED):** seed a forgotten event and a live event; assert
      `load_events` and `list_events` return only the live one; assert the replay
      scan still returns both. Capture RED.
- [ ] **Step 2:** run — FAIL.
- [ ] **Step 3:** implement.
- [ ] **Step 4:** run — PASS. Degraded shape: no `forgotten_at` column — both
      readers compile and return everything.
- [ ] **Step 5:** commit — `feat(events): event readers honor forgotten_at (issue #378)`.

### Task 5: starvation, cross-surface, closeout gates (spec D3, test plan 6/9)

**Files:**
- Test: extend the wisdom_match / tool_dispatch test files.

- [ ] **Step 1 (RED):** starvation fixture — one fully-forgotten entry scoring
      highest among six; assert the top-k returns five live entries, no hole.
      Cross-surface fixture — the engine (TS, `core-llm-wiki` test harness) forgets
      an event; the Rust gate drops the entry per the survival guard. Capture RED.
- [ ] **Step 2:** run — the starvation test must already PASS (the design pushes the
      predicate into candidate selection); if it fails, the predicate leaked into a
      post-fetch position — fix the design, not the test. The cross-surface test may
      FAIL until Task 2–4 code is present; treat its RED as confirmation of scope.
- [ ] **Step 3:** commit — `test(read-paths): top-k starvation guard + engine-agreement fixtures (issue #378)`.
- [ ] **Step 4 (closeout gates):** FULL unfiltered CI-exact suite
      (`cargo test --features test-utils,mcp-server -- --test-threads=1`) and
      CI-exact clippy (`cargo clippy --all-targets --features test-utils -- -D warnings`),
      both under `heavy-build`, receipts captured. Aggregate pass/fail counts from
      the log; read real output.

### Task 6: review loop, cleanup, ready flip (controller-owned)

- [ ] Independent reviewer subagent over the full branch diff (diff vs spec/plan,
      real test output, `git status --short`, deviation adjudication). Fix findings
      with new commits; re-review until clean.
- [ ] Cleanup commit: spec Status → `Implemented`; delete `plan.md` and `LEDGER.md`.
- [ ] Push; `gh pr ready`; bot-gauntlet loop per the delivery flow; owner merge.
