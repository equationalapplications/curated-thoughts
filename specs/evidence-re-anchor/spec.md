# `ct evidence re-anchor`: pre-purge re-anchoring of unanchored librarian evidence

**Date:** 2026-11-10
**Status:** Draft (design for the `ct evidence re-anchor` implementation task)
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

Re-anchoring is content-addressed, never proximity-guessed. For each live `unanchored = 1` evidence blob:

1. **Exact chunk hash match.** Match `evidence_json[].content_hash` against live `llm_wiki_chunks` chunk hashes. A hit re-anchors: `UPDATE librarian_evidence SET unanchored = 0` with the new `chunk_id` (same hash ⇒ same bytes ⇒ the quote is verbatim — no fabrication risk).
2. **Doc-level fallback (same document, re-chunked).** Where the doc is re-chunked and no single chunk hash matches (boundaries moved), match on (source document, `content_hash` of the quoted span recomputed from chunk text) — a hit re-anchors to the chunk that now contains the quote. A multi-hit is refused for that row (ambiguous), reported, and left flagged; never best-guess.
3. **No match.** Row stays `unanchored = 1`. It is NOT deleted here — it re-enters the existing `ct evidence regrade` flow, which owns the gated export-then-purge of genuinely doomed rows.

Re-anchor UPDATEs are recovery actions: they commit regardless of the pass's overall verdict (mirrors the issue-271 re-anchor semantics), and re-running the command is a no-op once everything that can match has matched.

## 4. Gates

Borrowed wholesale from `evidence_regrade.rs` / the shared breaker:

- **Breaker-governed.** A re-anchor pass may also *clear* flags (harmless) but any fall-through that hands rows to the destructive path must check the shared bulk-deletion breaker inside the transaction; a refusal deletes nothing and reports loudly.
- **Dry-run by default.** Like `graph_reanchor`: read-only plan printed unless `--yes`.
- **Backup before first write.** `VACUUM INTO` crash-consistent snapshot (`brain.db.bak-pre-reanchor-<ts>`), same as the 2026-10-18 run — this backup is the documented rollback path.
- **Report, don't improvise.** Every skip is a loud WARN naming the idempotent manual recovery (`ct evidence re-anchor`, or `ct evidence regrade` for the doomed remainder).

## 5. Export-first

Before any fall-through rows reach the regrade purge, the pass exports the full row (`DoomedRow` shape: entry + body + `source_ref` + `evidence_json` + `proposal_id`) to `<db_dir>/repair-export-reanchor/<id>.json` — its own directory so an operator can tell this repair's exports from `repair-export-phase2` and `repair-export-186` at a glance. The token `source_ref` is content-free, so anything less than the full row destroys the only surviving provenance copy (spec C1 posture).

## 6. Idempotency and the report

A `ReAnchorReport`, in the `RegradeReport` mold:

- `matched_exact` / `matched_rechunked` — flags cleared with a new anchor;
- `ambiguous` — multi-match candidates, left flagged, named by id in stdout;
- `unmatched` — still anchorless, exported, handed to the regrade flow;
- `already_anchored` — re-run no-ops.

All UPDATEs are idempotent (`SET unanchored = 0` where the predicate still holds); re-running converges. Soft-deleted stock is never touched — the 90-day prune owns it (regrade invariant).

## 7. Testing posture

Tests follow the `commit.rs` grounding style — small in-memory `Connection`s, assertions that name the invariant they pin, not just the counts:

- exact-hash hit re-anchors and clears the flag; a re-run of the same DB changes nothing (idempotency);
- re-chunked doc re-anchors to the chunk containing the quote; multi-match leaves the row flagged and reported, never silently best-guessed;
- unmatched rows export the FULL row (assert `evidence_json` and `proposal_id` survive into the JSON on disk) before any purge path;
- breaker refusal deletes nothing and reports `skipped` with the recovery command in the WARN;
- soft-deleted rows untouched;
- a direct heal-reset fixture (flags treated as doomed, no match attempt) asserts the ~240-fact loss class cannot recur: rows whose chunk exists in live chunks are never purged.
