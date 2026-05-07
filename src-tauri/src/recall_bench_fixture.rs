//! Precomputed embeddings for YAML/code recall benchmarks (`tests/*/yaml_*`, `code_*`).
//!
//! **Artifacts:** gzip-compressed JSON (not Zip), same layout as SciFact:
//! `{ "doc_id": [[384 f32], …] }` — one row per `chunk_text()` chunk (`multichunk-per-doc`).
//! Filenames encode model, width, and how text was chunked before embedding — see module
//! constants below. **Commit these `*.json.gz` files** next to each suite’s `corpus.jsonl` so CI
//! and fresh clones reuse frozen vectors without re-running FastEmbed.

pub const YAML_SYNTHETIC_EMBEDDINGS_GZIP: &str =
    "yaml-synthetic-embeddings_all-minilm-l6-v2_dim384_sentence-chunk_t100_chunk-text_multichunk-per-doc.json.gz";
pub const YAML_K8S_CURATED_EMBEDDINGS_GZIP: &str =
    "yaml-k8s-curated-embeddings_all-minilm-l6-v2_dim384_sentence-chunk_t100_chunk-text_multichunk-per-doc.json.gz";

pub const CODE_SYNTHETIC_EMBEDDINGS_GZIP: &str =
    "code-synthetic-embeddings_all-minilm-l6-v2_dim384_sentence-chunk_t100_chunk-text_multichunk-per-doc.json.gz";
pub const CODE_CURATED_EMBEDDINGS_GZIP: &str =
    "code-curated-embeddings_all-minilm-l6-v2_dim384_sentence-chunk_t100_chunk-text_multichunk-per-doc.json.gz";
