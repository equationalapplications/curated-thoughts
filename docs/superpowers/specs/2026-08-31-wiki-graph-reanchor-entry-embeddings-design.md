# Spec: Wiki Graph Re-anchoring & Write-Time Entry Embeddings

**Date:** 2026-08-31
**Status:** IMPLEMENTED — see `docs/superpowers/plans/2026-08-31-wiki-graph-reanchor-entry-embeddings.md`
**Type:** bug fix + librarian contract + one-time migration
**Rev 2 changes:** reader-side filter found already implemented (§2, AC5
downgraded to characterization); sweep trigger specified (none existed to
inherit); edge outbox requirement removed as incoherent with unreplicated edge
inserts; `prune_old_librarian_inferred` ms/s bug added as prerequisite §2.1;
deletion-site table corrected and completed; Part B split into sweep-first
(B.1) + write-time optimization (B.2) with the required `CommitContext`
restructuring spelled out.
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
or `target_id` matches the dying entry's `id`. Deletion-site list, re-verified
against baseline `86e18b4` (line numbers are advisory — grep the `fn` name; the
implementer must re-run this grep and confirm the list is still complete):

| Site | Path | Kind | Cascade needed? |
|---|---|---|---|
| `commit_fact_archive` | `src-tauri/src/db/commit.rs:865` | librarian regeneration (archive old fact) | yes |
| `archive_fact` | `src-tauri/src/db/facts.rs:225` | Brain-mode archive | yes |
| `heal_invalid_sources` | `src-tauri/src/lib.rs:398` | auto-heal soft-delete | yes |
| `heal_lost_librarian_inferred` | `src-tauri/src/lib.rs:1674` | manual heal soft-delete | yes |
| `prune_old_librarian_inferred` | `src-tauri/src/lib.rs:1717` (hard `DELETE FROM llm_wiki_entries` after 7-day window) | hard delete | yes — but see §2.1, this site is inert today |
| `run_wiki_forget` | `src-tauri/src/lib.rs:1867` (hard `DELETE FROM llm_wiki_entries WHERE source_ref = ?1 OR source_ref = ?2`) | "forget this source file" command | yes |
| `clear_entity_content` | `src-tauri/src/db/bundle_apply.rs:606` | OKF bundle import, entity replacement (hard `DELETE ... WHERE entity_id=?1`) | **yes — see note below** |

`clear_entity_content` deletes by `entity_id = ?1` on `llm_wiki_entries` and
`llm_wiki_tasks`, but edges are stamped with `ctx.entity_id` while pointing
across entities (source from one, target from another). The original
`WHERE entity_id = ?1` DELETE on `llm_wiki_edges` strands the partner's
edges as orphan-class. The fix: replace with the heterogeneous-aware
predicate from Part C Step 1, restricted to the imported entity's id set,
so edges with a live endpoint anywhere in any of the three tables survive.

