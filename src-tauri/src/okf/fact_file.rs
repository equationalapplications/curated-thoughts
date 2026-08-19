//! Fact concept documents (profile §5.1).

use anyhow::{Context, Result};

use crate::okf::concept::parse_concept;
use crate::okf::frontmatter::serialize_scalar_string;
use crate::okf::related_section::{append_related_section, split_related_section};
use crate::okf::timefmt::{iso_from_ms, ms_from_iso};
use crate::okf::types::{OkfFrontmatterValue, OkfFrontmatterValue as V, OkfMarkdownLink, LLM_WIKI_PROFILE, WikiFact};

pub struct ParsedFact {
    pub fact: WikiFact,
    pub related: Vec<OkfMarkdownLink>,
}

/// `related` is `(edge_type, relative_path)` per outgoing edge; dangling
/// targets must already be filtered out by the caller (§6).
/// `profile` selects the wire shape: `"llm-wiki/1"` emits only the v0.1 field
/// set (no `status`/v0.2 keys); `"llm-wiki/2"` (and any other value) emits the
/// v0.2 field set including `status` and provenance keys.
pub fn build_fact_file(
    fact: &WikiFact,
    related: &[(String, String)],
    profile: &str,
) -> String {
    let mut pairs: Vec<(&str, V)> = vec![
        (
            "type",
            V::String(fact.okf_type.clone().unwrap_or_else(|| "fact".into())),
        ),
        ("title", V::String(fact.title.clone())),
    ];
    if !fact.tags.is_empty() {
        pairs.push(("tags", V::StringList(fact.tags.clone())));
    }
    pairs.push(("timestamp", V::String(iso_from_ms(fact.updated_at))));
    if let Some(resource) = &fact.source_ref {
        pairs.push(("resource", V::String(resource.clone())));
    }
    pairs.push(("id", V::String(fact.id.clone())));
    pairs.push(("entity_id", V::String(fact.entity_id.clone())));
    pairs.push(("confidence", V::String(fact.confidence.clone())));
    pairs.push(("source_type", V::String(fact.source_type.clone())));
    if let Some(hash) = &fact.source_hash {
        pairs.push(("source_hash", V::String(hash.clone())));
    }
    pairs.push(("created_at", V::Number(fact.created_at as f64)));
    if fact.access_count != 0 {
        pairs.push(("access_count", V::Number(fact.access_count as f64)));
    }
    if let Some(ms) = fact.last_accessed_at {
        pairs.push(("last_accessed_at", V::Number(ms as f64)));
    }
    if let Some(ms) = fact.deleted_at {
        pairs.push(("deleted_at", V::Number(ms as f64)));
    }

    // OKF v0.2 fields — emitted only when populated (omitted otherwise, per upstream §4.7).
    // Skipped entirely under profile 1 (`llm-wiki/1`) — v0.1 has no `status`
    // (lifecycle) or provenance keys.
    if profile != LLM_WIKI_PROFILE {
        pairs.push(("status", V::String(fact.lifecycle_status.clone())));
        if let Some(ms) = fact.stale_after {
            let date = crate::okf::timefmt::utc_date_from_ms(ms);
            pairs.push(("stale_after", V::String(date))); // YYYY-MM-DD
        }
        if let Some(actor) = &fact.generated_by {
            pairs.push((
                "generated",
                V::String(format!(
                    "{{ by: {}, at: {} }}",
                    serialize_actor_string(actor),
                    iso_from_ms(fact.updated_at),
                )),
            ));
        }
        if let Some(verified_json) = &fact.okf_verified {
            if !verified_json.is_empty() && verified_json != "[]" {
                // Re-shape JSON array into a flow sequence of flow mappings.
                let flow = json_array_to_flow_sequence(verified_json, "verified")
                    .unwrap_or_else(|| format!("[{verified_json}]"));
                pairs.push(("verified", V::String(flow)));
            }
        }
        if let Some(sources_json) = &fact.okf_sources {
            if !sources_json.is_empty() && sources_json != "[]" {
                let flow = json_array_to_flow_sequence(sources_json, "sources")
                    .unwrap_or_else(|| format!("[{sources_json}]"));
                pairs.push(("sources", V::String(flow)));
            }
        }
        if let Some(window) = &fact.okf_usage_window {
            pairs.push((
                "usage_window",
                V::String(flow_mapping_from_json(window, "usage_window").unwrap_or_else(|| window.clone())),
            ));
        }
    }

    let refs: Vec<(&str, &str)> = related
        .iter()
        .map(|(t, p)| (t.as_str(), p.as_str()))
        .collect();
    let body = append_related_section(&fact.body, &refs);
    let body = if body.ends_with('\n') {
        body
    } else {
        format!("{body}\n")
    };
    format!("{}\n{}", serialize_pairs_with_flow(&pairs), body)
}

pub fn parse_fact_file(content: &str) -> Result<ParsedFact> {
    let (fm, raw_body) = parse_concept(content);
    let (body, related) = split_related_section(&raw_body);
    let fact = WikiFact {
        id: fm.get_str("id").context("fact file missing id")?.to_string(),
        entity_id: fm
            .get_str("entity_id")
            .context("fact file missing entity_id")?
            .to_string(),
        title: fm.get_str("title").unwrap_or_default().to_string(),
        body: body.trim_end().to_string(),
        tags: fm
            .get_string_list("tags")
            .map(<[String]>::to_vec)
            .unwrap_or_default(),
        confidence: fm.get_str("confidence").unwrap_or("inferred").to_string(),
        source_type: fm
            .get_str("source_type")
            .unwrap_or("librarian_inferred")
            .to_string(),
        source_hash: fm.get_str("source_hash").map(str::to_string),
        source_ref: fm.get_str("resource").map(str::to_string),
        created_at: fm.get_number("created_at").map(|n| n as i64).unwrap_or(0),
        updated_at: fm.get_str("timestamp").and_then(ms_from_iso).unwrap_or(0),
        last_accessed_at: fm.get_number("last_accessed_at").map(|n| n as i64),
        access_count: fm.get_number("access_count").map(|n| n as i64).unwrap_or(0),
        deleted_at: fm.get_number("deleted_at").map(|n| n as i64),
        okf_type: fm
            .get_str("type")
            .filter(|t| !t.is_empty() && *t != "fact")
            .map(str::to_string),
        lifecycle_status: fm.get_str("status").unwrap_or("stable").to_string(),
        stale_after: parse_stale_after_ms(&fm),  // helper below; returns None for absent / unparseable
        generated_by: parse_generated_by(&fm),   // helper below; returns None when absent
        okf_sources: flow_to_json_string(fm.get_str("sources")),
        okf_verified: flow_to_json_string(fm.get_str("verified")),
        okf_usage_window: flow_to_json_string(fm.get_str("usage_window")),
        last_verified_at: latest_verified_at(&fm),
        last_verified_by: latest_verified_by(&fm),
    };
    Ok(ParsedFact { fact, related })
}

