//! Brace/statement-aware splitting with comment and string awareness (no Tree-sitter).

use super::{
    limits::{code_overlap_lines, overlap_chars, target_chars},
    lines_for_byte_span, split_oversized_block_spans, Chunk, ChunkStrategyTag,
};

#[derive(Clone, Copy)]
enum State {
    Code,
    LineComment,
    BlockComment,
    Str(char),
    Template,
}

/// Walk `text` once; emit byte offsets immediately after newlines where a statement/top-level break is likely.
/// Used by AST symbol splitting to avoid slicing mid-statement (spec §6).
pub(super) fn statement_boundary_offsets(text: &str) -> Vec<usize> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut state = State::Code;
    let mut brace: i32 = 0;
    let mut paren: i32 = 0;
    let mut bracket: i32 = 0;
    let mut tpl_depth: i32 = 0;

    while i < bytes.len() {
        let ch = text[i..].chars().next().unwrap();
        match state {
            State::LineComment => {
                if ch == '\n' {
                    state = State::Code;
                }
                i += ch.len_utf8();
            }
            State::BlockComment => {
                if ch == '*' && bytes.get(i + 1) == Some(&b'/') {
                    i += 2;
                    state = State::Code;
                    continue;
                }
                i += ch.len_utf8();
            }
            State::Str(q) => {
                if ch == '\\' {
                    i += ch.len_utf8();
                    if let Some(nxt) = text[i..].chars().next() {
                        i += nxt.len_utf8();
                    }
                    continue;
                }
                if ch == q {
                    state = State::Code;
                }
                i += ch.len_utf8();
            }
            State::Template => {
                if ch == '\\' {
                    i += ch.len_utf8();
                    if let Some(nxt) = text[i..].chars().next() {
                        i += nxt.len_utf8();
                    }
                    continue;
                }
                if ch == '`' && tpl_depth == 0 {
                    state = State::Code;
                    i += 1;
                    continue;
                }
                if ch == '$' && bytes.get(i + 1) == Some(&b'{') {
                    tpl_depth += 1;
                    brace += 1;
                    i += 2;
                    continue;
                }
                if ch == '}' && tpl_depth > 0 {
                    tpl_depth -= 1;
                    brace -= 1;
                    i += 1;
                    continue;
                }
                i += ch.len_utf8();
            }
            State::Code => {
                if ch == '/' && bytes.get(i + 1) == Some(&b'/') {
                    state = State::LineComment;
                    i += 2;
                    continue;
                }
                if ch == '/' && bytes.get(i + 1) == Some(&b'*') {
                    state = State::BlockComment;
                    i += 2;
                    continue;
                }
                if ch == '"' || ch == '\'' {
                    state = State::Str(ch);
                    i += ch.len_utf8();
                    continue;
                }
                if ch == '`' {
                    state = State::Template;
                    tpl_depth = 0;
                    i += ch.len_utf8();
                    continue;
                }

                match ch {
                    '{' => brace += 1,
                    '}' => brace -= 1,
                    '(' => paren += 1,
                    ')' => paren -= 1,
                    '[' => bracket += 1,
                    ']' => bracket -= 1,
                    _ => {}
                }

                if ch == '\n' {
                    let prev_line_end = i;
                    let line_start = text[..prev_line_end]
                        .rfind('\n')
                        .map(|p| p + 1)
                        .unwrap_or(0);
                    consider_boundary(
                        text,
                        line_start,
                        prev_line_end,
                        brace,
                        paren,
                        bracket,
                        &mut out,
                    );
                }

                i += ch.len_utf8();
            }
        }
    }
    out
}

fn consider_boundary(
    text: &str,
    line_start: usize,
    prev_line_end: usize,
    brace: i32,
    paren: i32,
    bracket: i32,
    out: &mut Vec<usize>,
) {
    if brace != 0 || paren != 0 || bracket != 0 {
        return;
    }
    let line = text[line_start..prev_line_end].trim_end();
    if line.is_empty() {
        return;
    }
    if line.ends_with(';')
        || line.ends_with('}')
        || line == "}"
        || line.starts_with("fn ")
        || line.starts_with("async fn ")
        || line.starts_with("pub fn ")
        || line.starts_with("impl ")
        || line.starts_with("export ")
        || line.starts_with("function ")
        || line.starts_with("class ")
    {
        out.push(prev_line_end);
    }
}

