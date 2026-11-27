use crate::okf::types::OkfLogEntry;
use std::collections::BTreeMap;

pub fn build_log_md(entries: &[OkfLogEntry]) -> String {
    let mut groups: BTreeMap<&str, Vec<&OkfLogEntry>> = BTreeMap::new();
    for entry in entries {
        groups.entry(entry.date.as_str()).or_default().push(entry);
    }

    let mut lines: Vec<String> = Vec::new();
    let mut dates: Vec<&str> = groups.keys().copied().collect();
    dates.sort_by(|a, b| b.cmp(a));

    for date in dates {
        lines.push(format!("## {date}"));
        lines.push(String::new());
        for entry in groups.get(date).into_iter().flatten() {
            lines.push(format!("- {}", entry.text));
        }
        lines.push(String::new());
    }

    if lines.is_empty() {
        return String::new();
    }
    let mut out = lines.join("\n");
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

pub fn append_event_id_comment(text: &str, event_id: &str) -> String {
    // Same non-empty guard as `append_event_id_comment_forgotten`: an empty
    // id must never mint a junk `<!-- id:  -->` comment (T1.5 r2 MINOR 2).
    if !event_id.is_empty()
        && event_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
    {
        format!("{text} <!-- id: {event_id} -->")
    } else {
        text.to_string()
    }
}

/// Appends the event id comment to `text`, optionally recording the
/// `forgotten:` timestamp key. The id-validation guard applies on BOTH paths.
pub fn append_event_id_comment_forgotten(
    text: &str,
    event_id: &str,
    forgotten_at: Option<i64>,
) -> String {
    let valid = !event_id.is_empty()
        && event_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-');
    match (valid, forgotten_at) {
        (true, Some(ms)) if ms >= 0 => format!("{text} <!-- id: {event_id} forgotten:{ms} -->"),
        (true, _) => append_event_id_comment(text, event_id),
        (false, _) => text.to_string(),
    }
}

/// Parses the trailing event id comment from `text`.
///
/// Returns `(stripped_text, event_id, forgotten_at)`.
///
/// Compared to the previous behavior, every well-formed line the old parser
/// accepted parses identically. Three edge behaviors changed (all
/// improvements): an empty id (`<!-- id: -->`) and a bare `note id:` prefix
/// now yield `None` (previously `Some("")`, which would insert `id = ''`);
/// a trailing junk token (`<!-- id: evt_x junk -->`) now yields
/// `Some("evt_x")` (previously the whole comment was rejected).
pub fn parse_event_id_comment(text: &str) -> (String, Option<String>, Option<i64>) {
    let trimmed = text.trim_end();
    let Some(id_pos) = trimmed.rfind("<!--") else {
        return (text.to_string(), None, None);
    };
    let suffix = &trimmed[id_pos..];
    if !suffix.contains("id:") || !suffix.contains("-->") {
        return (text.to_string(), None, None);
    }
    let inner = suffix
        .trim_start_matches("<!--")
        .trim_end_matches("-->")
        .trim();
    let mut event_id: Option<String> = None;
    let mut forgotten_at: Option<i64> = None;
    if let Some(rest) = inner.strip_prefix("id:") {
        let rest = rest.trim();
        let (id_tok, tail) = match rest.split_once(char::is_whitespace) {
            Some((a, b)) => (a, b),
            None => (rest, ""),
        };
        if !id_tok.is_empty()
            && id_tok
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
        {
            event_id = Some(id_tok.to_string());
            for tok in tail.split_whitespace() {
                if let Some(v) = tok.strip_prefix("forgotten:") {
                    forgotten_at = v.parse::<i64>().ok().filter(|ms| *ms >= 0);
                }
            }
        }
    }
    let stripped = trimmed[..id_pos].trim_end().to_string();
    (stripped, event_id, forgotten_at)
}

pub fn parse_log_md(content: &str) -> Vec<OkfLogEntry> {
    let mut entries = Vec::new();
    let mut current_date: Option<String> = None;

    for line in content.lines() {
        if let Some(date) = line.strip_prefix("## ").map(str::trim) {
            if date.len() == 10 && date.as_bytes().get(4) == Some(&b'-') {
                current_date = Some(date.to_string());
            }
            continue;
        }
        if let Some(text) = line.strip_prefix("- ") {
            if let Some(ref date) = current_date {
                entries.push(OkfLogEntry {
                    date: date.clone(),
                    text: text.to_string(),
                });
            }
        }
    }

    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_writes_forgotten_key_when_present() {
        let t = append_event_id_comment_forgotten("(note) hi", "evt_x", Some(1732713600000));
        assert_eq!(t, "(note) hi <!-- id: evt_x forgotten:1732713600000 -->");
    }

    #[test]
    fn append_delegates_to_id_only_when_absent() {
        let t = append_event_id_comment_forgotten("(note) hi", "evt_x", None);
        assert_eq!(t, "(note) hi <!-- id: evt_x -->");
    }

    #[test]
    fn append_invalid_id_writes_no_comment_even_with_forgotten() {
        // r1 minor 3: the id-validation guard applies on BOTH paths.
        let t = append_event_id_comment_forgotten("(note) hi", "bad id!", Some(1000));
        assert_eq!(t, "(note) hi");
    }

    #[test]
    fn parse_reads_forgotten_from_trailing_comment_only() {
        // `forgotten:` in the summary must NOT be parsed (owner note 1).
        let (text, id, forgotten) = parse_event_id_comment(
            "(note) forgotten:999 <!-- id: evt_x forgotten:1732713600000 -->",
        );
        assert_eq!(text, "(note) forgotten:999");
        assert_eq!(id.as_deref(), Some("evt_x"));
        assert_eq!(forgotten, Some(1732713600000));
    }

    #[test]
    fn parse_id_only_line_yields_none_and_identical_text() {
        let line = "(action) did a thing <!-- id: evt_9 -->";
        let (text, id, forgotten) = parse_event_id_comment(line);
        assert_eq!(text, "(action) did a thing");
        assert_eq!(id.as_deref(), Some("evt_9"));
        assert_eq!(forgotten, None);
    }

    #[test]
    fn parse_non_numeric_or_negative_forgotten_parses_as_absent() {
        // r1 minor 2: "-5".parse::<i64>() succeeds — filter negatives out to
        // match the writer's ms >= 0 guard.
        for bad in ["notanumber", "-5"] {
            let line = format!("(note) hi <!-- id: evt_x forgotten:{bad} -->");
            let (text, id, forgotten) = parse_event_id_comment(&line);
            assert_eq!(id.as_deref(), Some("evt_x"));
            assert_eq!(forgotten, None);
            assert_eq!(text, "(note) hi");
        }
    }
}
