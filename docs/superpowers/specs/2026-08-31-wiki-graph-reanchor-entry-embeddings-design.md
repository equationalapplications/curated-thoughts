# Spec: Wiki Graph Re-anchoring & Write-Time Entry Embeddings

**Date:** 2026-08-31
**Status:** DRAFT — awaiting Kurt's review
**Type:** bug fix + librarian contract + one-time migration
**Baseline:** main @ v1.39.0 (`86e18b4`)
**Merge convention:** regular merge commit (never squash, per Aug 2026 convention)
**Docs:** this spec rides its implementation PR (Aug 31 2026 convention)

## 1. Executive Summary & Problem Context

### The debt

PR #78 (`fix/mcp-stale-sql-handlers`, merged 2026-08-25) was a **deliberate
stopgap**. Its plan (`docs/superpowers/plans/2026-08-24-fix-stale-sql-mcp-handlers.md`)
records the decision locked at the time:

> wiki leg stays but returns empty until librarian populates `llm_wiki_entries`
> (no prose fallback)

and its Risks section states the punt explicitly:

> llm_wiki_entries has no embeddings today → wiki legs must handle empty
> gracefully (short-circuit) rather than crash.

PR #78 fixed the sidecar's schema-invalid SQL. The upstream work — populating
entry embeddings and keeping the edge table coherent across librarian
regeneration — was never specced. The wiki legs of `wiki_search` /
`wiki_traverse_graph` have returned empty **by design** ever since.

### The current state (gate-run findings, 2026-08-31T22:38Z, brain.db schema v15)

Full audit: vault `immutable-source-files/agents/operations/ct-v139-graphrag-audit.md`.

- **41 edges, 100% orphaned.** Every row in `llm_wiki_edges` references `ent_*`
  ids; zero `ent_*` rows exist in `llm_wiki_entries` — not even soft-deleted
  (hard-deleted in the pre-wipe generation). All 62 live entries are `fact_*`.
- **62/62 live entries have `embedding_blob = NULL`.** `wiki_search` embeds the
  query, finds no comparable entry vectors, returns `[]` for any query.
- `wiki_traverse_graph` returns `{edges:[], nodes:[]}` for every input — its
  entry point is a `wiki_search` result id, and traversal joins land on nothing.
- The knowledge graph is completely severed from the LLM: chunk-level RAG works
  (verified live, OpenRouter qwen3-embedding-4b healthy), wiki graph retrieval
  is structurally dead.
