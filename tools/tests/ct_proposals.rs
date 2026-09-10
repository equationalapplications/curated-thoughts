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
fn proposals_show_text_mode_renders_summary() {
    with_seeded_proposals(|dir| {
        let out = run_ct(dir, &["proposals", "show", "prop-a"]);
        assert!(
            out.status.success(),
            "ct proposals show failed: {} stderr={}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.starts_with("prop-a\t"), "id line first: {text}");
        assert!(text.contains("2 item(s)"), "item count line: {text}");
    });
}

#[test]
fn proposals_show_unknown_id_exits_two() {
    with_seeded_proposals(|dir| {
        let out = run_ct(dir, &["proposals", "show", "no-such-id", "--json"]);
        assert_eq!(out.status.code(), Some(2), "unknown id must exit 2");
    });
}

// ---------------------------------------------------------------------------
// hvg Task 5: `ct proposals review` + evidence-rendering `proposals show`
// ---------------------------------------------------------------------------

use std::io::Write;
use std::path::Path;
use std::process::Stdio;

use tauri_app_lib::chunker::{Chunk, ChunkStrategyTag};
use tauri_app_lib::db::proposals::{
    insert_proposal, NewProposal, NewProposalItem, NewProposalSource, ProposalKind,
    ProposalSourceRole, StoredEvidenceChunk,
};

/// The CLI's default reject reason — must stay in sync with
/// `proposals_review_cmd` in `tools/src/cmds.rs`.
const CLI_DEFAULT_REJECT_REASON: &str = "Rejected during review";

/// The CLI reviewer identity fallback — `proposals_review_cmd` uses the
/// `USER` env var (or `USERNAME`), and "cli-operator" when neither is set.
/// The review e2e runs strip both, so this is also the value asserted
/// against `reviewed_by`.
const CLI_FALLBACK_REVIEWER: &str = "cli-operator";

/// Seed a pending new_entity proposal with one anchored fact_add item via the
/// real `insert_proposal` path (same seam the librarian synthesis uses). The
/// evidence chunk lives in a real `documents`/`chunks` row so hydration
/// resolves `doc_path`; `content_hash` is set so the stable-hash lookup wins.
fn insert_anchored_proposal(dir: &Path, id: &str, created_at: i64) {
    let conn = rusqlite::Connection::open(dir.join("brain.db")).unwrap();
    // The anchor document row is shared; only insert it the first time
    // (seeding two proposals in one brain hits the documents.path UNIQUE).
    conn.execute(
        "INSERT OR IGNORE INTO documents (path, hash, tier, status) VALUES (?1, 'h_anchor', 'user_doc', 'indexed')",
        ["/vault/anchor-doc.md"],
    )
    .unwrap();
    let doc_id: i64 = conn
        .query_row(
            "SELECT id FROM documents WHERE path = ?1",
            ["/vault/anchor-doc.md"],
            |r| r.get(0),
        )
        .unwrap();
    let chunk = Chunk {
        text: "the quokka is a small macropod".into(),
        start_line: 3,
        end_line: 7,
        symbol_name: None,
        defined_symbol: None,
        strategy: ChunkStrategyTag::Prose,
    };
    let chunk_id: i64 = {
        let existing: Option<i64> = conn
            .query_row(
                "SELECT id FROM chunks WHERE doc_id = ?1 AND content_hash = 'hash-anchor'",
                [doc_id],
                |r| r.get(0),
            )
            .ok();
        match existing {
            Some(id) => id,
            None => tauri_app_lib::retrieval::insert_chunk(
                &conn,
                doc_id,
                &chunk,
                0,
                "ent_anchor",
                "hash-anchor",
            )
            .unwrap(),
        }
    };
    insert_proposal(
        &conn,
        &NewProposal {
            id: id.into(),
            kind: ProposalKind::NewEntity,
            entity_id: None,
            proposed_name: Some(format!("Project {id}")),
            proposed_type: Some("project".into()),
            reasoning: Some("Because.".into()),
            model: "fixture-model".into(),
        },
        &[NewProposalItem {
            id: format!("{id}-item-0"),
            item_type: "fact_add".into(),
            target_id: None,
            payload: serde_json::json!({
                "body": "A verified fact.",
                "tags": [],
                "confidence": "inferred"
            }),
            evidence: vec![StoredEvidenceChunk {
                chunk_id: Some(chunk_id),
                content_hash: "hash-anchor".into(),
                quote: "the quokka is a small macropod".into(),
                start_line: Some(3),
                end_line: Some(7),
                source_kind: None,
            }],
        }],
        &[NewProposalSource {
            doc_id,
            role: ProposalSourceRole::Trigger,
        }],
    )
    .unwrap();
    // insert_proposal stamps created_at with wall clock; the fixture wants a
    // deterministic, ordered value like the SQL-seeded rows above.
    conn.execute(
        "UPDATE curated_proposals SET created_at = ?1 WHERE id = ?2",
        rusqlite::params![created_at, id],
    )
    .unwrap();
}

/// `run_ct` with piped stdin: write `input` to the child, then collect the
/// full output (the harness extension the plan sketched).
fn run_ct_with_stdin(dir: &Path, args: &[&str], input: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ct"))
        .env("CURATED_BRAIN_DIR", dir)
        .env_remove("CURATED_BRAIN_DB")
        .env_remove("CURATED_BRAIN_CONFIG")
        .env_remove("USER")
        // Windows has no standard `USER`; `cli_reviewer` falls back to
        // `USERNAME` before "cli-operator", so the fallback assertion needs
        // both stripped.
        .env_remove("USERNAME")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn ct");
    child
        .stdin
        .as_mut()
        .expect("piped stdin")
        .write_all(input)
        .expect("write review answers to ct stdin");
    child.wait_with_output().expect("ct proposals review")
}

fn proposal_status(dir: &Path, id: &str) -> String {
    let conn = rusqlite::Connection::open(dir.join("brain.db")).unwrap();
    conn.query_row(
        "SELECT status FROM curated_proposals WHERE id = ?1",
        [id],
        |r| r.get(0),
    )
    .unwrap()
}

#[test]
fn review_command_empty_queue_exits_zero() {
    common::with_seeded_brain(|| {
        let out = common::run_ct(&["proposals", "review"]);
        assert!(
            out.status.success(),
            "review on empty queue must exit 0: {} stderr={}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("0 pending"),
            "stdout must report the empty queue: {}",
            String::from_utf8_lossy(&out.stdout)
        );
    });
}

#[test]
fn review_command_approves_via_piped_y() {
    let brain = tempdir().unwrap();
    let dir = brain.path().to_path_buf();
    let dir_str = dir.to_str().unwrap().to_string();
    with_vars([("CURATED_BRAIN_DIR", Some(dir_str.as_str()))], move || {
        init_brain_db(&dir);
        insert_anchored_proposal(&dir, "prop-y", 1_000);
        let out = run_ct_with_stdin(&dir, &["proposals", "review"], b"y\n");
        assert!(
            out.status.success(),
            "review must exit 0: {} stderr={}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("prop-y"), "card shows the id: {text}");
        assert!(text.contains("approved"), "decision echo: {text}");
        assert_eq!(proposal_status(&dir, "prop-y"), "approved");
        let conn = rusqlite::Connection::open(dir.join("brain.db")).unwrap();
        let reviewed_by: String = conn
            .query_row(
                "SELECT reviewed_by FROM curated_proposals WHERE id = 'prop-y'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(reviewed_by, CLI_FALLBACK_REVIEWER);
        let confirmed: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_entries WHERE source_type = 'user_confirmed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(confirmed, 1, "approved entry is user_confirmed");
    });
}

#[test]
fn review_command_rejects_via_piped_n() {
    let brain = tempdir().unwrap();
    let dir = brain.path().to_path_buf();
    let dir_str = dir.to_str().unwrap().to_string();
    with_vars([("CURATED_BRAIN_DIR", Some(dir_str.as_str()))], move || {
        init_brain_db(&dir);
        insert_anchored_proposal(&dir, "prop-n", 1_000);
        let out = run_ct_with_stdin(&dir, &["proposals", "review"], b"n\n");
        assert!(
            out.status.success(),
            "review must exit 0: {} stderr={}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("rejected"), "decision echo: {text}");
        assert_eq!(proposal_status(&dir, "prop-n"), "rejected");
        let conn = rusqlite::Connection::open(dir.join("brain.db")).unwrap();
        let reason: Option<String> = conn
            .query_row(
                "SELECT reject_reason FROM curated_proposals WHERE id = 'prop-n'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            reason.as_deref(),
            Some(CLI_DEFAULT_REJECT_REASON),
            "reject stores the CLI default reason"
        );
        let entries: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_entries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(entries, 0, "a reject writes no wiki entries");
    });
}

/// F1 regression: `s` (skip) must advance past the skipped proposal to the
/// next queue head instead of re-prompting on the same one forever. Seeds
/// TWO anchored proposals; piping `s` then `y` must approve the SECOND
/// proposal and leave the first pending.
#[test]
fn review_command_skip_advances_to_next_proposal() {
    let brain = tempdir().unwrap();
    let dir = brain.path().to_path_buf();
    let dir_str = dir.to_str().unwrap().to_string();
    with_vars([("CURATED_BRAIN_DIR", Some(dir_str.as_str()))], move || {
        init_brain_db(&dir);
        insert_anchored_proposal(&dir, "prop-s1", 1_000);
        insert_anchored_proposal(&dir, "prop-s2", 2_000);
        let out = run_ct_with_stdin(&dir, &["proposals", "review"], b"s\ny\n");
        assert!(
            out.status.success(),
            "review must exit 0: {} stderr={}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("skipped prop-s1"), "skip echo: {text}");
        assert!(text.contains("approved"), "decision echo: {text}");
        assert_eq!(
            proposal_status(&dir, "prop-s1"),
            "pending",
            "skipped proposal must stay pending"
        );
        assert_eq!(
            proposal_status(&dir, "prop-s2"),
            "approved",
            "skip must advance to the second proposal"
        );
    });
}

#[test]
fn proposals_show_renders_evidence_quotes() {
    let brain = tempdir().unwrap();
    let dir = brain.path().to_path_buf();
    let dir_str = dir.to_str().unwrap().to_string();
    with_vars([("CURATED_BRAIN_DIR", Some(dir_str.as_str()))], move || {
        init_brain_db(&dir);
        insert_anchored_proposal(&dir, "prop-quote", 1_000);
        let out = run_ct(&dir, &["proposals", "show", "prop-quote"]);
        assert!(
            out.status.success(),
            "show failed: {} stderr={}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("prop-quote"), "id: {text}");
        assert!(text.contains("new_entity"), "kind: {text}");
        assert!(text.contains("Project prop-quote"), "proposed_name: {text}");
        assert!(
            text.contains("the quokka is a small macropod"),
            "hydrated evidence quote: {text}"
        );
        assert!(text.contains("3-7"), "line range: {text}");
        assert!(text.contains("/vault/anchor-doc.md"), "doc path: {text}");
    });
}
