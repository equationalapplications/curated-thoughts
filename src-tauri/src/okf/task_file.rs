//! Task concept documents (profile §5.2). Body is empty in profile 1;
//! non-empty foreign bodies are tolerated on parse and dropped (we have
//! no task-body field), which §5.2 permits.

use anyhow::{Context, Result};

use crate::okf::concept::parse_concept;
use crate::okf::frontmatter::serialize_frontmatter_pairs;
use crate::okf::related_section::{append_related_section, split_related_section};
use crate::okf::timefmt::{iso_from_ms, ms_from_iso};
use crate::okf::types::{OkfFrontmatterValue as V, OkfMarkdownLink, WikiTask};

pub struct ParsedTask {
    pub task: WikiTask,
    pub related: Vec<OkfMarkdownLink>,
}

fn opt_ms(value: Option<i64>) -> V {
    match value {
        Some(ms) => V::Number(ms as f64),
        None => V::Null,
    }
}

pub fn build_task_file(task: &WikiTask, related: &[(String, String)]) -> String {
    let pairs: Vec<(&str, V)> = vec![
        (
            "type",
            V::String(task.okf_type.clone().unwrap_or_else(|| "task".into())),
        ),
        ("title", V::String(task.description.clone())),
        ("timestamp", V::String(iso_from_ms(task.updated_at))),
        ("id", V::String(task.id.clone())),
        ("entity_id", V::String(task.entity_id.clone())),
        ("status", V::String(task.status.clone())),
        ("priority", V::Number(task.priority as f64)),
        ("created_at", V::Number(task.created_at as f64)),
        ("resolved_at", opt_ms(task.resolved_at)),
        ("deleted_at", opt_ms(task.deleted_at)),
    ];
    let refs: Vec<(&str, &str)> = related
        .iter()
        .map(|(t, p)| (t.as_str(), p.as_str()))
        .collect();
    let body = append_related_section("", &refs);
    format!("{}\n{}", serialize_frontmatter_pairs(&pairs), body)
}

pub fn parse_task_file(content: &str) -> Result<ParsedTask> {
    let (fm, raw_body) = parse_concept(content);
    let (_body, related) = split_related_section(&raw_body);
    let task = WikiTask {
        id: fm.get_str("id").context("task file missing id")?.to_string(),
        entity_id: fm
            .get_str("entity_id")
            .context("task file missing entity_id")?
            .to_string(),
        description: fm.get_str("title").unwrap_or_default().to_string(),
        status: fm.get_str("status").unwrap_or("pending").to_string(),
        priority: fm.get_number("priority").map(|n| n as i64).unwrap_or(0),
        created_at: fm.get_number("created_at").map(|n| n as i64).unwrap_or(0),
        updated_at: fm.get_str("timestamp").and_then(ms_from_iso).unwrap_or(0),
        resolved_at: fm.get_number("resolved_at").map(|n| n as i64),
        deleted_at: fm.get_number("deleted_at").map(|n| n as i64),
        okf_type: fm
            .get_str("type")
            .filter(|t| !t.is_empty() && *t != "task")
            .map(str::to_string),
    };
    Ok(ParsedTask { task, related })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOLDEN_TASK: &str =
        include_str!("../../fixtures/okf/golden-v1/entities/demo/tasks/task_follow.md");

    #[test]
    fn parses_golden_task() {
        let parsed = parse_task_file(GOLDEN_TASK).unwrap();
        assert_eq!(parsed.task.id, "task_follow");
        assert_eq!(parsed.task.entity_id, "demo");
        assert_eq!(parsed.task.description, "Follow up");
        assert_eq!(parsed.task.status, "pending");
        assert_eq!(parsed.task.priority, 1);
        assert_eq!(parsed.task.created_at, 1719835800000);
        assert_eq!(parsed.task.resolved_at, None);
        assert_eq!(parsed.task.deleted_at, None);
        assert!(parsed.related.is_empty());
    }

    #[test]
    fn round_trips_golden_task_bytes() {
        let parsed = parse_task_file(GOLDEN_TASK).unwrap();
        let rebuilt = build_task_file(&parsed.task, &[]);
        assert_eq!(
            format!("{}\n", rebuilt.trim_end()),
            format!("{}\n", GOLDEN_TASK.trim_end())
        );
    }
}
