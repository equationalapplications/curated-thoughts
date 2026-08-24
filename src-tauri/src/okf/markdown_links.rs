use crate::okf::types::OkfMarkdownLink;

const FENCED_CODE_MARKER: &str = "```";

pub fn extract_markdown_links(body: &str) -> Vec<OkfMarkdownLink> {
    let searchable = strip_fenced_code_blocks(body);
    let mut links = Vec::new();
    let chars: Vec<char> = searchable.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '[' {
            i += 1;
            continue;
        }
        i += 1;
        let label_start = i;
        while i < chars.len() && chars[i] != ']' {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }
        let label: String = chars[label_start..i].iter().collect();
        i += 1; // skip ]
        if i >= chars.len() || chars[i] != '(' {
            continue;
        }
        i += 1;
        let path_start = i;
        while i < chars.len() && chars[i] != ')' && !chars[i].is_whitespace() {
            i += 1;
        }
        let path: String = chars[path_start..i].iter().collect();
        if i < chars.len() && chars[i] == ')' {
            i += 1;
        }
        if path.starts_with("http:") || path.starts_with("https:") || path.starts_with("mailto:") {
            continue;
        }
        links.push(OkfMarkdownLink { text: label, path });
    }
    links
}

fn strip_fenced_code_blocks(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(start) = rest.find(FENCED_CODE_MARKER) {
        out.push_str(&rest[..start]);
        let after_open = &rest[start + FENCED_CODE_MARKER.len()..];
        if let Some(close_rel) = after_open.find(FENCED_CODE_MARKER) {
            let close = start + FENCED_CODE_MARKER.len() + close_rel + FENCED_CODE_MARKER.len();
            rest = &rest[close..];
        } else {
            return out;
        }
    }
    out.push_str(rest);
    out
}
