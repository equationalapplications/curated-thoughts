//! Precomputed embeddings for YAML/code recall benchmarks (`tests/*/yaml_*`, `code_*`).
//!
//! gzip JSON matches SciFact layout: `{ "doc_id": [[384 f32], ...] }` (sentence-chunk path).

pub const YAML_SYNTHETIC_EMBEDDINGS_GZIP: &str =
    "yaml-synthetic-embeddings_all-minilm-l6-v2_sentence-chunk_t100.json.gz";
pub const YAML_K8S_CURATED_EMBEDDINGS_GZIP: &str =
    "yaml-k8s-curated-embeddings_all-minilm-l6-v2_sentence-chunk_t100.json.gz";

pub const CODE_SYNTHETIC_EMBEDDINGS_GZIP: &str =
    "code-synthetic-embeddings_all-minilm-l6-v2_sentence-chunk_t100.json.gz";
pub const CODE_CURATED_EMBEDDINGS_GZIP: &str =
    "code-curated-embeddings_all-minilm-l6-v2_sentence-chunk_t100.json.gz";
