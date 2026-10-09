//! `ct heal` — write-gated heal subcommand (spec 2026-09-24 §6 + Tests;
//! ontology heal pass per the 2026-10-03 ontology spec §2.6).
//!
//! Contract:
//! - Without `--yes`: exit 1, stderr names the target db path, no mutation,
//!   stdout EMPTY (the read-only ontology census/drift goes to stderr).
//! - With `--yes`: the heal core soft-deletes the seeded ungrounded row,
//!   purges its dead-partner edges, writes a `healed` event, runs the
//!   ontology pass, and prints ONE JSON object on stdout:
//!   {"evaluated":N,"soft_deleted":N,"edges_purged":N,
//!   "ontology":{drift,census,retyped,queued,report_only,error,
//!   skipped_reason}} — the source-heal counts stay top-level.

use std::process::{Command, Output};

use temp_env::with_vars;
use tempfile::tempdir;

mod common;

use common::init_brain_db;

/// Seed one live ungrounded `librarian_inferred` entry (legacy path ref to a
/// document that does not exist in `documents`), one pre-soft-deleted dead
/// partner row, and two edges on the ungrounded row: one to the dead partner
/// (`edge_doomed`, purgeable per the partner-alive retention rule) and one
/// self-edge (`edge_self`, purged too — the heal's own soft-delete makes that
/// endpoint dead within the same transaction).
fn seed_heal_fixture(dir: &std::path::Path) {
    let conn = rusqlite::Connection::open(dir.join("brain.db")).unwrap();
    conn.execute(
        "INSERT INTO llm_wiki_entries (
            id, entity_id, title, body, tags, confidence, source_type,
            source_ref, created_at, updated_at, deleted_at
         ) VALUES ('lost', 'ent-1', 'Title lost', 'body', '[]', 'inferred',
                   'librarian_inferred', 'documents/vanished.md', 1, 1, NULL)",
        [],
    )
    .unwrap();
    // Dead partner: pre-soft-deleted, so edges touching it are purgeable.
    conn.execute(
        "INSERT INTO llm_wiki_entries (
            id, entity_id, title, body, tags, confidence, source_type,
            source_ref, created_at, updated_at, deleted_at
         ) VALUES ('dead', 'ent-1', 'Title dead', 'body', '[]', 'inferred',
                   'librarian_inferred', NULL, 1, 1, 100)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
         VALUES ('edge_doomed', 'ent-1', 'lost', 'dead', 'related_to', 1)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
         VALUES ('edge_self', 'ent-1', 'lost', 'lost', 'related_to', 1)",
        [],
    )
    .unwrap();
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

fn with_seeded_heal_brain<F: FnOnce(&std::path::Path)>(f: F) {
    let brain = tempdir().unwrap();
    let dir = brain.path().to_path_buf();
    let dir_str = dir.to_str().unwrap().to_string();
    with_vars([("CURATED_BRAIN_DIR", Some(dir_str.as_str()))], move || {
        init_brain_db(&dir);
        seed_heal_fixture(&dir);
        f(&dir);
    });
}

#[test]
fn heal_without_yes_exits_one_naming_db_and_does_not_mutate() {
    with_seeded_heal_brain(|dir| {
        let out = run_ct(dir, &["heal"]);
        assert_eq!(
            out.status.code(),
            Some(1),
            "refusal must exit 1, got {} stderr={}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("--yes"), "refusal must point at --yes: {err}");
        assert!(
            err.contains("brain.db"),
            "refusal must name the target db path: {err}"
        );
        // Nothing was actually healed.
        let conn = rusqlite::Connection::open(dir.join("brain.db")).unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_entries WHERE id = 'lost' AND deleted_at IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "refusal must not mutate");
    });
}

#[test]
fn heal_refusal_does_not_create_brain_db_on_a_fresh_brain() {
    // Round-2 M1 (Opus review of PR #228): the refusal's row count opened
    // brain.db with default flags, CREATING it on a fresh brain — the
    // refusal path must never write. The count falls back to "?" when the
    // db does not exist (read-only open fails).
    let brain = tempdir().unwrap();
    let dir = brain.path().to_path_buf();
    let dir_str = dir.to_str().unwrap().to_string();
    with_vars([("CURATED_BRAIN_DIR", Some(dir_str.as_str()))], move || {
        let out = run_ct(&dir, &["heal"]);
        assert_eq!(
            out.status.code(),
            Some(1),
            "refusal must exit 1 even with no brain.db"
        );
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            err.contains("? live librarian_inferred row"),
            "count must fall back to ? when the db is absent: {err}"
        );
        assert!(
            !dir.join("brain.db").exists(),
            "the refusal path must NOT create brain.db"
        );
    });
}

