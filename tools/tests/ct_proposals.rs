//! Task 6: `ct proposals list|show` — headless proposal inspection.
//!
//! Seeded fixture uses the real AppDb schema plus direct SQL inserts matching
//! the curated_proposals DDL (two pending proposals, distinct created_at and
//! item counts).

use std::process::{Command, Output};

use temp_env::with_vars;
use tempfile::tempdir;

mod common;

use common::{init_brain_db, insert_pending_proposal};

/// Run `f` with a fresh proposal-seeded temp brain as CURATED_BRAIN_DIR.
fn with_seeded_proposals<F: FnOnce(&std::path::Path)>(f: F) {
    let brain = tempdir().unwrap();
    let dir = brain.path().to_path_buf();
    let dir_str = dir.to_str().unwrap().to_string();
    with_vars([("CURATED_BRAIN_DIR", Some(dir_str.as_str()))], move || {
        init_brain_db(&dir);
        insert_pending_proposal(&dir, "prop-a", 2, 1_000);
        insert_pending_proposal(&dir, "prop-b", 1, 2_000);
        // Non-pending rows must be filtered out of `list`.
        insert_pending_proposal(&dir, "prop-approved", 5, 3_000);
        let conn = rusqlite::Connection::open(dir.join("brain.db")).unwrap();
        conn.execute(
            "UPDATE curated_proposals SET status='approved' WHERE id='prop-approved'",
            [],
        )
        .unwrap();
        drop(conn);
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
fn proposals_list_json_exits_zero_with_two_pending() {
    with_seeded_proposals(|dir| {
        let out = run_ct(dir, &["proposals", "list", "--json"]);
        assert!(
            out.status.success(),
            "ct proposals list failed: {} stderr={}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let arr = v.as_array().expect("list --json prints a bare JSON array");
        assert_eq!(arr.len(), 2, "only pending proposals: {v}");
        for entry in arr {
            assert!(entry.get("id").is_some(), "missing id: {entry}");
            assert!(
                entry.get("item_count").is_some(),
                "missing item_count: {entry}"
            );
            assert!(
                entry.get("created_at").is_some(),
                "missing created_at: {entry}"
            );
        }
        let ids: Vec<&str> = arr.iter().filter_map(|e| e["id"].as_str()).collect();
        assert_eq!(ids, ["prop-a", "prop-b"], "ordered by created_at");
        let counts: Vec<i64> = arr
            .iter()
            .filter_map(|e| e["item_count"].as_i64())
            .collect();
        assert_eq!(counts, [2, 1]);
    });
}

#[test]
fn proposals_show_json_contains_items() {
    with_seeded_proposals(|dir| {
        let out = run_ct(dir, &["proposals", "show", "prop-a", "--json"]);
        assert!(
            out.status.success(),
            "ct proposals show failed: {} stderr={}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["id"], "prop-a");
        let items = v["items"].as_array().expect("detail carries items");
        assert_eq!(items.len(), 2);
        assert!(items[0].get("payload").is_some(), "items include payloads");
    });
}

#[test]
fn proposals_show_unknown_id_exits_two() {
    with_seeded_proposals(|dir| {
        let out = run_ct(dir, &["proposals", "show", "no-such-id", "--json"]);
        assert_eq!(out.status.code(), Some(2), "unknown id must exit 2");
    });
}