fn looks_like_signature(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("fn ")
        || t.starts_with("async fn ")
        || t.starts_with("pub fn ")
        || t.starts_with("export function")
        || t.starts_with("export default function")
        || t.starts_with("function ")
        || t.starts_with("class ")
        || t.starts_with("const ")
        || t.starts_with("type ")
        || t.starts_with("interface ")
}

fn signature_prefix(prev_chunk: &str, lines: usize) -> String {
    let mut sigs: Vec<&str> = Vec::new();
    for line in prev_chunk.lines().rev() {
        if looks_like_signature(line) {
            sigs.push(line.trim_end());
            if sigs.len() >= lines {
                break;
            }
        }
    }
    sigs.reverse();
    sigs.join("\n")
}

/// Chunk code-like sources using heuristic boundaries + small signature overlap.
#[allow(dead_code)]
pub(super) fn chunk_code_like(text: &str) -> Vec<String> {
    chunk_code_like_chunks(text)
        .into_iter()
        .map(|c| c.text)
        .collect()
}

#[derive(Clone)]
struct SegmentAcc {
    buf: String,
    lo: usize,
    hi: usize,
}

fn span_lines_merged_gap(full: &str, lo: usize, hi: usize) -> (u32, u32) {
    let hi_clip = hi.min(full.len());
    let (sl, _) = lines_for_byte_span(full, lo, (lo + 1).min(full.len()));
    let (_, el) = lines_for_byte_span(full, hi_clip.saturating_sub(1), hi_clip);
    (sl, el.max(sl))
}

pub(super) fn chunk_code_like_chunks(text: &str) -> Vec<Chunk> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return vec![];
    }

    let max_c = target_chars();
    let ov = overlap_chars();
    let sig_lines = code_overlap_lines();
    let base = trimmed.as_ptr() as usize - text.as_ptr() as usize;

    let boundaries = statement_boundary_offsets(trimmed);
    let mut cuts: Vec<usize> = vec![0];
    cuts.extend(
        boundaries
            .into_iter()
            .filter(|&p| p > 0 && p <= trimmed.len()),
    );
    if cuts.last().copied().unwrap_or(0) < trimmed.len() {
        cuts.push(trimmed.len());
    }
    cuts.sort_unstable();
    cuts.dedup();

    let mut raw: Vec<(String, usize, usize)> = Vec::new();
    for w in cuts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let slice = &trimmed[a..b];
        let piece = slice.trim();
        if piece.is_empty() {
            continue;
        }
        let off = slice.find(piece).expect("trimmed substring");
        let lo = base + a + off;
        let hi = lo + piece.len();
        raw.push((piece.to_string(), lo, hi));
    }

    if raw.is_empty() {
        return split_by_chars_chunks(text, trimmed, base, max_c, ov);
    }

    let mut merged: Vec<(String, usize, usize)> = Vec::new();
    let mut cur: Option<SegmentAcc> = None;
    for (piece, lo, hi) in raw {
        match &mut cur {
            None => {
                cur = Some(SegmentAcc { buf: piece, lo, hi });
            }
            Some(acc) => {
                if acc.buf.len() + 2 + piece.len() <= max_c {
                    acc.buf.push_str("\n\n");
                    acc.buf.push_str(&piece);
                    acc.hi = hi;
                } else {
                    let done = cur.take().unwrap();
                    merged.push((done.buf, done.lo, done.hi));
                    cur = Some(SegmentAcc { buf: piece, lo, hi });
                }
            }
        }
    }
    if let Some(done) = cur {
        merged.push((done.buf, done.lo, done.hi));
    }

    let mut out: Vec<(String, usize, usize, bool)> = Vec::new();
    for (idx, (mut chunk_s, mut lo, hi)) in merged.into_iter().enumerate() {
        let mut merged_gap = false;
        if idx > 0 {
            let (prev_t, prev_lo, _, _) = out.last().unwrap();
            let sig = signature_prefix(prev_t, sig_lines);
            let tail = if sig.is_empty() {
                tail_overlap_chars(prev_t, ov)
            } else {
                sig
            };
            let ttrim = tail.trim();
            if !ttrim.is_empty() && !chunk_s.contains(ttrim) {
                let merged_txt = format!("{tail}\n{chunk_s}");
                if merged_txt.len() <= max_c.saturating_mul(2) {
                    if let Some(i) = prev_t.rfind(ttrim) {
                        lo = prev_lo + i;
                        merged_gap = true;
                        chunk_s = merged_txt;
                    }
                }
            }
        }

        if chunk_s.len() > max_c.saturating_mul(2) {
            let pieces = split_oversized_block_spans(&chunk_s, lo.max(base), max_c, ov);
            let split_merged_gap = merged_gap && pieces.len() > 1;
            for (p, pl, ph) in pieces {
                out.push((p, pl, ph, split_merged_gap));
            }
        } else {
            out.push((chunk_s, lo, hi, merged_gap));
        }
    }

    out.into_iter()
        .map(|(piece, lo, hi, merged_gap)| {
            let hi_c = hi.min(text.len());
            let (start_line, end_line) = if merged_gap {
                span_lines_merged_gap(text, lo, hi_c)
            } else {
                lines_for_byte_span(text, lo, hi_c)
            };
            Chunk {
                text: piece.trim().to_string(),
                start_line,
                end_line,
                symbol_name: None,
                strategy: ChunkStrategyTag::Scanner,
            }
        })
        .filter(|c| !c.text.is_empty())
        .collect()
}