/// Convert a verbatim YAML flow-text value (`{ k: v }` or `[ { k: v }, … ]`)
/// to its JSON-encoded form. Returns None if the input is missing or
/// unparseable.
///
/// The DB columns (`okf_sources`, `okf_verified`, `okf_usage_window`) hold
/// JSON strings per the typed-model contract; storing raw YAML flow text
/// there would make the entities.rs reader fall back to empty defaults
/// (`serde_json::from_str` fails on unquoted keys like `resource: …`).
pub(crate) fn flow_to_json_string(raw: Option<&str>) -> Option<String> {
    let raw = raw?;
    let value = parse_flow_value(raw)?;
    serde_json::to_string(&value).ok()
}

/// Recursive-descent parser for single-level YAML flow syntax. Returns a
/// `serde_json::Value` that mirrors the input structure. Single-level means
/// nested flow mappings are returned as JSON strings (verbatim) so the outer
/// walker can keep going without modeling deep structure.
fn parse_flow_value(raw: &str) -> Option<serde_json::Value> {
    let trimmed = raw.trim();
    let bytes = trimmed.as_bytes();
    match bytes.first()? {
        b'{' => {
            // Require a matching closing `}`; an unterminated flow value is
            // treated as malformed and returns None instead of slicing past
            // the end (which can panic on multi-byte boundaries).
            let inner = trimmed.strip_prefix('{')?.strip_suffix('}')?.trim();
            let mut map = serde_json::Map::new();
            for (k, v) in split_flow_pairs(inner)? {
                map.insert(k, v);
            }
            Some(serde_json::Value::Object(map))
        }
        b'[' => {
            // Same guard as the `{ ... }` branch.
            let inner = trimmed.strip_prefix('[')?.strip_suffix(']')?.trim();
            let mut arr = Vec::new();
            let mappings = collect_flow_mappings(inner);
            if mappings.is_empty() {
                // Empty sequence.
                if inner.is_empty() {
                    return Some(serde_json::Value::Array(Vec::new()));
                }
                return None;
            }
            for m in mappings {
                let v = parse_flow_value(m)?;
                arr.push(v);
            }
            Some(serde_json::Value::Array(arr))
        }
        _ => None,
    }
}

/// Split a flow-mapping body (`k: v, k2: v2`) into (key, JSON value) pairs,
/// respecting `"…"` / `'…'` quoting so commas / braces inside quoted strings
/// don't terminate the pair early.
fn split_flow_pairs(body: &str) -> Option<Vec<(String, serde_json::Value)>> {
    let mut out = Vec::new();
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Skip leading separators.
        while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b',') {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        // Read key until ':'.
        let key_start = i;
        while i < bytes.len() && bytes[i] != b':' {
            i += 1;
        }
        if i >= bytes.len() {
            return None; // malformed: key with no ':'
        }
        let key = body[key_start..i].trim().to_string();
        i += 1; // skip ':'
        // Skip whitespace.
        while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
            i += 1;
        }
        if i >= bytes.len() {
            return None; // malformed: key with no value
        }
        let (val_str, next_i, was_quoted) = read_flow_value(&body[i..]);
        // Quoted scalars are literal strings — skip type coercion so a
        // quoted `"true"` round-trips as the string `true`, not Bool(true).
        // Bare scalars still go through classify_flow_scalar for the
        // boolean / null / numeric shortcut. An unterminated quote is
        // detected by `was_quoted && val_str.is_empty()`; route that to
        // classify_flow_scalar so empty → Null is preserved.
        let value = if was_quoted && !val_str.is_empty() {
            serde_json::Value::String(val_str)
        } else {
            classify_flow_scalar(val_str)
        };
        out.push((key, value));
        i += next_i;
    }
    Some(out)
}

