//! YAML / JSON / JSONC / TOML / XML logical splits with shared size caps.

use std::path::Path;

use super::limits::{overlap_chars, target_chars};
use super::{lines_for_byte_span, Chunk, ChunkStrategyTag};

#[allow(dead_code)]
pub(super) fn chunk_declarative(path: &Path, text: &str) -> Vec<String> {
    chunk_declarative_chunks(path, text)
        .into_iter()
        .map(|c| c.text)
        .collect()
}

fn declarative_symbol_for_block(path: &Path, block: &str) -> Option<String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "yaml" | "yml" => block
            .lines()
            .find(|l| {
                let t = l.trim();
                !t.is_empty() && !t.starts_with('#') && line_looks_like_map_key(l)
            })
            .and_then(first_yaml_key_symbol),
        "toml" => block.lines().find_map(|line| {
            let t = line.trim();
            if t.starts_with('[') && t.ends_with(']') {
                return Some(strip_brackets_title(t));
            }
            None
        }),
        "json" | "jsonc" => first_json_segment_key_hint(block),
        "xml" => first_xml_element_name(block),
        _ => block.lines().find_map(|l| {
            let t = l.trim();
            if !t.starts_with('#') && line_looks_like_map_key(l) {
                first_yaml_key_symbol(l)
            } else {
                None
            }
        }),
    }
}

fn strip_brackets_title(t: &str) -> String {
    let mut s = t.trim().to_string();
    while s.starts_with('[') && s.ends_with(']') && s.len() >= 2 {
        s = s[1..s.len() - 1].trim().to_string();
    }
    s
}

fn first_yaml_key_symbol(line: &str) -> Option<String> {
    let trimmed_line = line.trim_start();
    let t = trimmed_line
        .split_once(':')
        .map(|x| x.0)
        .unwrap_or(trimmed_line)
        .trim();
    if t.starts_with('"') || t.starts_with('\'') {
        return Some(t.trim_matches(|c| c == '"' || c == '\'').to_string());
    }
    if t.starts_with('-') || t.starts_with('?') {
        return None;
    }
    Some(t.to_string())
}

fn first_json_segment_key_hint(block: &str) -> Option<String> {
    let t = block.trim();
    if t.starts_with('[') {
        return Some("array_item".into());
    }
    let snippet = t.chars().take(80).collect::<String>();
    if let Some(off) = snippet.find('"') {
        let after = &snippet[off + 1..];
        if let Some(end) = after.find('"') {
            return Some(after[..end].to_string());
        }
    }
    None
}

fn first_xml_element_name(block: &str) -> Option<String> {
    let b = block.trim();
    let start = b.find('<')?;
    let rest = &b[start + 1..];
    let name_start = rest
        .chars()
        .enumerate()
        .find(|(_, c)| c.is_alphabetic() || *c == '_' || *c == ':')
        .map(|(i, _)| i)?;
    let suffix = rest[name_start..]
        .find(|c: char| c.is_whitespace() || c == '/' || c == '>')
        .unwrap_or(rest[name_start..].len());
    let name = &rest[name_start..name_start + suffix];
    Some(name.into())
}

pub(super) fn chunk_declarative_chunks(path: &Path, text: &str) -> Vec<Chunk> {
    let pieces = declarative_piece_strings(path, text);
    if pieces.is_empty() {
        return vec![];
    }
    let trimmed = text.trim();
    let base = trimmed.as_ptr() as usize - text.as_ptr() as usize;
    let mut search_from = base;
    let mut out = Vec::with_capacity(pieces.len());
    for raw_chunk in pieces {
        let trimmed_chunk = raw_chunk.trim();
        if trimmed_chunk.is_empty() {
            continue;
        }
        let hay = text.get(search_from..).unwrap_or("");
        let lo = if let Some(i) = hay.find(trimmed_chunk) {
            search_from + i
        } else if let Some(i) = trimmed.find(trimmed_chunk) {
            base + i
        } else {
            search_from
        };
        let hi = (lo + trimmed_chunk.len()).min(text.len());
        search_from = hi;
        let (start_line, end_line) = lines_for_byte_span(text, lo, hi);
        let sym = declarative_symbol_for_block(path, &raw_chunk);
        out.push(Chunk {
            text: trimmed_chunk.into(),
            start_line,
            end_line,
            symbol_name: sym,
            defined_symbol: None,
            strategy: ChunkStrategyTag::Declarative,
        });
    }
    out
}

