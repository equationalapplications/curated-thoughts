//! Conservative chunking: paragraph-like splits, max size, overlap — no prose sentence heuristics.

use super::limits::{overlap_chars, target_chars};

/// Blank-line splits (`\n\n`), merged up to the shared target size; oversized paragraphs split on whitespace/newlines with overlap.
pub(super) fn chunk_fallback(text: &str) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() {
        return vec![];
    }

    let max_c = target_chars();
    let ov = overlap_chars();

    let paragraphs: Vec<&str> = text.split("\n\n").map(|s| s.trim()).filter(|s| !s.is_empty()).collect();

    if paragraphs.is_empty() {
        return split_oversized_block(text, max_c, ov);
    }

    let mut chunks: Vec<String> = Vec::new();
    let mut buf = String::new();

    for p in paragraphs {
        if buf.is_empty() {
            buf.push_str(p);
            continue;
        }
        if buf.len() + 2 + p.len() <= max_c {
            buf.push_str("\n\n");
            buf.push_str(p);
        } else {
            append_with_overlap(&mut chunks, split_oversized_block(&buf, max_c, ov), max_c, ov);
            buf.clear();
            buf.push_str(p);
        }
    }
    append_with_overlap(&mut chunks, split_oversized_block(&buf, max_c, ov), max_c, ov);

    chunks.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
}

fn append_with_overlap(chunks: &mut Vec<String>, pieces: Vec<String>, max_c: usize, ov: usize) {
    for mut piece in pieces {
        if piece.is_empty() {
            continue;
        }
        if let Some(last) = chunks.last() {
            let prefix = tail_overlap(last, ov);
            if !prefix.is_empty() && !piece.starts_with(prefix.trim()) {
                let merged = format!("{}\n{}", prefix.trim_end(), piece);
                if merged.len() <= max_c.saturating_mul(2) {
                    piece = merged;
                }
            }
        }
        chunks.push(piece);
    }
}

fn tail_overlap(s: &str, ov: usize) -> String {
    if s.len() <= ov {
        return s.to_string();
    }
    let mut start = s.len().saturating_sub(ov);
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    // Prefer starting after a newline inside the tail window
    if let Some(rel) = s[start..].find('\n') {
        start = (start + rel + 1).min(s.len());
        while start < s.len() && !s.is_char_boundary(start) {
            start += 1;
        }
    }
    s[start..].trim_start().to_string()
}

fn split_oversized_block(block: &str, max_c: usize, overlap: usize) -> Vec<String> {
    let block = block.trim();
    if block.is_empty() {
        return vec![];
    }
    if block.len() <= max_c {
        return vec![block.to_string()];
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
        let piece = block[start..end].trim();
        if !piece.is_empty() {
            out.push(piece.to_string());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_whitespace() {
        assert!(chunk_fallback("").is_empty());
        assert!(chunk_fallback("  \n\t  ").is_empty());
    }

    #[test]
    fn paragraph_splits() {
        let t = "Line one.\n\nLine two.\nStill two.\n\nThird block.";
        let c = chunk_fallback(t);
        assert_eq!(c.len(), 1);
        assert!(c[0].contains("Line one"));
        assert!(c[0].contains("Third block"));
    }

    #[test]
    fn lowercase_after_period_ok() {
        let t = "alpha. beta. gamma.\n\ndelta.";
        let c = chunk_fallback(t);
        assert_eq!(c.len(), 1);
        assert!(c[0].contains("alpha."));
        assert!(c[0].contains("delta."));
    }

    #[test]
    fn mixed_unstructured_text() {
        let t = "abc def\nghi\tjkl\n\n0123456789 !@#\nfoo";
        let c = chunk_fallback(t);
        assert_eq!(c.len(), 1);
        assert!(c[0].contains("abc") && c[0].contains("foo"));
    }

    #[test]
    fn large_body_splits() {
        let mut s = String::new();
        for i in 0..400 {
            use std::fmt::Write;
            write!(&mut s, "word{i} ").unwrap();
        }
        let c = chunk_fallback(&s);
        assert!(c.len() >= 2);
    }
}
