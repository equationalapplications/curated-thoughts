# Sentence-Aware Chunking Design

**Date:** 2026-05-06  
**Goal:** Improve semantic search Recall@10 by replacing the fixed-word sliding window with sentence-boundary chunking and neighbor-sentence padding.

---

## Background

Current chunker splits text into 180-word windows with 20-word overlap. This cuts sentences mid-way, diluting embedding signal. SciFact benchmark scores Recall@10 = 0.823 with 5183 single-chunk documents. Sentence-level chunks with padding are expected to push recall toward 0.85+.

---

## Architecture

### Sentence Splitter

Pure Rust, no external crates. Scan text char-by-char:

- Sentence boundary: `.`, `!`, or `?` followed by whitespace + uppercase letter (or end of string).
- Skip abbreviation-like patterns: preceding token is ≤3 chars (catches "et al.", "Fig.", "vs."), or the character before `.` is a digit (catches "0.05", "1.5").
- Output: `Vec<&str>` sentence slices.

### Chunk Composer

- Accumulate sentences until total word count ≥ `TARGET_WORDS` (100).
- On crossing threshold, emit a group and start a new one.
- Final group emitted even if below threshold.

### Neighbor Padding

For group `i` of N groups:

```
stored_text = [last sentence of group i-1]  (if i > 0)
            + [all sentences of group i]
            + [first sentence of group i+1] (if i < N-1)
```

The embedding is computed on `stored_text`. The chunk boundary (for deduplication and position tracking) is defined by the core group only.

### Constants

```rust
const TARGET_WORDS: usize = 100;
```

No separate overlap constant — padding replaces overlap.

---

## File Changes

| File | Change |
|---|---|
| `src-tauri/src/chunker/mod.rs` | Replace sliding-window with sentence chunker + padding. Update unit tests. |
| `src-tauri/src/bin/embed_scifact.rs` | Call `chunk_text` on each corpus doc. Embed per-chunk. Key = `{doc_id}:{chunk_idx}`. Store as `HashMap<String, Vec<Vec<f32>>>` (doc_id → vec of chunk vectors). |
| `src-tauri/tests/scifact.rs` | `seed_corpus`: for each doc, insert one row per chunk (same `doc_path` = corpus `_id`). Load multi-chunk embeddings from updated fixture format. |
| `src-tauri/tests/fixtures/scifact/scifact-embeddings.json.gz` | Regenerated: `{"doc_id": [[vec], [vec], ...]}` (list of chunk vectors per doc). |

---

## Fixture Format Change

Old: `{ "doc_id": [f32 × 384] }` — one vector per doc.  
New: `{ "doc_id": [[f32 × 384], ...] }` — list of vectors per doc (one per sentence chunk).

`embed_scifact.rs` writes the new format. `scifact.rs` reads and seeds accordingly.

---

## Recall Impact

Expected: Recall@10 ≥ 0.85 (up from 0.823). Each relevant document now has multiple shorter, focused embeddings that better match specific claim phrasing.

Assertion threshold remains 0.60 (conservative) — raise to 0.80 after confirming stable results.

---

## Testing

1. Unit tests for sentence splitter: empty, single sentence, multi-sentence, abbreviations, decimals.
2. Unit tests for chunk composer: single group, multi-group, padding at boundaries (first/last group have no prev/next neighbor).
3. Integration: `cargo test` (42 existing unit tests still pass).
4. Benchmark: `cargo test --features test-utils,slow-tests --test scifact -- --nocapture` asserts Recall@10 ≥ 0.60 (expected ~0.85).
