//! Conservative chunking: paragraph-like splits, max size, overlap — no prose sentence heuristics.

use super::limits::{overlap_chars, target_chars};
use super::{lines_for_byte_span, Chunk, ChunkStrategyTag};

#[derive(Clone, Copy)]
struct BufSpan {
    start: usize,
    end: usize,
}

/// Blank-line splits (`\n\n`), merged up to the shared target size; oversized paragraphs split on whitespace/newlines with overlap.
#[allow(dead_code)]
pub(super) fn chunk_fallback(text: &str) -> Vec<String> {
    chunk_fallback_chunks(text)
        .into_iter()
        .map(|c| c.text)
        .collect()
}

fn paragraph_spans(body: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut i = 0usize;
    while i < body.len() {
        let next = body[i..]
            .find("\n\n")
            .map(|d| i + d)
            .unwrap_or(body.len());
        let raw = &body[i..next];
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            let off = raw.find(trimmed).expect("trimmed in raw") + i;
            spans.push((off, off + trimmed.len()));
        }
        if next >= body.len() {
            break;
        }
        i = next + 2;
    }
    spans
}

/// Returns trimmed piece text and `[lo, hi)` byte offsets relative to **`full_source`** (`text` argument to public entrypoints).
fn split_oversized_spans(
    block: &str,
    block_base_abs: usize,
    max_c: usize,
    overlap: usize,
) -> Vec<(String, usize, usize)> {
    super::split_oversized_block_spans(block, block_base_abs, max_c, overlap)
}

fn tail_overlap(s: &str, ov: usize) -> String {
    if s.len() <= ov {
        return s.to_string();
    }
    let mut start = s.len().saturating_sub(ov);
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    if let Some(rel) = s[start..].find('\n') {
        start = (start + rel + 1).min(s.len());
        while start < s.len() && !s.is_char_boundary(start) {
            start += 1;
        }
    }
    s[start..].trim_start().to_string()
}

fn append_with_overlap_chunks(
    out: &mut Vec<(String, usize, usize, bool)>,
    pieces: Vec<(String, usize, usize)>,
    max_c: usize,
    ov: usize,
) {
    for (mut piece, mut lo, hi) in pieces {
        if piece.is_empty() {
            continue;
        }
        let mut merged_gap = false;
        if let Some((last_t, last_lo, _last_hi, _)) = out.last() {
            let prefix = tail_overlap(last_t, ov);
            let ptrim = prefix.trim();
            if !ptrim.is_empty() && !piece.starts_with(ptrim) {
                let merged = format!("{}\n{}", prefix.trim_end(), piece);
                if merged.len() <= max_c.saturating_mul(2) {
                    if let Some(idx) = last_t.rfind(ptrim) {
                        lo = last_lo + idx;
                    }
                    piece = merged;
                    merged_gap = true;
                }
            }
        }
        out.push((piece, lo, hi, merged_gap));
    }
}

fn drafts_to_chunks(text: &str, drafts: Vec<(String, usize, usize, bool)>) -> Vec<Chunk> {
    drafts
        .into_iter()
        .filter(|d| !d.0.is_empty())
        .map(|(piece, lo, hi, merged_gap)| {
            let (start_line, end_line) = if merged_gap {
                let hi_clip = hi.min(text.len());
                let (sl, _) = lines_for_byte_span(text, lo, (lo + 1).min(text.len()));
                let (_, el) =
                    lines_for_byte_span(text, hi_clip.saturating_sub(1), hi_clip);
                (sl, el.max(sl))
            } else {
                lines_for_byte_span(text, lo, hi.min(text.len()))
            };
            Chunk {
                text: piece,
                start_line,
                end_line,
                symbol_name: None,
                strategy: ChunkStrategyTag::Fallback,
            }
        })
        .collect()
}

pub(super) fn chunk_fallback_chunks(text: &str) -> Vec<Chunk> {
    let body = text.trim();
    if body.is_empty() {
        return vec![];
    }

    let max_c = target_chars();
    let ov = overlap_chars();
    let base = body.as_ptr() as usize - text.as_ptr() as usize;

    let para_spans = paragraph_spans(body);
    if para_spans.is_empty() {
        let pieces = split_oversized_spans(body, base, max_c, ov);
        let mut acc = Vec::new();
        append_with_overlap_chunks(&mut acc, pieces, max_c, ov);
        return drafts_to_chunks(text, acc);
    }

    let mut acc: Vec<(String, usize, usize, bool)> = Vec::new();
    let mut buf_span: Option<BufSpan> = None;
    let mut buf_text = String::new();

    macro_rules! flush_buf {
        () => {
            if !buf_text.is_empty() {
                if let Some(span) = buf_span.take() {
                    let pieces =
                        split_oversized_spans(&buf_text, span.start, max_c, ov);
                    append_with_overlap_chunks(&mut acc, pieces, max_c, ov);
                }
                buf_text.clear();
            }
        };
    }

    for pspan in para_spans {
        let p = &body[pspan.0..pspan.1];
        if buf_span.is_none() {
            buf_text.push_str(p);
            buf_span = Some(BufSpan {
                start: base + pspan.0,
                end: base + pspan.1,
            });
            continue;
        }
        let b = buf_span.as_mut().unwrap();
        if buf_text.len() + 2 + p.len() <= max_c {
            buf_text.push_str("\n\n");
            buf_text.push_str(p);
            b.end = base + pspan.1;
        } else {
            flush_buf!();
            buf_text.push_str(p);
            buf_span = Some(BufSpan {
                start: base + pspan.0,
                end: base + pspan.1,
            });
        }
    }
    flush_buf!();

    drafts_to_chunks(text, acc)
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