#[test]
fn heal_with_yes_heals_purges_and_prints_summary_json() {
    with_seeded_heal_brain(|dir| {
        let out = run_ct(dir, &["heal", "--yes"]);
        assert!(
            out.status.success(),
            "--yes heal failed: {} stderr={}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        let summary: serde_json::Value = serde_json::from_str(stdout.trim())
            .unwrap_or_else(|e| panic!("stdout must be a single JSON object ({e}): {stdout}"));
        assert_eq!(
            summary["evaluated"], 1,
            "only the live row is evaluated: {summary}"
        );
        assert_eq!(
            summary["soft_deleted"], 1,
            "the ungrounded row heals: {summary}"
        );
        assert_eq!(
            summary["edges_purged"], 2,
            "both edges purge: the dead partner AND the self-edge whose other endpoint is the healed row: {summary}"
        );

        let conn = rusqlite::Connection::open(dir.join("brain.db")).unwrap();
        let lost_deleted: Option<i64> = conn
            .query_row(
                "SELECT deleted_at FROM llm_wiki_entries WHERE id = 'lost'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            lost_deleted.is_some(),
            "ungrounded row must be soft-deleted"
        );

        let doomed: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_edges WHERE id = 'edge_doomed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(doomed, 0, "dead-partner edge must be purged");
        let kept: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_edges WHERE id = 'edge_self'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kept, 0, "the self-edge purges once its row is healed");

        let events: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_events WHERE event_type = 'healed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(events, 1, "one healed event for the affected entity");
    });
}

#[test]
fn heal_with_yes_on_clean_brain_exits_zero_with_zero_summary() {
    // M4 (Opus review of PR #228): the nothing-to-do case must be exit 0
    // with a zeroed summary — a no-op heal is a SUCCESS, not an error.
    let brain = tempdir().unwrap();
    let dir = brain.path().to_path_buf();
    let dir_str = dir.to_str().unwrap().to_string();
    with_vars([("CURATED_BRAIN_DIR", Some(dir_str.as_str()))], move || {
        init_brain_db(&dir);
        let out = run_ct(&dir, &["heal", "--yes"]);
        assert!(
            out.status.success(),
            "clean-brain heal must exit 0, got {} stderr={}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        let summary: serde_json::Value = serde_json::from_str(stdout.trim())
            .unwrap_or_else(|e| panic!("stdout must be a single JSON object ({e}): {stdout}"));
        assert_eq!(summary["evaluated"], 0, "{summary}");
        assert_eq!(summary["soft_deleted"], 0, "{summary}");
        assert_eq!(summary["edges_purged"], 0, "{summary}");
    });
}

#[test]
fn heal_refusal_reports_the_live_row_count_it_would_evaluate() {
    // Spec §6: the refusal names the db path AND the live-row count (m1).
    with_seeded_heal_brain(|dir| {
        let out = run_ct(dir, &["heal"]);
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            err.contains("1 live librarian_inferred row"),
            "refusal must include the evaluated-row count: {err}"
        );
    });
}

// ---------------------------------------------------------------------------
// Ontology heal pass (Task 5, spec 2026-10-03 §2.6 / R2.2.8)
// ---------------------------------------------------------------------------

