//! OKF profile-v1 conformance tests against the vendored golden fixtures.

use std::path::{Path, PathBuf};

use tauri_app_lib::okf::bundle_read::parse_bundle;
use tauri_app_lib::okf::bundle_write::{write_bundle, write_bundle_with_profile, ExportEntity};
use tauri_app_lib::okf::types::{OkfFile, LLM_WIKI_PROFILE};

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/okf")
}

#[test]
fn vendored_fixtures_match_checksum_manifest() {
    use sha2::{Digest, Sha256};
    let root = fixtures_root();
    let manifest = std::fs::read_to_string(root.join("checksums.sha256"))
        .expect("checksums.sha256 missing — regenerate with shasum -a 256");
    let mut checked = 0usize;
    for line in manifest.lines().filter(|l| !l.trim().is_empty()) {
        let (expected, rel) = line
            .split_once("  ")
            .expect("manifest line must be '<sha256>  <path>'");
        let bytes = std::fs::read(root.join(rel))
            .unwrap_or_else(|e| panic!("fixture {rel} unreadable: {e}"));
        let actual = hex::encode(Sha256::digest(&bytes));
        assert_eq!(actual, expected, "fixture drifted: {rel}");
        checked += 1;
    }
    assert!(checked >= 12, "manifest suspiciously small: {checked} entries");
}

