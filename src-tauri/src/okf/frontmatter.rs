use crate::okf::types::{OkfFrontmatter, OkfFrontmatterValue};
use std::collections::HashSet;

const RESERVED_LITERALS: &[&str] = &["true", "false", "yes", "no", "on", "off", "null", "~", ""];

fn looks_like_number(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    let mut i = 0;
    if bytes[i] == b'+' || bytes[i] == b'-' {
        i += 1;
    }
    let start = i;
    let mut saw_digit = false;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        saw_digit = true;
        i += 1;
    }
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            saw_digit = true;
            i += 1;
        }
    }
    if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
        i += 1;
        if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
            i += 1;
        }
        let exp_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == exp_start {
            return false;
        }
        return saw_digit && i == bytes.len();
    }
    saw_digit && i == bytes.len() && i > start
}

fn is_iso8601_timestamp(value: &str) -> bool {
    // Subset matching the TS reference check.
    let bytes = value.as_bytes();
    if bytes.len() < 20 {
        return false;
    }
    let dash_positions = [4, 7];
    let t_pos = 10;
    let colon_positions = [13, 16];
    for &p in &dash_positions {
        if bytes.get(p) != Some(&b'-') {
            return false;
        }
    }
    if bytes.get(t_pos) != Some(&b'T') {
        return false;
    }
    for &p in &colon_positions {
        if bytes.get(p) != Some(&b':') {
            return false;
        }
    }
    true
}

fn needs_quoting(value: &str) -> bool {
    if value != value.trim() {
        return true;
    }
    if value.chars().any(|c| matches!(c, '\n' | '\r' | '\t')) {
        return true;
    }
    if is_iso8601_timestamp(value) {
        return false;
    }
    if let Some(first) = value.chars().next() {
        if matches!(first, '-' | '?' | ':') && value.chars().nth(1) == Some(' ') {
            return true;
        }
        if matches!(
            first,
            '[' | ']' | '{' | '}' | '&' | ',' | '*' | '%' | '!' | '|' | '>' | '\'' | '"' | '@' | '`'
        ) {
            return true;
        }
    }
    if value.contains(':') || value.contains('#') {
        return true;
    }
    if RESERVED_LITERALS
        .iter()
        .any(|lit| lit.eq_ignore_ascii_case(value))
    {
        return true;
    }
    if looks_like_number(value) {
        return true;
    }
    false
}

fn quote_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            other => escaped.push(other),
        }
    }
    escaped.push('"');
    escaped
}

pub fn serialize_scalar_string(value: &str) -> String {
    if needs_quoting(value) {
        quote_string(value)
    } else {
        value.to_string()
    }
}

fn serialize_key(key: &str) -> String {
    if key.chars().any(char::is_whitespace) || needs_quoting(key) || is_iso8601_timestamp(key) {
        quote_string(key)
    } else {
        key.to_string()
    }
}

fn serialize_value(value: &OkfFrontmatterValue) -> String {
    match value {
        OkfFrontmatterValue::Null => "null".to_string(),
        OkfFrontmatterValue::Bool(b) => b.to_string(),
        OkfFrontmatterValue::Number(n) => n.to_string(),
        OkfFrontmatterValue::String(s) => serialize_scalar_string(s),
        OkfFrontmatterValue::StringList(_) => {
            panic!("serialize_value called on StringList")
        }
    }
}

pub fn serialize_frontmatter(fm: &OkfFrontmatter) -> String {
    let mut lines = vec!["---".to_string()];
    let mut keys: Vec<_> = fm.fields.keys().collect();
    keys.sort();
    for key in keys {
        let value = &fm.fields[key];
        match value {
            OkfFrontmatterValue::StringList(items) if items.is_empty() => {
                lines.push(format!("{}: []", serialize_key(key)));
            }
            OkfFrontmatterValue::StringList(items) => {
                lines.push(format!("{}:", serialize_key(key)));
                for item in items {
                    lines.push(format!("  - {}", serialize_scalar_string(item)));
                }
            }
            other => {
                lines.push(format!("{}: {}", serialize_key(key), serialize_value(other)));
            }
        }
    }
    lines.push("---".to_string());
    format!("{}\n", lines.join("\n"))
}

fn unescape_frontmatter_string(escaped: &str) -> String {
    let mut result = String::with_capacity(escaped.len());
    let chars: Vec<char> = escaped.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' && i + 1 < chars.len() {
            match chars[i + 1] {
                'n' => {
                    result.push('\n');
                    i += 2;
                    continue;
                }
                'r' => {
                    result.push('\r');
                    i += 2;
                    continue;
                }
                't' => {
                    result.push('\t');
                    i += 2;
                    continue;
                }
                '"' => {
                    result.push('"');
                    i += 2;
                    continue;
                }
                '\\' => {
                    result.push('\\');
                    i += 2;
                    continue;
                }
                _ => {}
            }
        }
        result.push(chars[i]);
        i += 1;
    }
    result
}

fn unescape_single_quoted_string(escaped: &str) -> String {
    escaped.replace("''", "'")
}

fn parse_quoted_scalar(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.len() < 2 {
        return None;
    }
    if trimmed.starts_with('"') && trimmed.ends_with('"') {
        return Some(unescape_frontmatter_string(&trimmed[1..trimmed.len() - 1]));
    }
    if trimmed.starts_with('\'') && trimmed.ends_with('\'') {
        return Some(unescape_single_quoted_string(&trimmed[1..trimmed.len() - 1]));
    }
    None
}

