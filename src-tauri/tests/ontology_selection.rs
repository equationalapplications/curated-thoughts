//! Round-trip tests for the `ontology` block in config.json.

use std::fs;
use tauri_app_lib::config::BrainConfig;
use tauri_app_lib::ontology_config::OntologySelection;
use tauri_app_lib::retrieval::BrainPaths;
use tempfile::TempDir;

fn paths(temp: &TempDir) -> BrainPaths {
    BrainPaths {
        brain_dir: temp.path().to_path_buf(),
        config_path: temp.path().join("config.json"),
        db_path: temp.path().join("brain.db"),
    }
}

#[test]
fn absent_ontology_block_loads_as_none() {
    let temp = TempDir::new().unwrap();
    let p = paths(&temp);
    fs::write(
        &p.config_path,
        r#"{"vault_path":"/tmp/v","migrated_to_v2":true}"#,
    )
    .unwrap();

    let cfg = BrainConfig::load(&p).expect("load succeeds");
    assert_eq!(cfg.ontology.schema, None);
}

#[test]
fn selection_round_trips_through_write() {
    let temp = TempDir::new().unwrap();
    let p = paths(&temp);
    fs::write(
        &p.config_path,
        r#"{"vault_path":"/tmp/v","migrated_to_v2":true}"#,
    )
    .unwrap();

    let mut cfg = BrainConfig::load(&p).unwrap();
    cfg.ontology.schema = Some(OntologySelection::SchemaSoftwareOrg);
    cfg.write(&p).expect("write succeeds");

    let text = fs::read_to_string(&p.config_path).unwrap();
    assert!(
        text.contains(r#""schema": "schema-software-org""#),
        "serialized slug missing, got: {text}"
    );
    let reloaded = BrainConfig::load(&p).unwrap();
    assert_eq!(
        reloaded.ontology.schema,
        Some(OntologySelection::SchemaSoftwareOrg)
    );
}

#[test]
fn unknown_keys_inside_ontology_block_survive_a_write_cycle() {
    let temp = TempDir::new().unwrap();
    let p = paths(&temp);
    fs::write(
        &p.config_path,
        r#"{"vault_path":"/tmp/v","migrated_to_v2":true,"ontology":{"schema":"emergent","future_key":42}}"#,
    )
    .unwrap();

    let cfg = BrainConfig::load(&p).unwrap();
    assert_eq!(cfg.ontology.schema, Some(OntologySelection::Emergent));
    cfg.write(&p).unwrap();

    let text = fs::read_to_string(&p.config_path).unwrap();
    assert!(text.contains("future_key"), "unknown key dropped: {text}");
}

#[test]
fn unparseable_selection_is_lenient_not_fatal() {
    let temp = TempDir::new().unwrap();
    let p = paths(&temp);
    fs::write(
        &p.config_path,
        r#"{"vault_path":"/tmp/v","migrated_to_v2":true,"ontology":{"schema":"not-a-real-schema"}}"#,
    )
    .unwrap();

    let cfg = BrainConfig::load(&p).expect("bad selection must not be fatal");
    assert_eq!(cfg.ontology.schema, None);
}

/// `load_lenient`'s report must distinguish "ontology block present but
/// unparseable" from "ontology block absent" — the Tauri
/// `get_ontology_selection` command uses this flag to propagate the parse
/// failure instead of masking it as the desktop default (CodeRabbit review
/// on PR #124: `{"ontology":{"schema":"unknown"}}` must not silently start
/// the General ontology).
#[test]
fn load_lenient_flags_unparseable_ontology_block() {
    let temp = TempDir::new().unwrap();
    let p = paths(&temp);
    fs::write(
        &p.config_path,
        r#"{"vault_path":"/tmp/v","migrated_to_v2":true,"ontology":{"schema":"not-a-real-schema"}}"#,
    )
    .unwrap();

    let report = BrainConfig::load_lenient(&p).expect("lenient load succeeds");
    assert!(
        report.ontology_unparseable,
        "unparseable ontology block must be flagged, diagnostics: {:?}",
        report.diagnostics
    );
    assert_eq!(report.config.ontology.schema, None);
}

/// An absent `ontology` block (never chosen) must NOT set the unparseable
/// flag — that's the "use the desktop default" case, distinct from a
/// present-but-invalid block.
#[test]
fn load_lenient_does_not_flag_absent_ontology_block() {
    let temp = TempDir::new().unwrap();
    let p = paths(&temp);
    fs::write(
        &p.config_path,
        r#"{"vault_path":"/tmp/v","migrated_to_v2":true}"#,
    )
    .unwrap();

    let report = BrainConfig::load_lenient(&p).expect("lenient load succeeds");
    assert!(!report.ontology_unparseable);
    assert_eq!(report.config.ontology.schema, None);
}