fn split_by_chars_chunks(
    source: &str,
    trimmed: &str,
    trimmed_base: usize,
    max_c: usize,
    ov: usize,
) -> Vec<Chunk> {
    split_oversized_block_spans(trimmed, trimmed_base, max_c, ov)
        .into_iter()
        .map(|(piece, lo, hi)| {
            let hi_c = hi.min(source.len());
            let (start_line, end_line) = lines_for_byte_span(source, lo, hi_c);
            Chunk {
                text: piece,
                start_line,
                end_line,
                symbol_name: None,
                strategy: ChunkStrategyTag::Scanner,
            }
        })
        .filter(|c| !c.text.is_empty())
        .collect()
}

fn tail_overlap_chars(s: &str, ov: usize) -> String {
    if s.len() <= ov {
        return s.to_string();
    }
    let mut start = s.len().saturating_sub(ov);
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    if let Some(pos) = s[start..].find('\n') {
        start = (start + pos + 1).min(s.len());
        while start < s.len() && !s.is_char_boundary(start) {
            start += 1;
        }
    }
    s[start..].trim_start().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_braces_multiline_fn() {
        let src = r#"fn foo() {
    if true {
        bar();
    }
}

fn baz() {
    return 1;
}
"#;
        let c = chunk_code_like(src);
        assert!(!c.is_empty(), "{c:?}");
        let joined = c.join("\n");
        assert!(
            joined.contains("fn foo") && joined.contains("fn baz"),
            "expected both functions in chunks: {joined:?}"
        );
    }

    #[test]
    fn string_with_braces_inside() {
        let src = r#"const s = "fake }; ending";
console.log(s);

"#;
        let c = chunk_code_like(src);
        let joined = c.join(" ");
        assert!(
            joined.contains(r#"fake }; ending"#),
            "must not split inside string: {joined:?}"
        );
    }

    #[test]
    fn jsx_like_angle_brackets() {
        let src = r#"function App() {
  return (
    <div className="x">
      {items.map(i => <span key={i}>{i}</span>)}
    </div>
  );
}
"#;
        let c = chunk_code_like(src);
        assert!(!c.is_empty());
        assert!(c[0].contains("function App"));
    }
}
