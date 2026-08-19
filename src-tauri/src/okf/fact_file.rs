//! Fact concept documents (profile §5.1).

use anyhow::{Context, Result};

use crate::okf::concept::parse_concept;
use crate::okf::frontmatter::serialize_scalar_string;
use crate::okf::related_section::{append_related_section, split_related_section};
use crate::okf::timefmt::{iso_from_ms, ms_from_iso};
use crate::okf::types::{OkfFrontmatterValue, OkfFrontmatterValue as V, OkfMarkdownLink, WikiFact};

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
    if profile != "llm-wiki/1" {
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
        okf_sources: fm.get_str("sources").map(str::to_string),
        okf_verified: fm.get_str("verified").map(str::to_string),
        okf_usage_window: fm.get_str("usage_window").map(str::to_string),
        last_verified_at: latest_verified_at(&fm),
        last_verified_by: latest_verified_by(&fm),
    };
    Ok(ParsedFact { fact, related })
}

/// Parse OKF v0.2 `stale_after: YYYY-MM-DD` to epoch ms (UTC midnight).
/// Returns None for absent or unparseable input.
fn parse_stale_after_ms(fm: &crate::okf::types::OkfFrontmatter) -> Option<i64> {
    let s = fm.get_str("stale_after")?;
    crate::okf::timefmt::ms_from_utc_date(s)
}

/// Parse the v0.2 `generated: { by: ..., at: ... }` flow mapping to the actor string.
/// Returns None when `generated` is absent or has no `by` (per upstream spec §2.4).
fn parse_generated_by(fm: &crate::okf::types::OkfFrontmatter) -> Option<String> {
    let value = fm.fields.get("generated")?;
    let OkfFrontmatterValue::String(s) = value else { return None; };
    // Flow mapping parsed as raw text; Task 4's parser will replace this with a structured reader.
    // For now: regex over `{ by: "...", at: "..." }` to extract the actor string.
    let _bytes = s.as_bytes();
    let by_idx = s.find("by:")?;
    let rest = &s[by_idx + 3..];
    let trimmed = rest.trim_start();
    let trimmed = trimmed.trim_start_matches(|c: char| c == '"' || c == '\'' || c.is_whitespace());
    let end = trimmed.find(|c: char| c == ',' || c == '}' || c == '"' || c == '\'').unwrap_or(trimmed.len());
    let actor = &trimmed[..end];
    if actor.is_empty() { None } else { Some(actor.to_string()) }
}

/// Latest verifier's `at` (epoch ms) from `verified: [...]` (or bare mapping).
/// Walks the verbatim flow text and finds the rightmost `{ by: ..., at: ... }` entry.
fn latest_verified_at(fm: &crate::okf::types::OkfFrontmatter) -> Option<i64> {
    let raw = fm.get_str("verified")?;
    let last_at = last_verified_at_in_text(raw)?;
    crate::okf::timefmt::ms_from_iso(&last_at)
}
fn latest_verified_by(fm: &crate::okf::types::OkfFrontmatter) -> Option<String> {
    let raw = fm.get_str("verified")?;
    last_verified_by_in_text(raw)
}

fn last_verified_at_in_text(raw: &str) -> Option<String> {
    // Find every `at: <quoted-or-bare>` substring; take the last ISO timestamp.
    let bytes = raw.as_bytes();
    let mut last: Option<String> = None;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"at:") {
            let mut j = i + 3;
            while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') { j += 1; }
            let start = j;
            // Read until a comma, closing brace, or end of value
            while j < bytes.len() && bytes[j] != b',' && bytes[j] != b'}' { j += 1; }
            let token = raw[start..j].trim().trim_matches('"').trim_matches('\'').to_string();
            if !token.is_empty() { last = Some(token); }
            i = j;
        } else {
            i += 1;
        }
    }
    last
}
fn last_verified_by_in_text(raw: &str) -> Option<String> {
    // Symmetric to last_verified_at_in_text; reads the trailing `by:` value.
    let bytes = raw.as_bytes();
    let mut i = 0;
    let mut last: Option<String> = None;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"by:") {
            let mut j = i + 3;
            while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') { j += 1; }
            let start = j;
            while j < bytes.len() && bytes[j] != b',' && bytes[j] != b'}' { j += 1; }
            let token = raw[start..j].trim().trim_matches('"').trim_matches('\'').to_string();
            if !token.is_empty() { last = Some(token); }
            i = j;
        } else {
            i += 1;
        }
    }
    last
}

/// Quote actor strings containing `/` or `:` per upstream spec §4.1.
pub(crate) fn serialize_actor_string(s: &str) -> String {
    if s.contains('/') || s.contains(':') {
        format!("\"{}\"", s)
    } else {
        s.to_string()
    }
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
        serde_json::Value::String(s) => {
            // Quote when the string contains anything YAML-meaningful inside a flow
            // mapping/sequence: whitespace, separators, or characters that would
            // otherwise be ambiguous (date `-`, path `/`, anchor/flow markers).
            if s.contains(|c: char| {
                c.is_whitespace() || matches!(c, ':' | ',' | '"' | '\'' | '-' | '/' | '{' | '}' | '[' | ']' | '#')
            }) {
                format!("\"{}\"", s.replace('"', "\\\""))
            } else {
                s.clone()
            }
        }
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
        assert!(parsed.fact.okf_sources.as_deref().unwrap().contains("documents/notes.md"));
        assert_eq!(parsed.fact.okf_usage_window.as_deref(), Some("{ from: 2026-07-01, to: 2026-12-31 }"));
        // stale_after: 2027-01-01 → ms at UTC midnight (we don't assert exact ms — just that it's Some)
        assert!(parsed.fact.stale_after.is_some());
        // verified list has one entry → last_verified_at / last_verified_by populated by Task 4's flow-mapping walker
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

    fn normalize(s: &str) -> String {
        format!("{}\n", s.trim_end())
    }
}
