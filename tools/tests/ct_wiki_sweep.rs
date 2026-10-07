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

/// Controller ruling R10 (Task 8 fix round 1): the sweep's `--yes` apply arm
/// is the RETYPING-ONLY pass — it applies an alias retype (agent→role) but
/// must write NONE of heal's bookkeeping: no `ontology_config_watermark`
/// stamp (heal is the sole watermark writer, R2.2.8) and no
/// `alias_remap_completed` marker (the R2.7.1 merge precondition must not be
/// claimable by a sweep).
#[test]
fn sweep_yes_applies_retype_but_writes_no_heal_bookkeeping() {
    with_brain(|dir| {
        // tier_fact manifest declares `role` (the alias target) AND a
        // fallback: the retyping-only pass skips the §2.4.4 ensure, so a
        // strict row WITHOUT a fallback resolves StrictNoVocab and retypes
        // nothing (rows queue until `ct heal --yes` runs the ensure) — this
        // fixture pins the post-ensure state.
        let conn = rusqlite::Connection::open(dir.join("brain.db")).unwrap();
        let manifest = r#"{"node_types":[{"type":"action"},{"type":"role"},{"type":"service"}],"edge_types":[],"fallback_node_type":"role"}"#;
        conn.execute(
            "INSERT INTO llm_wiki_entity_manifests (entity_id, mode, manifest_json, updated_at)
             VALUES ('tier_fact', 'strict', ?1, 1)",
            [manifest],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO curated_entities (id, name, entity_type, summary, summary_embedding, created_at, updated_at, deleted_at)
             VALUES ('ent_drift', 'Drifted', 'agent', 's', NULL, 1, 1, NULL)",
            [],
        )
        .unwrap();
        // A resolvable source fact: source-less entities climb the host
        // default (rung 3), which SKIPs on a configless fixture — the
        // remap pass needs a resolved path to reach the strict tier_fact
        // row via rung 4 (same shape as the ct_heal fixtures).
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                source_ref, created_at, updated_at, deleted_at
             ) VALUES ('f1', 'ent_drift', 't', 'b', '[]', 'inferred',
                       'user', 'documents/note.md', 1, 1, NULL)",
            [],
        )
        .unwrap();
        drop(conn);

        let out = run_ct(dir, &["wiki", "sweep", "--yes"]);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("retyped 1"), "retype applied: {err}");

        // The retype landed...
        let conn = rusqlite::Connection::open(dir.join("brain.db")).unwrap();
        let t: String = conn
            .query_row(
                "SELECT entity_type FROM curated_entities WHERE id = 'ent_drift'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(t, "role", "agent→role alias retype");
        // ...but heal's bookkeeping did NOT.
        let bookkeeping: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_meta \
                 WHERE key IN ('ontology_config_watermark', 'alias_remap_completed')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            bookkeeping, 0,
            "sweep --yes must not stamp the watermark or the remap marker"
        );
    });
}
