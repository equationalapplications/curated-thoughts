//! Task concept documents (profile §5.2). Body is empty in profile 1;
//! non-empty foreign bodies are tolerated on parse and dropped (we have
//! no task-body field), which §5.2 permits.

use anyhow::{Context, Result};

use crate::okf::concept::parse_concept;
use crate::okf::fact_file::{
    flow_mapping_from_json, json_array_to_flow_sequence, serialize_actor_string,
    serialize_pairs_with_flow,
};
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
    let mut pairs: Vec<(&str, V)> = vec![
        (
            "type",
            V::String(task.okf_type.clone().unwrap_or_else(|| "task".into())),
        ),
        ("title", V::String(task.description.clone())),
        ("timestamp", V::String(iso_from_ms(task.updated_at))),
        ("id", V::String(task.id.clone())),
        ("entity_id", V::String(task.entity_id.clone())),
        ("priority", V::Number(task.priority as f64)),
        ("created_at", V::Number(task.created_at as f64)),
        ("resolved_at", opt_ms(task.resolved_at)),
        ("deleted_at", opt_ms(task.deleted_at)),
    ];

    // OKF v0.2 status-rename rule (upstream §2.3):
    // profile-2 wire format puts lifecycle under `status` and execution under
    // `execution_status`. The DB column `task.status` continues to mean execution;
    // `task.lifecycle_status` carries the v0.2 lifecycle.
    pairs.push(("status", V::String(task.lifecycle_status.clone()))); // v0.2 lifecycle
    pairs.push(("execution_status", V::String(task.status.clone()))); // v0.2 execution

    // OKF v0.2 fields — emitted only when populated (per upstream §4.7).
    if let Some(ms) = task.stale_after {
        let date = crate::okf::timefmt::utc_date_from_ms(ms);
        pairs.push(("stale_after", V::String(date))); // YYYY-MM-DD
    }
    if let Some(actor) = &task.generated_by {
        pairs.push((
            "generated",
            V::String(format!(
                "{{ by: {}, at: {} }}",
                serialize_actor_string(actor),
                iso_from_ms(task.updated_at),
            )),
        ));
    }
    if let Some(verified_json) = &task.okf_verified {
        if !verified_json.is_empty() && verified_json != "[]" {
            let flow = json_array_to_flow_sequence(verified_json, "verified")
                .unwrap_or_else(|| format!("[{verified_json}]"));
            pairs.push(("verified", V::String(flow)));
        }
    }
    if let Some(sources_json) = &task.okf_sources {
        if !sources_json.is_empty() && sources_json != "[]" {
            let flow = json_array_to_flow_sequence(sources_json, "sources")
                .unwrap_or_else(|| format!("[{sources_json}]"));
            pairs.push(("sources", V::String(flow)));
        }
    }
    if let Some(window) = &task.okf_usage_window {
        pairs.push((
            "usage_window",
            V::String(flow_mapping_from_json(window, "usage_window").unwrap_or_else(|| window.clone())),
        ));
    }

    let refs: Vec<(&str, &str)> = related
        .iter()
        .map(|(t, p)| (t.as_str(), p.as_str()))
        .collect();
    let body = append_related_section("", &refs);
    format!("{}\n{}", serialize_pairs_with_flow(&pairs), body)
}

pub fn parse_task_file(content: &str) -> Result<ParsedTask> {
    let (fm, raw_body) = parse_concept(content);
    let (_body, related) = split_related_section(&raw_body);

    // v0.1 → v0.2 fallback for tasks (mirror of the fact fallback).
    // Per upstream §4.8, when both `generated.at` and `timestamp` are present,
    // `generated.at` wins — Task 1 already routes timestamp → updated_at, so no change needed.

    // v0.2 status-rename rule (upstream §2.3):
    // - profile-2 wire format: `status` = lifecycle; `execution_status` = execution
    // - profile-1 wire format: `status` = execution; lifecycle defaults to "stable"
    let is_profile_v2 =
        fm.get_str("status").is_some() && fm.get_str("execution_status").is_some();
    let (execution_status, lifecycle_status) = if is_profile_v2 {
        (
            fm.get_str("execution_status")
                .unwrap_or("pending")
                .to_string(),
            fm.get_str("status").unwrap_or("stable").to_string(),
        )
    } else {
        (
            fm.get_str("status").unwrap_or("pending").to_string(),
            "stable".to_string(), // profile-1 default per upstream §2.3
        )
    };

    let task = WikiTask {
        id: fm.get_str("id").context("task file missing id")?.to_string(),
        entity_id: fm
            .get_str("entity_id")
            .context("task file missing entity_id")?
            .to_string(),
        description: fm.get_str("title").unwrap_or_default().to_string(),
        status: execution_status,
        priority: fm.get_number("priority").map(|n| n as i64).unwrap_or(0),
        created_at: fm.get_number("created_at").map(|n| n as i64).unwrap_or(0),
        updated_at: fm.get_str("timestamp").and_then(ms_from_iso).unwrap_or(0),
        resolved_at: fm.get_number("resolved_at").map(|n| n as i64),
        deleted_at: fm.get_number("deleted_at").map(|n| n as i64),
        okf_type: fm
            .get_str("type")
            .filter(|t| !t.is_empty() && *t != "task")
            .map(str::to_string),
        lifecycle_status,
        stale_after: parse_stale_after_ms_task(&fm),
        generated_by: parse_generated_by_task(&fm),
        okf_sources: fm.get_str("sources").map(str::to_string),
        okf_verified: fm.get_str("verified").map(str::to_string),
        okf_usage_window: fm.get_str("usage_window").map(str::to_string),
        last_verified_at: latest_verified_at_task(&fm),
        last_verified_by: latest_verified_by_task(&fm),
    };
    Ok(ParsedTask { task, related })
}

