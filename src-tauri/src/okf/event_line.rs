//! Event-log line grammar (§7): `({event_type}) {summary-or-link} <!-- id: evt -->`.

use crate::okf::log_md::{append_event_id_comment, parse_event_id_comment};
use crate::okf::related_section::{escape_link_label, parse_inline_links};

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedEventLine {
    pub event_type: String,
    pub summary: String,
    pub related_path: Option<String>,
    pub event_id: Option<String>,
}

pub fn build_event_text(
    event_type: &str,
    summary: &str,
    related_path: Option<&str>,
    event_id: &str,
) -> String {
    let escaped = escape_link_label(summary);
    let core = match related_path {
        Some(path) => format!("({event_type}) [{escaped}]({path})"),
        None => format!("({event_type}) {escaped}"),
    };
    append_event_id_comment(&core, event_id)
}

pub fn parse_event_text(text: &str) -> Option<ParsedEventLine> {
    let (without_id, event_id) = parse_event_id_comment(text);
    let rest = without_id.trim().strip_prefix('(')?;
    let close = rest.find(')')?;
    let event_type = rest[..close].trim().to_string();
    if event_type.is_empty() {
        return None;
    }
    let tail = rest[close + 1..].trim();
    if tail.starts_with('[') {
        if let Some(link) = parse_inline_links(tail).into_iter().next() {
            return Some(ParsedEventLine {
                event_type,
                summary: link.text,
                related_path: Some(link.path),
                event_id,
            });
        }
    }
    let summary = tail
        .replace("\\]", "]")
        .replace("\\[", "[")
        .replace("\\\\", "\\");
    Some(ParsedEventLine {
        event_type,
        summary,
        related_path: None,
        event_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_linked_event_with_id() {
        let text = build_event_text(
            "observation",
            "Linked to alpha",
            Some("./facts/fact_alpha.md"),
            "evt_golden_1",
        );
        assert_eq!(
            text,
            "(observation) [Linked to alpha](./facts/fact_alpha.md) <!-- id: evt_golden_1 -->"
        );
    }

    #[test]
    fn parses_golden_lines() {
        let linked = parse_event_text(
            "(observation) [Linked to alpha](./facts/fact_alpha.md) <!-- id: evt_golden_1 -->",
        )
        .unwrap();
        assert_eq!(linked.event_type, "observation");
        assert_eq!(linked.summary, "Linked to alpha");
        assert_eq!(linked.related_path.as_deref(), Some("./facts/fact_alpha.md"));
        assert_eq!(linked.event_id.as_deref(), Some("evt_golden_1"));

        let plain = parse_event_text("(decision) Chose path B <!-- id: evt_golden_2 -->").unwrap();
        assert_eq!(plain.event_type, "decision");
        assert_eq!(plain.summary, "Chose path B");
        assert_eq!(plain.related_path, None);
        assert_eq!(plain.event_id.as_deref(), Some("evt_golden_2"));
    }

    #[test]
    fn tolerates_missing_id_comment_profile0() {
        let ev = parse_event_text("(action) Something happened").unwrap();
        assert_eq!(ev.event_id, None);
        assert_eq!(ev.summary, "Something happened");
    }

    #[test]
    fn escapes_bracket_summaries() {
        let text = build_event_text("action", "See [ref]", None, "evt_x");
        let parsed = parse_event_text(&text).unwrap();
        assert_eq!(parsed.summary, "See [ref]");
    }
}
