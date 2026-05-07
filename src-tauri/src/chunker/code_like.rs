//! Brace/statement-aware splitting with comment and string awareness (no Tree-sitter).

use super::limits::{code_overlap_lines, overlap_chars, target_chars};

#[derive(Clone, Copy)]
enum State {
    Code,
    LineComment,
    BlockComment,
    Str(char),
    Template,
}

/// Walk `text` once; emit byte offsets immediately after newlines where a statement/top-level break is likely.
fn statement_boundary_offsets(text: &str) -> Vec<usize> {
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
                    consider_boundary(text, line_start, prev_line_end, brace, paren, bracket, &mut out);
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
pub(super) fn chunk_code_like(text: &str) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() {
        return vec![];
    }

    let max_c = target_chars();
    let ov = overlap_chars();
    let sig_lines = code_overlap_lines();

    let boundaries = statement_boundary_offsets(text);
    let mut cuts: Vec<usize> = vec![0];
    cuts.extend(boundaries.into_iter().filter(|&p| p > 0 && p <= text.len()));
    if cuts.last().copied().unwrap_or(0) < text.len() {
        cuts.push(text.len());
    }
    cuts.sort_unstable();
    cuts.dedup();

    let mut raw: Vec<String> = Vec::new();
    for w in cuts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let piece = text[a..b].trim();
        if !piece.is_empty() {
            raw.push(piece.to_string());
        }
    }

    if raw.is_empty() {
        return split_by_chars(text, max_c, ov);
    }

    let mut merged: Vec<String> = Vec::new();
    let mut buf = String::new();
    for piece in raw {
        if buf.is_empty() {
            buf = piece;
            continue;
        }
        if buf.len() + 2 + piece.len() <= max_c {
            buf.push_str("\n\n");
            buf.push_str(&piece);
        } else {
            merged.push(buf);
            buf = piece;
        }
    }
    if !buf.is_empty() {
        merged.push(buf);
    }

    let mut out: Vec<String> = Vec::new();
    for (idx, chunk) in merged.into_iter().enumerate() {
        let mut s = chunk;
        if idx > 0 {
            let prev = out.last().unwrap();
            let sig = signature_prefix(prev, sig_lines);
            let tail = if sig.is_empty() {
                tail_overlap_chars(prev, ov)
            } else {
                sig
            };
            if !tail.is_empty() && !s.contains(tail.trim()) {
                s = format!("{tail}\n{s}");
            }
        }
        if s.len() > max_c.saturating_mul(2) {
            out.extend(split_by_chars(&s, max_c, ov));
        } else {
            out.push(s);
        }
    }

    out.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
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

fn split_by_chars(block: &str, max_c: usize, overlap: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut start = 0usize;
    while start < block.len() {
        let mut end = (start + max_c).min(block.len());
        if end < block.len() {
            let slice = &block[start..end];
            if let Some(rel) = slice.rfind('\n') {
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
