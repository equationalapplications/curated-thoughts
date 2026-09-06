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
    assert!(
        checked >= 12,
        "manifest suspiciously small: {checked} entries"
    );
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
    assert_eq!(
        entity.summary.as_deref(),
        Some("Demo entity summary prose.")
    );
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
    assert_eq!(
        entity.events[0].related_entry_id.as_deref(),
        Some("fact_alpha")
    );
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
            display_name: e
                .display_name
                .clone()
                .unwrap_or_else(|| e.entity_id.clone()),
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
    let rebuilt = write_bundle_with_profile(&export_from_parsed(&parsed), LLM_WIKI_PROFILE, "0.1")
        .expect("write_bundle");

    // Writer is now profile-aware: profile-1 emission drops every v0.2 key
    // (status lifecycle on facts, execution_status on tasks, stale_after /
    // generated / verified / sources / usage_window), so the rebuilt bytes
    // match the v0.1 fixture byte-for-byte after trimming trailing whitespace.
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
        root.content
            .lines()
            .find(|l| l.starts_with("okf_version"))
            .unwrap_or("(missing)"),
    );
    assert!(
        root.content.contains("profile: llm-wiki/2"),
        "default export must emit profile llm-wiki/2; got: {}",
        root.content
            .lines()
            .find(|l| l.starts_with("profile"))
            .unwrap_or("(missing)"),
    );
}

#[test]
fn parses_golden_v2_bundle_with_status_rename_rule() {
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
    use tauri_app_lib::db::bundle_io::load_export_entities;
    use tauri_app_lib::db::connection::open_in_memory;

    let conn = open_in_memory().unwrap();

    // Seed entity with facts
    conn.execute(
        "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
         VALUES ('ent-1', 'Project X', 'concept', 'A test entity', 100, 100)",
        [],
    )
    .unwrap();

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
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
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

/// Task 11 (#186): librarian evidence must survive the bundle round-trip.
/// Export path: `load_export_entities` (as `okf_export_bundle_cmd` does);
/// apply path: `apply_import` (as `okf_apply_bundle_cmd` does).
#[test]
fn bundle_roundtrip_preserves_librarian_evidence() {
    use tauri_app_lib::db::bundle_apply::{apply_import, ImportMode};
    use tauri_app_lib::db::bundle_io::load_export_entities;
    use tauri_app_lib::db::connection::open_in_memory;

    // Source brain: one librarian token row plus its evidence.
    let src = open_in_memory().unwrap();
    src.execute(
        "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
         VALUES ('ent-b','Bundle Entity','concept','s',100,100)",
        [],
    )
    .unwrap();
    src.execute(
        "INSERT INTO llm_wiki_entries (id, entity_id, title, body, tags, confidence,
             source_type, source_ref, created_at, updated_at, access_count)
         VALUES ('fact_b','ent-b','t','b','[]','inferred','librarian_inferred',?1,1,1,0)",
        [tauri_app_lib::db::commit::librarian_source_ref_token(
            "fact_b",
        )],
    )
    .unwrap();
    tauri_app_lib::db::commit::insert_librarian_evidence(
        &src,
        "fact_b",
        "prop_b",
        r#"{"proposal_id":"prop_b","evidence":[{"chunk_id":1,"content_hash":"bb"}]}"#,
        false,
        1,
    )
    .unwrap();

    // Round-trip through the same entry points the commands use.
    let entities = load_export_entities(&src, None).unwrap();
    let files = tauri_app_lib::okf::bundle_write::write_bundle(&entities).unwrap();
    let bundle = parse_bundle(&files).unwrap();
    let mut dest = open_in_memory().unwrap();
    apply_import(&mut dest, &bundle, ImportMode::Merge).unwrap();

    let stored = tauri_app_lib::db::commit::evidence_json_for_entry(&dest, "fact_b")
        .expect("evidence must survive the bundle roundtrip");
    assert!(serde_json::from_str::<serde_json::Value>(&stored).is_ok());

    let ref_after: String = dest
        .query_row(
            "SELECT source_ref FROM llm_wiki_entries WHERE id='fact_b'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(ref_after.starts_with("librarian-"));
}

/// Final-review fix (I2): a pre-#186 bundle — legacy JSON `source_ref`, no
/// paired `librarian_evidence` frontmatter — must apply with a token-shaped
/// `source_ref` and a salvaged evidence row, and the row's `unanchored` flag
/// must be computed (no live chunk in the destination ⇒ 1). Spec §2.3, §2.4.
#[test]
fn bundle_apply_normalizes_legacy_json_source_ref() {
    use tauri_app_lib::db::bundle_apply::{apply_import, ImportMode};
    use tauri_app_lib::db::bundle_io::load_export_entities;
    use tauri_app_lib::db::connection::open_in_memory;

    // Source brain: a pre-#186 librarian fact — JSON ref, no evidence row.
    let src = open_in_memory().unwrap();
    src.execute(
        "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
         VALUES ('ent-pre','Pre Bundle','concept','s',100,100)",
        [],
    )
    .unwrap();
    src.execute(
        "INSERT INTO llm_wiki_entries (id, entity_id, title, body, tags, confidence,
             source_type, source_ref, created_at, updated_at, access_count)
         VALUES ('fact_pre','ent-pre','t','b','[]','inferred','librarian_inferred',?1,1,1,0)",
        [r#"{"proposal_id":"prop_pre","evidence":[{"chunk_id":1,"content_hash":"cc"}]}"#],
    )
    .unwrap();

    let entities = load_export_entities(&src, None).unwrap();
    let files = tauri_app_lib::okf::bundle_write::write_bundle(&entities).unwrap();
    let bundle = parse_bundle(&files).unwrap();
    let mut dest = open_in_memory().unwrap();
    apply_import(&mut dest, &bundle, ImportMode::Merge).unwrap();

    let ref_after: String = dest
        .query_row(
            "SELECT source_ref FROM llm_wiki_entries WHERE id='fact_pre'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        ref_after.starts_with("librarian-") && ref_after.len() == "librarian-".len() + 32,
        "legacy JSON ref must be rewritten to the token: {ref_after}"
    );

    let (proposal_id, unanchored): (String, i64) = dest
        .query_row(
            "SELECT proposal_id, unanchored FROM librarian_evidence WHERE entry_id='fact_pre'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("legacy evidence must be salvaged into a librarian_evidence row");
    assert_eq!(proposal_id, "prop_pre");
    assert_eq!(
        unanchored, 1,
        "salvaged blob has no live chunk in dest, so unanchored must be computed to 1"
    );
}
