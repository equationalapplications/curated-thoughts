# Cross-partition graph traversal + namespace discovery (issue #190)

**Date:** 2026-09-08
**Status:** Implemented 2026-09-08 (PR #197) — engine + MCP surface; live post-install verification pending release
**Branch:** spec/issue190-traversal-discovery
**Priority:** P2

## Problem (verified current-state evidence)

The knowledge graph's edges live partitioned by curated-entity namespace
(`llm_wiki_edges.entity_id = ent_<hash>` of the owning curated entity; 215 live
edges across 105 distinct partitions, measured live Sep 8 2026). 
`wiki_traverse_graph` (Rust, `wiki_graph.rs:633`) scopes strictly:
`WHERE e.entity_id = ?1 AND {anchor} = ?2`. Consequences, all reproduced live:

1. **MCP callers cannot discover the partition.** The MCP tool
   `wiki_traverse_graph(entityId, sourceId)` requires the caller to already
   know which `ent_*` partition owns the edges. Seeding with a fact id, or
   with an `ent_*` id that is the *subject* of edges rather than their
   *partition owner*, returns `{"edges": [], "nodes": [<seed only>]}` — a
   silent empty, indistinguishable from "no graph data" (reproduced: fact-seed
   and wrong-namespace-seed both empty; correct-namespace seed walks 4+ edges).
2. **`wiki_context` is the only bridge** — it resolves fact hits to entity
   neighborhoods server-side. There is no path from "entity id" →
   "partitions holding its edges".
3. **The documented happy path is the bug** (M2): `wiki_search`'s description
   says it returns ids "for use with wiki_traverse_graph", and the traverse
   tool says "use wiki_search first to obtain sourceId" — i.e. the docs steer
   agents into feeding entry/fact ids to a traversal whose live edges are all
   entity-space.
4. Silent-empty results already misled operations once (the recorded "P0
   namespace gap" note, Sep 6).

Related but already-working (verified, do NOT re-fix):
- Sweep arming: `purge_off_manifest_edges` resolves vocabulary per entity via
  `resolve_strict_edge_vocabulary` → `tier_fact` fallback, so
  `ct wiki sweep` is live today without per-ent manifest rows.
- Insert-time canonicalization (#192, v2.6.2): verified live Sep 8.

## Approach

Make partition discovery the engine's job, not the caller's — as an
**explicit mode, never a silent semantic change** (I1).

**A. Engine: cross-partition mode (Rust, `wiki_graph.rs`).**
`wiki_traverse_graph` gains an explicit mode discriminator:

- **Scoped mode (default, unchanged):** today's behavior byte-for-byte.
  `entityId` required; scoped-empty means "no edges in THIS partition" —
  the contract existing callers rely on is preserved.
- **Cross-partition mode:** triggered ONLY when the MCP caller omits
  `entityId` (I1: no fallback-on-empty — a correct-partition call whose node
  has edges only elsewhere keeps returning a clean scoped empty). The engine
  fn signature changes to `entity_id: Option<&str>` (I5); `Option::None`
  selects cross-partition mode. Seed resolution in cross-partition mode:
  resolve the seed in entity space only (`curated_entities` by id, ignoring
  partition); entry-space seeds (fact/entry ids) are documented as
  unsupported for cross-partition mode and return an explicit error message
  naming the tool that does support them (`wiki_context`) — NOT a silent
  empty (I5). Scoped mode's entry-space-first resolution is untouched.

Cross-partition walk semantics (I2):
- **Seed-hop only.** The cross-partition pass returns the node's
  single-hop neighborhood across all partitions; BFS does NOT continue
  (mid-walk mode switches are surprising and unbounded). Callers wanting
  deeper walks take a returned neighbor + its owning partition (from the
  edge's `entity_id`) into a scoped-mode call.
- Respects `MAX_TRAVERSAL_NODES` (50) as today, counted across partitions.

**Per-partition vocabulary gating (C1):** the cross-partition pass collects
candidate partitions first (`SELECT DISTINCT entity_id … WHERE source_id = ?
OR target_id = ?`), then for each partition resolves that partition's own
strict vocabulary (`resolve_strict_edge_vocabulary(conn, partition)`) and
filters that partition's edges through it before adding them to the result.
A partition whose vocabulary rejects an edge type excludes that edge — read
and write gates stay consistent (the #158 class of bug stays closed).
Vocabulary resolution failures for a partition skip that partition with a
warn, never abort the call.

**Deterministic partition ordering (I3):** partitions are ranked by matching
edge count descending, then `entity_id` ascending — no SQLite row-order
dependence. The partition cap (8) applies after ranking. Truncation is
reported in a NEW dedicated field `partitions_truncated: bool` on the result
(the existing `truncated` flag keeps its single meaning: 50-node cap hit).

**Node entity_id stamping (M3):** in cross-partition results, each node's
`entity_id` is stamped with the partition of the first-ranked edge that
reached it; edge rows always carry their true owning partition. Callers
needing the full partition set for a node get it from the edges.

**B. MCP surface.** `wiki_traverse_graph` tool: `entityId` becomes OPTIONAL
(omission = cross-partition mode). No signature break — the parameter was
required before, remains accepted now. Tool description rewritten (M2):
"entityId scopes the walk to one curated namespace; omit it to discover the
node's edges across all namespaces. sourceId must be a curated-entity id
(ent_*); entry/fact ids are not graph endpoints — use wiki_context for
fact-anchored context." `wiki_search`'s description drops the "for use with
wiki_traverse_graph" steer or qualifies it to entity-space results.

**C. `wiki_get_ontology` no-op clarification:** per-ent manifest rows are NOT
added by this change (the tier fallback already arms sweep + canonicalization);
recorded in Out-of-scope so #190's original "activate manifests" framing is
honored in effect, not letter.

### Rejected alternatives

- **Fallback-on-empty (silent):** changes the scoped-empty contract with no
  opt-out (I1) — rejected in favor of explicit omitted-entityId mode.
- **Per-ent manifest rows seeded at ingest:** 197+ rows duplicating
  `tier_fact`'s content; write amplification, no behavioral delta. Revisit
  only with a real per-entity-divergence requirement.
- **wiki_context-only ergonomics (do nothing):** leaves the dedicated
  traversal tool returning misleading silent empties.
- **Denormalizing a partition column onto curated_entities:** schema churn in
  the engine-owned table for a read-path concern.
- **Cross-partition BFS (multi-hop):** unbounded surprise; rejected for
  seed-hop-only + explicit follow-up calls (I2).

## Testing

- Rust unit tests (wiki_graph.rs test mod):
  - Scoped mode unchanged: existing tests pass byte-identical (correct
    partition + node-with-edges-in-it returns same output as today).
  - Scoped-empty contract: correct partition, node with edges ONLY elsewhere
    → still empty (no silent fallback).
  - Cross-partition mode: omitted entityId + node with edges in 3 partitions
    → all 3 partitions' edges returned, each vocabulary-gated (seed a
    partition with a strict manifest rejecting one edge type; assert that
    edge is excluded while others surface — pins C1).
  - Partition cap: node with edges in 10 partitions → top-8 by edge count,
    `partitions_truncated: true`, deterministic order (same result across
    two calls and a VACUUM).
  - Entry-id seed in cross-partition mode → explicit error naming
    wiki_context, not a silent empty.
  - MAX_TRAVERSAL_NODES interaction: hub node with >50 neighbors →
    `truncated: true` as today.
- MCP integration (mcp-server feature): tool call without entityId returns
  cross-partition neighborhood for a known node; with entityId, output
  byte-identical to today's.
- Live verification (post-install, per the Sep 8 lesson): assert binary
  provenance (build date ≥ merge date) before testing; query a hub node
  cross-partition on the live brain.

## Error handling

Read-path only; no writes. DB errors propagate as today. Per-partition
vocabulary resolution failure → skip partition + warn (never abort).
Cross-partition mode with a non-entity seed → explicit tool error (above).

## Out of scope / open questions

- Per-entity ontology divergence (real manifests per ent) — deferred until a
  product need exists.
- `wiki_search` returning partition ids alongside hits (would help agents
  pick the scoped fast path) — follow-up candidate; description fix only in
  this PR.
- Entity merge/dedupe (14 rows named "Tessera", 1 live, measured Sep 8) —
  separate workstream; the partition cap exists because one id can anchor in
  many partitions (I4's corrected rationale).
- Entry-space cross-partition walks — if a need emerges, a separate design.
