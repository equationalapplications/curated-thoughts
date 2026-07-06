//! Fact concept documents (profile §5.1).

use anyhow::{Context, Result};

use crate::okf::concept::parse_concept;
use crate::okf::frontmatter::serialize_frontmatter_pairs;
use crate::okf::related_section::{append_related_section, split_related_section};
use crate::okf::timefmt::{iso_from_ms, ms_from_iso};
use crate::okf::types::{OkfFrontmatterValue as V, OkfMarkdownLink, WikiFact};

pub struct ParsedFact {
    pub fact: WikiFact,
    pub related: Vec<OkfMarkdownLink>,
}

/// `related` is `(edge_type, relative_path)` per outgoing edge; dangling
/// targets must already be filtered out by the caller (§6).
pub fn build_fact_file(fact: &WikiFact, related: &[(String, String)]) -> String {
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
    format!("{}\n{}", serialize_frontmatter_pairs(&pairs), body)
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
    };
    Ok(ParsedFact { fact, related })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOLDEN_FACT: &str =
        include_str!("../../fixtures/okf/golden-v1/entities/demo/facts/fact_alpha.md");

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
        let rebuilt = build_fact_file(&parsed.fact, &related);
        assert_eq!(normalize(&rebuilt), normalize(GOLDEN_FACT));
    }

    fn normalize(s: &str) -> String {
        format!("{}\n", s.trim_end())
    }
}