Purge statement (per dying id, inside the site's existing transaction):

```sql
DELETE FROM llm_wiki_edges
 WHERE source_id = ?1 OR target_id = ?1;
```

`prune_old_librarian_inferred` deletes by predicate, not by id. Its cascade
must collect the doomed ids first (`SELECT id ... WHERE <same predicate>`) and
purge edges for that set inside the same transaction, rather than running the
per-id statement above.

**Edges are not replicated, and this spec does not change that.** A previous
draft required each purge to push `OutboxOperation::Delete` rows for the
removed edge ids. That is wrong in the current design: `commit_edge_add`
(`commit.rs:1056`) and the bundle import path (`bundle_apply.rs:486`) both
insert edges with **no** outbox push, and no `push_edges_outbox` helper exists
(only `push_entries_outbox`, `commit.rs:501`, and `push_tasks_outbox`, `:523`).
Replicating edge deletes while edge inserts were never replicated would make
the remote diverge, not converge. `push_outbox_row` (`outbox_format.rs:60`)
does take a generic `table_name`, so an edges channel is buildable — but
"start replicating the edge table" is its own scope item with its own
consumer-side work, and it is a **non-goal here** (§6). Purges are local-only,
exactly like the inserts they undo.

**Reviewed decision — soft-delete purges edges too.** Cascading at the
soft-delete (archive/heal) sites means a recovered row (the
`UPDATE ... SET deleted_at = NULL` recovery recipe) comes back without its
edges. Accepted, on two grounds: edges are cheap derived structure the
librarian can rebuild, and — the load-bearing reason — one invariant
("an edge exists only between two live entries") is cheaper to keep true and to
test than a two-tier rule where edge liveness is a join away and every future
reader has to remember it. Kurt's acceptance criterion ("regeneration
automatically drops its old edges without leaving ghosts") chooses the strict
rule.

### 2.1 Prerequisite bug — `prune_old_librarian_inferred` is inert

The hard-delete site in the table above **does not delete anything in
production today**, and the spec must not build a cascade on top of it without
saying so. It compares `deleted_at < ?1` where `?1` is unix **seconds**
(`lib.rs:1780`, `SystemTime::…as_secs()`), while every writer stores
**milliseconds**: `ms_now()` (`commit.rs:88`), `ctx.now_ms` in
`commit_fact_archive`, and `ms_now()` in `heal_lost_librarian_inferred`. A
millisecond stamp (~1.7e12) is never less than `unix_secs - 604800` (~1.7e9),
so the 7-day prune has never fired. The existing test passes only because its
fixture inserts `deleted_at` in seconds (`lib.rs:4095-4117`) — it agrees with
the bug rather than catching it.

**In scope for this PR:** fix the comparison to operate in milliseconds, and
change the test fixture to store millisecond stamps so it exercises the
production shape. Do this **before** wiring the edge cascade at that site,
otherwise the cascade is untested-in-practice dead code and any new test
written against a seconds-based fixture will go green while production stays
broken.

Two consequences to keep in mind while fixing it: the prune becomes live for
the first time, so the first run after release may delete a backlog of
long-soft-deleted `librarian_inferred` rows (expected and correct — but it is
why Part C's preflight backup matters even for users who skip the migration);
and it hard-deletes without pushing entries-outbox deletes, unlike
`clear_entity_content`. That outbox gap is now in scope: the ms/s fix
makes this hard-delete live for the first time, so the blast radius changed.
`prune_old_librarian_inferred` must push one `OutboxOperation::Delete` row
per pruned id, matching `clear_entity_content` and `wiki_forget`.

### Defense in depth — reader-side filter (ALREADY PRESENT, no work)

A previous draft asked the implementer to add `AND s.deleted_at IS NULL` /
`AND t.deleted_at IS NULL` to the traversal joins. **Both direction queries
already carry both predicates on both joins** — `fetch_outbound_neighbors`
(`wiki_graph.rs:233`) and `fetch_inbound_neighbors` (`wiki_graph.rs:270`).
There is nothing to change here. The only deliverable is a characterization
test pinning the behavior so a future refactor cannot silently drop it (see
acceptance criterion 5).

Note the consequence for the soft-delete decision below: because the sole
traversal reader already filters soft-deleted endpoints, the "ghost edges
visible whenever a reader forgets the `deleted_at` filter" argument does not
actually apply to any code that exists today. The strict rule is still
adopted, but on the narrower and more honest grounds stated next.

### Transaction boundary

The entry deletion/regeneration and the edge purge **must run in the same
SQLite transaction** so a crash between the two writes can never mint a new
orphan. All listed sites already execute inside `BEGIN IMMEDIATE`
transactions (established pattern, see okf-backend-migration spec §"Commit
path"); the purge statements join that transaction — no new transaction
nesting, no network I/O added inside it.

## 3. Part B — Write-Time Embeddings

Closes the loop so `wiki_search` actually works.

### Sequencing: the sweep is the mechanism, write-time embed is the optimization

Land these in order, ideally as two reviewable commits:

- **B.1 — the null-embedding sweep** (see "Failure isolation" below). This alone
  makes `wiki_search` work: entries land with `embedding_blob = NULL` exactly as
  they do today, and the sweep fills them shortly after. Zero changes to the
  commit path, zero new failure modes on the write path, and it is the same code
  Part C's migration invokes.
- **B.2 — write-time embedding.** Purely a latency optimization: it closes the
  window (one maintenance interval, worst case) during which a just-committed
  fact is invisible to `wiki_search`. It is worth doing — the librarian commits
  a fact and an agent may query for it moments later — but it is not what makes
  the feature work, and it carries the restructuring cost below. If B.2 slips,
  the feature still ships.

### The write path (B.2)

`commit_fact_add` (`src-tauri/src/db/commit.rs:624`) currently hard-inserts
`embedding_blob = NULL, embedding = NULL` (`commit.rs:668`). Change: compute the
entry embedding **before** opening the commit transaction, and insert the blob
when available.

**This does not fit the current function shape, and the restructuring is part of
the work.** `commit_fact_add` receives `conn: &Connection` already inside the
caller's transaction, parses `body` out of the payload itself, and runs the
normalized-body dedupe check itself (`commit.rs:648-660`). It has no pre-tx
phase to hook. Required structure:

1. **Pre-pass, before `BEGIN IMMEDIATE`:** walk the loaded items, parse the
   `body`/`title` of each `fact_add` (and each `fact_update` whose body
   changed), and batch-embed them in one `embed_batch` call (≤64 per call).
2. **Thread the results in:** carry a `HashMap<item_id, Vec<f32>>` on
   `CommitContext`. `commit_fact_add` looks its own `item.id` up and inserts the
   blob if present, `NULL` if absent. A missing entry is not an error — it is
   the failure-isolation path below.
3. **Accept that the pre-pass embeds before dedupe.** The dedupe check needs the
   transaction (it reads sibling entries), so duplicates will burn provider
   calls. Acceptable: proposal batches are small and exact-duplicate `fact_add`s
   are rare. Do **not** try to hoist dedupe out of the transaction to avoid it —
   that trades a few wasted API calls for a TOCTOU race on the dedupe invariant.

- **Text embedded:** `format!("{title}\n\n{body}")` — the entry's prose, same
  convention as chunk text (prose is what the librarian curated; keep it
  provider-agnostic).
- **Provider:** `load_embed_profile` (`src-tauri/src/retrieval/mod.rs:96`) — the
  same active profile that embeds chunks (currently OpenRouter
  `qwen/qwen3-embedding-4b`). Entry vectors and chunk vectors must share the
  profile so the wiki-graph-tools dimension guard
  (`length(embedding_blob) / 4 == active profile dim`, mcp-wiki-graph-tools
  design line 46) holds.
- **API:** `embedder::embed_batch(&profile, texts)`
  (`src-tauri/src/embedder/mod.rs:173`) for the pre-pass;
  `embed_one` (`:196`) only where a single entry is genuinely all there is.
  Blob format little-endian f32, 4 bytes/dim — identical to the chunks
  embedding path (`src-tauri/src/db/queries.rs:81`). Both are synchronous
  blocking calls, which is precisely why they must run outside the transaction.
- **Ordering:** embed → open tx → insert entry + outbox row → commit. Never a
  network call inside the transaction (lock-hold hazard per
  architectural-pitfalls).
- **Update path:** `commit_fact_update` (`commit.rs:726`) and `update_fact`
  (`facts.rs:215`) set `embedding_blob = NULL` when the row is edited. The
  sweep then re-derives the vector from the new title/body. This delegates
  re-embedding to the next sweep trigger (startup or `run_wiki_heal`) rather
  than running a blocking network call inside the edit transaction.
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

**The sweep's trigger must be built — there is no periodic tick to inherit.** A
previous draft placed the sweep on "the existing periodic maintenance tick
(same scheduler slot the heal/prune maintenance run uses)". No such slot
exists: `heal_invalid_sources` runs off a debounced mpsc channel driven by
vault events (`lib.rs:506`), and `heal_lost_librarian_inferred` /
`prune_old_librarian_inferred` are reachable only from the `run_wiki_heal` /
`run_wiki_forget` Tauri commands (`lib.rs:1783`) — all event- or user-triggered,
none periodic. Since the sweep is the sole recovery path for a failed embed,
leaving its trigger unspecified would mean failed embeds are never retried.

Required triggers (all three, cheapest first — the sweep's `WHERE
embedding_blob IS NULL` predicate makes a no-op run nearly free):

1. **After each successful commit batch**, on the same background thread that
   already services the commit — the common case, and what keeps a
   just-committed fact searchable when B.2's pre-pass failed.
2. **At app startup**, once, after the schema guard runs — catches anything
   left null by a crash or by a previous build predating B.1.
3. **On the existing `run_wiki_heal` command path**, so there is a manual
   recovery lever that does not require a restart.

Each run batches pending ids (≤64 per
`embedder::embed_batch` call, `embedder/mod.rs:173`), and updates rows one
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
-- Migration Step 1: purge edges with no live endpoint anywhere.
-- Heterogeneous endpoint contract: an endpoint is alive if it exists in
-- llm_wiki_entries (deleted_at IS NULL), curated_entities (deleted_at IS NULL),
-- or llm_wiki_tasks (deleted_at IS NULL). An edge is orphan iff an endpoint
-- is absent from every table. Matches Part A's revised contract.
DELETE FROM llm_wiki_edges
 WHERE NOT (
       EXISTS (SELECT 1 FROM llm_wiki_entries s
                WHERE s.id = llm_wiki_edges.source_id AND s.deleted_at IS NULL)
    OR EXISTS (SELECT 1 FROM curated_entities ce
                WHERE ce.id = llm_wiki_edges.source_id AND ce.deleted_at IS NULL)
    OR EXISTS (SELECT 1 FROM llm_wiki_tasks st
                WHERE st.id = llm_wiki_edges.source_id AND st.deleted_at IS NULL)
 )
 OR NOT (
       EXISTS (SELECT 1 FROM llm_wiki_entries t
                WHERE t.id = llm_wiki_edges.target_id AND t.deleted_at IS NULL)
    OR EXISTS (SELECT 1 FROM curated_entities ce
                WHERE ce.id = llm_wiki_edges.target_id AND ce.deleted_at IS NULL)
    OR EXISTS (SELECT 1 FROM llm_wiki_tasks st
                WHERE st.id = llm_wiki_edges.target_id AND st.deleted_at IS NULL)
 );
```

On today's DB both forms delete the same 41 rows (all endpoints are
hard-absent), so the broader contract is a no-op here; the heterogeneous form
is the one that stays correct as the table evolves, because the contract was
broadened to handle the three endpoint tables. (`NOT IN` → `NOT EXISTS` also
sidesteps the classic NULL-subquery trap.) Post-condition check, must print 0:

```sql
SELECT COUNT(*) FROM llm_wiki_edges e
 WHERE NOT (
       EXISTS (SELECT 1 FROM llm_wiki_entries s
                WHERE s.id = e.source_id AND s.deleted_at IS NULL)
    OR EXISTS (SELECT 1 FROM curated_entities ce
                WHERE ce.id = e.source_id AND ce.deleted_at IS NULL)
    OR EXISTS (SELECT 1 FROM llm_wiki_tasks st
                WHERE st.id = e.source_id AND st.deleted_at IS NULL)
 )
 OR NOT (
       EXISTS (SELECT 1 FROM llm_wiki_entries t
                WHERE t.id = e.target_id AND t.deleted_at IS NULL)
    OR EXISTS (SELECT 1 FROM curated_entities ce
                WHERE ce.id = e.target_id AND ce.deleted_at IS NULL)
    OR EXISTS (SELECT 1 FROM llm_wiki_tasks st
                WHERE st.id = e.target_id AND st.deleted_at IS NULL)
 );
```

Run inside one `BEGIN IMMEDIATE` transaction. No outbox rows: edges are not
replicated, and the migration matches the runtime path (§2).

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
   test's transaction. Same assertion for the prune (hard-delete) path — and
   that test's fixture **must write `deleted_at` in milliseconds**, matching
   production writers. A seconds-based fixture goes green against the §2.1 bug
   and proves nothing; that is exactly how the existing prune test
   (`lib.rs:4095-4117`) missed it.
3a. **The prune actually prunes.** Regression test for §2.1: insert a
   `librarian_inferred` row with a millisecond `deleted_at` older than 7 days,
   run `prune_old_librarian_inferred` with a millisecond `now`, assert the row
   is deleted and a fresher row survives.
4. **Embed failure never loses curation.** Unit test with a failing embed
   profile: fact commit still succeeds, row lands with `embedding_blob IS
   NULL`, sweep run against a mock-good provider fills it.
5. **Reader filter holds (characterization — behavior already correct).**
   Pin the existing `wiki_graph.rs` filtering so a refactor cannot drop it:
   manually insert an edge whose endpoint is soft-deleted (simulating a
   pre-contract ghost) → both direction queries exclude it. This test is
   expected to pass on an unmodified `wiki_graph.rs`; if it fails, the premise
   in §2 is wrong and the review should stop there.
6. **PR #78's stopgap stays green.** Existing short-circuit tests (wiki legs
   return empty, not error, when the table is empty) still pass — this spec
   adds population, it does not remove the graceful-empty contract.

## 5.1 Operator runbook — switching embedding models

Entry vectors and chunk vectors must share the active profile. `wiki_search`
compares `length(embedding_blob) / 4` against the query vector's dimension and
**silently skips** rows that disagree (`wiki_graph.rs:150`) — a model switch
therefore degrades wiki search to empty results with no error in the logs.

After changing the embed profile, with the app and watcher stopped:

1. Back up: `cp ~/.brain/brain.db ~/.brain/brain.db.bak-pre-modelswitch-$(date +%s)`
2. Invalidate every entry vector:
   `sqlite3 ~/.brain/brain.db "UPDATE llm_wiki_entries SET embedding_blob = NULL;"`
3. Re-embed at the new dimension:
   `cargo run --manifest-path tools/Cargo.toml --bin graph_reanchor -- --yes`
   (Step 1's edge purge is a no-op on a healthy DB; step 2 does the backfill.)
4. Verify: `wiki_search` returns hits again for a known entry title.

Chunk vectors need the separate `bulk_reindex` tool; this procedure covers wiki
entries only.

## 6. Non-Goals

- Code/AST symbol graph and `[[wiki-link]]` edge indexing — separate backlog
  items; this spec is strictly about the wiki fact graph.
- Replicating embeddings through the outbox (remote re-derives).
- **Replicating the edge table through the outbox.** Edge inserts are not
  replicated today (§2), so edge purges are not either. Starting edge CDC is a
  separate scope item with consumer-side work; file it as a follow-up.
- Changing `wiki_search`'s public tool contract (its shape is already correct;
  only its results were empty).
