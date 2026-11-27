# Spec: forgotten-gate follow-ups — bundle round-trip, traversal hoist, live index (#398)

**Status:** Implemented (2026-11-27 — all 7 plan tasks landed TDD-first and task-reviewed; gates green. Originally Reviewed: owner-approved 2026-11-27 — Kurt, Discord msg 1575947241263472833; two implementation notes folded into plan: strict comment-scoped `forgotten:` parsing with byte-identical id-only lines, and full traversal-entry-point sweep for arm pass-down. r1 2026-11-27: Opus-medium plan review REQUEST CHANGES → all findings verified and folded; spec amended: round-trip fixture redesign, pre-v19 export tolerance, dup reconciliation, one-way compat note, traversal-entry wording. r2 2026-11-27: delta review REQUEST CHANGES → folded; spec amended: TWO public traversal entries (`wiki_traverse_graph` + `wiki_context_in_scheme`), dup UPDATE column-guarded, `okf_bundle` integration gate added. r3 2026-11-27: delta review REQUEST CHANGES (0 blockers) → folded; spec test-plan amended: tools gate plain `--lib`, second-entry gate coverage via caller-resolved scheme)
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
  format churn. The log-line grammar extension is forward compatible
  (old bundles import unchanged). It is ONE-WAY compatible backward:
  an older binary importing a new bundle parses the extended comment as
  an invalid id and imports the affected event under a generated id
  (r1-M2, disclosed to owner 2026-11-27; affects only events carrying
  the `forgotten:` key, and only cross-version bundle exchange).
