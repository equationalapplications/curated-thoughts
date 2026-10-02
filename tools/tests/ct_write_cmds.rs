//! `ct ingest|librarian run` — write commands with --yes confirmation rules;
//! plus INTENT rule 1 regression tests that `ct approve` stays gone (removed
//! with the direct-write tool surface; human review lives in the TTY-gated
//! `ct proposals review`).
//!
//! Confirmation contract:
//! - `ct approve` (any form) must not exist and must change nothing.
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

fn proposal_status(dir: &std::path::Path, id: &str) -> String {
    let conn = rusqlite::Connection::open(dir.join("brain.db")).unwrap();
    conn.query_row(
        "SELECT status FROM curated_proposals WHERE id = ?1",
        [id],
        |r| r.get(0),
    )
    .unwrap()
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
fn approve_subcommand_is_gone() {
    with_seeded_proposals(|dir| {
        let out = run_ct(dir, &["approve", "prop-a"]);
        assert!(
            !out.status.success(),
            "`ct approve` must no longer exist (INTENT rule 1)"
        );
        assert_eq!(
            proposal_status(dir, "prop-a"),
            "pending",
            "removal must not have approved anything"
        );
    })
}

#[test]
fn approve_subcommand_is_gone_on_empty_queue_too() {
    with_seeded_proposals(|dir| {
        let out = run_ct(dir, &["approve"]);
        assert!(
            !out.status.success(),
            "`ct approve` must no longer exist (INTENT rule 1)"
        );
    })
}

#[test]
fn approve_removal_leaves_proposals_untouched() {
    with_seeded_proposals(|dir| {
        let out = run_ct(dir, &["approve", "--all"]);
        assert!(!out.status.success());
        assert_eq!(proposal_status(dir, "prop-a"), "pending");
        assert_eq!(proposal_status(dir, "prop-b"), "pending");
        let confirmed: i64 = {
            let conn = rusqlite::Connection::open(dir.join("brain.db")).unwrap();
            conn.query_row(
                "SELECT COUNT(*) FROM llm_wiki_entries WHERE source_type = 'user_confirmed'",
                [],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(
            confirmed, 0,
            "no wisdom rows may appear from the removed path"
        );
    })
}

#[test]
fn approve_all_is_gone_even_with_yes() {
    with_seeded_proposals(|dir| {
        let out = run_ct(dir, &["approve", "--all", "--yes"]);
        assert!(
            !out.status.success(),
            "`ct approve --all` must no longer exist (INTENT rule 1)"
        );
        assert_eq!(proposal_status(dir, "prop-a"), "pending");
        assert_eq!(proposal_status(dir, "prop-b"), "pending");
    })
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
