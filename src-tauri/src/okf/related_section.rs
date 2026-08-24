use crate::okf::types::OkfMarkdownLink;

pub(crate) fn escape_link_label(label: &str) -> String {
    label
        .replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace("\r\n", " ")
        .replace('\n', " ")
        .replace('\r', " ")
}

pub fn append_related_section(body: &str, links: &[(&str, &str)]) -> String {
    if links.is_empty() {
        return body.to_string();
    }
    let prefix = if body.is_empty() {
        String::new()
    } else if body.ends_with("\n\n") {
        body.to_string()
    } else if body.ends_with('\n') {
        format!("{body}\n")
    } else {
        format!("{body}\n\n")
    };
    let mut lines = vec!["## Related".to_string(), String::new()];
    for (edge_type, path) in links {
        lines.push(format!("- [{}]({})", escape_link_label(edge_type), path));
    }
    format!("{}{}\n", prefix, lines.join("\n"))
}

pub fn split_related_section(body: &str) -> (String, Vec<OkfMarkdownLink>) {
    let lines: Vec<&str> = body.lines().collect();
    let mut end = lines.len();
    while end > 0 && lines[end - 1].trim().is_empty() {
        end -= 1;
    }

    let mut scan = end;
    if scan > 0 {
        scan -= 1;
    }
    while scan > 0 && lines[scan].trim_start().starts_with("- ") {
        if scan == 0 {
            break;
        }
        scan -= 1;
    }
    while scan > 0 && lines[scan].trim().is_empty() {
        scan -= 1;
    }

    if scan >= lines.len() || lines[scan].trim() != "## Related" {
        return (body.to_string(), vec![]);
    }

    let related_start = scan;
    let content_body = lines[..related_start].join("\n");
    let related_block = lines[related_start..end].join("\n");
    let mut related_links = Vec::new();

    for line in related_block.lines() {
        let Some(bullet) = line.trim_start().strip_prefix("- ") else {
            continue;
        };
        related_links.extend(parse_inline_links(bullet));
    }

    let normalized_body = if content_body.is_empty() {
        String::new()
    } else if content_body.ends_with('\n') {
        content_body
    } else {
        format!("{content_body}\n")
    };

    (normalized_body, related_links)
}

pub(crate) fn parse_inline_links(text: &str) -> Vec<OkfMarkdownLink> {
    let mut links = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '[' {
            i += 1;
            continue;
        }
        i += 1;
        let mut label = String::new();
        while i < chars.len() {
            if chars[i] == '\\' && i + 1 < chars.len() {
                label.push(chars[i + 1]);
                i += 2;
                continue;
            }
            if chars[i] == ']' {
                i += 1;
                break;
            }
            label.push(chars[i]);
            i += 1;
        }
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
        let text = label
            .replace("\\]", "]")
            .replace("\\[", "[")
            .replace("\\\\", "\\");
        links.push(OkfMarkdownLink { text, path });
    }
    links
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_trailing_related_section() {
        let body = "Alpha body.\n\n## Related\n\n- [references](./fact_beta.md)\n";
        let (stripped, links) = split_related_section(body);
        assert_eq!(stripped.trim_end(), "Alpha body.");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].text, "references");
        assert_eq!(links[0].path, "./fact_beta.md");
    }
}
