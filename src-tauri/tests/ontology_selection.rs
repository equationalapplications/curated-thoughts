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
    fs::write(&p.config_path, r#"{"vault_path":"/tmp/v","migrated_to_v2":true}"#).unwrap();

    let cfg = BrainConfig::load(&p).expect("load succeeds");
    assert_eq!(cfg.ontology.schema, None);
}

#[test]
fn selection_round_trips_through_write() {
    let temp = TempDir::new().unwrap();
    let p = paths(&temp);
    fs::write(&p.config_path, r#"{"vault_path":"/tmp/v","migrated_to_v2":true}"#).unwrap();

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