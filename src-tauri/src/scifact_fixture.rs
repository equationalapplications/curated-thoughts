//! Corpus vectors for SciFact integration tests (`tests/scifact.rs`, `embed_scifact` bin).
///
/// Filename encodes embedding model, vector size, chunking preset (sentence boundaries,
/// `TARGET_WORDS`, neighbor padding) so checked-in artifacts stay self-describing.

pub const EMBEDDINGS_GZIP_FILENAME: &str =
    "scifact-embeddings_all-minilm-l6-v2_dim384_sentence-chunk_t100_neighbor-pad.json.gz";