fn parse_key(raw: &str) -> String {
    parse_quoted_scalar(raw).unwrap_or_else(|| raw.trim().to_string())
}

fn parse_scalar_value(raw: &str) -> OkfFrontmatterValue {
    if let Some(quoted) = parse_quoted_scalar(raw) {
        return OkfFrontmatterValue::String(quoted);
    }
    let trimmed = raw.trim();
    if trimmed == "null" {
        return OkfFrontmatterValue::Null;
    }
    if trimmed == "true" {
        return OkfFrontmatterValue::Bool(true);
    }
    if trimmed == "false" {
        return OkfFrontmatterValue::Bool(false);
    }
    if looks_like_number(trimmed) {
        if let Ok(n) = trimmed.parse::<f64>() {
            return OkfFrontmatterValue::Number(n);
        }
    }
    OkfFrontmatterValue::String(trimmed.to_string())
}

fn extract_key(line: &str) -> Option<(String, usize)> {
    let bytes = line.as_bytes();
    let mut i = 0;
    if i < bytes.len() && bytes[i] == b'"' {
        i += 1;
        while i < bytes.len() {
            if bytes[i] == b'\\' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            if bytes[i] == b'"' {
                let key = line[1..i].to_string();
                return Some((key, i + 1));
            }
            i += 1;
        }
        return None;
    }
    if i < bytes.len() && bytes[i] == b'\'' {
        i += 1;
        while i < bytes.len() {
            if bytes[i] == b'\'' {
                if i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                    i += 2;
                    continue;
                }
                let key = line[1..i].to_string();
                return Some((key, i + 1));
            }
            i += 1;
        }
        return None;
    }
    let colon = line.find(':')?;
    Some((line[..colon].to_string(), colon))
}

fn match_frontmatter_key_value(line: &str) -> Option<(String, String, bool)> {
    let (key_raw, colon_end) = extract_key(line)?;
    if line.as_bytes().get(colon_end) != Some(&b':') {
        return None;
    }
    let tail = &line[colon_end + 1..];
    let has_value = !tail.trim().is_empty();
    Some((key_raw, tail.trim_start().to_string(), has_value))
}

/// Parses the YAML frontmatter subset emitted by [`serialize_frontmatter`].
pub fn parse_frontmatter(content: &str) -> (OkfFrontmatter, String) {
    let lines: Vec<&str> = content.lines().collect();
    let mut fallback_fm = OkfFrontmatter::default();
    fallback_fm.insert_str("type", "");

    if lines.first().map(|l| l.trim()) != Some("---") {
        return (fallback_fm, content.to_string());
    }

    let closing_index = lines
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, line)| line.trim() == "---")
        .map(|(i, _)| i);

    let Some(closing_index) = closing_index else {
        return (fallback_fm, content.to_string());
    };

    let mut frontmatter = OkfFrontmatter::default();
    let mut i = 1;
    while i < closing_index {
        let line = lines[i];
        let Some((key_raw, value, has_value)) = match_frontmatter_key_value(line) else {
            i += 1;
            continue;
        };
        let key = parse_key(&key_raw);
        if value.trim() == "[]" {
            frontmatter.insert_string_list(key, vec![]);
            i += 1;
            continue;
        }
        if !has_value {
            let mut items = Vec::new();
            i += 1;
            while i < closing_index {
                let list_line = lines[i];
                let trimmed = list_line.trim_start();
                if let Some(rest) = trimmed.strip_prefix("- ") {
                    items.push(match parse_scalar_value(rest) {
                        OkfFrontmatterValue::String(s) => s,
                        OkfFrontmatterValue::Number(n) => n.to_string(),
                        OkfFrontmatterValue::Bool(b) => b.to_string(),
                        OkfFrontmatterValue::Null => "null".to_string(),
                        OkfFrontmatterValue::StringList(_) => String::new(),
                    });
                    i += 1;
                } else {
                    break;
                }
            }
            frontmatter.insert_string_list(key, items);
            continue;
        }
        frontmatter.fields.insert(key, parse_scalar_value(&value));
        i += 1;
    }

    if frontmatter.get_str("type").is_none() {
        frontmatter.insert_str("type", "");
    }

    let rest_lines = &lines[closing_index + 1..];
    let rest = if rest_lines.is_empty() {
        String::new()
    } else {
        rest_lines.join("\n")
    };

    (frontmatter, rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_scalar_fields() {
        let mut fm = OkfFrontmatter::default();
        fm.insert_str("type", "fact");
        fm.insert_str("title", "Hello: world");
        fm.insert_number("created_at", 1719835200000.0);
        fm.insert_null("deleted_at");
        fm.insert_string_list("tags", vec!["demo".into()]);
        let serialized = serialize_frontmatter(&fm);
        let (parsed, _) = parse_frontmatter(&serialized);
        assert_eq!(parsed.get_str("type"), Some("fact"));
        assert_eq!(parsed.get_str("title"), Some("Hello: world"));
        assert_eq!(parsed.get_number("created_at"), Some(1719835200000.0));
        assert!(parsed.fields.contains_key("deleted_at"));
        assert_eq!(parsed.get_string_list("tags"), Some(&["demo".as_ref()][..]));
    }
}
