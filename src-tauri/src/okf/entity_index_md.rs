use crate::okf::index_md::build_index_md;
use crate::okf::types::OkfIndexSection;

const EVENT_LOG_LINK: &str = "[Event log](./log.md)";

pub fn build_entity_index_md(summary: Option<&str>, sections: &[OkfIndexSection]) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(summary) = summary.filter(|s| !s.trim().is_empty()) {
        parts.push(summary.trim_end().to_string());
        parts.push(String::new());
    }
    let sections_md = build_index_md(sections).trim_end().to_string();
    if !sections_md.is_empty() {
        parts.push(sections_md);
        parts.push(String::new());
    }
    parts.push(EVENT_LOG_LINK.to_string());
    format!("{}\n", parts.join("\n"))
}

pub fn parse_entity_index_md(content: &str) -> (String, Vec<OkfIndexSection>) {
    let lines: Vec<&str> = content.lines().collect();

    let mut start = 0usize;
    if lines.first().map(|l| l.trim()) == Some("---") {
        if let Some(closing) = lines
            .iter()
            .enumerate()
            .skip(1)
            .find(|(_, line)| line.trim() == "---")
            .map(|(i, _)| i)
        {
            start = closing + 1;
        }
    }

    let first_section_idx = lines
        .iter()
        .enumerate()
        .skip(start)
        .find(|(_, line)| line.trim().starts_with("## "))
        .map(|(i, _)| i);

    let summary_end = first_section_idx.unwrap_or(lines.len());
    let summary_lines: Vec<&str> = lines[start..summary_end]
        .iter()
        .copied()
        .filter(|line| line.trim() != EVENT_LOG_LINK)
        .collect();

    let mut summary_start = 0usize;
    while summary_start < summary_lines.len() && summary_lines[summary_start].trim().is_empty() {
        summary_start += 1;
    }
    if summary_start < summary_lines.len()
        && summary_lines[summary_start]
            .trim()
            .starts_with("# ")
            && !summary_lines[summary_start].trim().starts_with("## ")
    {
        summary_start += 1;
    }
    let summary = summary_lines[summary_start..]
        .join("\n")
        .trim()
        .to_string();

    let mut sections = Vec::new();
    let Some(first_section_idx) = first_section_idx else {
        return (summary, sections);
    };

    let mut current_idx: Option<usize> = None;
    for line in &lines[first_section_idx.max(start)..] {
        if line.trim() == EVENT_LOG_LINK {
            continue;
        }
        if let Some(heading) = line.strip_prefix("## ") {
            let section = OkfIndexSection {
                heading: heading.trim().to_string(),
                entries: Vec::new(),
            };
            sections.push(section);
            current_idx = Some(sections.len() - 1);
            continue;
        }
        let Some(idx) = current_idx else {
            continue;
        };
        if let Some(entry) = parse_index_entry(line) {
            sections[idx].entries.push(entry);
        }
    }

    (summary, sections)
}

fn parse_index_entry(line: &str) -> Option<crate::okf::types::OkfIndexEntry> {
    let trimmed = line.trim();
    if !trimmed.starts_with("* [") {
        return None;
    }
    let rest = &trimmed[3..];
    let (title, path, tail) = parse_bracket_link(rest)?;
    let description = tail
        .find(" - ")
        .map(|dash_idx| unescape_index_title(tail[dash_idx + 3..].trim()));
    Some(crate::okf::types::OkfIndexEntry {
        title: unescape_index_title(&title),
        path,
        description,
    })
}

fn parse_bracket_link(input: &str) -> Option<(String, String, &str)> {
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    let mut title = String::new();
    while i < chars.len() {
        if chars[i] == '\\' && i + 1 < chars.len() {
            title.push(chars[i + 1]);
            i += 2;
            continue;
        }
        if chars[i] == ']' {
            break;
        }
        title.push(chars[i]);
        i += 1;
    }
    if i >= chars.len() || chars[i] != ']' {
        return None;
    }
    i += 1;
    if i >= chars.len() || chars[i] != '(' {
        return None;
    }
    i += 1;
    let path_start = i;
    while i < chars.len() && chars[i] != ')' {
        i += 1;
    }
    if i >= chars.len() || chars[i] != ')' {
        return None;
    }
    let path: String = chars[path_start..i].iter().collect();
    i += 1;
    let byte_offset: usize = chars[..i].iter().map(|c| c.len_utf8()).sum();
    Some((title, path, &input[byte_offset..]))
}

fn unescape_index_title(title: &str) -> String {
    title
        .replace("\\]", "]")
        .replace("\\[", "[")
        .replace("\\\\", "\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_description_and_non_ascii_title() {
        let line = "* [café](facts/a.md) - short summary";
        let entry = parse_index_entry(line).expect("entry");
        assert_eq!(entry.title, "café");
        assert_eq!(entry.path, "facts/a.md");
        assert_eq!(entry.description.as_deref(), Some("short summary"));
    }
}
