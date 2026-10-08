//! `ct wiki merge-duplicates` — spec 2026-10-03 §2.7 / R2.7.1 CLI contract.
//!
//! Contract:
//! - Without `--yes`: the READ-ONLY report prints (one JSON object on
//!   stdout) and the command exits 1 — report first, destructive pass
//!   behind `--yes`.
//! - `--yes` refuses (`alias_remap_not_run`, exit 1) until the signed-alias
//!   remap marker exists (R2.7.1).
//! - The R2.2.8 FINAL-RULE drift gate: unconfirmed drift blocks the merge;
//!   `--confirm-drift <old-hash>` / `--waive-drift <old-hash>` with the
//!   ECHOED old hash clear it; a mismatch is blocked loud. The merge NEVER
//!   writes the watermark.

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

/// One duplicate pair (R2.7.2 punctuation-normalized grouping): same name
/// modulo case/punctuation, both with NON-EMPTY EQUAL summaries (R2.7.6:
/// both-empty is not agreement). `ent_a` < `ent_b` byte-wise →
/// deterministic survivor (R2.7.3).
fn seed_duplicate_pair(dir: &std::path::Path) {
    let conn = rusqlite::Connection::open(dir.join("brain.db")).unwrap();
    for (id, name) in [("ent_a", "Adrian Smith"), ("ent_b", "adrian  smith!")] {
        conn.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, summary_embedding, created_at, updated_at, deleted_at)
             VALUES (?1, ?2, 'concept', 'same summary', NULL, 1, 1, NULL)",
            [id, name],
        )
        .unwrap();
    }
}

fn seed_marker(dir: &std::path::Path) {
    let conn = rusqlite::Connection::open(dir.join("brain.db")).unwrap();
    conn.execute(
        "INSERT INTO llm_wiki_meta (key, value) VALUES ('alias_remap_completed', '1')",
        [],
    )
    .unwrap();
}