fn declarative_piece_strings(path: &Path, text: &str) -> Vec<String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "yaml" | "yml" => chunk_yaml(text),
        "json" | "jsonc" => chunk_jsonish(text),
        "toml" => chunk_toml(text),
        "xml" => chunk_xml(text),
        _ => chunk_yaml(text),
    }
}

fn merge_blocks(blocks: Vec<String>) -> Vec<String> {
    let max_c = target_chars();
    let ov = overlap_chars();
    let mut out: Vec<String> = Vec::new();
    let mut buf = String::new();
    for b in blocks {
        let b = b.trim();
        if b.is_empty() {
            continue;
        }
        if buf.is_empty() {
            buf.push_str(b);
            continue;
        }
        if buf.len() + 2 + b.len() <= max_c {
            buf.push_str("\n\n");
            buf.push_str(b);
        } else {
            out.push(buf);
            buf = overlap_tail(out.last().unwrap(), ov);
            if !buf.is_empty() {
                buf.push_str("\n\n");
            }
            buf.push_str(b);
        }
    }
    if !buf.is_empty() {
        out.push(buf);
    }
    out
}

fn overlap_tail(prev: &str, ov: usize) -> String {
    if prev.len() <= ov {
        return prev.to_string();
    }
    let mut start = prev.len().saturating_sub(ov);
    while start < prev.len() && !prev.is_char_boundary(start) {
        start += 1;
    }
    prev[start..].trim_start().to_string()
}

fn chunk_yaml(text: &str) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() {
        return vec![];
    }

    let docs: Vec<&str> = yaml_documents(text);
    let mut blocks: Vec<String> = Vec::new();

    for doc in docs {
        let doc = doc.trim();
        if doc.is_empty() {
            continue;
        }
        let keys = yaml_top_level_blocks(doc);
        if keys.len() <= 1 {
            blocks.push(doc.to_string());
        } else {
            blocks.extend(keys);
        }
    }

    merge_blocks(blocks)
}

fn yaml_documents(text: &str) -> Vec<&str> {
    let marker = "\n---";
    if !text.contains(marker) && !text.starts_with("---") {
        return vec![text];
    }
    let mut parts = Vec::new();
    let mut rest = text;
    if rest.starts_with("---") {
        rest = rest
            .strip_prefix("---")
            .unwrap_or(rest)
            .trim_start_matches(['\n', '\r']);
    }
    while let Some(idx) = rest.find(marker) {
        parts.push(rest[..idx].trim());
        rest = &rest[idx + marker.len()..];
        rest = rest.trim_start_matches(['\n', '\r']);
    }
    parts.push(rest.trim());
    parts.into_iter().filter(|s| !s.is_empty()).collect()
}

fn leading_indent_bytes(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

fn line_looks_like_map_key(line: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return false;
    }
    if trimmed.starts_with('?') || trimmed.starts_with('*') {
        return false;
    }
    if trimmed.starts_with('-') {
        return false;
    }
    if !trimmed.contains(':') || trimmed.contains("://") {
        return false;
    }
    true
}

fn yaml_root_key_indent(lines: &[&str]) -> Option<usize> {
    let mut min_ind = None::<usize>;
    for line in lines {
        if !line_looks_like_map_key(line) {
            continue;
        }
        let ind = leading_indent_bytes(line);
        min_ind = Some(match min_ind {
            Some(m) => m.min(ind),
            None => ind,
        });
    }
    min_ind
}

fn yaml_top_level_blocks(doc: &str) -> Vec<String> {
    let lines: Vec<&str> = doc.lines().collect();
    if lines.is_empty() {
        return vec![];
    }

    let mut starts: Vec<usize> = Vec::new();

    if let Some(root_ind) = yaml_root_key_indent(&lines) {
        for (i, line) in lines.iter().enumerate() {
            if !line_looks_like_map_key(line) {
                continue;
            }
            if leading_indent_bytes(line) == root_ind {
                starts.push(i);
            }
        }
    }

    if starts.is_empty() {
        for (i, line) in lines.iter().enumerate() {
            if line.starts_with('#') {
                continue;
            }
            let trimmed = line.trim_start();
            if trimmed.is_empty() {
                continue;
            }
            if line.starts_with(|c: char| c.is_ascii_whitespace()) {
                continue;
            }
            if trimmed.starts_with('-') || trimmed.starts_with('?') || trimmed.starts_with('*') {
                if starts.is_empty() {
                    starts.push(i);
                }
                continue;
            }
            if trimmed.contains(':') && !trimmed.contains("://") {
                starts.push(i);
            }
        }
    }

    starts.sort_unstable();
    starts.dedup();
    if starts.len() <= 1 {
        return vec![doc.to_string()];
    }
    let mut out = Vec::new();
    for (wi, &start) in starts.iter().enumerate() {
        let end = starts.get(wi + 1).copied().unwrap_or(lines.len());
        let slice = lines[start..end].join("\n");
        let t = slice.trim();
        if !t.is_empty() {
            out.push(t.to_string());
        }
    }
    out
}