/// Read a single flow value starting at position 0 of `s`. Returns the value
/// string, how many bytes were consumed, and whether the value was wrapped in
/// `"` or `'` quotes. Quoted values consume through the matching close-quote
/// (with escape handling); bare values consume until the next `,` at depth 0
/// or end of input. The quoted flag lets callers skip `classify_flow_scalar`
/// coercion: a quoted `"true"` is the literal string `true`, not the boolean.
fn read_flow_value(s: &str) -> (String, usize, bool) {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return (String::new(), 0, false);
    }
    match bytes[0] {
        b'"' => {
            let mut j = 1;
            let mut out = String::new();
            // Iterate on UTF-8 character boundaries so non-ASCII content
            // round-trips byte-for-byte; `bytes[j]` would otherwise read a
            // mid-codepoint byte and `as char` would corrupt it.
            while j < bytes.len() {
                let c = match s[j..].chars().next() {
                    Some(c) => c,
                    None => break, // unterminated quote / malformed UTF-8
                };
                if c == '"' {
                    break;
                }
                let char_len = c.len_utf8();
                if c == '\\' && j + char_len < bytes.len() {
                    // Unescape the sequence: `\\` -> `\`, `\"` -> `"`. Any
                    // other backslash-escape is preserved verbatim so we
                    // don't lose data the encoder didn't intentionally emit.
                    match s[j + char_len..].chars().next() {
                        Some(next_c) => {
                            match next_c {
                                '\\' => out.push('\\'),
                                '"' => out.push('"'),
                                'n' => out.push('\n'),
                                'r' => out.push('\r'),
                                't' => out.push('\t'),
                                other => {
                                    out.push('\\');
                                    out.push(other);
                                }
                            }
                            j += char_len + next_c.len_utf8();
                        }
                        None => {
                            // Trailing backslash with no escape: keep it
                            // literal and stop so we don't overrun `bytes`.
                            out.push('\\');
                            j += char_len;
                        }
                    }
                } else {
                    out.push(c);
                    j += char_len;
                }
            }
            if j < bytes.len() {
                // Properly terminated quote: the content is `out` and the
                // consumed span is `j + 1` (includes the close quote).
                (out, j + 1, true)
            } else {
                // Unterminated quote (malformed frontmatter): surface an
                // empty value so `classify_flow_scalar` records Null rather
                // than emitting a partial string into the JSON column.
                (String::new(), bytes.len(), true)
            }
        }
        b'\'' => {
            let mut j = 1;
            while j < bytes.len() && bytes[j] != b'\'' {
                j += 1;
            }
            if j < bytes.len() {
                (s[1..j].to_string(), j + 1, true)
            } else {
                // Unterminated single quote: surface as Null for the same
                // reason as the double-quote branch.
                (String::new(), bytes.len(), true)
            }
        }
        _ => {
            // Bare value: until next `,` or end. Quoted spans are opaque —
            // we just look for the unquoted comma terminator. Clamp the
            // post-quote `j` to `bytes.len()` so an unterminated quote cannot
            // push the index past the slice end (bundle files are untrusted).
            let mut j = 0;
            while j < bytes.len() {
                match bytes[j] {
                    b',' => break,
                    b'"' => {
                        j += 1;
                        while j < bytes.len() && bytes[j] != b'"' {
                            if bytes[j] == b'\\' && j + 1 < bytes.len() {
                                j += 2;
                            } else {
                                j += 1;
                            }
                        }
                        j = (j + 1).min(bytes.len());
                    }
                    b'\'' => {
                        j += 1;
                        while j < bytes.len() && bytes[j] != b'\'' {
                            j += 1;
                        }
                        j = (j + 1).min(bytes.len());
                    }
                    _ => j += 1,
                }
            }
            (s[..j].trim().to_string(), j, false)
        }
    }
}

/// Classify a bare or quoted scalar string into the appropriate JSON value.
fn classify_flow_scalar(s: String) -> serde_json::Value {
    if s.is_empty() {
        return serde_json::Value::Null;
    }
    if s == "null" {
        return serde_json::Value::Null;
    }
    if s == "true" {
        return serde_json::Value::Bool(true);
    }
    if s == "false" {
        return serde_json::Value::Bool(false);
    }
    // Integer / float detection: digits with optional sign and decimal point.
    if s.chars().all(|c| c.is_ascii_digit() || c == '-' || c == '+')
        || (s.chars().enumerate().all(|(i, c)| {
            c.is_ascii_digit() || (i == 0 && (c == '-' || c == '+')) || c == '.'
        }) && s.chars().filter(|c| *c == '.').count() <= 1
            && s.chars().any(|c| c.is_ascii_digit()))
    {
        if let Ok(n) = s.parse::<i64>() {
            return serde_json::Value::Number(n.into());
        }
        if let Ok(f) = s.parse::<f64>() {
            if let Some(n) = serde_json::Number::from_f64(f) {
                return serde_json::Value::Number(n);
            }
        }
    }
    serde_json::Value::String(s)
}

/// Parse OKF v0.2 `stale_after: YYYY-MM-DD` to epoch ms (UTC midnight).
/// Returns None for absent or unparseable input.
pub(crate) fn parse_stale_after_ms(fm: &crate::okf::types::OkfFrontmatter) -> Option<i64> {
    let s = fm.get_str("stale_after")?;
    crate::okf::timefmt::ms_from_utc_date(s)
}

/// Parse the v0.2 `generated: { by: ..., at: ... }` flow mapping to the actor string.
/// Returns None when `generated` is absent or has no `by` (per upstream spec §2.4).
pub(crate) fn parse_generated_by(fm: &crate::okf::types::OkfFrontmatter) -> Option<String> {
    let value = fm.fields.get("generated")?;
    let OkfFrontmatterValue::String(s) = value else { return None; };
    flow_mapping_field(s, "by")
}

