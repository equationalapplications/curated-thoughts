//! Task 7: `ct approve|ingest|librarian run` — write commands with --yes
//! confirmation rules.
//!
//! Confirmation contract (SDD ruling):
//! - `ct approve <id>` exits 0; re-running exits 1 ("proposal not pending").
//! - `ct approve --all` on an empty pending set exits 0 printing `approved: 0`.
//! - `ct approve --all` with pending items WITHOUT `--yes` exits 1 listing
//!   what would be accepted; WITH `--yes` it proceeds and exits 0.
//! - `ct ingest` / `ct librarian run` require `--yes`, else exit 1 printing
//!   the planned action (script-friendly, no prompts).

use std::process::{Command, Output};

use temp_env::with_vars;
use tempfile::tempdir;

mod common;

use common::{init_brain_db, insert_pending_proposal};

/// Run `f` with a fresh proposal-seeded temp brain as CURATED_BRAIN_DIR
/// (`prop-a` 2 items, `prop-b` 1 item).
fn with_seeded_proposals<F: FnOnce(&std::path::Path)>(f: F) {
    let brain = tempdir().unwrap();
    let dir = brain.path().to_path_buf();
    let dir_str = dir.to_str().unwrap().to_string();
    with_vars([("CURATED_BRAIN_DIR", Some(dir_str.as_str()))], move || {
        init_brain_db(&dir);
        insert_pending_proposal(&dir, "prop-a", 2, 1_000);
        insert_pending_proposal(&dir, "prop-b", 1, 2_000);
        f(&dir);
    });
}

fn run_ct(dir: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ct"))
        .env("CURATED_BRAIN_DIR", dir)
        .env_remove("CURATED_BRAIN_DB")
        .env_remove("CURATED_BRAIN_CONFIG")
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn approve_one_exits_zero_then_not_pending_exits_one() {
    with_seeded_proposals(|dir| {
        let out = run_ct(dir, &["approve", "prop-a"]);
        assert!(
            out.status.success(),
            "first approve failed: {} stderr={}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        // Ruling: approve_one prints the summary it accepted — id, item count,
        // source doc.
        assert!(stdout.contains("prop-a"), "stdout: {stdout}");
        assert!(stdout.contains("2"), "item count in stdout: {stdout}");

        let again = run_ct(dir, &["approve", "prop-a"]);
        assert_eq!(again.status.code(), Some(1), "re-approve must exit 1");
        let err = String::from_utf8_lossy(&again.stderr);
        assert!(err.to_lowercase().contains("not pending"), "stderr: {err}");

        let unknown = run_ct(dir, &["approve", "no-such"]);
        assert_eq!(unknown.status.code(), Some(1));
    });
}

#[test]
fn approve_all_empty_exits_zero_printing_approved_zero() {
    let brain = tempdir().unwrap();
    let dir = brain.path().to_path_buf();
    let dir_str = dir.to_str().unwrap().to_string();
    with_vars([("CURATED_BRAIN_DIR", Some(dir_str.as_str()))], move || {
        init_brain_db(&dir);
        let out = run_ct(&dir, &["approve", "--all"]);
        assert!(out.status.success(), "empty --all must exit 0");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("approved: 0"), "stdout: {stdout}");
    });
}

#[test]
fn approve_all_without_yes_exits_one_listing_pending() {
    with_seeded_proposals(|dir| {
        let out = run_ct(dir, &["approve", "--all"]);
        assert_eq!(
            out.status.code(),
            Some(1),
            "--all without --yes must refuse"
        );
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("--yes"), "must point at --yes: {err}");
        assert!(err.contains("prop-a"), "must list prop-a: {err}");
        assert!(err.contains("prop-b"), "must list prop-b: {err}");
        // Nothing was actually approved.
        let conn = rusqlite::Connection::open(dir.join("brain.db")).unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM curated_proposals WHERE status='pending'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 2, "dry run must not mutate");
    });
}

#[test]
fn approve_all_with_yes_exits_zero_and_approves() {
    with_seeded_proposals(|dir| {
        let out = run_ct(dir, &["approve", "--all", "--yes"]);
        assert!(
            out.status.success(),
            "--all --yes failed: {} stderr={}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("approved: 2"), "stdout: {stdout}");
        let conn = rusqlite::Connection::open(dir.join("brain.db")).unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM curated_proposals WHERE status='pending'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0);
    });
}

/// Run `f` with a brain-dir fixture (config.json pointing `vault_path` at a
/// temp vault containing one markdown file) as CURATED_BRAIN_DIR.
fn with_ingest_fixture<F: FnOnce(&std::path::Path)>(f: F) {
    let brain = tempdir().unwrap();
    let vault = tempdir().unwrap();
    let dir = brain.path().to_path_buf();
    let dir_str = dir.to_str().unwrap().to_string();
    let vault_str = vault.path().to_str().unwrap().to_string();
    std::fs::create_dir_all(vault.path().join("notes")).unwrap();
    std::fs::write(vault.path().join("notes/a.md"), "# hello\n\nworld\n").unwrap();
    with_vars(
        [
            ("CURATED_BRAIN_DIR", Some(dir_str.as_str())),
            ("CURATED_EMBED_STUB", Some("constant8")),
        ],
        move || {
            init_brain_db(&dir);
            std::fs::write(
                dir.join("config.json"),
                format!(r#"{{"vault_path":"{vault_str}"}}"#),
            )
            .unwrap();
            f(&dir);
        },
    );
}

#[test]
fn ingest_requires_yes() {
    with_ingest_fixture(|dir| {
        let out = run_ct(dir, &["ingest"]);
        assert_eq!(
            out.status.code(),
            Some(1),
            "ingest without --yes must exit 1"
        );
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("--yes"), "must mention --yes: {err}");
        assert!(!err.is_empty(), "must print the planned action");

        // With --yes it runs the real ingest flow against the fixture vault.
        let ok = run_ct(dir, &["ingest", "--yes"]);
        assert!(
            ok.status.success(),
            "ingest --yes failed: {} stderr={}",
            ok.status,
            String::from_utf8_lossy(&ok.stderr)
        );
        let stdout = String::from_utf8_lossy(&ok.stdout);
        assert!(stdout.contains("ingesting 1 file(s)"), "stdout: {stdout}");
    });
}

#[test]
fn librarian_run_requires_yes() {
    with_seeded_proposals(|dir| {
        let out = run_ct(dir, &["librarian", "run"]);
        assert_eq!(
            out.status.code(),
            Some(1),
            "librarian run without --yes must exit 1"
        );
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("--yes"), "must mention --yes: {err}");

        // With --yes it proceeds (librarian bails gracefully without docs is
        // fine too — we only assert the gate opened, i.e. not the refusal).
        let ok = run_ct(dir, &["librarian", "run", "--yes"]);
        assert_ne!(
            ok.status.code(),
            Some(1),
            "with --yes the gate must be open"
        );
    });
}
