mod classify;
mod code_like;
mod declarative;
mod fallback;
mod limits;
mod prose;

pub use classify::{classify, should_ingest_extension, ChunkStrategy};

use std::path::Path;

/// Legacy prose-only API (sentence-aware); retained for benchmarks and tests.
pub fn chunk_text(text: &str) -> Vec<String> {
    prose::chunk_prose(text)
}

/// Choose chunking strategy from path extension and dispatch.
pub fn chunk_autodetect(path: &Path, text: &str) -> Vec<String> {
    let strategy = classify(path);
    if cfg!(debug_assertions) {
        eprintln!(
            "[ingest-chunk] {} strategy={:?}",
            path.display(),
            strategy
        );
    }

    match strategy {
        ChunkStrategy::Prose => prose::chunk_prose(text),
        ChunkStrategy::CodeLike => code_like::chunk_code_like(text),
        ChunkStrategy::Declarative => declarative::chunk_declarative(path, text),
        ChunkStrategy::Fallback => fallback::chunk_fallback(text),
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn md_matches_legacy_chunk_text() {
        let p = PathBuf::from("/v/note.md");
        let text = "Aa bb cc. Dd ee ff.";
        assert_eq!(chunk_autodetect(&p, text), chunk_text(text));
    }

    #[test]
    fn txt_matches_legacy_chunk_text() {
        let p = PathBuf::from("/v/readme.txt");
        let text = "One two. Three four.";
        assert_eq!(chunk_autodetect(&p, text), chunk_text(text));
    }
}
