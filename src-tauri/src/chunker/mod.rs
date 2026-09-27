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
        if !block.is_char_boundary(start) {
            // Previous piece may have ended mid-code-point if its end was
            // clamped; advance to the next boundary.
            while start < block.len() && !block.is_char_boundary(start) {
                start += 1;
            }
            continue;
        }
        let mut end = (start + max_c).min(block.len());
        // Clamp to a char boundary (multi-byte code points can straddle max_c).
        while end < block.len() && !block.is_char_boundary(end) {
            end += 1;
        }
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

#[derive(Clone, Debug, Eq, Hash, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ChunkStrategyTag {
    AstSymbolRust,
    AstSymbolTypeScript,
    AstSymbolJavaScript,
    AstSymbolPython,
    AstSymbolGo,
    AstRef,
    AstRefUse,
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
            ChunkStrategyTag::AstRefUse => "ast_ref_use",
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

    // F3 (spec 2026-09-27-vault-ingest-policy): strip the LEADING YAML
    // frontmatter fence before dispatch, so no emitted chunk ever contains
    // raw frontmatter tokens (`okf_version:`, `updated_at:`). The indexer's
    // structured metadata path parses frontmatter separately from the file
    // bytes, so this only affects chunk text. Only the `---`-delimited block
    // at byte 0 is metadata; the same fence later in the document is content
    // (an hr + setext rule) and must survive. Applies to every strategy:
    // frontmatter pollution was confirmed in EMBEDDED prose chunks, and an
    // `.md` file routed to a code/declarative strategy would leak the same
    // tokens through those emitters.
    let (body, fm_lines) = split_leading_frontmatter(text);

    let mut chunks = match strategy {
        ChunkStrategy::AstSymbol(lang) => {
            let use_tsx = path_uses_tsx(path);
            let chunks = ast_symbol::chunk(lang, body, use_tsx);
            if chunks.is_empty() {
                code_like::chunk_code_like_chunks(body)
            } else {
                chunks
            }
        }
        ChunkStrategy::Prose => prose::chunk_prose_chunks(body),
        ChunkStrategy::CodeLike => code_like::chunk_code_like_chunks(body),
        ChunkStrategy::Declarative => declarative::chunk_declarative_chunks(path, body),
        ChunkStrategy::Fallback => fallback::chunk_fallback_chunks(body),
    };
    // Emitters count lines within `body`; stored spans are SOURCE-file lines
    // (the librarian cites them as `lines N-M`), so shift past the stripped
    // fence.
    if fm_lines > 0 {
        for c in &mut chunks {
            c.start_line += fm_lines;
            c.end_line += fm_lines;
        }
    }
    chunks
}