- **Memoize probes per connection instead of pass-down (#400).**
  Rejected: connection-level caches hide mid-life schema changes and
  add hidden state. Pass-down keeps one explicit shape resolution per
  walk. Reviewer-suggested shape in #400 is pass-down.

## Proposed change

### Task 1 — carry `forgotten_at` through export (#398)

- Add `pub forgotten_at: Option<i64>` to `ExportEvent`
  (`src-tauri/src/okf/bundle_write.rs:18`).
- `load_events` (`src-tauri/src/db/bundle_io.rs:222`) selects
  `forgotten_at` and populates the field. The export entry probes the
  column ONCE with `existing_columns` and selects `NULL AS forgotten_at`
  on a pre-v19 brain, so exporting never fails on a degraded shape
  (r1-M1 — same tolerance every other surface shows).
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
- **Dup reconciliation (r1-M3):** a duplicate that arrives forgotten
  must tombstone the target's live row (`UPDATE … SET forgotten_at = ?
  WHERE id = ? AND forgotten_at IS NULL`), or an already-synced brain
  keeps the resurrection alive forever. Tombstone wins on conflict: a
  target row already forgotten is never un-forgotten by an incoming
  live event. The UPDATE is guarded by the same column probe as the
  INSERT (r2-B2): a pre-v19 target never names the column, so a dup
  import on a degraded shape cannot abort the transaction.

### Task 3 — compute gate arms once per traversal (#400, closes #399)

- Build the three arm strings (`arm_s`, `arm_t`, entry arm) ONCE at EACH
  PUBLIC traversal entry (r2-B1: there are TWO — `wiki_traverse_graph`
  `src-tauri/src/wiki_graph.rs:779` AND `wiki_context_in_scheme`
  `src-tauri/src/wiki_graph.rs:2418`, which `wiki_context` `:2404`
  delegates to; `wiki_context`'s `CompositeWalk::walk_seed` sites
  `:2274/:2306/:2344` are production code and take the walk's gate) —
  and pass them down through
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

1. **Round-trip test (#398 acceptance, r1-B2):** the survival guard
   keeps an entry alive when ANY live backing event exists, so the two
   events must back DIFFERENT entries. Seed TWO live entries (`fact_a`,
   `fact_b` — neither `user_stated`, `id = entity_id`, nor `tier_*`,
   avoiding all carve-outs), link `evt_dead` (forgotten via the
   engine-shape UPDATE) to `fact_a` and `evt_live` to `fact_b`, export,
   apply to a fresh temp brain, and assert the imported `evt_dead` row
   carries `forgotten_at`, `fact_a` is hidden by the gate on the
   importing brain, and `fact_b` stays visible.
2. **Back-compat tests:** a bundle with no `forgotten:` key imports as
   live events; applying to a pre-v19 target (column dropped) inserts
   without error and unchanged counts; exporting from a pre-v19 brain
   succeeds (r1-M1); a duplicate arriving forgotten tombstones the
   target's live row (r1-M3).
3. **Grammar unit tests:** append/parse round trip with and without
   the key; non-numeric value parses as absent; existing id-only lines
   byte-identical.
4. **Traversal tests (existing, must stay green):
   `forgotten_gate_closes_graph_bypass_sites`,
   `forgotten_neighbor_edges_are_gated_in_traversal`.** Plus a new
   no-meta-read characterization for gate probes (task 3): with
   `llm_wiki_meta` dropped, both public traversals' gate arms work and
   `wiki_context` still fails closed on its scoring-path scheme read
   (r2-B1/M2).
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
7. **Full gates:** `cargo test --manifest-path src-tauri/Cargo.toml
   --lib --features test-utils`, the INTEGRATION target `cargo test
   --manifest-path src-tauri/Cargo.toml --test okf_bundle
   --features test-utils` (r2-M1 — `--lib` gates cannot see it), and
   `cargo test --manifest-path tools/Cargo.toml --lib` PLAIN (r3 M-A1 —
   the tools crate declares no features), all green; no new clippy
   findings versus the base commit.

## Non-goals

- Gate semantics stay frozen: no change to fail-closed behavior, the
  survival-guard predicate, or any carve-out.
- No bundle-format version bump; no engine bump required.
- #397 and #390 stay out (rejected alternatives above).
- Per Kurt's 2026-11-27 directive: no new issues from this lane — any
  new finding lands in this PR if review-manageable.

## Errata

### EQP on the arm's live subquery (correlated-query form, per r2 minor 2)

sqlite 3.53.4, real correlated query from the plan's Task 6 Step 4 —
BEFORE (arm subquery scans `llm_wiki_events`):

```
QUERY PLAN
|--SCAN e
|--CORRELATED SCALAR SUBQUERY 1
|  `--SEARCH fe USING INDEX llm_wiki_events_related_forgotten_idx (related_entry_id=?)
`--CORRELATED SCALAR SUBQUERY 2
   `--SCAN le
```

AFTER (`llm_wiki_events_related_live_idx` serves the live half):

```
QUERY PLAN
|--SCAN e
|--CORRELATED SCALAR SUBQUERY 1
|  `--SEARCH fe USING INDEX llm_wiki_events_related_forgotten_idx (related_entry_id=?)
`--CORRELATED SCALAR SUBQUERY 2
   `--SEARCH le USING INDEX llm_wiki_events_related_live_idx (related_entry_id=?)
```

### Mid-flight deltas folded during implementation (2026-11-27)

- **Plan r3 minor 4 was stale:** the `wisdom_match.rs` fixture at the old
  `:1965` anchor (`forgotten_probe_requires_related_entry_id`, which drops
  COLUMN `related_entry_id`) DID need a change once
  `llm_wiki_events_related_live_idx` exists — the index's WHERE clause
  references `related_entry_id`, so SQLite refuses the DROP COLUMN (same
  r1-B3 constraint as `forgotten_at`). Its batch now adds
  `DROP INDEX IF EXISTS llm_wiki_events_related_live_idx;` (verified
  minimal + semantics-preserving at task review).
- The generated migrated-shape golden fixture
  (`src-tauri/src/db/testdata/ct-engine9-migrated.llm_wiki.schema.sql`)
  was regenerated via the prescribed `CT_REGEN_FIXTURES=1` path (+3
  lines: the new index block) — forced fallout of the new index.
- clippy `-D warnings`: `fetch_neighbors` (wiki_graph.rs) reached 8
  arguments when the per-walk `TraversalGate` param landed (owner note
  2); carries a targeted `#[allow(clippy::too_many_arguments)]` rather
  than a signature refactor, to preserve the converged plan's
  prescribed shapes.
- **Dup tombstone scope evolved past this text:** §102-era wording has
  the dup UPDATE as `WHERE id = ? AND forgotten_at IS NULL`. The
  shipped scope is `AND entity_id IN (<target cluster>)` — evolved in
  two review folds (T1.5 r1: entity scoping against unrelated-entity
  collisions; CodeRabbit CR-1 + T4 r1 M1: widened to the CLUSTER via
  `entities::cluster_ids` so merged-loser-owned rows reconcile, and a
  forgotten FRESH INSERT whose related entry sits outside the cluster
  is downgraded to a live insert with a warning instead of hiding a
  foreign entry — the survival guard is entity-agnostic).
- **Parse edge behaviors:** §85's "lines without the key parse exactly
  as today" holds for well-formed lines, but three edge behaviors
  changed (documented in `log_md.rs::parse_event_id_comment`): an empty
  id and a bare `id:` prefix yield `None` (previously `Some("")`), and
  a trailing junk token yields `Some("<first token>")` (previously the
  whole comment was rejected).
- **Files touched is wider than §151:** also `okf/event_line.rs`,
  `db/test_support.rs`, `wisdom_match.rs`, `db/connection.rs`, and the
  regenerated migrated-shape golden fixture noted above.
