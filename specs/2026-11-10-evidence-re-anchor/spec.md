# `ct evidence re-anchor`: pre-purge re-anchoring of unanchored librarian evidence

**Date:** 2026-11-10
**Status:** Implemented (2026-11-10: applied to the live brain after dry-run gates — backup `brain.db.bak-pre-reanchor-1794342813`; 173 entries re-anchored/regrounded, 0 race-skips, idempotent re-run clean; residual 129 entries adjudication pending with Kurt; spec §3 round 2 normalized-prefix strategy per the reanchored=0-by-construction handoff)
**Supersedes the ad-hoc repair runs of 2026-10-18** (`brain.db.bak-pre-anchor-heal-20261018`, exports under `~/ct-anchor-heal-exports/`) — those were manual; this makes the recovery a gated, idempotent CLI command.

## 1. Problem

`librarian_evidence` rows flagged `unanchored = 1` have lost the chunk anchor that gives their fact provenance (`db/evidence_regrade.rs` header). Two things make this state *recoverable*, not doomed:

1. Chunking/re-anchoring may bring a chunk back that the blob's `content_hash` matches — the `unanchored` flag is potentially stale (the regrade header says stale **in both directions**).
2. The fact itself is usually still true; only its pointer is severed.

Today the only first-class tool for anchorless stock is destructive: `ct evidence regrade` purges provenance-less facts (correctly — for rows nothing can ever re-anchor). An operator facing a mass of `unanchored` rows therefore reaches for the nearest reset lever, which is the problem this command closes.

## 2. Why a direct heal-reset without re-anchoring destroys ~240 still-true facts

The anchorless-evidence incident of 2026-10-18 (verify-scratch lossless blocker; 48 fully anchorless entries exported pre-forget in `ct-anchor-heal-exports/48-anchorless-entries-20261018.json`, with the broader flagged stock at roughly **~240 facts**) demonstrated the failure mode:

- A heal/reset pass that treats `unanchored = 1` as *doomed* deletes the fact **and** its only surviving provenance copy (`evidence_json` carries the quote, `chunk_id`, `content_hash`, line range, `proposal_id`). Once purged, no retraction can ever match it and no proposal can ever be re-attributed through it — the regrade module's own definition of "doomed", applied to rows that were merely *detached*.
- The facts were still true. Only the pointer (chunk path/source file re-chunked, scratch rebuilt, doc re-ingested) was severed. Purging converts a recoverable index fault into permanent knowledge loss that then also silently degrades retrieval: the brain answers without those facts and cannot know it is missing them.
- Hence the ordering invariant this command exists to enforce: **re-anchor first, and only what provably cannot re-anchor may fall through to the (existing) gated purge.** `unanchored` is a pointer fault, not a truth verdict.

## 3. Match strategy

Re-anchoring is content-addressed, never proximity-guessed. **Adjudicated
2026-11-10, round 2 (handoff: live dry-run scored `reanchored=0` BY
CONSTRUCTION)**: the librarian quotes the ENTIRE old chunk (median ~1.2k
chars), and after a re-split no current chunk contains the full quote — the
round-1 verbatim-substring search can never match. Implemented strategy:
normalize both sides (lowercase, whitespace/punctuation runs → single space),
then:

1. the quote's ≥60-char normalized **prefix** matches exactly ONE current
   chunk → re-anchor: rewrite `content_hash` (recomputed via
   `compute_chunk_hash` from the matched chunk — never trusted from the row),
   `chunk_id`, `start_line`, `end_line`;
2. several chunks share the prefix → the quote's last-60-chars normalized
   **suffix** narrows them (prefix+suffix agreement is the strong case);
   still tied — or no chunk agrees on both — → **ambiguous** (left flagged;
   guessing would fabricate provenance);
3. zero chunks → **unresolved** (left flagged; the heal/regrade flow owns it);
4. matched chunk's stored hash ≠ recomputed hash → **chunker drift**; skip.

When EVERY item of an entry resolves, `evidence_has_live_chunk` — the same
grounding contract the commit gate and regrade use — agrees, and `unanchored`
is cleared. Re-running the command is a no-op once everything that can match
has matched.

## 4. Gates

(adjudicated 2026-11-10)

- **Backup before first write — KEPT.** Even though the pass is UPDATE-only, it is still a mass brain write: before any apply, `VACUUM INTO` a crash-consistent snapshot (`brain.db.bak-pre-reanchor-<ts>` in the db dir) — cheap, zero-friction insurance, and the documented rollback path.
- **Dry-run by default.** Like `graph_reanchor`: read-only plan printed unless `--yes`.
- **No breaker interaction — WIP posture adopted.** Rows are only ever UPDATED, never deleted, so the shared bulk-deletion budget is untouched and the breaker is not consulted. Unresolved rows stay flagged `unanchored = 1` for the EXISTING `ct evidence regrade` flow, which owns the gated export-then-purge of genuinely doomed rows — re-anchor never hands rows to a destructive path.

## 5. Idempotency and the report

A `ReAnchorReport` (WIP fields, adjudicated):

- `items_reanchored` — dead hashes rewritten to the matched current chunk (flag cleared when the whole entry grounds);
- `items_ambiguous` — multi-match candidates, left flagged, named by id in stdout;
- `items_unresolved` — still anchorless, left flagged for the existing regrade flow (NOT exported here; regrade owns export-then-purge);
- `items_hash_mismatch` — chunker drift, skipped;
- `entries_already_anchored` — re-run no-ops;
- `entries_race_skipped` — concurrent mutation detected under the lock; re-run converges.

All UPDATEs are idempotent; re-running converges. Soft-deleted stock is never touched — the 90-day prune owns it (regrade invariant).

## 6. Testing posture

Tests follow the `commit.rs` grounding style — small in-memory `Connection`s, assertions that name the invariant they pin, not just the counts:

- a dead hash whose quote lives in exactly one current chunk re-anchors (hash recomputed from the matched chunk, never trusted from the row) and clears the flag when the entry grounds; a re-run of the same DB changes nothing (idempotency);
- a re-chunked doc's shifted boundaries still re-anchor via the verbatim quote; multi-match leaves the row flagged and reported, never silently best-guessed;
- unresolved rows stay flagged, are never deleted and never exported by this command (the regrade flow owns the purge);
- a soft-deleted row is untouched by a full pass;
- the apply path snapshots first: `VACUUM INTO` to `brain.db.bak-pre-reanchor-<ts>` runs before the first UPDATE, and the dry-run path writes no backup file;
- a direct heal-reset fixture (flags treated as doomed, no match attempt) asserts the ~240-fact loss class cannot recur: rows whose chunk exists in live chunks are never purged.