fn seed_watermark(dir: &std::path::Path, hash: &str) {
    let conn = rusqlite::Connection::open(dir.join("brain.db")).unwrap();
    conn.execute(
        "INSERT INTO llm_wiki_meta (key, value)
         VALUES ('ontology_config_watermark', ?1)",
        [format!(r#"{{"hash":"{hash}","stamped_at":1}}"#)],
    )
    .unwrap();
}

fn meta_value(dir: &std::path::Path, key: &str) -> Option<String> {
    let conn = rusqlite::Connection::open(dir.join("brain.db")).unwrap();
    conn.query_row(
        "SELECT value FROM llm_wiki_meta WHERE key = ?1",
        [key],
        |r| r.get(0),
    )
    .ok()
}

fn redirect_count(dir: &std::path::Path) -> i64 {
    rusqlite::Connection::open(dir.join("brain.db"))
        .unwrap()
        .query_row("SELECT COUNT(*) FROM entity_redirects", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn report_without_yes_prints_report_and_exits_one() {
    with_brain(|dir| {
        seed_duplicate_pair(dir);
        seed_marker(dir);
        let out = run_ct(dir, &["wiki", "merge-duplicates"]);
        assert_eq!(out.status.code(), Some(1));
        let report = parse_stdout_object(&out);
        assert_eq!(report["groups"].as_array().unwrap().len(), 1, "{report}");
        assert_eq!(report["groups"][0]["survivor"], "ent_a", "{report}");
        assert_eq!(redirect_count(dir), 0, "report arm must not write");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("--yes"), "{err}");
    });
}

#[test]
fn yes_without_remap_marker_refuses() {
    with_brain(|dir| {
        seed_duplicate_pair(dir);
        let out = run_ct(dir, &["wiki", "merge-duplicates", "--yes"]);
        assert_eq!(out.status.code(), Some(1));
        let report = parse_stdout_object(&out);
        assert_eq!(report["skipped_reason"], "alias_remap_not_run", "{report}");
        assert_eq!(redirect_count(dir), 0);
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("heal --yes"), "{err}");
    });
}

/// Review finding: merge-duplicates refuses its destructive pass under a
/// degraded/tied config, the same posture as heal and `ct ontology set`
/// (plan-p7-m3) — not every degraded state moves the drift hash.
#[test]
fn yes_with_degraded_config_refuses() {
    with_brain(|dir| {
        seed_duplicate_pair(dir);
        seed_marker(dir);
        std::fs::write(dir.join("config.json"), b"{not json").unwrap();
        let out = run_ct(dir, &["wiki", "merge-duplicates", "--yes"]);
        assert_eq!(out.status.code(), Some(1));
        let report = parse_stdout_object(&out);
        assert_eq!(report["skipped_reason"], "degraded_config", "{report}");
        assert_eq!(redirect_count(dir), 0);
    });
}

/// Review finding: the redirect-cycle census runs on the REPORT-ONLY arm
/// too (it used to run only after a successful apply).
#[test]
fn report_only_run_surfaces_redirect_cycles() {
    with_brain(|dir| {
        let conn = rusqlite::Connection::open(dir.join("brain.db")).unwrap();
        for (a, b) in [("ent_c1", "ent_c2"), ("ent_c2", "ent_c1")] {
            conn.execute(
                "INSERT INTO entity_redirects (entity_id, merged_into, created_at)
                 VALUES (?1, ?2, 1)",
                rusqlite::params![a, b],
            )
            .unwrap();
        }
        drop(conn);
        let out = run_ct(dir, &["wiki", "merge-duplicates"]);
        let report = parse_stdout_object(&out);
        let cycles = report["cycles"].as_array().expect("cycles array");
        assert!(
            !cycles.is_empty(),
            "report-only run must list cycles: {report}"
        );
    });
}

#[test]
fn yes_unconfirmed_drift_blocks_the_merge() {
    with_brain(|dir| {
        seed_duplicate_pair(dir);
        seed_marker(dir);
        seed_watermark(dir, "deadbeef");
        let out = run_ct(dir, &["wiki", "merge-duplicates", "--yes"]);
        assert_eq!(out.status.code(), Some(1));
        let report = parse_stdout_object(&out);
        assert_eq!(report["skipped_reason"], "unconfirmed_drift", "{report}");
        let echoed = report["drift"]["old_hash"].as_str().unwrap();
        assert_eq!(echoed, "deadbeef", "{report}");
        assert_eq!(redirect_count(dir), 0);
    });
}

#[test]
fn yes_confirm_drift_with_matching_hash_merges() {
    with_brain(|dir| {
        seed_duplicate_pair(dir);
        seed_marker(dir);
        seed_watermark(dir, "deadbeef");
        let out = run_ct(
            dir,
            &[
                "wiki",
                "merge-duplicates",
                "--yes",
                "--confirm-drift",
                "deadbeef",
            ],
        );
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let report = parse_stdout_object(&out);
        assert_eq!(report["merged_groups"], 1, "{report}");
        assert_eq!(
            report["redirects_written"][0],
            serde_json::json!(["ent_b", "ent_a"]),
            "{report}"
        );
        assert_eq!(redirect_count(dir), 1);
        // R2.2.8 writer rule: the merge NEVER writes the watermark.
        let stored = meta_value(dir, "ontology_config_watermark").unwrap();
        assert!(stored.contains("deadbeef"), "{stored}");
    });
}

#[test]
fn yes_waive_drift_with_matching_hash_merges() {
    with_brain(|dir| {
        seed_duplicate_pair(dir);
        seed_marker(dir);
        seed_watermark(dir, "deadbeef");
        let out = run_ct(
            dir,
            &[
                "wiki",
                "merge-duplicates",
                "--yes",
                "--waive-drift",
                "deadbeef",
            ],
        );
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let report = parse_stdout_object(&out);
        assert_eq!(report["drift"]["waived"], true, "{report}");
        assert_eq!(redirect_count(dir), 1);
    });
}

#[test]
fn yes_mismatched_drift_hash_is_blocked_loud() {
    with_brain(|dir| {
        seed_duplicate_pair(dir);
        seed_marker(dir);
        seed_watermark(dir, "deadbeef");
        let out = run_ct(
            dir,
            &[
                "wiki",
                "merge-duplicates",
                "--yes",
                "--confirm-drift",
                "cafebabe",
            ],
        );
        assert_eq!(out.status.code(), Some(1));
        let report = parse_stdout_object(&out);
        assert_eq!(report["skipped_reason"], "unconfirmed_drift", "{report}");
        assert_eq!(redirect_count(dir), 0);
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("does not match"), "{err}");
    });
}