/// Split the leading YAML frontmatter fence (`---\n…\n---`) off `text`,
/// returning the body and the number of source lines removed ahead of it.
///
/// Recognizes the fence only at byte 0 (an optional UTF-8 BOM precedes it —
/// the indexer's metadata reader accepts one, so chunking must agree).
/// The closing fence must sit on its own line; `\r\n` line endings are
/// handled. An unterminated fence is NOT metadata (it's an hr at the top of
/// the body) and is returned verbatim with a zero line count. The body is
/// always a suffix of `text`, so `lines_removed` + a body-relative line is
/// the source-file line.
pub fn split_leading_frontmatter(text: &str) -> (&str, u32) {
    let rest = text.strip_prefix('\u{feff}').unwrap_or(text);
    // `split_inclusive` keeps each line's terminator, so the running offset
    // is exact on `\r\n` files too (`lines()` drops the `\r`, and counting
    // `len() + 1` under-counts one byte per CRLF line).
    let mut lines = rest.split_inclusive('\n');
    let is_fence = |l: &str| l.trim_end_matches(['\n', '\r']) == "---";
    // `---` exactly is the OKF convention (frontmatter.rs).
    let Some(first) = lines.next().filter(|l| is_fence(l)) else {
        return (text, 0);
    };
    let mut offset = first.len();
    for line in lines {
        offset += line.len();
        if is_fence(line) {
            let body = &rest[offset..];
            let removed = &text[..text.len() - body.len()];
            let lines_removed = removed.bytes().filter(|&b| b == b'\n').count() as u32;
            return (body, lines_removed);
        }
    }
    // Unterminated fence: not frontmatter, return the input untouched.
    (text, 0)
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use std::path::PathBuf;

    // ---- F3 frontmatter strip (spec 2026-09-27-vault-ingest-policy) ----

    /// CRLF offsets are byte-exact: the body starts at `Body`, not inside
    /// the closing fence (`lines()` drops `\r`, so `len() + 1` arithmetic
    /// used to land two bytes early here and leak `--` into the body).
    #[test]
    fn crlf_frontmatter_body_is_byte_exact() {
        let text = "---\r\nokf_version: 0.1\r\n---\r\n\r\nBody\r\n";
        assert_eq!(split_leading_frontmatter(text), ("\r\nBody\r\n", 3));
        let lf = "---\nokf_version: 0.1\n---\n\nBody\n";
        assert_eq!(split_leading_frontmatter(lf), ("\nBody\n", 3));
        // BOM-prefixed fence is recognized; the BOM goes with the fence.
        let bom = "\u{feff}---\nk: v\n---\nBody\n";
        assert_eq!(split_leading_frontmatter(bom), ("Body\n", 3));
        // Fence closing at EOF without a newline.
        assert_eq!(split_leading_frontmatter("---\nk: v\n---"), ("", 2));
    }

    /// Emitted spans are SOURCE-file lines, not body-relative ones.
    #[test]
    fn chunk_lines_are_source_relative_after_strip() {
        // Frontmatter occupies lines 1-3, blank line 4, body starts line 5.
        let text = "---\ntitle: T\n---\n\nFirst body sentence here.\n";
        let chunks = chunk_autodetect(&PathBuf::from("/v/note.md"), text);
        let first = chunks.first().expect("body must chunk");
        assert_eq!(first.start_line, 5, "chunk: {first:?}");
        // No frontmatter → spans unchanged.
        let plain = chunk_autodetect(&PathBuf::from("/v/note.md"), "Only line.\n");
        assert_eq!(plain[0].start_line, 1);
    }

    fn frontmatter_fixture() -> String {
        "---\nokf_version: 0.1\nprofile: llm-wiki/1\ntitle: Test note\n\
         entity_type: fact\ncreated_at: 2026-09-27T00:00:00Z\nupdated_at: 2026-09-27T01:00:00Z\n\
         ---\n\nThis is the body after the frontmatter. It has real content.\n"
            .to_string()
    }

    /// Zero emitted chunks contain frontmatter tokens (the production
    /// pollution: 378 chunks carried `okf_version:` / `updated_at:`).
    #[test]
    fn no_chunk_contains_frontmatter_tokens() {
        let text = frontmatter_fixture();
        let p = PathBuf::from("/v/note.md");
        let chunks = chunk_autodetect(&p, &text);
        assert!(!chunks.is_empty(), "body must still produce chunks");
        for c in &chunks {
            assert!(
                !c.text.contains("okf_version:"),
                "chunk leaked okf_version: {:?}",
                c.text
            );
            assert!(
                !c.text.contains("updated_at:"),
                "chunk leaked updated_at: {:?}",
                c.text
            );
            assert!(
                !c.text.contains("profile: llm-wiki/1"),
                "chunk leaked profile: {:?}",
                c.text
            );
        }
    }

    /// No chunk contains the leading fence itself.
    #[test]
    fn no_chunk_contains_leading_fence() {
        let text = frontmatter_fixture();
        let p = PathBuf::from("/v/note.md");
        for c in chunk_autodetect(&p, &text) {
            assert!(
                !c.text.starts_with("---"),
                "chunk starts with the fence: {:?}",
                c.text
            );
        }
    }

    /// Body content survives the strip.
    #[test]
    fn body_content_still_chunked_after_strip() {
        let text = frontmatter_fixture();
        let p = PathBuf::from("/v/note.md");
        let chunks = chunk_autodetect(&p, &text);
        let joined: String = chunks
            .iter()
            .map(|c| c.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            joined.contains("body after the frontmatter"),
            "body lost: {joined:?}"
        );
    }

    /// Only a LEADING fence is stripped; horizontal rules after the first
    /// line are ordinary content.
    #[test]
    fn non_leading_fence_is_not_stripped() {
        let text = "Intro paragraph before any fence.\n\n---\n\nmiddle text\n";
        let p = PathBuf::from("/v/note.md");
        let chunks = chunk_autodetect(&p, text);
        assert!(!chunks.is_empty());
        let joined: String = chunks
            .iter()
            .map(|c| c.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            joined.contains("Intro paragraph"),
            "leading body lost: {joined:?}"
        );
    }

    /// A file with NO frontmatter chunks identically to before (regression
    /// guard: the strip must not eat ordinary content).
    #[test]
    fn plain_body_unchanged_by_strip() {
        let text = "Aa bb cc. Dd ee ff.";
        let p = PathBuf::from("/v/note.md");
        let with_strip: Vec<String> = chunk_autodetect(&p, text)
            .into_iter()
            .map(|c| c.text)
            .collect();
        assert_eq!(with_strip, chunk_text(text));
    }

    /// Legacy prose API keeps its own behavior; the strip lives at the
    /// autodetect dispatch, not inside chunk_prose_chunks. (Verified against
    /// pre-change behavior: chunk_prose_chunks on this fixture emits the
    /// fence block as its first chunk.)
    #[test]
    fn legacy_chunk_text_api_is_untouched() {
        let text = frontmatter_fixture();
        let chunks = chunk_prose_chunks(&text);
        let first = chunks.first().expect("fixture must chunk");
        assert!(
            first.text.contains("okf_version:"),
            "legacy API contract changed unexpectedly: first chunk {:?}",
            first.text
        );
    }

    /// The strip handles CRLF files (lines() normalizes \\r\\n).
    #[test]
    fn crlf_frontmatter_is_stripped() {
        let text = "---\r\nokf_version: 0.1\r\n---\r\n\r\nBody line one.\r\n";
        let p = PathBuf::from("/v/note.md");
        for c in chunk_autodetect(&p, text) {
            assert!(
                !c.text.contains("okf_version:"),
                "CRLF chunk leaked frontmatter: {:?}",
                c.text
            );
        }
    }

    /// Unterminated fence (no closing ---): must NOT eat the whole file —
    /// chunk everything as if no frontmatter existed.
    #[test]
    fn unterminated_fence_chunks_everything() {
        let text = "---\nokf_version: 0.1\nnever closed\nbody survives\n";
        let p = PathBuf::from("/v/note.md");
        let chunks = chunk_autodetect(&p, text);
        let joined: String = chunks
            .iter()
            .map(|c| c.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            joined.contains("body survives"),
            "unterminated fence swallowed the body: {joined:?}"
        );
    }

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
    fn split_oversized_block_handles_multibyte_chars() {
        // Regression: an em-dash (3 bytes) straddling the max_c boundary used
        // to panic with "byte index is not a char boundary" (found ingesting
        // clanker-ai docs). Separator-free input forces every cap to land
        // inside a multi-byte char; slices must land on char boundaries and
        // reassemble EXACTLY.
        let block = "—".repeat(300); // 900 bytes of 3-byte chars, no whitespace
        let pieces = split_oversized_block_spans(&block, 0, 100, 0);
        assert!(pieces.len() > 1);
        let joined: String = pieces.iter().map(|(t, _, _)| t.as_str()).collect();
        assert_eq!(joined, block);
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
        assert_eq!(ChunkStrategyTag::AstRef.as_db_str(), "ast_ref");
        assert_eq!(ChunkStrategyTag::AstRefUse.as_db_str(), "ast_ref_use");
    }
}
