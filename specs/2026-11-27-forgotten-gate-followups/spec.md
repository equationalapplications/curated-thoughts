# Spec: forgotten-gate follow-ups — bundle round-trip, traversal hoist, live index (#398)

**Status:** Reviewed (owner-approved 2026-11-27 — Kurt, Discord msg 1575947241263472833; two implementation notes folded into plan: strict comment-scoped `forgotten:` parsing with byte-identical id-only lines, and full traversal-entry-point sweep for arm pass-down)
**Date:** 2026-11-27
**Ruling issue:** equationalapplications/curated-thoughts#398
**Folded issues:** #399, #400, #401 (one PR, owner-directed single-delivery fold; owner directive 2026-11-27: no new issues this session — every new finding lands in this PR)
**All file:line evidence against `origin/main` @ `fc715239` (3.18.0).**

## Problem

Four follow-ups from the #378 lane (PR #395) share one subsystem and one
root cause: the forgotten gate's data and probes are incomplete or
repeated. The PR fixes all four in one review-manageable diff.

1. **Bundle round-trip loses `forgotten_at` (#398).** Export reads
   `id, event_type, summary, related_entry_id, created_at` with no
   `forgotten_at` (`src-tauri/src/db/bundle_io.rs:226-233`). `ExportEvent`
   has no field for it (`src-tauri/src/okf/bundle_write.rs:18-29`). Import
   inserts fresh live rows
   (`src-tauri/src/db/bundle_apply.rs:737-748`). A bundle exported from a
   brain with forgotten events resurrects them as live events on every
   importing brain. This breaks INTENT rule 12 (the Rust side follows the
   engine's words) for the backup/restore surface.
2. **Per-node schema probes in graph traversal (#400, plus the perf
   residue of #399).** `load_live_entry`
   (`src-tauri/src/wiki_graph.rs:466-490`) and `live_entry_exists`
   (`src-tauri/src/wiki_graph.rs:1150`) run two `PRAGMA table_info`
   probes per node visited. `fetch_outbound_neighbors` and
   `fetch_inbound_neighbors` (`src-tauri/src/wiki_graph.rs:628-707`)
   build the gate arm twice per call, four probes per node per
   direction. Deep or wide walks repeat identical checks per node.
3. **Live-half lookup has no serving index (#401).** The survival guard
   (`src-tauri/src/wisdom_match.rs:644-657`) keeps an entry when a live
   backing event exists (`le.forgotten_at IS NULL`). The partial index
   `llm_wiki_events_related_forgotten_idx` serves only the forgotten
   half; its rows all have `forgotten_at NOT NULL`. `EXPLAIN QUERY PLAN`
   shows `SCAN le` for the live subquery. Cost materializes only on
   brains with forgotten events; production census at filing time:
   0/1,458 rows affected.
4. **Scheme-free probe (#399 item 1) is already fixed on main.** Commit
   `4ac2ca12` made every gate probe shape-only via
   `temporal_columns_shape_only` (`src-tauri/src/wisdom_match.rs:321-345`):
   gate reads no longer touch `llm_wiki_meta` and cannot fail on a corrupt
   scheme value. This PR verifies that claim with a test and closes #399
   with #400's perf work. No new code is needed for #399 item 1.

## Rejected alternatives (fold scope)

- **Fold #397 and #390 too.** Rejected. #397 touches the kick-ledger
  privacy path; #390 spans commit.rs and nine unrelated nits. Neither
  shares files or review surface with this diff. Multi-purpose PRs get
  split; both keep their own issues and lanes.
- **Gate the export reads instead of carrying the flag (#398 sketch).**
  Rejected per #398: dropping forgotten rows from bundles loses
  tombstone data and changes backup semantics.
- **New bundle format version or JSON sidecar for events.** Rejected:
  format churn. The log-line grammar extension below is backward
  compatible in both directions.
- **Memoize probes per connection instead of pass-down (#400).**
  Rejected: connection-level caches hide mid-life schema changes and
  add hidden state. Pass-down keeps one explicit shape resolution per
  walk. Reviewer-suggested shape in #400 is pass-down.

## Proposed change

### Task 1 — carry `forgotten_at` through export (#398)

- Add `pub forgotten_at: Option<i64>` to `ExportEvent`
  (`src-tauri/src/okf/bundle_write.rs:18`).
- `load_events` (`src-tauri/src/db/bundle_io.rs:222`) selects
  `forgotten_at` and populates the field.
- Extend the event-line grammar. The log line today is
  `({event_type}) {summary} <!-- id: {event_id} -->`. When
  `forgotten_at` is `Some(ms)`, the comment gains a `forgotten:` key:
  `<!-- id: evt_x forgotten:1732713600000 -->`. Extend
  `append_event_id_comment` and `parse_event_id_comment`
  (`src-tauri/src/okf/log_md.rs:33-70`) to write and read the key.
  Lines without the key parse exactly as today. A non-numeric
  `forgotten:` value parses as absent.

### Task 2 — apply `forgotten_at` verbatim on import (#398)

- Add `pub forgotten_at: Option<i64>` to `ParsedEvent`
  (`src-tauri/src/okf/bundle_read.rs:18-25`); `bundle_read.rs:258`
  populates it from the parsed comment.
- The shared event insert path (`src-tauri/src/db/bundle_apply.rs:737-748`)
  must set `forgotten_at` when the row carries one and the target
  `llm_wiki_events` table has the column. Probe the column once per
  `apply_import` call with `existing_columns`, not once per event.
- Pre-v19 targets (no `forgotten_at` column): insert without the
  column, no error, counts unchanged. Bundles exported by older
  binaries (no `forgotten:` key): import as live events, exactly as
  today. Merge and Replace modes share the path and both carry the
  flag.

### Task 3 — compute gate arms once per traversal (#400, closes #399)

- Build the three arm strings (`arm_s`, `arm_t`, entry arm) once at
  each traversal entry — `scoped_traverse`
  (`src-tauri/src/wiki_graph.rs:799`) and `cross_partition_traverse`
  (`src-tauri/src/wiki_graph.rs:909`) — and pass them down through
  `fetch_neighbors` into `fetch_outbound_neighbors` and
  `fetch_inbound_neighbors`, and to `load_live_entry` and
  `live_entry_exists`.
- Gate behavior must not change. The fail-closed contract on scoring
  paths stays. `temporal_columns_shape_only` remains the only probe
  the gate uses. A schema change mid-walk now observes the walk-start
  shape; this is accepted and must be stated in a code comment.
- Add one test asserting `entry_not_forgotten_arm_for` and the
  shape-only readers never query `llm_wiki_meta` (closes the #399
  item-1 verification claim).

### Task 4 — live-half partial index (#401)

- In `apply_llm_wiki_v18_v19_partial_indexes`
  (`src-tauri/src/db/okf_ddl.rs:379-401`), inside the existing
  `forgotten_at` + `related_entry_id` guard, create:

  ```sql
  CREATE INDEX IF NOT EXISTS llm_wiki_events_related_live_idx
    ON llm_wiki_events(related_entry_id)
    WHERE forgotten_at IS NULL AND related_entry_id IS NOT NULL
  ```

- Register `index:llm_wiki_events_related_live_idx` as CT_OWNED in the
  engine-parity allowlist (`src/__tests__/engine9-setup-compat.test.ts`,
  next to `index:llm_wiki_events_related_forgotten_idx`).
- Index creation must tolerate pre-v19 shapes, like the forgotten
  index does (same guard already provides this).
- The both-columns guard must mirror the probe guard in
  `forgotten_entry_arm` (`src-tauri/src/wisdom_match.rs:644-657`).

## Files touched

- `src-tauri/src/okf/bundle_write.rs` — ExportEvent field, event text.
- `src-tauri/src/okf/log_md.rs` — comment grammar write + parse.
- `src-tauri/src/okf/bundle_read.rs` — ParsedEvent field.
- `src-tauri/src/db/bundle_io.rs` — export SELECT.
- `src-tauri/src/db/bundle_apply.rs` — import INSERT + shape probe.
- `src-tauri/src/wiki_graph.rs` — arm pass-down (tasks 3).
- `src-tauri/src/db/okf_ddl.rs` — live-half index (task 4).
- `src/__tests__/engine9-setup-compat.test.ts` — allowlist row (task 4).
- Unit tests live in the files above, matching existing conventions.

## Test plan

All Rust suites run with
`CARGO_TARGET_DIR=<repo>/.cargo-shared-target` exported and ≥30G free
on the build-cache filesystem (repo AGENTS.md build discipline).

1. **Round-trip test (#398 acceptance):** seed an entry plus two
   events in a temp brain, set `forgotten_at` on one event via the
   engine-shape UPDATE, export a bundle, apply it to a fresh temp
   brain, assert the imported event row carries `forgotten_at` and the
   entry stays hidden by the gate on the importing brain.
2. **Back-compat tests:** a bundle with no `forgotten:` key imports as
   live events; applying to a pre-v19 target (column dropped) inserts
   without error and unchanged counts.
3. **Grammar unit tests:** append/parse round trip with and without
   the key; non-numeric value parses as absent; existing id-only lines
   byte-identical.
4. **Traversal tests (existing, must stay green):
   `forgotten_gate_closes_graph_bypass_sites`,
   `forgotten_neighbor_edges_are_gated_in_traversal`.** Plus a new
   no-meta-read test for gate probes (task 3).
5. **Index tests:** a guard test mirroring
   `related_forgotten_idx_is_guarded_on_both_columns`
   (`src-tauri/src/db/connection.rs:8200`); `EXPLAIN QUERY PLAN` on the
   arm's live subquery shows the new index serving it. Record the EQP
   output in this spec's Errata section — the durable home, since
   plan.md and LEDGER.md are deleted at cleanup.
6. **Frontend:** the engine-parity vitest suite
   (`src/__tests__/engine9-setup-compat.test.ts`) must pass; run it
   per the frontend-vitest-gates reference (jsdom builtin bundling,
   cold-cache repro rules).
7. **Full gates:** `cargo --manifest-path src-tauri/Cargo.toml test
   --lib --features test-utils` and `cargo --manifest-path
   tools/Cargo.toml test --lib --features test-utils`, both green; no
   new clippy findings versus the base commit.

## Non-goals

- Gate semantics stay frozen: no change to fail-closed behavior, the
  survival-guard predicate, or any carve-out.
- No bundle-format version bump; no engine bump required.
- #397 and #390 stay out (rejected alternatives above).
- Per Kurt's 2026-11-27 directive: no new issues from this lane — any
  new finding lands in this PR if review-manageable.
