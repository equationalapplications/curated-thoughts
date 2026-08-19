//! Task concept documents (profile §5.2). Body is empty in profile 1;
//! non-empty foreign bodies are tolerated on parse and dropped (we have
//! no task-body field), which §5.2 permits.

use anyhow::{Context, Result};

use crate::okf::concept::parse_concept;
use crate::okf::fact_file::{
    flow_mapping_from_json, flow_to_json_string, json_array_to_flow_sequence,
    latest_verified_at, latest_verified_by, parse_generated_by, parse_stale_after_ms,
    serialize_actor_string, serialize_pairs_with_flow,
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

pub fn build_task_file(
    task: &WikiTask,
    related: &[(String, String)],
    profile: &str,
) -> String {
    // Wire shape is profile-specific. Field ordering matters because the
    // golden-v1 fixture is byte-for-byte; we preserve the v0.1 ordering
    // (`status` lives between `entity_id` and `priority`) on profile-1.
    // v0.2 reorders so lifecycle precedes the v0.2 provenance block — the
    // golden-v2 fixture will lock that order in (Task 7).
    let mut pairs: Vec<(&str, V)> = vec![
        (
            "type",
            V::String(task.okf_type.clone().unwrap_or_else(|| "task".into())),
        ),
        ("title", V::String(task.description.clone())),
        ("timestamp", V::String(iso_from_ms(task.updated_at))),
        ("id", V::String(task.id.clone())),
        ("entity_id", V::String(task.entity_id.clone())),
    ];

    if profile == "llm-wiki/1" {
        // profile-1: execution status under `status` only (per upstream §2.3);
        // field order matches the v0.1 golden fixture (status then priority,
        // created_at, resolved_at, deleted_at).
        pairs.push(("status", V::String(task.status.clone())));
        pairs.push(("priority", V::Number(task.priority as f64)));
        pairs.push(("created_at", V::Number(task.created_at as f64)));
        pairs.push(("resolved_at", opt_ms(task.resolved_at)));
        pairs.push(("deleted_at", opt_ms(task.deleted_at)));
    } else {
        // profile-2: status-rename rule (upstream §2.3).
        pairs.push(("priority", V::Number(task.priority as f64)));
        pairs.push(("created_at", V::Number(task.created_at as f64)));
        pairs.push(("resolved_at", opt_ms(task.resolved_at)));
        pairs.push(("deleted_at", opt_ms(task.deleted_at)));
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
    // Profile-2 detection requires BOTH keys present with non-empty values —
    // an empty `status:` (e.g. an unrelated convention) should not flip the
    // semantics, nor should a profile-1 task that happens to carry an
    // `execution_status:` custom field.
    let status_v = fm.get_str("status").filter(|s| !s.is_empty());
    let exec_v = fm.get_str("execution_status").filter(|s| !s.is_empty());
    let is_profile_v2 = status_v.is_some() && exec_v.is_some();
    let (execution_status, lifecycle_status) = if is_profile_v2 {
        (
            exec_v.unwrap().to_string(),
            status_v.unwrap().to_string(),
        )
    } else {
        (
            status_v.unwrap_or("pending").to_string(),
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
        stale_after: parse_stale_after_ms(&fm),
        generated_by: parse_generated_by(&fm),
        okf_sources: flow_to_json_string(fm.get_str("sources")),
        okf_verified: flow_to_json_string(fm.get_str("verified")),
        okf_usage_window: flow_to_json_string(fm.get_str("usage_window")),
        last_verified_at: latest_verified_at(&fm),
        last_verified_by: latest_verified_by(&fm),
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
        let rebuilt = build_task_file(&parsed.task, &[], "llm-wiki/1");
        assert_eq!(
            format!("{}\n", rebuilt.trim_end()),
            format!("{}\n", GOLDEN_TASK.trim_end())
        );
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
        let md = build_task_file(&task, &[], "llm-wiki/2");
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

    #[test]
    fn builds_task_v01_uses_status_for_execution() {
        let task = WikiTask {
            id: "task_x".into(),
            entity_id: "ent_demo".into(),
            description: "Description".into(),
            status: "in_progress".into(),
            priority: 1,
            created_at: 1719835800000,
            updated_at: 1719835800000,
            resolved_at: None,
            deleted_at: None,
            okf_type: None,
            lifecycle_status: "draft".into(),
            stale_after: Some(crate::okf::timefmt::ms_from_utc_date("2027-01-01").unwrap()),
            generated_by: Some("human:alice".into()),
            okf_sources: Some(r#"[{"resource":"documents/notes.md"}]"#.into()),
            okf_verified: Some(r#"[{"by":"process:nightly","at":"2026-07-02T00:00:00.000Z"}]"#.into()),
            okf_usage_window: Some(r#"{"from":"2026-07-01","to":"2026-12-31"}"#.into()),
            last_verified_at: Some(crate::okf::timefmt::ms_from_iso("2026-07-02T00:00:00.000Z").unwrap()),
            last_verified_by: Some("process:nightly".into()),
        };
        let md = build_task_file(&task, &[], "llm-wiki/1");
        // profile-1: `status` carries execution, no `execution_status`, no v0.2 keys.
        assert!(md.contains("status: in_progress"), "missing execution status under status: {md}");
        assert!(!md.contains("execution_status:"), "v0.1 must not emit execution_status: {md}");
        assert!(!md.contains("status: draft"), "v0.1 must not emit lifecycle_status under status: {md}");
        assert!(!md.contains("stale_after:"), "v0.1 must not emit stale_after: {md}");
        assert!(!md.contains("generated:"), "v0.1 must not emit generated: {md}");
        assert!(!md.contains("verified:"), "v0.1 must not emit verified: {md}");
        assert!(!md.contains("sources:"), "v0.1 must not emit sources: {md}");
        assert!(!md.contains("usage_window:"), "v0.1 must not emit usage_window: {md}");
    }
}