- v1.39.0 (PR #130, the ingest drain-stall watchdog) does not touch retrieval —
  its V13 watchdog tables are present and clean (0 stalls, 0 quarantined).

### Why the orphans exist (mechanism, for the implementer)

`commit_edge_add` (`src-tauri/src/db/commit.rs:1025`) resolves edge endpoints
via `resolve_edge_ref` at commit time and drops unresolvable refs into
`ctx.dropped_edges`. So edges are only written when endpoints exist **then**.
But nothing deletes an edge when its endpoint entry is later archived or
regenerated. The Aug 2026 librarian regeneration swapped an `ent_*`-keyed
generation for today's `fact_*` generation and stranded every edge. The purge
side of the contract simply does not exist yet — this spec adds it.

## 2. Part A — The Librarian Edge Contract (re-anchoring)

New rule of engagement: **edges die with their endpoints, in the same
transaction, everywhere an entry dies.**

### Purge trigger and sites

When the librarian (or any core process) deletes, archives, or regenerates a
`llm_wiki_entries` row, run a cascading delete of every edge whose `source_id`
or `target_id` matches the dying entry's `id`. Exhaustive deletion-site list
(grep-verified at baseline; the implementer must re-run this grep and confirm
the list is still complete):

| Site | Path | Kind |
|---|---|---|
| `commit_fact_archive` | `src-tauri/src/db/commit.rs:865` | librarian regeneration (archive old fact) |
| `archive_fact` | `src-tauri/src/db/facts.rs:225` | Brain-mode archive |
| `heal_invalid_sources` | `src-tauri/src/lib.rs:380` | auto-heal soft-delete |
| `heal_lost_librarian_inferred` | `src-tauri/src/lib.rs:1511` | manual heal soft-delete |
| `prune_old_librarian_inferred` | `src-tauri/src/lib.rs` (hard `DELETE FROM llm_wiki_entries` after 7-day window) | hard delete |

Purge statement (per dying id, inside the site's existing transaction):

```sql
DELETE FROM llm_wiki_edges
 WHERE source_id = ?1 OR target_id = ?1;
```

Every purge must also push outbox delete operations for the removed edge ids
(same pattern as `push_entries_outbox`, `OutboxOperation::Delete`), so the
Postgres/remote replica converges. An edge purge that skips the outbox
re-creates the orphan problem downstream.

**Reviewed decision — soft-delete purges edges too.** Cascading at the
soft-delete (archive/heal) sites means a recovered row (the
`UPDATE ... SET deleted_at = NULL` recovery recipe) comes back without its
edges. Accepted: edges are cheap derived structure the librarian can rebuild,
and the alternative (purge only on hard delete) leaves ghost edges visible
whenever a reader forgets the `deleted_at` filter. Kurt's acceptance criterion
("regeneration automatically drops its old edges without leaving ghosts")
chooses the strict rule.

### Defense in depth — reader-side filter

`wiki_graph.rs` traversal SQL joins `llm_wiki_entries` on endpoint ids. Add
`AND s.deleted_at IS NULL` / `AND t.deleted_at IS NULL` to both direction
queries (source-direction at `wiki_graph.rs:225ff`, target-direction at
`:270ff`) so a pre-contract ghost edge can never surface even if one is
reintroduced by a bug. This is belt-and-braces; Part A's transactional purge is
the real fix.

### Transaction boundary

The entry deletion/regeneration and the edge purge **must run in the same
SQLite transaction** so a crash between the two writes can never mint a new
orphan. All listed sites already execute inside `BEGIN IMMEDIATE`
transactions (established pattern, see okf-backend-migration spec §"Commit
path"); the purge statements join that transaction — no new transaction
nesting, no network I/O added inside it.

## 3. Part B — Write-Time Embeddings

Closes the loop so `wiki_search` actually works.

### The write path

`commit_fact_add` (`src-tauri/src/db/commit.rs:624`) currently hard-inserts
`embedding_blob = NULL, embedding = NULL`. Change: compute the entry embedding
**before** opening the commit transaction, and insert the blob when available.

- **Text embedded:** `format!("{title}\n\n{body}")` — the entry's prose, same
  convention as chunk text (prose is what the librarian curated; keep it
  provider-agnostic).
- **Provider:** `load_embed_profile` (`src-tauri/src/retrieval/mod.rs:96`) — the
  same active profile that embeds chunks (currently OpenRouter
  `qwen/qwen3-embedding-4b`). Entry vectors and chunk vectors must share the
  profile so the wiki-graph-tools dimension guard
  (`length(embedding_blob) / 4 == active profile dim`, mcp-wiki-graph-tools
  design line 46) holds.
- **API:** `embedder::embed_one(&profile, text)` (`src-tauri/src/embedder/mod.rs:191`),
  blob format little-endian f32, 4 bytes/dim — identical to the chunks
  embedding path (`src-tauri/src/db/queries.rs:81`).
- **Ordering:** embed → open tx → insert entry + outbox row → commit. Never a
  network call inside the transaction (lock-hold hazard per
  architectural-pitfalls).
- **Update path:** `commit_fact_update` (`commit.rs:726`) and the facts.rs edit
  path re-embed when `body` changes; unchanged bodies keep their blob.
- **Outbox:** `wiki_fact_outbox_payload` (`commit.rs:408`) is unchanged —
  embeddings are locally derived artifacts, not replicated state. Remote
  surfaces re-derive or backfill via Part C's sweep.

Same treatment applies to `facts.rs` fact-insert paths (the Brain-mode
`user_stated` facts) so every writer upholds the contract.

### Failure isolation (decided: write null + retry sweep, never roll back)

An embedding-provider timeout must not destroy the librarian's curation work.
On embed failure:

1. Insert the entry with `embedding_blob = NULL` (commit succeeds — curation
   is durable).
2. Log a `curated_agent_log` / warning row naming the entry id and error.
3. Leave recovery to the **null-embedding sweep**:

```sql
SELECT id, title, body FROM llm_wiki_entries
 WHERE deleted_at IS NULL AND embedding_blob IS NULL;
```

The sweep runs on the existing periodic maintenance tick (same scheduler slot
the heal/prune maintenance run uses), batches pending ids (≤64 per
`embedder::embed_batch` call, `embedder/mod.rs:168`), and updates rows one
`UPDATE llm_wiki_entries SET embedding_blob = ?1 WHERE id = ?2 AND
embedding_blob IS NULL` at a time. Bounded batch + bounded sweep duration,
mirroring the v1.39.0 watchdog's budget discipline. Because the sweep keys on
`embedding_blob IS NULL`, **Part C's backfill and Part B's retry are the same
mechanism** — no new queue table (YAGNI).

**Dimension-change note:** switching embed models invalidates entry blobs
exactly as it invalidates chunk blobs (dimension mismatch). The backfill sweep
must skip rows whose blob length/4 ≠ active profile dim, and a model switch
requires purging entry blobs (`UPDATE llm_wiki_entries SET embedding_blob =
NULL`) so the sweep re-embeds everything at the new dimension.

## 4. Part C — The One-Time Migration (healing the current DB)

The damage is already mapped (41 orphaned edges, 62 unembedded entries). The
migration ships as a one-shot binary in `tools/` (reusing the `embedder` crate —
no hand-rolled HTTP, no python). It is `--yes`-gated like ingest; without
`--yes` it runs dry-run and prints exactly what it would do.

**Preflight (mandatory):** copy `brain.db` (+ `-wal`, `-shm`) to
`~/.brain/brain.db.bak-pre-graphreanchor-<ts>` following the existing backup
naming convention, before any write.

### Step 1 — The edge purge

Kurt's outline proposed:

```sql
DELETE FROM llm_wiki_edges
 WHERE source_id NOT IN (SELECT id FROM llm_wiki_entries)
    OR target_id NOT IN (SELECT id FROM llm_wiki_entries);
```

Peer-review correction (the reason this section exists): the outline's
predicate treats **soft-deleted endpoints as alive** (their ids remain in the
table), so it would keep ghost edges dangling off healed/archived entries. It
also reads oddly against Part A's contract, which defines "dead" as
`deleted_at IS NOT NULL` or absent. Adopted form:

```sql
-- Migration Step 1: purge edges with no live endpoint.
-- Live = row present AND deleted_at IS NULL. Matches Part A's contract.
DELETE FROM llm_wiki_edges
 WHERE NOT EXISTS (SELECT 1 FROM llm_wiki_entries s
                    WHERE s.id = llm_wiki_edges.source_id AND s.deleted_at IS NULL)
    OR NOT EXISTS (SELECT 1 FROM llm_wiki_entries t
                    WHERE t.id = llm_wiki_edges.target_id AND t.deleted_at IS NULL);
```

On today's DB both forms delete the same 41 rows (all endpoints are
hard-absent); the `NOT EXISTS` + `deleted_at` form is the one that stays
correct as the table evolves. (`NOT IN` → `NOT EXISTS` also sidesteps the
classic NULL-subquery trap.) Post-condition check, must print 0:

```sql
SELECT COUNT(*) FROM llm_wiki_edges e
 WHERE NOT EXISTS (SELECT 1 FROM llm_wiki_entries s
                    WHERE s.id = e.source_id AND s.deleted_at IS NULL)
    OR NOT EXISTS (SELECT 1 FROM llm_wiki_entries t
                    WHERE t.id = e.target_id AND t.deleted_at IS NULL);
```

Run inside one `BEGIN IMMEDIATE` transaction; push outbox deletes for the
removed ids, same as the runtime path.

### Step 2 — The embedding backfill

Iterate live entries with `embedding_blob IS NULL` (all 62 today), embed in
batches of ≤64 via `embedder::embed_batch`, and update:

```sql
UPDATE llm_wiki_entries
   SET embedding_blob = ?1
 WHERE id = ?2 AND embedding_blob IS NULL;
```

- Same profile, text recipe, and blob format as Part B (identical code path —
  the migration literally invokes the sweep's batch routine).
- Idempotent and resumable: re-running picks up only still-null rows; a
  provider failure mid-run leaves earlier batches committed.
- Expected cost: 62 short prose entries against the external profile — seconds,
  not minutes (52 full docs took ~5 min on this profile in Aug 2026).
- Report at exit: entries backfilled, entries left null (with ids), dimension
  of the active profile.

### Rollback

Both steps are idempotent; full rollback = restore the preflight `.bak` copy
with the app/watcher stopped.

## 5. Validation / Acceptance Criteria

1. **`wiki_search` returns real hits.** Live probe on the migrated DB:
   `wiki_search("ingest drain stall watchdog")` returns ≥1 result whose id is a
   live `fact_*` entry. (Today: `[]`.)
2. **`wiki_traverse_graph` walks real edges.** Unit fixture: commit two facts +
   one edge, traverse depth 2 from the source fact, assert the edge and target
   node return. Live probe post-migration: traversal from any seeded
   fact→fact edge returns nodes/edges. (Today: empty for all inputs.)
3. **Regeneration drops old edges, no ghosts.** Unit test at the archive path:
   commit fact + edge → `commit_fact_archive` the fact → assert the edge row is
   gone and no `llm_wiki_edges` row references the archived id, all within the
   test's transaction. Same assertion for the prune (hard-delete) path.
4. **Embed failure never loses curation.** Unit test with a failing embed
   profile: fact commit still succeeds, row lands with `embedding_blob IS
   NULL`, sweep run against a mock-good provider fills it.
5. **Reader filter holds.** Regression test: manually insert an edge whose
   endpoint is soft-deleted (simulating a pre-contract ghost) → both
   `wiki_graph.rs` direction queries exclude it.
6. **PR #78's stopgap stays green.** Existing short-circuit tests (wiki legs
   return empty, not error, when the table is empty) still pass — this spec
   adds population, it does not remove the graceful-empty contract.

## 6. Non-Goals

- Code/AST symbol graph and `[[wiki-link]]` edge indexing — separate backlog
  items; this spec is strictly about the wiki fact graph.
- Replicating embeddings through the outbox (remote re-derives).
- Changing `wiki_search`'s public tool contract (its shape is already correct;
  only its results were empty).
