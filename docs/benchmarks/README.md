# Benchmarks

Historical retrieval benchmark snapshots (Recall@*k*) from the `src-tauri` integration harness. Each dated Markdown file records **what ran**, **which frozen embedding archives** were used, environment notes, and **printed metrics**.

## Layout

- `YYYY-MM-DD-<topic>.md` — one snapshot per run or batch (append new files over time; do not silently rewrite old dates).

## How to reproduce

All commands assume the **Rust crate root** (`src-tauri/`), not the repo root.

```bash
cd src-tauri

# Fast suite (integration tests; no slow retrieval benches)
cargo test --features test-utils

# Full suite including SciFact + YAML/code Recall@10 benches (~minutes; SciFact ~4 min typical)
cargo test --features "test-utils,slow-tests" -- --nocapture
```

Rust hides `println!` from passing tests unless you pass **`--nocapture`** (or run a single test crate with `--nocapture`).

### Individual suites

```bash
cd src-tauri
cargo test --features "test-utils,slow-tests" scifact_recall -- --nocapture --test-threads=1
cargo test --features "test-utils,slow-tests" --test code_bench_curated -- --nocapture
cargo test --features "test-utils,slow-tests" --test code_bench_synthetic -- --nocapture
cargo test --features "test-utils,slow-tests" --test yaml_bench_k8s_curated -- --nocapture
cargo test --features "test-utils,slow-tests" --test yaml_bench_synthetic -- --nocapture
```

## Embedding artifacts

Production vault ingest / `search_vault` use **Ollama** with the vault `embed_profile` (default **local** model `nomic-embed-code`). The SciFact + YAML/code Recall harnesses remain on **FastEmbed** `Embedder` and **frozen gzip** vectors at **384-d**; do not route those benches through Ollama. Pipeline integration tests set **`CURATED_EMBED_STUB=constant8`** so `embed_batch` returns small dummy vectors without a local Ollama.

Precomputed corpus vectors live next to each fixture under `src-tauri/tests/fixtures/<suite>/`. Filenames are defined in:

- `src-tauri/src/scifact_fixture.rs`
- `src-tauri/src/recall_bench_fixture.rs`

Regeneration (when intentionally refreshing vectors) is documented in those modules and in `src-tauri/tests/README.md` where applicable.

## Snapshots

| Date       | File                                   |
|-----------|----------------------------------------|
| 2026-05-07 | [2026-05-07-recall-benchmarks.md](./2026-05-07-recall-benchmarks.md) |
| 2026-05-07 | [2026-05-07-recall-benchmarks-v2.md](./2026-05-07-recall-benchmarks-v2.md) (rerun, `bbd2b97`) |