/// Read a single named field's value from a verbatim flow-mapping text (`{ k: v }`).
/// Returns None if the key is absent. Quote-aware: respects `"…"` / `'…'` spans so
/// commas or braces inside quoted strings don't terminate the value early.
fn flow_mapping_field(flow: &str, key: &str) -> Option<String> {
    let inner = flow
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))?
        .trim();
    let needle = format!("{key}:");
    let mut chars = inner.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c.is_whitespace() || c == ',' {
            continue;
        }
        if !inner[i..].starts_with(&needle) {
            // Not our key — skip this key:value pair.
            skip_flow_value(&mut chars);
            continue;
        }
        // Skip past "key:" and any following whitespace.
        let mut after = &inner[i + needle.len()..];
        after = after.trim_start();
        // Read the value: bare until `,` or `}`, or quoted until matching quote.
        let bytes = after.as_bytes();
        if bytes.first() == Some(&b'"') || bytes.first() == Some(&b'\'') {
            let quote = bytes[0];
            let quote_char = quote as char;
            let mut j = 1;
            let mut out = String::new();
            // Iterate on UTF-8 character boundaries so non-ASCII content
            // round-trips byte-for-byte; the previous `bytes[j] as char`
            // form read mid-codepoint bytes and corrupted non-ASCII metadata.
            while j < bytes.len() {
                let c = match after[j..].chars().next() {
                    Some(c) => c,
                    None => break, // malformed UTF-8
                };
                if c == quote_char {
                    break;
                }
                let char_len = c.len_utf8();
                if c == '\\' && j + char_len < bytes.len() {
                    // Same escape semantics as `read_flow_value`: decode the
                    // standard `\n` / `\r` / `\t` controls (emitted by
                    // `encode_flow_scalar`) and preserve unknown escapes
                    // verbatim so we don't lose data the encoder didn't emit.
                    match after[j + char_len..].chars().next() {
                        Some(next_c) => {
                            match next_c {
                                '\\' => out.push('\\'),
                                q if q == quote_char => out.push(q),
                                'n' => out.push('\n'),
                                'r' => out.push('\r'),
                                't' => out.push('\t'),
                                other => {
                                    out.push('\\');
                                    out.push(other);
                                }
                            }
                            j += char_len + next_c.len_utf8();
                        }
                        None => {
                            out.push('\\');
                            j += char_len;
                        }
                    }
                } else {
                    out.push(c);
                    j += char_len;
                }
            }
            if j >= bytes.len() {
                return None; // unterminated quote
            }
            return Some(out);
        }
        let end = after
            .find(|c: char| c == ',' || c == '}')
            .unwrap_or(after.len());
        let value = after[..end].trim();
        if value.is_empty() {
            return None;
        }
        return Some(value.to_string());
    }
    None
}

fn skip_flow_value(chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>) {
    // Skip until the next `,` at depth 0. Quotes don't carry depth here because
    // our flow values are single-level (no nested mappings inside this walker).
    while let Some((_, c)) = chars.next() {
        if c == ',' {
            return;
        }
    }
}

/// Latest verifier's `at` (epoch ms) from `verified: [...]` (or bare mapping).
/// Walks the verbatim flow text and finds the rightmost `{ by: ..., at: ... }` entry.
pub(crate) fn latest_verified_at(fm: &crate::okf::types::OkfFrontmatter) -> Option<i64> {
    let raw = fm.get_str("verified")?;
    let last_at = last_verified_at_in_text(raw)?;
    crate::okf::timefmt::ms_from_iso(&last_at)
}
pub(crate) fn latest_verified_by(fm: &crate::okf::types::OkfFrontmatter) -> Option<String> {
    let raw = fm.get_str("verified")?;
    last_verified_by_in_text(raw)
}

pub(crate) fn last_verified_at_in_text(raw: &str) -> Option<String> {
    last_flow_mapping_field(raw, "at")
}
pub(crate) fn last_verified_by_in_text(raw: &str) -> Option<String> {
    last_flow_mapping_field(raw, "by")
}

/// Walk a verbatim flow-sequence-of-flow-mappings (`[ { k: v }, { k: v } ]`) or
/// bare flow-mapping text and return the named field's value from the LAST
/// mapping element. Quote-aware so commas / braces inside `"…"` / `'…'` spans
/// don't terminate the value early. Returns None when the input is malformed
/// or no entry has the requested field.
fn last_flow_mapping_field(raw: &str, key: &str) -> Option<String> {
    let inner = raw.trim();
    let mappings: Vec<&str> = if let Some(stripped) = inner
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
    {
        // Flow sequence — extract each `{ ... }` element via depth tracking.
        collect_flow_mappings(stripped.trim())
    } else if inner.starts_with('{') && inner.ends_with('}') {
        vec![inner]
    } else {
        return None;
    };
    let mut result: Option<String> = None;
    for mapping in mappings {
        if let Some(v) = flow_mapping_field(mapping, key) {
            result = Some(v);
        }
    }
    result
}

/// Collect each `{ ... }` flow-mapping element from a flow-sequence body,
/// respecting `"…"` / `'…'` quoting so braces inside quoted spans don't
/// change nesting depth.
fn collect_flow_mappings(body: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b',' | b' ' | b'\t' | b'\n' => i += 1,
            b'{' => {
                let start = i;
                let mut depth: i32 = 1;
                i += 1;
                while i < bytes.len() && depth > 0 {
                    match bytes[i] {
                        b'"' => {
                            i += 1;
                            while i < bytes.len() && bytes[i] != b'"' {
                                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                                    i += 2;
                                } else {
                                    i += 1;
                                }
                            }
                            // Clamp so an unterminated quote cannot push `i`
                            // past `bytes.len()` and panic the slice below.
                            i = (i + 1).min(bytes.len());
                        }
                        b'\'' => {
                            i += 1;
                            while i < bytes.len() && bytes[i] != b'\'' {
                                i += 1;
                            }
                            i = (i + 1).min(bytes.len());
                        }
                        b'{' => {
                            depth += 1;
                            i += 1;
                        }
                        b'}' => {
                            depth -= 1;
                            i += 1;
                        }
                        _ => i += 1,
                    }
                }
                out.push(&body[start..i]);
            }
            _ => return out, // unexpected token — bail
        }
    }
    out
}

