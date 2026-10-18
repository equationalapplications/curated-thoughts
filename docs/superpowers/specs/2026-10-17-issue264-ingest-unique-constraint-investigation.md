# Investigation — issue #264: intermittent `UNIQUE constraint failed: chunks.doc_id, chunks.content_hash` on re-ingest

**Date:** 2026-10-17 · **Investigator:** Tessera (controller) · **Status:** fix already on main verified (PR #283, `4517f860`); historical trigger explained as the most-consistent hypothesis (§3) and the defect class proven removed by construction on main (§4–§5).
**Reported against:** curated-thoughts 3.2.0 dpkg (Kurt's report notes 2026-10-05/06; dpkg now at 3.5.0 — both predate the fix)

## 1. The report

`ct ingest --yes` intermittently fails a small, run-varying subset of docs with
`UNIQUE constraint failed: chunks.doc_id, chunks.content_hash` (SQLite 2067) on a
**completely unchanged** vault; a file failing in run N succeeds in run N+1. Breaks
scripted post-edit sweeps (exit non-zero with no real failure). Self-heals on retry.

## 2. Schema facts (live brain, read-only probes 2026-10-17)

- The 2067 comes from `idx_chunks_doc_hash` — `CREATE UNIQUE INDEX ... ON
  chunks(doc_id, content_hash) WHERE content_hash != ''` (partial: legacy
  empty-hash rows are exempt). App-enforced chunk identity is DB-enforced.
- Verified live: **0 orphan chunks** of 1451 (`chunks.doc_id NOT IN documents`
  = 0) — no accumulated corruption from the historical races.
- `documents.id`/`chunks.id` both `INTEGER PRIMARY KEY AUTOINCREMENT` → no rowid reuse
  class (a deleted chunk's id is never silently re-bound).
- `chunks.doc_id REFERENCES documents(id) ON DELETE CASCADE`; `PRAGMA foreign_keys=ON`
  is set per-connection (connection.rs:221) — cascade health is not the issue.

## 3. The faulty shape (verified present at tag v3.5.0, `8b9b57a2`; `ingest_file_virtual`)

```rust
if let Some(doc) = get_document_by_path(conn, virtual_path)? {
    if !force_rechunk && doc.hash == hash && doc.status == "indexed" { return Ok(()); }
    delete_document_chunks(conn, doc.id)?;          // ① delete FIRST, own autocommit
}
...                                                  // ② network embed (SLOW, seconds)
let doc_id = upsert_document(conn, virtual_path, &hash)?;
for (i, ...) {
    insert_chunk(conn, doc_id, chunk, i, &eid, &content_hash)?;   // ③ bare autocommit INSERTs
}
```

**Arming.** The guard (`!force_rechunk && doc.hash == hash && doc.status == "indexed"`)
means only docs whose content changed OR whose status is not `indexed` re-enter
this code at the default `force_rechunk == false`; a `--force`/rechunk pass
arms EVERY doc, including unchanged indexed ones. The issue was filed during
"routine post-edit vault re-ingests", so the armed
population on each run is: newly edited docs, plus every doc a PREVIOUS racy
run left in a non-indexed state (a failed run updates `documents.hash` via
`upsert_document` but leaves status `error`/`pending` with a partial or foreign
chunk set) — each such doc re-arms on the next run until one completes cleanly.
That is why the failing set varies between runs with no new edits (2→5→5→8:
the armed set evolves as prior failures re-arm), and why a file that failed in
run N succeeds in run N+1.

**The race.** The delete (①) and the inserts (③) are separate autocommit
transactions separated by a multi-second network embed (②). Two actors — the
watcher's recovery path, a scheduled sweep, and a manual/cron `ct ingest` all
ran on this machine — that reach the same armed doc with staggered timing
interleave like this:

- Actor A: deletes the doc's chunks → embeds (in flight, seconds) …
- Actor B (armed by the same non-indexed status): deletes (no-op), embeds …
- A commits its full chunk set, marks indexed …
- B commits its chunk set → collides with A's rows on (doc_id, content_hash)
  → **2067 on an arbitrary subset** of the armed files.

The exact production interleaving cannot be replayed retrospectively; this is
the most consistent explanation of every reported property (run-varying
subset on unchanged input, self-heal on retry, no permanent corruption, and
escalating counts as failures re-arm more docs).

## 4. The fix already on main (`4517f860`, PR #283, merged 2026-10-17 — "rev-5 M2 diff-swap")

`ingest_file_virtual` on current main:

1. **One `IMMEDIATE` transaction** spans: `upsert_document` → re-read stored chunk
   hashes **under the lock** (`stored_now`) → `swap_race_check` aborts cleanly
   (retryable) if the under-lock read mismatches the pre-embed read → diff application
   (kept-hash chunks UPDATE in place; new hashes INSERT; removed hashes DELETE).
2. INSERT fires **only** for hashes absent from the under-lock stored set; since the
   IMMEDIATE lock is held across the entire swap, a concurrent writer cannot commit
   colliding (doc_id, content_hash) rows between the check and the insert — writers
   serialize.
3. The network embed happens BEFORE the transaction opens; no multi-statement window
   across the embed exists anymore.
4. Regression coverage landed with it: swap-race parity test + SQL literal pins
   (RED-proven by mutation in Task 14's review), dup-cleanup doc-scope regression test
   (`5d2162d8`, CodeRabbit Critical).

## 5. Verification on current main (2026-10-17, A/B: v3.5.0 binary vs main `cf3d2b90`)

Harness: scratch vaults, temp brains, `CURATED_EMBED_STUB=constant8` where
stated; the real OpenRouter embedder (qwen3-embedding-4b) where stated — the
network embed is what makes the delete→insert window production-sized. A
critique round (GLM-5.3, tool-verifying) rejected an earlier touch-only
"concurrency" probe as vacuous — `touch` changes mtime only, the hash guard
early-returns, no write path runs — so every probe below uses REAL content
mutations, and the table records the failed reproduction attempts too.

| scenario | v3.5.0 | main |
|---|---|---|
| unchanged double-run (stub, 40 docs) | (guard early-returns both runs — same at both versions) | 40/40 ok ×2, exit 0 |
| 3 concurrent ingests after real edits, stub, simultaneous | not run | 40/40 ok ×3, exit 0 |
| 2 concurrent, real edits, REAL embedder, simultaneous start | race2 fails cleanly at open: `database is locked` (code 5) | same clean open failure |
| 2 concurrent, staggered mid-run, REAL embedder | no 2067; interference surfaces as OpenRouter 429 rate-limits (4+2 failed docs) | — |
| **2 concurrent, near-overlap start (actor 2 at file ~3), REAL embedder** | no 2067; 429 failures | **both actors 12/12 ok, 0 failed, 0 2067** |

**Reading.** The historical 2067 could NOT be replayed from two CLI processes
on v3.5.0 in any scenario: the content-hash guard, the watchdog heartbeat
supersede (a second worker entering a file's stage aborts the first), and
open-time lock contention each close the window the CLI can reach. The
production trigger (Oct 5/6, 3.2.0) therefore ran through an actor combination
outside that pair — watcher recovery vs bulk/manual paths with divergent job
subjects — and cannot be reconstructed retrospectively. What IS established:

1. the unsound shape (per-statement autocommit delete → network embed → bare
   INSERTs) is present at v3.5.0 and is unserializable under multi-actor
   interleaving in principle;
2. main replaces it with one IMMEDIATE transaction — `upsert_document`,
   under-lock stored-hash re-read, swap-race refusal, diff application — so an
   INSERT can only ever run for a hash absent from the locked re-read, making
   the 2067 structurally unreachable regardless of actor count or timing;
3. main completed every scenario above with zero constraint errors, including
   both actors succeeding concurrently under real-embed load.

That is the closure basis: the defect class is removed by construction, and
the strongest reproducible-today interference main shows is a clean retryable
refusal — never a partial state or a constraint crash.

**Scope of the guarantee.** It holds only for writers running the fixed path
(≥3.6.0, i.e. post-`4517f860`). A legacy writer (≤3.5.0, e.g. a not-yet-upgraded
sidecar or watcher process) still runs the old autocommit delete → embed →
insert shape and can interleave with a current-transaction writer — the mixed
state is exactly what the unique index then rejects. **Every writer sharing a
brain must be upgraded/restarted onto ≥3.6.0 before relying on this guarantee**
(deploy the 3.6.0 sidecar and restart it before scripted multi-actor ingest
against that brain).

## 6. Conclusion + disposition

**Issue #264 is fixed on main** by the same PR-#283 wave that delivered #271 —
the report (filed against 3.2.0 on Oct 5/6; the installed dpkg was 3.5.0 at
investigation time) predates the fix by 12 days, and the racy shape is
verified present at v3.5.0 and absent on main. Disposition:

1. #264 was closed (2026-10-18) with a comment citing this doc, `4517f860`,
   and the verification table above; this investigation is linked from the
   issue as its record.
2. **Operational note for Kurt:** upgrade the installed dpkg (3.5.0) to 3.6.0
   (release commit `91b77baf`) to retire the behavior on the installed CLI.
3. Hardening idea, NOT needed for closure: a `ct doctor`-style census that
   counts docs whose `status='indexed'` but whose chunk sets look partial
   (chunk count regressed vs `last_indexed_hash` history is not stored — so
   practically: docs in `error`/`pending_reindex` older than N days) would
   surface residual armed docs from any future write-path regression.

## 7. Residual risk

- **Contended runs still exit non-zero on main — but cleanly.** Under
  simultaneous starts the loser fails at open with `database is locked`
  (Error code 5, after the 5 s busy timeout), and the swap-race check aborts a
  mid-flight writer with a retryable refusal. The reported *symptom class*
  (non-zero exit breaking scripted sweeps) is therefore narrowed, not
  eliminated: a contended run can still require a retry, but it can no longer
  produce a 2067, a partial chunk set, or a constraint crash mid-loop. Scripts
  that retry on failure get a consistent, recoverable result.
- The stub-embed probes exercise the full transaction path but not the
  embed-latency window; the real-embed A/B run in §5 covers that window.
- Duplicate (doc_id, content_hash) rows cannot persist through the partial
  unique index — the residue class a historical racy era COULD have left is
  *partial chunk sets* (an actor killed between delete and insert), for which
  no cheap census exists today; the dup-embeddings cleanup guard (rev-33)
  bounds the embeddings side of that same window.
