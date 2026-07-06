//! OKF profile-v1 conformance tests against the vendored golden fixtures.

use std::path::{Path, PathBuf};

use tauri_app_lib::okf::bundle_read::parse_bundle;
use tauri_app_lib::okf::bundle_write::{write_bundle, ExportEntity};
use tauri_app_lib::okf::types::OkfFile;

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
    let rebuilt = write_bundle(&export_from_parsed(&parsed)).expect("write_bundle");

    let norm = |files: &[OkfFile]| -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = files
            .iter()
            .map(|f| (f.path.clone(), format!("{}\n", f.content.trim_end())))
            .collect();
        v.sort();
        v
    };
    assert_eq!(norm(&rebuilt), norm(&original));
}