/// Quote a string as a YAML flow scalar, escaping `\` and `"` so the
/// encoded form round-trips through the scalar decoder (see
/// `read_flow_value`). Bare strings containing only safe characters AND
/// that `classify_flow_scalar` would round-trip back as a `String` are
/// emitted unquoted; anything with whitespace, structural YAML
/// punctuation, escape-significant characters, or a value the classifier
/// would coerce (boolean / null / numeric) is wrapped in double quotes
/// with `\` → `\\`, `"` → `\"`, and the control chars `\n` / `\r` / `\t`
/// escaped so a quoted scalar stays on one physical line and decodes
/// back to the original control characters. This is the single source of
/// truth for flow-scalar encoding; both `json_value_to_flow` and
/// `serialize_actor_string` delegate here.
pub(crate) fn encode_flow_scalar(s: &str) -> String {
    let safe_chars = !s.contains(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                ':' | ','
                    | '"'
                    | '\''
                    | '-'
                    | '/'
                    | '{'
                    | '}'
                    | '['
                    | ']'
                    | '#'
                    | '\\'
            )
    });
    // Bare form only safe if classify_flow_scalar would round-trip back
    // to a String. "true" / "false" / "null" / "1.5" etc. would otherwise
    // be silently coerced to Bool / Null / Number on import.
    let round_trips_as_string =
        matches!(classify_flow_scalar(s.to_string()), serde_json::Value::String(_));
    if !s.is_empty() && safe_chars && round_trips_as_string {
        return s.to_string();
    }
    // Empty / needs-quoting: wrap in double quotes and escape backslashes
    // first so a `\"` we add below is not itself escaped on the next pass;
    // then escape the control chars `\n` / `\r` / `\t` so the encoded
    // scalar never spans physical lines and decodes back identically via
    // `read_flow_value`.
    let escaped = s
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t");
    format!("\"{escaped}\"")
}

/// Quote actor strings containing `/` or `:` per upstream spec §4.1.
pub(crate) fn serialize_actor_string(s: &str) -> String {
    encode_flow_scalar(s)
}

/// Parse a JSON array string and re-emit it as a flow sequence of flow mappings:
/// `[ { a: 1, b: 2 }, { a: 3, b: 4 } ]`. The `key` argument is the frontmatter
/// key (for error messages). Returns `None` if the JSON is malformed.
pub(crate) fn json_array_to_flow_sequence(json: &str, _key: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let arr = v.as_array()?;
    let mut out = String::from("[ ");
    for (i, entry) in arr.iter().enumerate() {
        if i > 0 { out.push_str(", "); }
        let obj = entry.as_object()?;
        out.push_str("{ ");
        for (j, (k, val)) in obj.iter().enumerate() {
            if j > 0 { out.push_str(", "); }
            out.push_str(&format!("{}: {}", k, json_value_to_flow(val)));
        }
        out.push_str(" }");
    }
    out.push_str(" ]");
    Some(out)
}

/// Render a JSON object as a flow mapping: `{ k: v, k2: v2 }`.
pub(crate) fn flow_mapping_from_json(json: &str, _key: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let obj = v.as_object()?;
    let mut out = String::from("{ ");
    for (i, (k, val)) in obj.iter().enumerate() {
        if i > 0 { out.push_str(", "); }
        out.push_str(&format!("{}: {}", k, json_value_to_flow(val)));
    }
    out.push_str(" }");
    Some(out)
}

pub(crate) fn json_value_to_flow(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => encode_flow_scalar(s),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Null => "null".into(),
        other => format!("\"{}\"", other),
    }
}