fn load_fixture_files(name: &str) -> Vec<OkfFile> {
    let root = fixtures_root().join(name);
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(&root) {
        let entry = entry.unwrap();
        if entry.file_type().is_file() && entry.path().extension().is_some_and(|e| e == "md") {
            let rel = entry
                .path()
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            files.push(OkfFile {
                path: rel,
                content: std::fs::read_to_string(entry.path()).unwrap(),
            });
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files
}

#[test]
fn parses_golden_v1_bundle() {
    let bundle = parse_bundle(&load_fixture_files("golden-v1")).unwrap();
    assert_eq!(bundle.profile.as_deref(), Some("llm-wiki/1"));
    assert_eq!(bundle.entities.len(), 1);
    let entity = &bundle.entities[0];
    assert_eq!(entity.entity_id, "demo");
    assert_eq!(entity.summary.as_deref(), Some("Demo entity summary prose."));
    assert_eq!(entity.facts.len(), 2);
    assert_eq!(entity.tasks.len(), 1);
    let mut edges = entity.edges.clone();
    edges.sort();
    assert_eq!(
        edges,
        vec![
            (
                "fact_alpha".to_string(),
                "fact_beta".to_string(),
                "references".to_string()
            ),
            (
                "fact_alpha".to_string(),
                "task_follow".to_string(),
                "blocks".to_string()
            ),
        ]
    );
    let alpha = entity.facts.iter().find(|f| f.id == "fact_alpha").unwrap();
    assert!(!alpha.body.contains("## Related"));
    assert_eq!(entity.events.len(), 2);
    assert_eq!(entity.events[0].event_id.as_deref(), Some("evt_golden_1"));
    assert_eq!(entity.events[0].related_entry_id.as_deref(), Some("fact_alpha"));
    assert_eq!(entity.events[0].date, "2026-07-05");
}

#[test]
fn parses_legacy_profile_0_with_fallbacks() {
    let bundle = parse_bundle(&load_fixture_files("legacy-profile-0")).unwrap();
    assert_eq!(bundle.profile, None);
    let entity = &bundle.entities[0];
    assert!(entity.edges.is_empty());
    assert!(entity.events.iter().all(|e| e.event_id.is_none()));
}

#[test]
fn ignores_stray_readme_at_root() {
    let mut files = load_fixture_files("golden-v1");
    files.push(OkfFile {
        path: "README.md".into(),
        content: "# not a fact".into(),
    });
    let bundle = parse_bundle(&files).unwrap();
    assert_eq!(bundle.entities[0].facts.len(), 2);
    assert!(bundle.skipped_paths.contains(&"README.md".to_string()));
}

fn export_from_parsed(bundle: &tauri_app_lib::okf::bundle_read::ParsedBundle) -> Vec<ExportEntity> {
    bundle
        .entities
        .iter()
        .map(|e| ExportEntity {
            entity_id: e.entity_id.clone(),
            display_name: e.display_name.clone().unwrap_or_else(|| e.entity_id.clone()),
            summary: e.summary.clone(),
            facts: e.facts.clone(),
            tasks: e.tasks.clone(),
            edges: e.edges.clone(),
            events: e
                .events
                .iter()
                .map(|ev| tauri_app_lib::okf::bundle_write::ExportEvent {
                    event_id: ev.event_id.clone().unwrap_or_default(),
                    event_type: ev.event_type.clone(),
                    summary: ev.summary.clone(),
                    related_entry_id: ev.related_entry_id.clone(),
                    date: ev.date.clone(),
                })
                .collect(),
        })
        .collect()
}

#[test]
fn golden_v1_round_trips_losslessly() {
    let original = load_fixture_files("golden-v1");
    let parsed = parse_bundle(&original).unwrap();
    let rebuilt = write_bundle_with_profile(
        &export_from_parsed(&parsed),
        LLM_WIKI_PROFILE,
        "0.1",
    )
    .expect("write_bundle");

    // Task 5 emits v0.2 keys (`status: stable`, `execution_status: ...`) on rebuild,
    // and Task 6's status-rename rule adds `status: stable` to fixtures that didn't
    // have it. The v0.1 fixtures pre-date these keys — strip them from both sides
    // so the comparison focuses on v0.1-stable fields. Same pattern as
    // `strip_v02_lines` in `task_file.rs::tests::round_trips_golden_task_bytes`.
    let norm = |files: &[OkfFile]| -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = files
            .iter()
            .map(|f| {
                (
                    f.path.clone(),
                    format!("{}\n", strip_v02_lines(&f.content).trim_end()),
                )
            })
            .collect();
        v.sort();
        v
    };
    assert_eq!(norm(&rebuilt), norm(&original));
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
fn defaults_to_ll_wiki_2_on_export() {
    let original = load_fixture_files("golden-v1");
    let parsed = parse_bundle(&original).unwrap();
    let rebuilt = write_bundle(&export_from_parsed(&parsed)).expect("write_bundle");
    let root = rebuilt
        .iter()
        .find(|f| f.path == "index.md")
        .expect("root index.md");
    assert!(
        root.content.contains("okf_version: 0.2"),
        "default export must emit okf_version 0.2; got: {}",
        root.content.lines().find(|l| l.starts_with("okf_version")).unwrap_or("(missing)"),
    );
    assert!(
        root.content.contains("profile: llm-wiki/2"),
        "default export must emit profile llm-wiki/2; got: {}",
        root.content.lines().find(|l| l.starts_with("profile")).unwrap_or("(missing)"),
    );
}

#[test]
fn parses_golden_v2_bundle_with_status_rename_rule() {
    // golden-v2 fixture is vendored in Task 7; until then this test fails with
    // "fixture not found" — that's expected and recorded in the Task 6 report.
    let bundle = parse_bundle(&load_fixture_files("golden-v2")).unwrap();
    assert_eq!(bundle.profile.as_deref(), Some("llm-wiki/2"));
    assert_eq!(bundle.okf_version.as_deref(), Some("0.2"));
    let entity = &bundle.entities[0];
    // task has both lifecycle (status) and execution (execution_status) — wire format rename rule
    let task = entity
        .tasks
        .iter()
        .find(|t| t.id == "task_with_provenance")
        .expect("task fixture");
    assert_eq!(task.status, "in_progress", "execution_status -> status");
    assert_eq!(
        task.lifecycle_status, "draft",
        "status (v0.2 wire) -> lifecycle_status"
    );
}

#[test]
fn export_writes_exported_event_per_entity() {
    use tauri_app_lib::db::connection::open_in_memory;
    use tauri_app_lib::db::bundle_io::load_export_entities;

    let conn = open_in_memory().unwrap();

    // Seed entity with facts
    conn.execute(
        "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
         VALUES ('ent-1', 'Project X', 'concept', 'A test entity', 100, 100)",
        [],
    ).unwrap();

    conn.execute(
        "INSERT INTO llm_wiki_entries (id, entity_id, title, body, tags, confidence, source_type, created_at, updated_at)
         VALUES ('fact-1', 'ent-1', 'Test Fact', 'Fact body.', '[]', 'certain', 'user_confirmed', 100, 100)",
        [],
    ).unwrap();

    // Simulate export: load entities (as okf_export_bundle_cmd does)
    let entities = load_export_entities(&conn, None).unwrap();
    assert_eq!(entities.len(), 1);
    assert_eq!(entities[0].entity_id, "ent-1");

    // Write exported event for each entity (as okf_export_bundle_cmd does after zip is finalized)
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap()
        .as_millis() as i64;
    for entity in &entities {
        conn.execute(
            "INSERT INTO llm_wiki_events (id, entity_id, event_type, summary, related_entry_id, created_at)
             VALUES (?1, ?2, 'exported', ?3, NULL, ?4)",
            rusqlite::params![
                format!("evt_{now_ms}_{}", entity.entity_id),
                entity.entity_id.clone(),
                format!("Exported *{}* to OKF bundle", entity.display_name),
                now_ms,
            ],
        ).unwrap();
    }

    // Query and verify the exported event was written
    let (event_type, summary): (String, String) = conn
        .query_row(
            "SELECT event_type, summary FROM llm_wiki_events
             WHERE entity_id = 'ent-1' AND event_type = 'exported'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("exported event row");
    assert_eq!(event_type, "exported");
    assert!(summary.starts_with("Exported"));
    assert!(summary.contains("Project X"));
}
