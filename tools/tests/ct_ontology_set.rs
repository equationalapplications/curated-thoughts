//! `ct ontology set` — spec 2026-10-03 §2.11 CLI contract.
//!
//! Contract:
//! - Bare `--mode` (no --entity/--dir) writes `ingest.ontology_default` in
//!   config (NOT a manifest row); `--dir <prefix>` writes the
//!   `ingest.folder_ontology` map; one target per invocation.
//! - `--entity <id> --mode off` writes a `ct_entity_optouts` row (D8);
//!   `--entity <id> --mode strict` DELETES it in the same transaction as a
//!   strict manifest-ROW write resolved verbatim from `tier_fact`
//!   (r13-MAJOR-2 / r13-m4).
//! - Degraded config → loud refusal, exit 1, no write (r3-M3).
//! - No-manifest brain → warning, write still recorded (§2.11).

use std::process::{Command, Output};

use temp_env::with_vars;
use tempfile::tempdir;

mod common;

use common::init_brain_db;

fn run_ct(dir: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ct"))
        .env("CURATED_BRAIN_DIR", dir)
        .env_remove("CURATED_BRAIN_DB")
        .env_remove("CURATED_BRAIN_CONFIG")
        .args(args)
        .output()
        .unwrap()
}

fn with_brain<F: FnOnce(&std::path::Path)>(f: F) {
    let brain = tempdir().unwrap();
    let dir = brain.path().to_path_buf();
    let dir_str = dir.to_str().unwrap().to_string();
    with_vars([("CURATED_BRAIN_DIR", Some(dir_str.as_str()))], move || {
        init_brain_db(&dir);
        f(&dir);
    });
}

fn parse_stdout_object(out: &Output) -> serde_json::Value {
    let text = String::from_utf8_lossy(&out.stdout);
    let start = text.find('{').expect("stdout must carry a JSON object");
    serde_json::from_str(text[start..].trim()).expect("valid JSON object on stdout")
}

fn seed_tier_fact_manifest(dir: &std::path::Path) {
    let conn = rusqlite::Connection::open(dir.join("brain.db")).unwrap();
    let manifest = r#"{"node_types":[{"type":"concept"},{"type":"document"},{"type":"process"}],"edge_types":[],"fallback_node_type":"document"}"#;
    conn.execute(
        "INSERT INTO llm_wiki_entity_manifests (entity_id, mode, manifest_json, updated_at)
         VALUES ('tier_fact', 'strict', ?1, 1)",
        [manifest],
    )
    .unwrap();
}

fn optout_count(dir: &std::path::Path, id: &str) -> i64 {
    let conn = rusqlite::Connection::open(dir.join("brain.db")).unwrap();
    conn.query_row(
        "SELECT COUNT(*) FROM ct_entity_optouts WHERE entity_id = ?1",
        [id],
        |r| r.get(0),
    )
    .unwrap()
}

#[test]
fn bare_mode_writes_config_default_not_a_manifest_row() {
    with_brain(|dir| {
        let out = run_ct(dir, &["ontology", "set", "--mode", "strict"]);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let v = parse_stdout_object(&out);
        assert_eq!(v["config_written"], true, "{v}");
        assert_eq!(v["manifest_row_written"], false, "{v}");
        let cfg = std::fs::read_to_string(dir.join("config.json")).unwrap();
        assert!(cfg.contains("\"ontology_default\""), "{cfg}");
        // Explicitly NOT a manifest row (§2.11).
        let rows: i64 = rusqlite::Connection::open(dir.join("brain.db"))
            .unwrap()
            .query_row("SELECT COUNT(*) FROM llm_wiki_entity_manifests", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(rows, 0, "bare --mode must not create a manifest row");
    });
}

#[test]
fn dir_mode_writes_the_folder_ontology_map() {
    with_brain(|dir| {
        let out = run_ct(
            dir,
            &[
                "ontology",
                "set",
                "--mode",
                "off",
                "--dir",
                "documents/drafts",
            ],
        );
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let cfg = std::fs::read_to_string(dir.join("config.json")).unwrap();
        assert!(cfg.contains("documents/drafts"), "{cfg}");
    });
}

#[test]
fn entity_and_dir_are_rejected() {
    with_brain(|dir| {
        let out = run_ct(
            dir,
            &[
                "ontology", "set", "--mode", "off", "--entity", "ent_x", "--dir", "d",
            ],
        );
        assert_eq!(out.status.code(), Some(1));
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            err.contains("mutually exclusive") || err.contains("cannot be used"),
            "{err}"
        );
    });
}

/// §2.11 matrix: off → strict → the NEXT mint is gated (the reversal rule
/// deletes the opt-out row; the gate verdict flip is pinned lib-side in
/// `db/ontology_set.rs`; here we pin the CLI-visible row state).
#[test]
fn entity_off_then_strict_reversal() {
    with_brain(|dir| {
        seed_tier_fact_manifest(dir);
        let out = run_ct(
            dir,
            &["ontology", "set", "--mode", "off", "--entity", "ent_x"],
        );
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(optout_count(dir, "ent_x"), 1);
        let v = parse_stdout_object(&out);
        assert_eq!(v["optout_written"], true, "{v}");

        let out = run_ct(
            dir,
            &["ontology", "set", "--mode", "strict", "--entity", "ent_x"],
        );
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(optout_count(dir, "ent_x"), 0, "r13-MAJOR-2 reversal");
        let v = parse_stdout_object(&out);
        assert_eq!(v["optout_deleted"], true, "{v}");
        assert_eq!(v["manifest_row_written"], true, "{v}");
        // r13-m4: node_types + fallback copied verbatim from tier_fact.
        let row: String = rusqlite::Connection::open(dir.join("brain.db"))
            .unwrap()
            .query_row(
                "SELECT manifest_json FROM llm_wiki_entity_manifests WHERE entity_id = 'ent_x'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            row.contains("\"concept\""),
            "verbatim copy incl. concept: {row}"
        );
        assert!(row.contains("fallback_node_type"), "{row}");
    });
}

#[test]
fn degraded_config_refuses_loudly() {
    with_brain(|dir| {
        std::fs::write(dir.join("config.json"), b"{not json").unwrap();
        let out = run_ct(dir, &["ontology", "set", "--mode", "strict"]);
        assert_eq!(out.status.code(), Some(1));
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("DEGRADED"), "{err}");
        // Nothing written.
        assert!(
            String::from_utf8_lossy(&out.stdout).trim().is_empty(),
            "refusal must leave stdout empty"
        );
    });
}

#[test]
fn no_manifest_rows_warns() {
    with_brain(|dir| {
        let out = run_ct(
            dir,
            &["ontology", "set", "--mode", "off", "--entity", "ent_x"],
        );
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("NO manifest rows"), "{err}");
        let v = parse_stdout_object(&out);
        assert!(v["warning"].is_string(), "{v}");
    });
}

#[test]
fn fallback_writes_into_tier_fact_manifest() {
    with_brain(|dir| {
        seed_tier_fact_manifest(dir);
        let out = run_ct(
            dir,
            &[
                "ontology",
                "set",
                "--mode",
                "strict",
                "--fallback",
                "document",
            ],
        );
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let v = parse_stdout_object(&out);
        assert_eq!(v["fallback_written"], true, "{v}");
        let row: String = rusqlite::Connection::open(dir.join("brain.db"))
            .unwrap()
            .query_row(
                "SELECT manifest_json FROM llm_wiki_entity_manifests WHERE entity_id = 'tier_fact'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(row.contains("fallback_node_type"), "{row}");
    });
}
