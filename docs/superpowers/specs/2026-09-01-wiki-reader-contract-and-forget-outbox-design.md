# Spec: Wiki Reader Contract & Forget Outbox

**Date:** 2026-09-01
**Status:** IMPLEMENTED — see `docs/superpowers/plans/2026-09-01-wiki-reader-contract-and-forget-outbox.md`
**Shipped in:** PR #135 (branch `spec/wiki-reader-contract-forget-outbox`)
**Type:** bug fix (reader contract) + privacy defect (replication gap) + spec correction
**Closes:** #133, #134, #132
**Baseline:** main @ v1.40.0 (`50ef618`)
**Merge convention:** regular merge commit (never squash, per Aug 2026 convention)
**Docs:** this spec rides its implementation PR (Aug 31 2026 convention)

## 1. Executive Summary & Problem Context

Three defects ship together because two of them are the same defect wearing
different clothes.

`wiki_search` and `wiki_traverse_graph` are the two MCP read surfaces over the
wiki graph. Both return empty on a fully-populated, fully-embedded brain — not
because the data is missing, but because **each reader encodes an assumption
about the data that the writers never honored.**

- `wiki_search` assumes entries are namespaced by *tier* (`tier_fact`,
  `tier_wisdom`). Every row the librarian writes is namespaced by *entity*
  (`ent_<hash>`). The default filter can never match a row.
- `wiki_traverse_graph` assumes edges connect *entries*. Every live edge
  connects `curated_entities`. The join can never match an edge.

PR #131 (v1.40.0) made these visible rather than causing them: its embedding
backfill is what finally made the read surfaces testable end-to-end. Both
contract mismatches predate it.

The third defect is adjacent and mechanical. `wiki_forget` — the "forget this
source file" command — hard-deletes entries and pushes **no** outbox rows.
Replicas built on `@equationalapplications/prisma-outbox` keep serving facts
the user explicitly asked to erase. Because the triggering action is an
explicit erasure request, this is a privacy defect, not a consistency one.

It also carries a documentation defect that will mislead the next implementer:
the PR #131 design spec asserts at line 209 that `wiki_forget` already pushes
deletes. It never has.

### Verification constraint (read this before trusting any live-data claim)

The evidence in issues #132/#133/#134 was gathered on a populated brain at
schema v15 (140 entries, 88 edges). **That database is not present on the
development machine as of 2026-09-01.** The only `~/.brain/brain.db` here is at
`schema_version` 6, holds zero entries, and contains neither `llm_wiki_edges`
nor `curated_entities` — it has the older chunks-anchored
`curated_relationships` table instead.

Consequence: **acceptance for this spec rests entirely on in-repo fixtures**
that reproduce the shapes the issues documented. A live-brain re-probe is a
manual post-merge step for the maintainer, not an automatable gate. Any future
reader of this spec who wants to confirm the live numbers must re-probe a v15
brain themselves.

## 2. Part A — `wiki_search` default entity filter (#133)

### Current behavior

`src-tauri/src/wiki_graph.rs:11`:

```rust
pub const DEFAULT_ENTITY_IDS: &[&str] = &["tier_fact", "tier_wisdom"];
```

`src-tauri/src/tool_dispatch.rs:198-213` — `dispatch_wiki_search` substitutes
those defaults whenever the caller omits `entityIds`, then
`wiki_graph::wiki_search` (`wiki_graph.rs:110-134`) runs:

```sql
WHERE entity_id IN (?, ?) AND deleted_at IS NULL AND embedding_blob IS NOT NULL
```

Librarian-generated entries carry `ent_<hash>` ids. The `IN` clause matches
nothing, so the default call path — the normal call path — returns `[]`.

### Design

`wiki_search` takes `entity_ids: Option<&[&str]>`:

| Argument | SQL predicate | Rationale |
|---|---|---|
| `None` | `WHERE deleted_at IS NULL AND embedding_blob IS NOT NULL` | search every live embedded entry |
| `Some(&[])` | *(short-circuit, return `Vec::new()`)* | preserves today's explicit-empty behavior exactly |
| `Some(ids)` | `WHERE entity_id IN (…) AND …` | unchanged from today; verified working |

`dispatch_wiki_search` passes the caller's `Option` through unchanged rather
than substituting anything.