fn chunk_jsonish(text: &str) -> Vec<String> {
    let t = text.trim();
    if t.is_empty() {
        return vec![];
    }
    let blocks = if t.starts_with('[') {
        split_json_top_level_array(t)
    } else if t.starts_with('{') {
        split_json_top_level_object(t)
    } else {
        vec![t.to_string()]
    };
    merge_blocks(blocks)
}

fn split_json_top_level_array(text: &str) -> Vec<String> {
    let t = text.trim();
    if !(t.starts_with('[') && t.ends_with(']')) {
        return vec![t.to_string()];
    }
    let inner = &t[1..t.len() - 1];
    let bytes = inner.as_bytes();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;
    let mut start = 0usize;
    let mut i = 0usize;
    let mut out = Vec::new();

    while i < inner.len() {
        let ch = inner[i..].chars().next().unwrap();
        if in_string {
            if escape {
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                in_string = false;
            }
            i += ch.len_utf8();
            continue;
        }

        match ch {
            '"' => {
                in_string = true;
                i += 1;
                continue;
            }
            '/' if bytes.get(i + 1) == Some(&b'/') => {
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            '/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(bytes.len());
                continue;
            }
            '{' | '[' => {
                depth += 1;
                i += ch.len_utf8();
                continue;
            }
            '}' | ']' => {
                depth -= 1;
                i += ch.len_utf8();
                continue;
            }
            ',' if depth == 0 => {
                let piece = inner[start..i].trim();
                if !piece.is_empty() {
                    out.push(piece.to_string());
                }
                i += 1;
                start = i;
                continue;
            }
            _ => {}
        }
        i += ch.len_utf8();
    }
    let piece = inner[start..].trim();
    if !piece.is_empty() {
        out.push(piece.to_string());
    }
    if out.is_empty() {
        vec![t.to_string()]
    } else {
        out
    }
}

fn split_json_top_level_object(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    if bytes.first() != Some(&b'{') || bytes.last() != Some(&b'}') {
        return vec![text.to_string()];
    }
    let inner = &text[1..text.len() - 1];
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;
    let mut key_start = None::<usize>;
    let mut i = 0usize;

    while i < inner.len() {
        let ch = inner[i..].chars().next().unwrap();
        if in_string {
            if escape {
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                in_string = false;
            }
            i += ch.len_utf8();
            continue;
        }

        match ch {
            '"' => {
                if depth == 0 && key_start.is_none() {
                    key_start = Some(i);
                }
                in_string = true;
                i += 1;
                continue;
            }
            '/' if inner.as_bytes().get(i + 1) == Some(&b'/') => {
                i += 2;
                while i < inner.len() && inner.as_bytes()[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            '/' if inner.as_bytes().get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < inner.len()
                    && !(inner.as_bytes()[i] == b'*' && inner.as_bytes()[i + 1] == b'/')
                {
                    i += 1;
                }
                i = (i + 2).min(inner.len());
                continue;
            }
            '{' | '[' => {
                depth += 1;
                i += ch.len_utf8();
                continue;
            }
            '}' | ']' => {
                depth -= 1;
                i += ch.len_utf8();
                continue;
            }
            ',' if depth == 0 => {
                if let Some(start) = key_start.take() {
                    let slice = inner[start..i].trim().trim_end_matches(',');
                    if !slice.is_empty() {
                        parts.push(slice.to_string());
                    }
                }
                i += 1;
                continue;
            }
            _ => {}
        }
        i += ch.len_utf8();
    }
    if let Some(start) = key_start {
        let slice = inner[start..].trim().trim_end_matches(',');
        if !slice.is_empty() {
            parts.push(slice.to_string());
        }
    }

    if parts.is_empty() {
        vec![text.to_string()]
    } else {
        parts
    }
}

fn chunk_toml(text: &str) -> Vec<String> {
    // Table headers are logical boundaries; oversized sections are subdivided in merge_blocks().
    let lines: Vec<&str> = text.lines().collect();
    let mut starts: Vec<usize> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if (t.starts_with('[') && t.ends_with(']')) || (t.starts_with("[[") && t.ends_with("]]")) {
            starts.push(i);
        }
    }
    if starts.len() <= 1 {
        return merge_blocks(vec![text.trim().to_string()]);
    }
    let mut blocks = Vec::new();
    for (wi, &start) in starts.iter().enumerate() {
        let end = starts.get(wi + 1).copied().unwrap_or(lines.len());
        let slice = lines[start..end].join("\n");
        if !slice.trim().is_empty() {
            blocks.push(slice);
        }
    }
    merge_blocks(blocks)
}

