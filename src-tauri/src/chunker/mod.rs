mod ast_symbol;
mod classify;
mod code_like;
mod declarative;
mod fallback;
mod limits;
mod prose;

pub use classify::{classify, path_uses_tsx, should_ingest_extension, AstLang, ChunkStrategy};

use std::path::Path;

/// 1-indexed inclusive lines for `source[start_byte..end_byte]` (byte indices must be on char boundaries).
pub fn lines_for_byte_span(source: &str, start_byte: usize, end_byte: usize) -> (u32, u32) {
    let len = source.len();
    let start_byte = start_byte.min(len);
    let mut end_byte = end_byte.min(len);
    if end_byte < start_byte {
        end_byte = start_byte;
    }
    let start_line = 1 + source[..start_byte].bytes().filter(|&b| b == b'\n').count() as u32;
    let end_line = 1 + source[..end_byte].bytes().filter(|&b| b == b'\n').count() as u32;
    (start_line, end_line.max(start_line))
}

/// Split a block into capped pieces with spans as byte offsets in the vault file `source` string.
pub(super) fn split_oversized_block_spans(
    block: &str,
    block_base_abs: usize,
    max_c: usize,
    overlap: usize,
) -> Vec<(String, usize, usize)> {
    let block = block.trim();
    if block.is_empty() {
        return vec![];
    }
    if block.len() <= max_c {
        let trimmed = block.trim();
        let off = block.find(trimmed).expect("trim");
        let lo = block_base_abs + off;
        let hi = lo + trimmed.len();
        return vec![(trimmed.to_string(), lo, hi)];
    }

    let mut out = Vec::new();
    let mut start = 0usize;
    while start < block.len() {
        let mut end = (start + max_c).min(block.len());
        if end < block.len() {
            let slice = &block[start..end];
            if let Some(rel) = slice.rfind('\n') {
                end = start + rel + 1;
            } else if let Some(rel) = slice.rfind(' ') {
                end = start + rel + 1;
            }
        }
        let raw = &block[start..end];
        let piece = raw.trim();
        if !piece.is_empty() {
            let off = raw.find(piece).expect("trim") + start;
            let lo = block_base_abs + off;
            let hi = lo + piece.len();
            out.push((piece.to_string(), lo, hi));
        }
        if end >= block.len() {
            break;
        }
        start = end.saturating_sub(overlap);
        while start < block.len() && !block.is_char_boundary(start) {
            start += 1;
        }
        if start >= end {
            start = end;
        }
    }
    out
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ChunkStrategyTag {
    AstSymbolRust,
    AstSymbolTypeScript,
    AstSymbolJavaScript,
    AstSymbolPython,
    AstSymbolGo,
    AstRef,
    Prose,
    Scanner,
    Declarative,
    Fallback,
}

impl ChunkStrategyTag {
    pub fn as_db_str(&self) -> &'static str {
        match self {
            ChunkStrategyTag::AstSymbolRust => "ast_symbol_rust",
            ChunkStrategyTag::AstSymbolTypeScript => "ast_symbol_typescript",
            ChunkStrategyTag::AstSymbolJavaScript => "ast_symbol_javascript",
            ChunkStrategyTag::AstSymbolPython => "ast_symbol_python",
            ChunkStrategyTag::AstSymbolGo => "ast_symbol_go",
            ChunkStrategyTag::AstRef => "ast_ref",
            ChunkStrategyTag::Prose => "prose",
            ChunkStrategyTag::Scanner => "scanner",
            ChunkStrategyTag::Declarative => "declarative",
            ChunkStrategyTag::Fallback => "fallback",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Chunk {
    pub text: String,
    pub start_line: u32,
    pub end_line: u32,
    pub symbol_name: Option<String>,
    pub defined_symbol: Option<String>,
    pub strategy: ChunkStrategyTag,
}

/// Legacy prose-only API (sentence-aware); retained for benchmarks and tests.
pub fn chunk_text(text: &str) -> Vec<String> {
    chunk_prose_chunks(text)
        .into_iter()
        .map(|c| c.text)
        .collect()
}

pub fn chunk_prose_chunks(text: &str) -> Vec<Chunk> {
    prose::chunk_prose_chunks(text)
}

/// Choose chunking strategy from path extension and dispatch.
pub fn chunk_autodetect(path: &Path, text: &str) -> Vec<Chunk> {
    let strategy = classify(path);
    if cfg!(debug_assertions) {
        eprintln!("[ingest-chunk] {} strategy={:?}", path.display(), strategy);
    }

    match strategy {
        ChunkStrategy::AstSymbol(lang) => {
            let use_tsx = path_uses_tsx(path);
            let chunks = ast_symbol::chunk(lang, text, use_tsx);
            if chunks.is_empty() {
                code_like::chunk_code_like_chunks(text)
            } else {
                chunks
            }
        }
        ChunkStrategy::Prose => prose::chunk_prose_chunks(text),
        ChunkStrategy::CodeLike => code_like::chunk_code_like_chunks(text),
        ChunkStrategy::Declarative => declarative::chunk_declarative_chunks(path, text),
        ChunkStrategy::Fallback => fallback::chunk_fallback_chunks(text),
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
        let a: Vec<String> = chunk_autodetect(&p, text)
            .into_iter()
            .map(|c| c.text)
            .collect();
        assert_eq!(a, chunk_text(text));
    }

    #[test]
    fn txt_matches_legacy_chunk_text() {
        let p = PathBuf::from("/v/readme.txt");
        let text = "One two. Three four.";
        let a: Vec<String> = chunk_autodetect(&p, text)
            .into_iter()
            .map(|c| c.text)
            .collect();
        assert_eq!(a, chunk_text(text));
    }

    #[test]
    fn ast_symbol_tags_serialize() {
        assert_eq!(
            ChunkStrategyTag::AstSymbolRust.as_db_str(),
            "ast_symbol_rust"
        );
        assert_eq!(
            ChunkStrategyTag::AstSymbolTypeScript.as_db_str(),
            "ast_symbol_typescript"
        );
        assert_eq!(
            ChunkStrategyTag::AstSymbolJavaScript.as_db_str(),
            "ast_symbol_javascript"
        );
        assert_eq!(
            ChunkStrategyTag::AstSymbolPython.as_db_str(),
            "ast_symbol_python"
        );
        assert_eq!(ChunkStrategyTag::AstSymbolGo.as_db_str(), "ast_symbol_go");
    }
}