fn seed_watermark(dir: &std::path::Path, hash: &str, stamped_at: i64) {
    let conn = rusqlite::Connection::open(dir.join("brain.db")).unwrap();
    conn.execute(
        "INSERT INTO llm_wiki_meta (key, value)
         VALUES ('ontology_config_watermark', ?1)",
        [format!(r#"{{"hash":"{hash}","stamped_at":{stamped_at}}}"#)],
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

fn seed_tier_fact_manifest(dir: &std::path::Path) {
    // EA-seed strict manifest (pre-wave-1: no `document`/`process`, no
    // fallback) — the ensure extends it before the census.
    let types: Vec<String> = [
        "action",
        "creativework",
        "design_spec",
        "event",
        "handoff",
        "organization",
        "person",
        "place",
        "procedure",
        "product",
        "project",
        "reference_doc",
        "review",
        "role",
        "service",
        "session_recap",
        "software_application",
    ]
    .iter()
    .map(|s| format!(r#"{{"type":"{s}"}}"#))
    .collect();
    let manifest = format!(r#"{{"node_types":[{}],"edge_types":[]"#, types.join(",")) + "}";
    let conn = rusqlite::Connection::open(dir.join("brain.db")).unwrap();
    conn.execute(
        "INSERT INTO llm_wiki_entity_manifests (entity_id, mode, manifest_json, updated_at)
         VALUES ('tier_fact', 'strict', ?1, 1)",
        [&manifest],
    )
    .unwrap();
}

fn seed_entity(dir: &std::path::Path, id: &str, entity_type: &str) {
    let conn = rusqlite::Connection::open(dir.join("brain.db")).unwrap();
    conn.execute(
        "INSERT INTO curated_entities (
            id, name, entity_type, summary, summary_embedding,
            created_at, updated_at, deleted_at
         ) VALUES (?1, ?1, ?2, '', NULL, 1, 1, NULL)",
        [id, entity_type],
    )
    .unwrap();
}

fn entity_type_of(dir: &std::path::Path, id: &str) -> String {
    let conn = rusqlite::Connection::open(dir.join("brain.db")).unwrap();
    conn.query_row(
        "SELECT entity_type FROM curated_entities WHERE id = ?1",
        [id],
        |r| r.get(0),
    )
    .unwrap()
}

fn parse_stdout_object(out: &Output) -> serde_json::Value {
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stdout must be a single JSON object ({e}): {stdout}"))
}

/// plan-p14-m4: `--confirm-drift` WITHOUT `--yes` is a usage error and
/// touches nothing (clap `requires` behaviour is version-dependent — pin).
#[test]
fn heal_confirm_drift_without_yes_is_a_usage_error() {
    with_seeded_heal_brain(|dir| {
        let out = run_ct(dir, &["heal", "--confirm-drift", "abc"]);
        assert_ne!(
            out.status.code(),
            Some(0),
            "confirm-drift without --yes must be a usage error"
        );
        // This bin maps clap usage errors to exit 1 — distinguish the
        // USAGE error from the heal refusal by its stderr text.
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            err.contains("required arguments") || err.contains("--yes"),
            "must be clap's usage error, not the refusal: {err}"
        );
        assert!(
            !err.contains("refusing"),
            "the refusal arm must not run: {err}"
        );
        let conn = rusqlite::Connection::open(dir.join("brain.db")).unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_entries WHERE id = 'lost' AND deleted_at IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "usage error must touch nothing");
    });
}

/// plan-p4-MAJOR-1/plan-p9-M1: unconfirmed drift blocks ONLY the ontology
/// section — source-heal still mutates, stdout is one JSON object, exit 1.
#[test]
fn heal_yes_with_unconfirmed_drift_still_source_heals_and_skips_ontology() {
    with_seeded_heal_brain(|dir| {
        seed_watermark(dir, "deadbeef", 7);
        let out = run_ct(dir, &["heal", "--yes"]);
        assert_eq!(out.status.code(), Some(1), "unconfirmed drift exits 1");
        let summary = parse_stdout_object(&out);
        assert_eq!(summary["soft_deleted"], 1, "source-heal ran: {summary}");
        assert_eq!(
            summary["ontology"]["skipped_reason"], "unconfirmed_drift",
            "{summary}"
        );
        assert_eq!(summary["ontology"]["retyped"], 0, "{summary}");
        let drift = &summary["ontology"]["drift"];
        assert_eq!(drift["old_hash"], "deadbeef", "{drift}");
        assert_eq!(drift["old_stamped_at"], 7, "{drift}");
        assert_eq!(drift["confirmed"], false, "{drift}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("deadbeef"), "drift echo on stderr: {err}");
    });
}

/// plan-p9-M2: waive with the matching hash → exit 0, waived, zero retypes,
/// watermark unchanged, no `alias_remap_completed` marker.
#[test]
fn heal_yes_waive_matching_hash_skips_remap_and_keeps_watermark() {
    with_seeded_heal_brain(|dir| {
        seed_watermark(dir, "deadbeef", 7);
        seed_tier_fact_manifest(dir);
        seed_entity(dir, "e1", "agent");
        let out = run_ct(dir, &["heal", "--yes", "--waive-drift", "deadbeef"]);
        assert_eq!(out.status.code(), Some(0), "waived drift exits 0");
        let summary = parse_stdout_object(&out);
        assert_eq!(summary["ontology"]["drift"]["waived"], true, "{summary}");
        assert_eq!(
            summary["ontology"]["skipped_reason"], "drift_waived",
            "{summary}"
        );
        assert_eq!(summary["ontology"]["retyped"], 0, "{summary}");
        assert_eq!(entity_type_of(dir, "e1"), "agent", "no retype on waive");
        assert_eq!(
            meta_value(dir, "ontology_config_watermark").as_deref(),
            Some(r#"{"hash":"deadbeef","stamped_at":7}"#),
            "watermark unchanged on waive"
        );
        assert!(
            meta_value(dir, "alias_remap_completed").is_none(),
            "marker never set on waive"
        );
    });
}

/// plan-p13-m4: MISMATCHED hash on either flag → exit 1 + loud stderr.
#[test]
fn heal_yes_waive_mismatched_hash_exits_one() {
    with_seeded_heal_brain(|dir| {
        seed_watermark(dir, "deadbeef", 7);
        let out = run_ct(dir, &["heal", "--yes", "--waive-drift", "wrong"]);
        assert_eq!(out.status.code(), Some(1));
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            err.to_lowercase().contains("not confirmed"),
            "mismatch must be loud: {err}"
        );
        let summary = parse_stdout_object(&out);
        assert_eq!(
            summary["ontology"]["skipped_reason"], "unconfirmed_drift",
            "{summary}"
        );
    });
}

/// `--confirm-drift <old-hash>` proceeds, retypes, and stores the NEW
/// watermark (heal is the sole watermark writer).
#[test]
fn heal_yes_confirm_matching_hash_proceeds_and_stores_watermark() {
    with_seeded_heal_brain(|dir| {
        seed_watermark(dir, "deadbeef", 7);
        seed_tier_fact_manifest(dir);
        seed_entity(dir, "e1", "agent");
        let out = run_ct(dir, &["heal", "--yes", "--confirm-drift", "deadbeef"]);
        assert_eq!(out.status.code(), Some(0));
        let summary = parse_stdout_object(&out);
        assert_eq!(summary["ontology"]["drift"]["confirmed"], true, "{summary}");
        assert_eq!(summary["ontology"]["retyped"], 1, "{summary}");
        assert_eq!(entity_type_of(dir, "e1"), "role");
        let stored = meta_value(dir, "ontology_config_watermark").unwrap();
        assert!(
            !stored.contains("deadbeef"),
            "new watermark stored: {stored}"
        );
        assert!(
            meta_value(dir, "alias_remap_completed").is_some(),
            "marker set after a successful remap pass"
        );
        let conn = rusqlite::Connection::open(dir.join("brain.db")).unwrap();
        let (reason, original): (String, Option<String>) = conn
            .query_row(
                "SELECT reason, original_type FROM entity_type_origin WHERE entity_id = 'e1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(reason, "alias_retype");
        assert_eq!(original.as_deref(), Some("agent"));
    });
}

/// plan-p14-m3: flags with NO drift report are ignored with a note, exit 0.
#[test]
fn heal_yes_drift_flags_ignored_when_no_drift_fires() {
    let brain = tempdir().unwrap();
    let dir = brain.path().to_path_buf();
    let dir_str = dir.to_str().unwrap().to_string();
    with_vars([("CURATED_BRAIN_DIR", Some(dir_str.as_str()))], move || {
        init_brain_db(&dir);
        let out = run_ct(&dir, &["heal", "--yes", "--confirm-drift", "whatever"]);
        assert_eq!(
            out.status.code(),
            Some(0),
            "ignored flag must not fail the run"
        );
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("ignored"), "note on stderr: {err}");
        let summary = parse_stdout_object(&out);
        assert!(summary["ontology"]["drift"].is_null(), "{summary}");
        assert!(summary["ontology"]["skipped_reason"].is_null(), "{summary}");
    });
}

/// plan-p13-m5 no-config regression pin: on the bare `init_brain_db` fixture
/// (no config.json beyond `{}`, migration-seeded state) the ensure + census
/// is a clean no-op — ontology section present, no error, exit 0.
#[test]
fn heal_yes_on_configless_fixture_reports_clean_ontology_section() {
    let brain = tempdir().unwrap();
    let dir = brain.path().to_path_buf();
    let dir_str = dir.to_str().unwrap().to_string();
    with_vars([("CURATED_BRAIN_DIR", Some(dir_str.as_str()))], move || {
        init_brain_db(&dir);
        let out = run_ct(&dir, &["heal", "--yes"]);
        assert_eq!(out.status.code(), Some(0));
        let summary = parse_stdout_object(&out);
        assert!(summary["ontology"].is_object(), "{summary}");
        assert!(summary["ontology"]["error"].is_null(), "{summary}");
        assert!(summary["ontology"]["skipped_reason"].is_null(), "{summary}");
        assert_eq!(summary["ontology"]["retyped"], 0, "{summary}");
    });
}

/// plan-p7-m3: degraded config — source-heal still runs; the ONTOLOGY
/// section is refused with `skipped_reason = "degraded_config"`, exit 1.
#[test]
fn heal_yes_with_degraded_config_still_source_heals_but_refuses_ontology() {
    with_seeded_heal_brain(|dir| {
        std::fs::write(dir.join("config.json"), b"{not json").unwrap();
        let out = run_ct(dir, &["heal", "--yes"]);
        assert_eq!(out.status.code(), Some(1));
        let summary = parse_stdout_object(&out);
        assert_eq!(summary["soft_deleted"], 1, "source-heal ran: {summary}");
        assert_eq!(
            summary["ontology"]["skipped_reason"], "degraded_config",
            "{summary}"
        );
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("degraded"), "loud refusal: {err}");
    });
}

/// plan-p9-M3: read-only `ct heal` (no --yes) on an OLD-SCHEMA database
/// reports "schema pending (read-only)" instead of failing; stdout empty.
#[test]
fn heal_refusal_on_old_schema_db_reports_schema_pending_readonly() {
    let brain = tempdir().unwrap();
    let dir = brain.path().to_path_buf();
    let dir_str = dir.to_str().unwrap().to_string();
    with_vars([("CURATED_BRAIN_DIR", Some(dir_str.as_str()))], move || {
        // Minimal pre-V27 schema: entries table only, no llm_wiki_meta /
        // entity_type_origin.
        {
            let conn = rusqlite::Connection::open(dir.join("brain.db")).unwrap();
            conn.execute(
                "CREATE TABLE llm_wiki_entries (
                    id TEXT PRIMARY KEY, entity_id TEXT, title TEXT, body TEXT,
                    tags TEXT, confidence TEXT, source_type TEXT, source_ref TEXT,
                    created_at INTEGER, updated_at INTEGER, deleted_at INTEGER
                 )",
                [],
            )
            .unwrap();
        }
        let out = run_ct(&dir, &["heal"]);
        assert_eq!(out.status.code(), Some(1));
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            err.contains("schema pending (read-only)"),
            "census must degrade to a note: {err}"
        );
        assert!(
            out.stdout.is_empty(),
            "refusal arm keeps stdout empty for scripts"
        );
    });
}

/// R2.6.2 end-to-end: pre-wave-1 `agent` row (ledger-less) → alias retype
/// to `role` with an `alias_retype` origin row; re-run surfaces nothing new.
/// A drifted `character` row goes to the queue untouched.
#[test]
fn heal_yes_alias_remaps_pre_wave1_rows_and_queues_the_ambiguous() {
    let brain = tempdir().unwrap();
    let dir = brain.path().to_path_buf();
    let dir_str = dir.to_str().unwrap().to_string();
    with_vars([("CURATED_BRAIN_DIR", Some(dir_str.as_str()))], move || {
        init_brain_db(&dir);
        seed_tier_fact_manifest(&dir);
        seed_entity(&dir, "e_agent", "agent");
        seed_entity(&dir, "e_char", "character");
        let out = run_ct(&dir, &["heal", "--yes"]);
        assert_eq!(out.status.code(), Some(0));
        let summary = parse_stdout_object(&out);
        assert_eq!(summary["ontology"]["retyped"], 1, "{summary}");
        assert_eq!(summary["ontology"]["queued"], 1, "{summary}");
        assert_eq!(entity_type_of(&dir, "e_agent"), "role");
        assert_eq!(entity_type_of(&dir, "e_char"), "character");
        // Idempotent re-run: nothing new surfaces.
        let out2 = run_ct(&dir, &["heal", "--yes"]);
        assert_eq!(out2.status.code(), Some(0));
        let summary2 = parse_stdout_object(&out2);
        assert_eq!(summary2["ontology"]["retyped"], 0, "{summary2}");
        assert_eq!(summary2["ontology"]["queued"], 1, "{summary2}");
    });
}

/// plan-p7-m2 test: a PRE-WAVE-1 manifest (no `document`) + a `document`
/// row → the ensure legalizes it BEFORE the census → zero queued.
#[test]
fn heal_yes_pre_wave1_manifest_document_rows_not_queued() {
    let brain = tempdir().unwrap();
    let dir = brain.path().to_path_buf();
    let dir_str = dir.to_str().unwrap().to_string();
    with_vars([("CURATED_BRAIN_DIR", Some(dir_str.as_str()))], move || {
        init_brain_db(&dir);
        seed_tier_fact_manifest(&dir);
        seed_entity(&dir, "e_doc", "document");
        let out = run_ct(&dir, &["heal", "--yes"]);
        assert_eq!(out.status.code(), Some(0));
        let summary = parse_stdout_object(&out);
        assert_eq!(summary["ontology"]["queued"], 0, "{summary}");
        assert_eq!(summary["ontology"]["retyped"], 0, "{summary}");
        assert_eq!(entity_type_of(&dir, "e_doc"), "document");
    });
}