/// Parse OKF v0.2 `stale_after: YYYY-MM-DD` to epoch ms (UTC midnight).
fn parse_stale_after_ms_task(fm: &crate::okf::types::OkfFrontmatter) -> Option<i64> {
    let s = fm.get_str("stale_after")?;
    crate::okf::timefmt::ms_from_utc_date(s)
}

/// Parse the v0.2 `generated: { by: ..., at: ... }` flow mapping to the actor string.
fn parse_generated_by_task(fm: &crate::okf::types::OkfFrontmatter) -> Option<String> {
    let value = fm.fields.get("generated")?;
    let crate::okf::types::OkfFrontmatterValue::String(s) = value else { return None; };
    let _bytes = s.as_bytes();
    let by_idx = s.find("by:")?;
    let rest = &s[by_idx + 3..];
    let trimmed = rest.trim_start();
    let trimmed = trimmed.trim_start_matches(|c: char| c == '"' || c == '\'' || c.is_whitespace());
    let end = trimmed.find(|c: char| c == ',' || c == '}' || c == '"' || c == '\'').unwrap_or(trimmed.len());
    let actor = &trimmed[..end];
    if actor.is_empty() { None } else { Some(actor.to_string()) }
}

fn latest_verified_at_task(fm: &crate::okf::types::OkfFrontmatter) -> Option<i64> {
    let _ = fm;
    None
}
fn latest_verified_by_task(fm: &crate::okf::types::OkfFrontmatter) -> Option<String> {
    let _ = fm;
    None
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
        // v0.2 fields are now emitted by default (Task 5) AND v0.2's
        // status-rename rule changes the wire shape (`status` = lifecycle,
        // `execution_status` = execution) — the golden-v1 fixture predates
        // the rename. Strip v0.2 lines from both sides so the byte-comparison
        // against the fixture stays meaningful.
        let stripped = strip_v02_lines(&rebuilt);
        let stripped_fixture = strip_v02_lines(GOLDEN_TASK);
        assert_eq!(
            format!("{}\n", stripped.trim_end()),
            format!("{}\n", stripped_fixture.trim_end())
        );
    }

    fn strip_v02_lines(s: &str) -> String {
        s.lines()
            .filter(|line| {
                !line.starts_with("status:")
                    && !line.starts_with("execution_status:")
                    && !line.starts_with("stale_after:")
                    && !line.starts_with("generated:")
                    && !line.starts_with("verified:")
                    && !line.starts_with("sources:")
                    && !line.starts_with("usage_window:")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn builds_task_with_v02_fields() {
        let task = WikiTask {
            id: "task_x".into(),
            entity_id: "ent_demo".into(),
            description: "Description".into(),
            status: "pending".into(),
            priority: 1,
            created_at: 1719835800000,
            updated_at: 1719835800000,
            resolved_at: None,
            deleted_at: None,
            okf_type: None,
            lifecycle_status: "stable".into(),
            stale_after: Some(crate::okf::timefmt::ms_from_utc_date("2027-01-01").unwrap()),
            generated_by: Some("human:alice".into()),
            okf_sources: Some(r#"[{"resource":"documents/notes.md"}]"#.into()),
            okf_verified: Some(r#"[{"by":"process:nightly","at":"2026-07-02T00:00:00.000Z"}]"#.into()),
            okf_usage_window: Some(r#"{"from":"2026-07-01","to":"2026-12-31"}"#.into()),
            last_verified_at: Some(crate::okf::timefmt::ms_from_iso("2026-07-02T00:00:00.000Z").unwrap()),
            last_verified_by: Some("process:nightly".into()),
        };
        let md = build_task_file(&task, &[]);
        // v0.2 rename rule (upstream §2.3):
        assert!(md.contains("status: stable"), "missing lifecycle status: {md}");
        assert!(md.contains("execution_status: pending"), "missing execution status: {md}");
        assert!(md.contains("stale_after: 2027-01-01"), "missing stale_after: {md}");
        assert!(md.contains("generated: { by: \"human:alice\""), "missing generated flow mapping: {md}");
        assert!(md.contains("verified:"), "missing verified key: {md}");
        assert!(md.contains("sources:"), "missing sources key: {md}");
        assert!(md.contains("usage_window: { from: \"2026-07-01\", to: \"2026-12-31\" }"), "missing usage_window: {md}");
        // Round-trip: parse what we just emitted
        let parsed = parse_task_file(&md).unwrap();
        assert_eq!(parsed.task.lifecycle_status, "stable");
        assert_eq!(parsed.task.generated_by.as_deref(), Some("human:alice"));
    }
}