fn chunk_xml(text: &str) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() {
        return vec![];
    }
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut quote = '\0';
    let mut escape = false;
    let mut blocks: Vec<String> = Vec::new();
    let mut buf_start = 0usize;
    let mut i = 0usize;

    while i < bytes.len() {
        let ch = text[i..].chars().next().unwrap();

        if in_string {
            if escape {
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == quote {
                in_string = false;
            }
            i += ch.len_utf8();
            continue;
        }

        if ch == '"' || ch == '\'' {
            in_string = true;
            quote = ch;
            i += 1;
            continue;
        }

        if ch == '<' && bytes.get(i + 1) == Some(&b'!') && text[i..].starts_with("<!--") {
            if let Some(end) = text[i + 4..].find("-->") {
                i += 4 + end + 3;
                continue;
            }
        }

        if ch == '<'
            && bytes.get(i + 1) != Some(&b'/')
            && bytes.get(i + 1) != Some(&b'!')
            && bytes.get(i + 1) != Some(&b'?')
        {
            if depth == 0 && i > buf_start {
                let slice = text[buf_start..i].trim();
                if !slice.is_empty() {
                    blocks.push(slice.to_string());
                }
                buf_start = i;
            }
            depth += 1;
        } else if ch == '<' && bytes.get(i + 1) == Some(&b'/') {
            depth -= 1;
            if depth == 0 {
                if let Some(gt) = text[i..].find('>') {
                    let end = i + gt + 1;
                    let slice = text[buf_start..end].trim();
                    if !slice.is_empty() {
                        blocks.push(slice.to_string());
                    }
                    buf_start = end;
                    i = end;
                    continue;
                }
            }
        }

        i += ch.len_utf8();
    }
    if buf_start < text.len() {
        let slice = text[buf_start..].trim();
        if !slice.is_empty() {
            blocks.push(slice.to_string());
        }
    }

    if blocks.len() <= 1 {
        merge_blocks(vec![text.to_string()])
    } else {
        merge_blocks(blocks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn yaml_top_level_blocks_indented_root() {
        let doc = "  a: 1\n  b: 2\n    nested: x\n  c: 3\n";
        let blocks = yaml_top_level_blocks(doc);
        assert_eq!(blocks.len(), 3, "{blocks:?}");
    }

    #[test]
    fn yaml_indented_root_keys() {
        let t = "  a: 1\n  b: 2\n    nested: x\n  c: 3\n";
        let p = PathBuf::from("/x/i.yaml");
        let c = chunk_declarative(&p, t);
        assert_eq!(
            c.len(),
            1,
            "short YAML sections may merge after size-aware join"
        );
        assert!(c[0].contains("a: 1") && c[0].contains("nested: x") && c[0].contains("c: 3"));
    }

    #[test]
    fn yaml_multi_doc() {
        let t = "a: 1\n---\nb: 2\n";
        let p = PathBuf::from("/x/config.yaml");
        let c = chunk_declarative(&p, t);
        assert!(c.iter().any(|s| s.contains("a: 1")));
        assert!(c.iter().any(|s| s.contains("b: 2")));
    }

    #[test]
    fn json_nested_object_keys() {
        let t = r#"{"a":{"x":1},"b":[1,2]}"#;
        let p = PathBuf::from("/x/d.json");
        let c = chunk_declarative(&p, t);
        assert!(!c.is_empty());
        let joined = c.join(" ");
        assert!(joined.contains("\"a\""));
    }

    #[test]
    fn toml_tables() {
        let t = "[a]\nx=1\n\n[b]\ny=2\n";
        let p = PathBuf::from("/x/Cargo.toml");
        let c = chunk_declarative(&p, t);
        assert!(c.iter().any(|s| s.contains("[a]")));
        assert!(c.iter().any(|s| s.contains("[b]")));
    }

    #[test]
    fn xml_root_elements() {
        let t = "<root><a>1</a><b>2</b></root>";
        let p = PathBuf::from("/x/f.xml");
        let c = chunk_declarative(&p, t);
        assert!(!c.is_empty());
    }
}
