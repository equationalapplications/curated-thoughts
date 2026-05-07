//! Precomputed SciFact corpus embeddings (`cargo run --bin embed_scifact -- <preset>`).
//!
//! Filenames encode the fixed choices for reproducibility: **model**, **dimensions**,
//! and how text was chunked (or not) before embedding.

/// **Bench 1 (legacy):** one embedding per document over full `title + text` (no `chunk_text`).
pub const FULLTEXT_SINGLE_EMBEDDINGS_GZIP_FILENAME: &str =
    "scifact-embeddings_all-minilm-l6-v2_dim384_fulltext_single-embedding-per-doc.json.gz";

/// **Bench 2:** `chunk_text()` sentence groups, target 100 words, neighbor sentence padding —
/// gzip JSON maps each doc id to a **list** of 384‑dim vectors (one per stored chunk).
pub const SENTENCE_CHUNK_MULTICHUNK_EMBEDDINGS_GZIP_FILENAME: &str =
    "scifact-embeddings_all-minilm-l6-v2_dim384_sentence-chunk_t100_neighbor-pad_multichunk-per-doc.json.gz";
