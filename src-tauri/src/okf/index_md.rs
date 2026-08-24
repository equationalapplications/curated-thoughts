use crate::okf::frontmatter::{parse_frontmatter, serialize_scalar_string};
use crate::okf::types::{OkfIndexEntry, OkfIndexSection};

fn escape_index_text(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace("\r\n", " ")
        .replace('\n', " ")
        .replace('\r', " ")
}

fn render_entry(entry: &OkfIndexEntry) -> String {
    let title = escape_index_text(&entry.title);
    match &entry.description {
        Some(desc) => format!(
            "* [{}]({}) - {}",
            title,
            entry.path,
            escape_index_text(desc)
        ),
        None => format!("* [{}]({})", title, entry.path),
    }
}

fn render_sections(sections: &[OkfIndexSection]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for section in sections {
        lines.push(format!("## {}", section.heading));
        lines.push(String::new());
        for entry in &section.entries {
            lines.push(render_entry(entry));
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

pub fn build_index_md(sections: &[OkfIndexSection]) -> String {
    render_sections(sections)
}

pub fn build_root_index_md(
    okf_version: &str,
    sections: &[OkfIndexSection],
    profile: Option<&str>,
) -> String {
    let mut lines = vec!["---".to_string(), format!("okf_version: {}", okf_version)];
    if let Some(profile) = profile {
        lines.push(format!("profile: {}", serialize_scalar_string(profile)));
    }
    lines.push("---".to_string());
    lines.push(String::new());
    format!("{}\n{}", lines.join("\n"), render_sections(sections))
}

pub fn parse_root_index_md(content: &str) -> (Option<String>, Option<String>) {
    let (frontmatter, _) = parse_frontmatter(content);
    let okf_version = frontmatter
        .get_str("okf_version")
        .map(str::to_string)
        .or_else(|| frontmatter.get_number("okf_version").map(|n| n.to_string()));
    let profile = frontmatter.get_str("profile").map(str::to_string);
    (okf_version, profile)
}