/// Serialize frontmatter pairs to a string, emitting flow-mapping values (whose
/// underlying string starts with `{` and ends with `}`) and flow-sequence values
/// (whose string starts with `[` and ends with `]`) without the outer double
/// quotes that `serialize_frontmatter_pairs` / `serialize_scalar_string` would
/// otherwise apply — quoting them would corrupt the YAML flow syntax.
///
/// This mirrors `serialize_frontmatter_pairs` exactly for non-flow values
/// (delegating to `serialize_scalar_string` for strings and replicating the
/// other variants inline).
pub(crate) fn serialize_pairs_with_flow(pairs: &[(&str, OkfFrontmatterValue)]) -> String {
    let mut lines = vec!["---".to_string()];
    for (key, value) in pairs {
        match value {
            OkfFrontmatterValue::String(s)
                if (s.starts_with('{') && s.ends_with('}'))
                    || (s.starts_with('[') && s.ends_with(']')) =>
            {
                // Flow mapping or sequence — emit raw without outer quoting.
                lines.push(format!("{key}: {s}"));
            }
            OkfFrontmatterValue::StringList(items) if items.is_empty() => {
                lines.push(format!("{key}: []"));
            }
            OkfFrontmatterValue::StringList(items) => {
                lines.push(format!("{key}:"));
                for item in items {
                    lines.push(format!("  - {}", serialize_scalar_string(item)));
                }
            }
            OkfFrontmatterValue::String(s) => {
                lines.push(format!("{key}: {}", serialize_scalar_string(s)));
            }
            OkfFrontmatterValue::Number(n) => lines.push(format!("{key}: {n}")),
            OkfFrontmatterValue::Bool(b) => lines.push(format!("{key}: {b}")),
            OkfFrontmatterValue::Null => lines.push(format!("{key}: null")),
        }
    }
    lines.push("---".to_string());
    format!("{}\n", lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOLDEN_FACT: &str =
        include_str!("../../fixtures/okf/golden-v1/entities/demo/facts/fact_alpha.md");

    const V02_FACT: &str = "---\n\
type: fact\ntitle: V02 fact\nid: fact_v02\nentity_id: ent_demo\n\
confidence: certain\nsource_type: user_stated\n\
timestamp: 2026-07-01T00:00:00.000Z\ncreated_at: 1719835200000\n\
generated: { by: human:alice, at: 2026-07-01T00:00:00.000Z }\n\
verified: [ { by: process:nightly, at: 2026-07-02T00:00:00.000Z } ]\n\
status: stable\nstale_after: 2027-01-01\nusage_window: { from: 2026-07-01, to: 2026-12-31 }\n\
sources: [ { resource: documents/notes.md, title: notes, usage_count: 3, last_modified: 2026-07-01 } ]\n---\nV02 body.\n";

    #[test]
    fn parses_v02_fact_fields() {
        let parsed = parse_fact_file(V02_FACT).unwrap();
        assert_eq!(parsed.fact.lifecycle_status, "stable");
        assert_eq!(parsed.fact.generated_by.as_deref(), Some("human:alice"));
        // okf_sources / okf_verified / okf_usage_window are stored as JSON
        // strings per the typed-model contract (entities.rs reads them via
        // serde_json::from_str). The parser converts the YAML flow text.
        let sources_json = parsed.fact.okf_sources.as_deref().unwrap();
        let sources: serde_json::Value = serde_json::from_str(sources_json).unwrap();
        assert_eq!(
            sources[0]["resource"].as_str(),
            Some("documents/notes.md")
        );
        assert_eq!(sources[0]["usage_count"].as_i64(), Some(3));
        let window: serde_json::Value =
            serde_json::from_str(parsed.fact.okf_usage_window.as_deref().unwrap()).unwrap();
        assert_eq!(window["from"].as_str(), Some("2026-07-01"));
        assert_eq!(window["to"].as_str(), Some("2026-12-31"));
        // stale_after: 2027-01-01 → ms at UTC midnight (we don't assert exact ms — just that it's Some)
        assert!(parsed.fact.stale_after.is_some());
        // verified list has one entry → last_verified_at / last_verified_by populated by the flow-mapping walker
        assert!(parsed.fact.last_verified_at.is_some());
        assert_eq!(parsed.fact.last_verified_by.as_deref(), Some("process:nightly"));
    }

    #[test]
    fn parses_v02_fact_verified_list() {
        let raw = "---\ntype: fact\ntitle: T\nid: f1\nentity_id: e1\n\
verified: [ { by: process:p1, at: 2026-07-01T00:00:00.000Z }, { by: human:alice, at: 2026-07-02T00:00:00.000Z } ]\n---\nB\n";
        let parsed = parse_fact_file(raw).unwrap();
        assert!(parsed.fact.okf_verified.as_deref().unwrap().contains("human:alice"));
        // ms_from_iso("2026-07-02T00:00:00.000Z") is implementation-defined; just check Some
        assert!(parsed.fact.last_verified_at.is_some());
        assert_eq!(parsed.fact.last_verified_by.as_deref(), Some("human:alice"));
    }

    #[test]
    fn builds_fact_with_v02_fields() {
        let fact = WikiFact {
            id: "fact_x".into(),
            entity_id: "ent_demo".into(),
            title: "Title".into(),
            body: "Body".into(),
            tags: vec![],
            confidence: "certain".into(),
            source_type: "user_stated".into(),
            source_hash: None,
            source_ref: None,
            created_at: 1719835200000,
            updated_at: 1719835200000,
            last_accessed_at: None,
            access_count: 0,
            deleted_at: None,
            okf_type: None,
            lifecycle_status: "stable".into(),
            stale_after: Some(crate::okf::timefmt::ms_from_utc_date("2027-01-01").unwrap()),
            generated_by: Some("human:alice".into()),
            okf_sources: Some(r#"[{"resource":"documents/notes.md"}]"#.into()),
            okf_verified: Some(r#"[{"by":"process:nightly","at":"2026-07-02T00:00:00.000Z"}]"#.into()),
            okf_usage_window: Some(r#"{"from":"2026-07-01","to":"2026-12-31"}"#.into()),
            last_verified_at: Some(crate::okf::timefmt::ms_from_iso("2026-07-02T00:00:00.000Z").unwrap()),
            last_verified_by: Some("process:nightly".into()),
        };
        let md = build_fact_file(&fact, &[], "llm-wiki/2");
        assert!(md.contains("status: stable"), "missing lifecycle status: {md}");
        assert!(md.contains("stale_after: 2027-01-01"), "missing stale_after: {md}");
        assert!(md.contains("generated: { by: \"human:alice\""), "missing generated flow mapping: {md}");
        assert!(md.contains("verified:"), "missing verified key: {md}");
        assert!(md.contains("sources:"), "missing sources key: {md}");
        assert!(md.contains("usage_window: { from: \"2026-07-01\", to: \"2026-12-31\" }"), "missing usage_window: {md}");
        // Round-trip: parse what we just emitted
        let parsed = parse_fact_file(&md).unwrap();
        assert_eq!(parsed.fact.lifecycle_status, "stable");
        assert_eq!(parsed.fact.generated_by.as_deref(), Some("human:alice"));
        assert!(parsed.fact.okf_sources.as_deref().unwrap().contains("documents/notes.md"));
    }

    #[test]
    fn builds_fact_v01_omits_v02_fields() {
        let fact = WikiFact {
            id: "fact_x".into(),
            entity_id: "ent_demo".into(),
            title: "Title".into(),
            body: "Body".into(),
            tags: vec![],
            confidence: "certain".into(),
            source_type: "user_stated".into(),
            source_hash: None,
            source_ref: None,
            created_at: 1719835200000,
            updated_at: 1719835200000,
            last_accessed_at: None,
            access_count: 0,
            deleted_at: None,
            okf_type: None,
            lifecycle_status: "stable".into(),
            stale_after: Some(crate::okf::timefmt::ms_from_utc_date("2027-01-01").unwrap()),
            generated_by: Some("human:alice".into()),
            okf_sources: Some(r#"[{"resource":"documents/notes.md"}]"#.into()),
            okf_verified: Some(r#"[{"by":"process:nightly","at":"2026-07-02T00:00:00.000Z"}]"#.into()),
            okf_usage_window: Some(r#"{"from":"2026-07-01","to":"2026-12-31"}"#.into()),
            last_verified_at: Some(crate::okf::timefmt::ms_from_iso("2026-07-02T00:00:00.000Z").unwrap()),
            last_verified_by: Some("process:nightly".into()),
        };
        let md = build_fact_file(&fact, &[], "llm-wiki/1");
        assert!(!md.contains("status:"), "v0.1 must not emit status: {md}");
        assert!(!md.contains("stale_after:"), "v0.1 must not emit stale_after: {md}");
        assert!(!md.contains("generated:"), "v0.1 must not emit generated: {md}");
        assert!(!md.contains("verified:"), "v0.1 must not emit verified: {md}");
        assert!(!md.contains("sources:"), "v0.1 must not emit sources: {md}");
        assert!(!md.contains("usage_window:"), "v0.1 must not emit usage_window: {md}");
    }

    #[test]
    fn parses_golden_fact() {
        let parsed = parse_fact_file(GOLDEN_FACT).unwrap();
        assert_eq!(parsed.fact.id, "fact_alpha");
        assert_eq!(parsed.fact.entity_id, "demo");
        assert_eq!(parsed.fact.title, "Alpha fact");
        assert_eq!(parsed.fact.body, "Alpha body text.");
        assert_eq!(parsed.fact.tags, vec!["demo".to_string()]);
        assert_eq!(parsed.fact.confidence, "certain");
        assert_eq!(parsed.fact.source_type, "user_stated");
        assert_eq!(parsed.fact.created_at, 1719835200000);
        assert_eq!(parsed.fact.updated_at, 1782907200000);
        assert_eq!(parsed.fact.okf_type, None);
        assert_eq!(parsed.related.len(), 2);
        assert_eq!(parsed.related[0].text, "references");
        assert_eq!(parsed.related[0].path, "./fact_beta.md");
        assert_eq!(parsed.related[1].path, "../tasks/task_follow.md");
    }

    #[test]
    fn round_trips_golden_fact_bytes() {
        let parsed = parse_fact_file(GOLDEN_FACT).unwrap();
        let related: Vec<(String, String)> = parsed
            .related
            .iter()
            .map(|l| (l.text.clone(), l.path.clone()))
            .collect();
        let rebuilt = build_fact_file(&parsed.fact, &related, "llm-wiki/1");
        assert_eq!(normalize(&rebuilt), normalize(GOLDEN_FACT));
    }

    #[test]
    fn flow_value_with_unterminated_quote_does_not_panic() {
        // Bundle files are untrusted; the bare-value scanner used to advance
        // `j` past `bytes.len()` on an unterminated quote and panic on the
        // following `s[..j]` slice. The clamp keeps both code paths total.
        // We only assert that parsing does not panic and that the parser
        // returns Some/None without aborting the import.
        let _ = parse_flow_value(r#"[ { resource: "missing-close-quote } ]"#);
        let _ = parse_flow_value(r#"[ { resource: 'missing-close-quote } ]"#);
        // Unterminated flow mapping must also not panic; we just exercise
        // the slice path.
        let _ = parse_flow_value(r#"{ resource: "missing-close-quote"#);
    }

    #[test]
    fn flow_value_with_unterminated_quote_ending_in_multibyte_does_not_panic() {
        // Regression: previously, both `b'"'` and `b'\''` arms of
        // `read_flow_value` sliced at `end.saturating_sub(1)` even when no
        // closing quote was found. A value ending in a multi-byte UTF-8
        // sequence (e.g. `é` = 0xC3 0xA9) would slice between the two
        // bytes and panic. These tests exercise both arms with that shape.
        let _ = parse_flow_value(r#"[ { resource: "missing-close-quote-é } ]"#);
        let _ = parse_flow_value(r#"[ { resource: 'missing-close-quote-é } ]"#);
        // Trailing multi-byte byte is the worst case: the slice would fall
        // strictly between the two bytes of the last character.
        let _ = parse_flow_value(r#"[ { resource: "missing-close-"\u{00e9} ]"#);
        let _ = parse_flow_value(r#"[ { resource: 'missing-close-\u{00e9} ]"#);
    }

    #[test]
    fn unterminated_quoted_scalar_is_classified_as_null() {
        // CodeRabbit follow-up: the previous decoder returned the entire
        // post-quote span as the "value", so a malformed input like
        // `{ resource: "missing-close }` would silently produce a string.
        // Reject it as Null instead. A bare flow-mapping input lets us
        // exercise the single-mapping walker (collect_flow_mappings on
        // an array context fails first when the surrounding array is also
        // truncated, which is correct behavior but masks the scalar
        // fix).
        let parsed = parse_flow_value(r#"{ resource: "missing-close }"#)
            .expect("malformed mapping must still parse as an object");
        let entry = parsed.as_object().unwrap();
        assert!(
            entry["resource"].is_null(),
            "resource must be Null: {entry:?}"
        );
    }

    /// Build a one-key flow mapping and round-trip the value through
    /// encode → decode, asserting the decoded JSON equals the original.
    /// Used to prove the encoder/decoder pair is symmetric for any
    /// string shape we might persist via OKF v0.2 columns.
    fn round_trip_flow_value(original: &str) -> serde_json::Value {
        let encoded = encode_flow_scalar(original);
        // Wrap in `{ k: <encoded> }` so we exercise the flow-mapping
        // walker (split_flow_pairs → read_flow_value) on the encoded
        // payload, not a bare scalar read.
        let raw = format!("{{ k: {encoded} }}");
        let parsed = parse_flow_value(&raw).unwrap();
        parsed.as_object().unwrap()["k"].clone()
    }

    #[test]
    fn round_trips_strings_with_quotes() {
        let original = r#"she said "hello" and left"#;
        let decoded = round_trip_flow_value(original);
        assert_eq!(decoded.as_str(), Some(original));
    }

    #[test]
    fn round_trips_strings_with_backslashes() {
        // Windows-style path with both kinds of escape-significant chars.
        let original = r#"C:\Users\alice\docs"#;
        let decoded = round_trip_flow_value(original);
        assert_eq!(decoded.as_str(), Some(original));
    }

    #[test]
    fn round_trips_strings_with_commas() {
        let original = "alpha, beta, gamma";
        let decoded = round_trip_flow_value(original);
        assert_eq!(decoded.as_str(), Some(original));
    }

    #[test]
    fn round_trips_strings_with_line_breaks() {
        let original = "line one\nline two\nline three";
        let decoded = round_trip_flow_value(original);
        assert_eq!(decoded.as_str(), Some(original));
    }

    #[test]
    fn round_trips_actor_strings_with_punctuation() {
        // serialize_actor_string is used for the `generated.by` field;
        // its input is a free-form actor identifier, so quote+escape
        // symmetry must hold for the same shapes as the columns above.
        let cases = [
            "human:alice",
            "process:nightly-job",
            "human:bot\\escape",
            r#"human:has "quote""#,
        ];
        for original in cases {
            let encoded = serialize_actor_string(original);
            let raw = format!("{{ by: {encoded} }}");
            let parsed = parse_flow_value(&raw).unwrap();
            assert_eq!(
                parsed["by"].as_str(),
                Some(original),
                "actor round-trip failed for {original:?}: encoded={encoded:?}",
            );
        }
    }

    #[test]
    fn generated_by_round_trips_through_build_and_parse() {
        // End-to-end check: a generated_by string with both backslashes
        // and quotes survives build_fact_file → parse_fact_file.
        let mut fact = WikiFact {
            id: "fact_rt".into(),
            entity_id: "ent_rt".into(),
            title: "Round trip".into(),
            body: "B".into(),
            tags: vec![],
            confidence: "certain".into(),
            source_type: "user_stated".into(),
            source_hash: None,
            source_ref: None,
            created_at: 1719835200000,
            updated_at: 1719835200000,
            last_accessed_at: None,
            access_count: 0,
            deleted_at: None,
            okf_type: None,
            lifecycle_status: "stable".into(),
            stale_after: None,
            generated_by: Some(r#"human:has "quote" and \backslash"#.into()),
            okf_sources: None,
            okf_verified: None,
            okf_usage_window: None,
            last_verified_at: None,
            last_verified_by: None,
        };
        let md = build_fact_file(&fact, &[], "llm-wiki/2");
        let parsed = parse_fact_file(&md).unwrap();
        assert_eq!(
            parsed.fact.generated_by.as_deref(),
            Some(r#"human:has "quote" and \backslash"#)
        );
        // And again, with a comma in the actor (would have been emitted
        // unquoted by the old encoder, breaking the YAML flow mapping).
        fact.generated_by = Some("process:a, b".into());
        let md = build_fact_file(&fact, &[], "llm-wiki/2");
        let parsed = parse_fact_file(&md).unwrap();
        assert_eq!(parsed.fact.generated_by.as_deref(), Some("process:a, b"));
    }

    fn normalize(s: &str) -> String {
        format!("{}\n", s.trim_end())
    }

    /// CodeRabbit follow-up (data-integrity): encode_flow_scalar used to
    /// cast individual bytes with `as char`, which truncated multi-byte
    /// UTF-8 sequences (e.g. `"café"` decoded as `"cafÃ©"`). The decoder
    /// now walks char boundaries.
    #[test]
    fn round_trips_non_ascii_source_metadata() {
        let original = "café note — résumé du jour";
        let decoded = round_trip_flow_value(original);
        assert_eq!(decoded.as_str(), Some(original));
    }

    /// Same root cause, exercised on the actor (generated.by) path.
    #[test]
    fn round_trips_non_ascii_actor_string() {
        let original = "human:josé — résumé";
        let encoded = serialize_actor_string(original);
        let raw = format!("{{ by: {encoded} }}");
        let parsed = parse_flow_value(&raw).unwrap();
        assert_eq!(
            parsed["by"].as_str(),
            Some(original),
            "non-ASCII actor must round-trip via UTF-8 char boundaries; encoded={encoded:?}",
        );
    }

    /// CodeRabbit follow-up (data-integrity): encode_flow_scalar used to
    /// emit `"true"` / `"false"` / `"null"` / `"42"` as bare values; the
    /// classifier then coerced them to Bool / Null / Number on import.
    /// Quote them so they survive round-trip as String.
    #[test]
    fn round_trips_type_sensitive_strings_as_strings() {
        for original in ["true", "false", "null", "42", "3.14", "-7", "0"] {
            let decoded = round_trip_flow_value(original);
            assert_eq!(
                decoded.as_str(),
                Some(original),
                "{original:?} must round-trip as String, got {decoded:?}",
            );
        }
    }

    /// End-to-end regression for control-character handling: a generated_by
    /// containing `\n` / `\r` / `\t` must encode inside a single quoted
    /// scalar (no physical line split) and decode back to the original
    /// control characters via build_fact_file / parse_fact_file.
    #[test]
    fn generated_by_with_control_chars_round_trips_through_build_and_parse() {
        let fact = WikiFact {
            id: "fact_cc".into(),
            entity_id: "ent_cc".into(),
            title: "Control chars".into(),
            body: "B".into(),
            tags: vec![],
            confidence: "certain".into(),
            source_type: "user_stated".into(),
            source_hash: None,
            source_ref: None,
            created_at: 1719835200000,
            updated_at: 1719835200000,
            last_accessed_at: None,
            access_count: 0,
            deleted_at: None,
            okf_type: None,
            lifecycle_status: "stable".into(),
            stale_after: None,
            generated_by: Some("human:multi\nline\twith\rcarriage".into()),
            okf_sources: None,
            okf_verified: None,
            okf_usage_window: None,
            last_verified_at: None,
            last_verified_by: None,
        };
        let md = build_fact_file(&fact, &[], "llm-wiki/2");
        // The encoded scalar must stay on a single physical line — control
        // chars escaped, not emitted raw, so the YAML frontmatter remains
        // well-formed. We slice the `generated:` line directly rather
        // than the whole frontmatter, which naturally spans many lines.
        let front = md.split_once("\n---\n").unwrap().0;
        let generated_line = front
            .lines()
            .find(|l| l.starts_with("generated:"))
            .expect("generated line must be present");
        assert!(
            !generated_line.contains('\n'),
            "encoded generated scalar must not contain literal newlines:\n{generated_line}",
        );
        let parsed = parse_fact_file(&md).unwrap();
        assert_eq!(
            parsed.fact.generated_by.as_deref(),
            Some("human:multi\nline\twith\rcarriage"),
        );
    }
}
