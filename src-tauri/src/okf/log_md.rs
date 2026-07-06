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
    if event_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
    {
        format!("{text} <!-- id: {event_id} -->")
    } else {
        text.to_string()
    }
}

pub fn parse_event_id_comment(text: &str) -> (String, Option<String>) {
    let trimmed = text.trim_end();
    let Some(id_pos) = trimmed.rfind("<!--") else {
        return (text.to_string(), None);
    };
    let suffix = &trimmed[id_pos..];
    if !suffix.contains("id:") || !suffix.contains("-->") {
        return (text.to_string(), None);
    }
    let inner = suffix
        .trim_start_matches("<!--")
        .trim_end_matches("-->")
        .trim();
    let event_id = inner.strip_prefix("id:").map(str::trim).unwrap_or("");
    let stripped = trimmed[..id_pos].trim_end().to_string();
    if event_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
    {
        (stripped, Some(event_id.to_string()))
    } else {
        (stripped, None)
    }
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
