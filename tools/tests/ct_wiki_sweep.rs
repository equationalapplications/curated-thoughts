//! `ct wiki sweep` §2.10 node-type extension — census DISPLAY only.
//!
//! The sweep gains a node-type drift pass over the SAME resolved vocabulary
//! and alias table the heal pass uses (report-only without `--yes`, applying
//! with it). The edge purge contract is unchanged: refusal without `--yes`,
//! one `purged N` stdout line with it; the node-type pass summary is human
//! text on STDERR in both arms.

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

#[test]
fn sweep_without_yes_reports_node_type_pass_readonly() {
    with_brain(|dir| {
        let out = run_ct(dir, &["wiki", "sweep"]);
        assert_eq!(out.status.code(), Some(1));
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("--yes"), "{err}");
        assert!(
            err.contains("ontology node-type pass (read-only)"),
            "§2.10 census display missing: {err}"
        );
        assert!(
            String::from_utf8_lossy(&out.stdout).trim().is_empty(),
            "refusal keeps stdout empty"
        );
    });
}

#[test]
fn sweep_with_yes_purges_edges_then_reports_node_type_pass() {
    with_brain(|dir| {
        let out = run_ct(dir, &["wiki", "sweep", "--yes"]);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let out_text = String::from_utf8_lossy(&out.stdout);
        assert!(out_text.contains("purged"), "{out_text}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("ontology node-type pass"), "{err}");
    });
}
