# Retrieval benchmarks — 2026-05-07

Snapshot of **Recall@10** runs from the `slow-tests` harness. Numbers below are copied from test output with **`--nocapture`** so `println!` metrics are visible.

## Commit and environment

| Field | Value |
|-------|--------|
| **Date (UTC)** | 2026-05-07 |
| **Git revision** | `b0ef54c` |
| **Crate** | `src-tauri` (`curated-thoughts` / `tauri_app_lib`) |
| **Features** | `test-utils`, `slow-tests` |

Machine/OS were not recorded for this snapshot; re-runs may differ slightly if the embedder or drivers change.

## Embedding model and dimensions

| Setting | Value |
|---------|--------|
| **Model** | FastEmbed `EmbeddingModel::AllMiniLML6V2` (same family as filenames: `all-minilm-l6-v2`) |
| **Vector width** | 384 |

SciFact tests **re-embed each query at runtime** via `Embedder::new()` while documents use **frozen** gzip JSON archives. YAML/code benches embed queries the same way against fixture-backed chunks.

## What was tested

| Suite | Test targets | Metric | Asserted floor (test code) |
|-------|----------------|--------|---------------------------|
| **SciFact** | `tests/scifact.rs`: full-document vs sentence-chunk corpus | Recall@10 vs 300-query eval | ≥ **0.60** |
| **Code** | `tests/code_bench_curated.rs`, `tests/code_bench_synthetic.rs` | Recall@10 | ≥ **0.90** |
| **YAML** | `tests/yaml_bench_k8s_curated.rs`, `tests/yaml_bench_synthetic.rs` | Recall@10 | ≥ **0.90** |

### SciFact corpus notes

- Fixture directory: `src-tauri/tests/fixtures/scifact/`
- Printed seed size: **5183** corpus documents (paths stored as doc ids in the harness).
- Eval denominator in logs: **300** queries (see `tests/scifact.rs`: `Recall@10 (hits/300)`).

**Chunking for the sentence-chunk preset:** seed path uses `chunk_text()` (sentence groups + neighbor padding) and asserts chunk counts match the multichunk embedding fixture.

### YAML / code corpus notes

Seeded with `chunk_text()` per chunk in `tests/helpers/recall_bench.rs` (`seed_sentence_chunks`). Fixture roots:

| Bench label | Fixture subdirectory under `tests/fixtures/` |
|-------------|-----------------------------------------------|
| code-curated | `code-bench-curated/` |
| code-synthetic | `code-bench-synthetic/` |
| yaml-k8s-curated | `yaml-bench-k8s-curated/` |
| yaml-synthetic | `yaml-bench-synthetic/` |

Printed doc counts (from logs): code-curated **52**, code-synthetic **96**, yaml-k8s-curated **48**, yaml-synthetic **80**. Query counts for Recall@10 lines: **72** where noted below.

## Frozen embedding archives (gzip JSON)

Paths are **relative to each fixture directory** (same folder as `corpus.jsonl`). Constants are in source for exact spelling:

### SciFact (`src-tauri/tests/fixtures/scifact/`)

| Preset | Embedding archive filename |
|--------|----------------------------|
| Full-document single vector per doc | `scifact-embeddings_all-minilm-l6-v2_dim384_fulltext_single-embedding-per-doc.json.gz` |
| Sentence-chunk multichunk per doc | `scifact-embeddings_all-minilm-l6-v2_dim384_sentence-chunk_t100_neighbor-pad_multichunk-per-doc.json.gz` |

### Code benches

| Bench | Embedding archive filename |
|-------|----------------------------|
| Curated | `code-curated-embeddings_all-minilm-l6-v2_dim384_sentence-chunk_t100_chunk-text_multichunk-per-doc.json.gz` |
| Synthetic | `code-synthetic-embeddings_all-minilm-l6-v2_dim384_sentence-chunk_t100_chunk-text_multichunk-per-doc.json.gz` |

### YAML benches

| Bench | Embedding archive filename |
|-------|----------------------------|
| K8s curated | `yaml-k8s-curated-embeddings_all-minilm-l6-v2_dim384_sentence-chunk_t100_chunk-text_multichunk-per-doc.json.gz` |
| Synthetic | `yaml-synthetic-embeddings_all-minilm-l6-v2_dim384_sentence-chunk_t100_chunk-text_multichunk-per-doc.json.gz` |

## Results (this run)

### SciFact Recall@10

| Preset | Recall@10 | Hits | Embedding archive |
|--------|-----------|------|-------------------|
| **fulltext-single** | **0.823** | 247/300 | `scifact-embeddings_all-minilm-l6-v2_dim384_fulltext_single-embedding-per-doc.json.gz` |
| **sentence-chunk** (neighbor-padded `chunk_text`) | **0.810** | 243/300 | `scifact-embeddings_all-minilm-l6-v2_dim384_sentence-chunk_t100_neighbor-pad_multichunk-per-doc.json.gz` |

Both tests printed a short list of missed claims (first 10); see raw logs when reproducing with `--nocapture`.

### Code Recall@10 (*k* = 10)

| Bench | Recall@10 | Hits | Embedding archive |
|-------|-----------|------|-------------------|
| **code-curated** | **1.000** | 72/72 | `code-curated-embeddings_all-minilm-l6-v2_dim384_sentence-chunk_t100_chunk-text_multichunk-per-doc.json.gz` |
| **code-synthetic** | **1.000** | 72/72 | `code-synthetic-embeddings_all-minilm-l6-v2_dim384_sentence-chunk_t100_chunk-text_multichunk-per-doc.json.gz` |

### YAML Recall@10 (*k* = 10)

| Bench | Recall@10 | Hits | Embedding archive |
|-------|-----------|------|-------------------|
| **yaml-k8s-curated** | **1.000** | 72/72 | `yaml-k8s-curated-embeddings_all-minilm-l6-v2_dim384_sentence-chunk_t100_chunk-text_multichunk-per-doc.json.gz` |
| **yaml-synthetic** | **0.958** | 69/72 | `yaml-synthetic-embeddings_all-minilm-l6-v2_dim384_sentence-chunk_t100_chunk-text_multichunk-per-doc.json.gz` |

## Relation to ingestion chunk autodetect

Production ingestion uses **`chunk_autodetect(path, text)`** for vault files. These benchmarks still seed corpora with **`chunk_text()`** where noted above, so metrics here are **not** a direct measure of autodetect strategies on these suites unless the harness is updated to match ingest chunking.

## Reproduce command (full slow suite)

```bash
cd src-tauri
cargo test --features "test-utils,slow-tests" -- --nocapture
```

SciFact is faster and quieter if limited to:

```bash
cargo test --features "test-utils,slow-tests" scifact_recall -- --nocapture --test-threads=1
```