**`DEFAULT_ENTITY_IDS` is deleted, not merely unused.** The constant *is* the
false contract. Leaving it in the module invites a future caller to re-adopt
it and reintroduce the same drift. Its removal must be verified by grep across
`src-tauri/` and `src/` before the change is considered complete.

### Scoring is unchanged

`tier_weight()` (`wiki_graph.rs:73-81`) already degrades to `1.0` for unknown
namespaces, which is why `ent_*` entries score correctly today when passed
explicitly. It stays exactly as written and continues to be applied per-row, so
a `tier_fact` entry keeps its 1.5× bonus in any brain where tier namespaces do
exist. **Only the filter was broken; the ranking never was.**

### Rejected alternatives

- **Migrate the writer to tier namespaces.** Fixes the drift at its source but
  requires migrating 140 rows and repointing 88 edges' expectations, touching
  PR #78's stale-SQL history. A reader-only change cannot corrupt data; this
  one can.
- **Keep tier defaults, add an `entityIds: "all"` sentinel.** Backward
  compatible, but leaves the default broken for every brain that exists today
  and pushes the workaround onto every caller.

## 3. Part B — Heterogeneous traversal (#134)

### Current behavior

`fetch_outbound_neighbors` / `fetch_inbound_neighbors`
(`wiki_graph.rs:222-296`) both join:

```sql
JOIN llm_wiki_entries s ON s.id = e.source_id AND s.deleted_at IS NULL AND s.entity_id = ?1
JOIN llm_wiki_entries t ON t.id = e.target_id AND t.deleted_at IS NULL AND t.entity_id = ?1
```

All 88 live edges anchor `curated_entities` ids. An edge whose endpoints live
in `curated_entities` can never satisfy this join, so traversal resolves the
seed node correctly and returns `edges: []`.

PR #131's rev-2 spec already made *edge lifecycle* handling heterogeneous —
a three-table endpoint contract over `llm_wiki_entries` ∪ `curated_entities` ∪
`llm_wiki_tasks`. The reader side never followed.

### Design

Introduce an explicit node space, decided **once, at the seed**:

```rust
enum NodeSpace { Entry, Entity }
```

`load_live_node(conn, entity_id, id) -> Result<Option<(WikiTraverseNode, NodeSpace)>>`
resolves in two steps:

1. `llm_wiki_entries` by `id` + `entity_id` match + `deleted_at IS NULL`
   (today's `load_live_entry`, unchanged) → `NodeSpace::Entry`.
2. Otherwise `curated_entities` by `id` + `deleted_at IS NULL` →
   `NodeSpace::Entity`.

If neither resolves, traversal returns the empty result it returns today.

`fetch_neighbors` dispatches on the space. `NodeSpace::Entry` uses the existing
SQL verbatim. `NodeSpace::Entity` uses parallel SQL joining `curated_entities`
on both endpoints, still partitioned by the edge row:

```sql
SELECT e.source_id, e.target_id, e.edge_type, t.id, t.name
FROM llm_wiki_edges e
JOIN curated_entities s ON s.id = e.source_id AND s.deleted_at IS NULL
JOIN curated_entities t ON t.id = e.target_id AND t.deleted_at IS NULL
WHERE e.entity_id = ?1 AND e.source_id = ?2 {edge_filter}
```

(and the mirror for inbound, selecting `s.id, s.name` and filtering
`e.target_id = ?2`).

**Why `curated_entities` is not filtered by `entity_id`:** the table has no
such column. Its schema (`src-tauri/src/db/okf_ddl.rs:148-157`) is
`id, name, entity_type, summary, summary_embedding, created_at, updated_at,
deleted_at`. The partition key lives on the **edge** row, so `e.entity_id = ?1`
is the only entity scoping available and is sufficient — an entity-space walk
never leaves the requested partition because every edge it traverses is
required to carry it.

### Node shape

`WikiTraverseNode { id, title, entity_id }` is unchanged. Entity-space nodes
map onto it:

| Field | Entry space | Entity space |
|---|---|---|
| `id` | `llm_wiki_entries.id` | `curated_entities.id` |
| `title` | `llm_wiki_entries.title` | `curated_entities.name` |
| `entity_id` | `llm_wiki_entries.entity_id` | the `entityId` argument (= the edge partition walked) |

No MCP response-shape change, so existing callers and tests keep working. The
accepted cost: **a caller cannot tell which table a node came from.** If that
becomes load-bearing, an additive `space` discriminator is the follow-up — it
is deliberately not added now.

### One space per walk

A walk stays in the space its seed resolved to. It does not cross. An entry
seed therefore still cannot see entity-space edges, and vice versa. This is the
deliberate cost of the "one tool, one mental model" choice: mixed-space results
would require the discriminator field above and a merge rule for two
differently-keyed neighbor sets, neither of which any caller needs today.

### Unchanged traversal mechanics

`MAX_TRAVERSAL_NODES = 50`, `clamp_max_depth` (1..3), BFS queue, the visited
set, edge-key deduplication, and the `truncated` flag are all shared by both
spaces. Only neighbor fetching branches.

### Out of scope

`llm_wiki_tasks` is the third endpoint table in PR #131's write contract. It
gets no reader support here — no live edge anchors it today. Recorded as
follow-up in §6.

### Rejected alternatives

- **A separate `wiki_traverse_entities` tool.** Cleaner internals, but callers
  must know which space an id lives in *before* choosing a tool, which they
  generally do not.
- **Materialized bridge edges** derived from shared `curated_entities`
  membership. Would let the existing reader work unchanged, but denormalizes,
  risks stale bridges, and is the heaviest option. Issue #134 flags this itself.

## 4. Part C — `wiki_forget` outbox deletes (#132)

### Current behavior

`forget_entries_by_source_refs` (`src-tauri/src/db/wiki_forget.rs:20-49`)
opens a transaction, collects the doomed ids **before** deleting (so the ids
survive the DELETE), hard-deletes by `source_ref`, purges edges via
`purge_edges_for_hard_deleted`, and commits. It pushes no outbox rows —
confirmed by grep across `wiki_forget.rs` and `run_wiki_forget`.

### Design

Mirror `prune_old_librarian_inferred` (`src-tauri/src/lib.rs:1752-1795`), which
gained exactly this in PR #131:

1. Widen the doomed SELECT from `SELECT id` to `SELECT id, entity_id`.
2. Before the DELETE, inside the **same** transaction, push one row per doomed
   entry:

```rust
crate::db::commit::push_entries_outbox(
    &tx,
    entity_id,
    id,
    crate::db::outbox_format::OutboxOperation::Delete,
    serde_json::json!({ "id": id }),
    now_ms,
)?;
```

3. `entity_id` comes from the doomed row itself, never assumed uniform — the
   outbox is keyed on entity and pushing the wrong one mis-attributes the
   delete.

`forget_entries_by_source_refs` gains a `now_ms: i64` parameter so tests can
pin the timestamp — the same signature shape as
`prune_old_librarian_inferred(conn, now_ms)` (`lib.rs:1737`), and the reason
that function's outbox test is deterministic. `run_wiki_forget`
(`lib.rs:1961`) supplies it from `crate::db::commit::now_timestamps()`, the
helper already used at the analogous call site (`lib.rs:463`). Everything
commits together: a crash between the delete and the outbox push remains
impossible.

### Doc comment correction

The module comment currently reads:

> Edges are not replicated (spec §6) — this purge is local-only, exactly like
> the inserts `commit_edge_add` issues without an outbox push.

That stays **true for edges** and must not be deleted. It gains a sentence
recording that entries now do replicate their deletes, so the two halves are
not confused.

### Spec correction (the documentation defect)

`docs/superpowers/specs/2026-08-31-wiki-graph-reanchor-entry-embeddings-design.md`
line 209:

> `prune_old_librarian_inferred` must push one `OutboxOperation::Delete` row
> per pruned id, matching `clear_entity_content` and `wiki_forget`.

The `wiki_forget` half is false and always was. Correct it to name only
`clear_entity_content`, and append a dated note recording that the divergence
was found on 2026-09-01 and closed by this spec. Do **not** silently edit the
claim away — the false premise is why the gap survived PR #131's review, and
that is worth leaving legible.

## 5. Testing

All tests are in-repo Rust unit tests over `open_in_memory` fixtures, following
the existing pattern in `wiki_forget.rs`'s test module. Per §1, fixtures are
the *only* gate available.

### Part A — search (`wiki_graph.rs` tests)

1. **`ent_*`-only brain, `entity_ids = None`** → returns scored hits. This is
   the exact live shape from #133 and the regression test that matters most.
2. **Explicit `entityIds` unchanged** → same hits as today for a `Some(ids)`
   call; guards against fixing the default by breaking the explicit path.
3. **`Some(&[])` returns `[]`** → explicit-empty short-circuit preserved.
4. **Mixed brain ranking** → given a `tier_fact` entry and an `ent_*` entry with
   equal cosine similarity, `tier_fact` ranks first (1.5× bonus survives the
   broadened filter).
5. **Soft-deleted and unembedded rows excluded** under `None`.

### Part B — traversal (`wiki_graph.rs` tests)

6. **Entity-anchored seed returns neighbors** — the #134 shape: edges whose
   endpoints are `curated_entities` rows, seeded from a `curated_entities` id.
7. **Entry-anchored brain unchanged** — an entries-only fixture produces
   byte-identical results to today.
8. **Soft-deleted endpoints excluded in both spaces** — a `deleted_at`-set
   endpoint is unreachable whether it lives in `llm_wiki_entries` or
   `curated_entities`.
9. **Depth clamp and 50-node truncation hold in entity space** — `truncated`
   is set correctly on an oversized entity-space graph.
10. **Unresolvable seed returns the empty result** in both spaces.
11. **`edge_types` filter applies in entity space** as it does in entry space.

### Part C — forget (`wiki_forget.rs` tests)

12. **One `Delete` row per deleted entry**, each carrying the entry's own
    `entity_id` — including a fixture where two doomed entries have *different*
    `entity_id`s, proving the value is read per-row.
13. **Rollback leaves no outbox rows** — a failure before commit produces
    neither deletions nor outbox rows.
14. **Empty input is a no-op** — returns 0, pushes nothing.
15. Existing edge-purge tests continue to pass unmodified.

## 6. Risks & Follow-ups

**Unfiltered search is a full scan.** With `entity_ids = None`, every live
embedded entry is loaded and cosine-scored in Rust. At ~140 rows this is free.
It is the known scaling boundary and is deliberately not solved here — no
index, no cap, no ANN. Revisit when entry counts reach a scale where it
measurably hurts; premature indexing here would be guesswork.

**One space per walk.** An entry seed cannot see entity-space edges. Accepted
above; the escape hatch is the additive `space` discriminator plus a merge
rule, if a caller ever needs both at once.

**`llm_wiki_tasks` endpoints have no reader.** The write contract admits three
endpoint tables; this spec teaches the reader two. No live edge anchors tasks
today, so this is latent, not broken.

**Node provenance is not exposed.** By the §3 node-shape decision, a caller
cannot distinguish an entry node from an entity node in the response.

**Live-brain acceptance is manual.** Per §1, the v15 brain that produced the
issue evidence is not available on the development machine. After merge, the
maintainer should re-run the two probes from #133 and #134 against a populated
brain and record the results on the issues before closing them.

## 7. Acceptance Criteria

- **AC1** — Default `wiki_search` (no `entityIds`) returns scored hits on a
  brain whose entries carry only `ent_*` ids.
- **AC2** — Explicit-`entityIds` search behavior is unchanged; `Some(&[])`
  still returns `[]`.
- **AC3** — `tier_weight`'s fact bonus still applies wherever `tier_*`
  namespaces exist.
- **AC4** — `DEFAULT_ENTITY_IDS` no longer exists anywhere in the tree.
- **AC5** — Traversal from an entity-anchored seed returns the live neighbor
  set for a `curated_entities`-anchored edge graph.
- **AC6** — Entry-space traversal behavior is unchanged for databases where
  entry-anchored edges exist.
- **AC7** — Soft-deleted endpoints remain excluded in both spaces.
- **AC8** — `wiki_forget` pushes exactly one `OutboxOperation::Delete` row per
  deleted entry, carrying that entry's own `entity_id`, in the same transaction
  as the delete.
- **AC9** — The false `wiki_forget` claim at line 209 of the 2026-08-31 spec is
  corrected and the divergence is dated and recorded.
- **AC10** — `cargo test`, `cargo clippy`, and `cargo fmt --check` pass. (Note:
  CI does not gate clippy; run it locally.)
