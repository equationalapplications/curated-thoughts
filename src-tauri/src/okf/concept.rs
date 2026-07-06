use crate::okf::frontmatter::{parse_frontmatter, serialize_frontmatter};
use crate::okf::types::OkfFrontmatter;

pub fn build_concept_document(fm: &OkfFrontmatter, body: &str) -> String {
    format!("{}\n{}", serialize_frontmatter(fm), body)
}

pub fn parse_concept(content: &str) -> (OkfFrontmatter, String) {
    let (frontmatter, rest) = parse_frontmatter(content);
    let body = rest.strip_prefix('\n').unwrap_or(&rest).to_string();
    (frontmatter, body)
}
