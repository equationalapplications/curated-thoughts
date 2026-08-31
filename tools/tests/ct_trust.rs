//! `ct trust` — approve, list, and revoke symlinks the ingest walker may follow.
//!
//! Mirrors the direct-TempDir + env-var harness used by `ct_status.rs` and
//! `ct_watch_exit_codes.rs` rather than introducing a `TestEnv` helper (one
//! of the things the plan defers until multiple test files actually need
//! it; we don't yet).

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::symlink;
use std::process::Command;
use tempfile::TempDir;

/// Build a config.json that points `vault_path` at `vault`, then return the
/// brain dir (where config.json + brain.db live) and the vault dir.
fn seed_env() -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = TempDir::new().unwrap();
    let brain = tmp.path().join("brain");
    let vault = tmp.path().join("vault");
    fs::create_dir_all(&brain).unwrap();
    fs::create_dir_all(&vault).unwrap();
    fs::write(
        brain.join("config.json"),
        format!(r#"{{"vault_path":"{}"}}"#, vault.display()),
    )
    .unwrap();
    (tmp, brain, vault)
}

fn run_ct(brain_dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ct"))
        .env("CURATED_BRAIN_DIR", brain_dir)
        .env_remove("CURATED_BRAIN_DB")
        .env_remove("CURATED_BRAIN_CONFIG")
        .args(args)
        .output()
        .expect("spawn ct")
}

#[test]
fn trust_approves_a_pending_link_and_persists_the_pair() {
    let (_tmp, brain, vault) = seed_env();
    let docs = vault.join("documents");
    fs::create_dir_all(&docs).unwrap();
    let outside = _tmp.path().join("repo-docs");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("spec.md"), "# Spec\n").unwrap();
    symlink(&outside, docs.join("specs")).unwrap();

    let out = run_ct(&brain, &["trust", "documents/specs"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let cfg: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(brain.join("config.json")).unwrap()).unwrap();
    let links = cfg["trusted_links"]
        .as_array()
        .expect("trusted_links array");
    assert_eq!(links.len(), 1);
    assert_eq!(links[0]["link"], "documents/specs");
    assert!(links[0]["target"].as_str().unwrap().ends_with("repo-docs"));
    assert!(links[0]["approved_at"].as_i64().unwrap() > 0);
}

#[test]
fn trust_refuses_a_non_approvable_target_and_names_the_rule() {
    let (_tmp, brain, vault) = seed_env();
    let docs = vault.join("documents");
    fs::create_dir_all(&docs).unwrap();
    symlink(_tmp.path(), docs.join("everything")).unwrap();

    let out = run_ct(&brain, &["trust", "documents/everything"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("vault"),
        "must name the rule, got: {stderr}"
    );
}

#[test]
fn trust_on_an_unknown_link_exits_1() {
    let (_tmp, brain, vault) = seed_env();
    fs::create_dir_all(vault.join("documents")).unwrap();

    let out = run_ct(&brain, &["trust", "documents/nope"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not a symlink") || stderr.contains("no such"),
        "stderr: {stderr}"
    );
}

#[test]
fn trust_list_prints_the_ledger_and_revoke_removes_an_entry() {
    let (_tmp, brain, vault) = seed_env();
    let docs = vault.join("documents");
    fs::create_dir_all(&docs).unwrap();
    let outside = _tmp.path().join("repo-docs");
    fs::create_dir_all(&outside).unwrap();
    symlink(&outside, docs.join("specs")).unwrap();

    let approved = run_ct(&brain, &["trust", "documents/specs"]);
    assert_eq!(approved.status.code(), Some(0));

    let listed = run_ct(&brain, &["trust", "--list"]);
    assert_eq!(listed.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&listed.stdout).contains("documents/specs"));

    let revoked = run_ct(&brain, &["trust", "--revoke", "documents/specs"]);
    assert_eq!(revoked.status.code(), Some(0));

    let cfg: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(brain.join("config.json")).unwrap()).unwrap();
    assert!(cfg["trusted_links"].as_array().unwrap().is_empty());
}
