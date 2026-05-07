# Integration tests

Rust integration tests in this crate live next to `src/` (see [Cargo’s test layout](https://doc.rust-lang.org/cargo/guide/tests.html)). Many tests need the `test-utils` feature; SciFact recall benchmarks additionally need `slow-tests` and take several minutes.

## SciFact search benchmarks (precomputed embeddings)

Recall@10 is measured against the SciFact corpus using frozen gzip JSON fixtures under `fixtures/scifact/`. You can rerun either benchmark **without regenerating embeddings** as long as the matching file is present.

### Embedding files vs benchmark

| Gzip filename | What this preset varies | Cargo test filter |
|---|---|---|
| `scifact-embeddings_all-minilm-l6-v2_dim384_fulltext_single-embedding-per-doc.json.gz` | Full `title + text` embedded once per document; JSON maps each doc id to a **single** flat `f32 × 384` array | `scifact_recall_fulltext_single_embedding_benchmark` |
| `scifact-embeddings_all-minilm-l6-v2_dim384_sentence-chunk_t100_neighbor-pad_multichunk-per-doc.json.gz` | Sentence-boundary grouping with **target 100 words**, **neighbor sentence padding**, one vector per stored chunk; JSON maps each doc id to a **list** of chunk vectors `[[384], …]` | `scifact_recall_sentence_chunk_neighbor_pad_benchmark` |

Rust constants for those basenames live in [`src/scifact_fixture.rs`](../src/scifact_fixture.rs) (`FULLTEXT_SINGLE_EMBEDDINGS_GZIP_FILENAME`, `SENTENCE_CHUNK_MULTICHUNK_EMBEDDINGS_GZIP_FILENAME`).

### Naming convention

All SciFact embedding fixtures use this pattern:

```text
scifact-embeddings_<model>_dim<size>_<encoding-preset>.json.gz
```

- **`<model>`** — Embedding model shorthand aligned with sentence-transformers, e.g. `all-minilm-l6-v2` (crate: `EmbeddingModel::AllMiniLML6V2`).
- **`dim<size>`** — Vector width, e.g. `dim384`.
- **`<encoding-preset>`** — How text was prepared before embedding (these are what we varied between the two benchmarks):
  - **`fulltext_single-embedding-per-doc`** — One string per doc (combined title + body), one vector each.
  - **`sentence-chunk_t100_neighbor-pad_multichunk-per-doc`** — Chunker uses sentence-aware groups (~100-word target), padding with adjacent sentences for context, multiple chunk rows (and vectors) per document.

Shared corpus inputs (`corpus.jsonl`, `queries.json`, `qrels.json`) stay in [`fixtures/scifact/`](fixtures/scifact/).

### Run benchmarks

From `src-tauri/`:

```bash
# Full-document embedding fixture (baseline / bench 1)
cargo test --features test-utils,slow-tests --test scifact scifact_recall_fulltext_single_embedding_benchmark -- --nocapture

# Sentence chunk + neighbor padding (bench 2)
cargo test --features test-utils,slow-tests --test scifact scifact_recall_sentence_chunk_neighbor_pad_benchmark -- --nocapture
```

Pass criterion: Recall@10 ≥ **0.60** (fixed in [`tests/scifact.rs`](scifact.rs)).

### Regenerate an embedding gzip (optional)

```bash
cd src-tauri
cargo run --bin embed_scifact -- fulltext-single   # overwrites …fulltext_single-embedding-per-doc…
cargo run --bin embed_scifact -- sentence-chunk    # default; overwrites …multichunk-per-doc…
```

See the `embed_scifact` crate binary doc comment for synonym flags (`v1`, `v2`, etc.).
