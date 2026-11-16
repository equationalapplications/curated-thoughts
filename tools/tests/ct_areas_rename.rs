//! `ct wisdom areas rename` contract tests (spec 2026-11-14-areas §12, rev 12;
//! owner rulings 2026-11-16: selector = `--hub <entity id>`, audit = one
//! `curated_agent_log` row in the rename transaction).
//!
//! Lib-side unit tests (transaction, merge-group expansion, rollback) live in
//! `src-tauri/src/db/areas_rename.rs`'s `mod tests`; this file pins the CLI
//! surface: flag wiring, dry-run DEFAULT, `--yes`, and refusal error text.

mod common;

use common::{run_ct, with_seeded_brain};
use rusqlite::params;
use std::path::PathBuf;
use tempfile::TempDir;

fn stderr(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

fn stdout(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// Open the fixture brain.db directly (with_seeded_brain pointed
/// CURATED_BRAIN_DIR at it).
fn brain_db() -> rusqlite::Connection {
    let dir = std::env::var("CURATED_BRAIN_DIR").expect("with_seeded_brain sets it");
    rusqlite::Connection::open(std::path::Path::new(&dir).join("brain.db")).unwrap()
}

/// Fixture: config.json carries `wisdom.areas_paths` mapping `ent-hub` to a
/// real areas file defining `old` and `new`; the brain holds live entity
/// `ent-hub`, a merged loser `ent-loser`, facts under both ids (area `old`),
/// and a stale-name row on an unrelated entity. Returns the tempdir that owns
/// the areas file (keep alive for the test body).
fn fixture_areas_and_config() -> TempDir {
    let dir = TempDir::new().unwrap();
    let areas = dir.path().join("checkout");
    std::fs::create_dir_all(&areas).unwrap();
    let areas_path = areas.join(".ct-areas.yaml");
    std::fs::write(
        &areas_path,
        "areas:\n  old:\n    - \"**\"\n  new:\n    - \"src/**\"\n",
    )
    .unwrap();
    let areas_path = areas_path.canonicalize().unwrap();
    let config = serde_json::json!({
        "vault_path": null,
        "wisdom": {
            "areas_paths": { "ent-hub": areas_path.display().to_string() }
        }
    });
    let brain_dir = std::env::var("CURATED_BRAIN_DIR").expect("with_seeded_brain sets it");
    std::fs::write(
        PathBuf::from(&brain_dir).join("config.json"),
        config.to_string(),
    )
    .unwrap();

    {
        let conn = brain_db();
        for id in ["ent-hub", "ent-loser", "ent-other"] {
            conn.execute(
                "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
                 VALUES (?1, ?1, 'concept', 'summary', 100, 100)",
                params![id],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO entity_redirects (entity_id, merged_into, created_at)
             VALUES ('ent-loser', 'ent-hub', 1)",
            [],
        )
        .unwrap();
        for (id, entity) in [
            ("fact-surv", "ent-hub"),
            ("fact-lose", "ent-loser"),
            ("fact-other", "ent-other"),
        ] {
            conn.execute(
                "INSERT INTO llm_wiki_entries (
                    id, entity_id, title, body, tags, confidence, source_type,
                    source_hash, source_ref, created_at, updated_at, last_accessed_at,
                    access_count, deleted_at, embedding_blob, embedding, area
                 ) VALUES (?1, ?2, ?1, ?1, '[]', 'inferred', 'librarian_inferred',
                           NULL, NULL, 100, 100, NULL, 0, NULL, NULL, NULL, 'old')",
                params![id, entity],
            )
            .unwrap();
        }
    }
    dir
}

fn fact_area(id: &str) -> Option<String> {
    let conn = brain_db();
    conn.query_row(
        "SELECT area FROM llm_wiki_entries WHERE id = ?1",
        params![id],
        |r| r.get(0),
    )
    .unwrap()
}

/// Dry-run DEFAULT: the preview prints (hub, old → new, row count) and the
/// database — rows, outbox, audit — is untouched (§12.1).
#[test]
fn dry_run_is_the_default_and_writes_nothing() {
    with_seeded_brain(|| {
        let _guard = fixture_areas_and_config();
        let out = run_ct(&["wisdom", "areas", "rename", "--hub", "ent-hub", "old", "new"]);
        assert_eq!(out.status.code(), Some(0), "stderr={}", stderr(&out));
        let so = stdout(&out);
        assert!(so.contains("dry-run"), "stdout={so}");
        assert!(so.contains("ent-hub") && so.contains("old") && so.contains("new"));
        assert!(so.contains('2'), "preview names the affected count: {so}");

        assert_eq!(fact_area("fact-surv"), Some("old".into()));
        assert_eq!(fact_area("fact-lose"), Some("old".into()));
        let conn = brain_db();
        let outbox: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_outbox", [], |r| r.get(0))
            .unwrap();
        assert_eq!(outbox, 0, "dry run stages no outbox rows");
        let audit: i64 = conn
            .query_row("SELECT COUNT(*) FROM curated_agent_log", [], |r| r.get(0))
            .unwrap();
        assert_eq!(audit, 0, "dry run writes no audit row");
    });
}

/// `--yes` executes: both merge-group rows rename and BOTH outbox Updates
/// emit (gate-5 MAJOR-1), plus the ONE audit row (owner ruling).
#[test]
fn yes_executes_the_rename_through_the_outbox() {
    with_seeded_brain(|| {
        let _guard = fixture_areas_and_config();
        let out = run_ct(&[
            "wisdom",
            "areas",
            "rename",
            "--hub",
            "ent-hub",
            "old",
            "new",
            "--yes",
        ]);
        assert_eq!(out.status.code(), Some(0), "stderr={}", stderr(&out));
        assert!(stdout(&out).contains("renamed 2"), "stdout={}", stdout(&out));

        assert_eq!(fact_area("fact-surv"), Some("new".into()));
        assert_eq!(fact_area("fact-lose"), Some("new".into()));
        assert_eq!(
            fact_area("fact-other"),
            Some("old".into()),
            "other groups untouched"
        );

        let conn = brain_db();
        let mut stmt = conn
            .prepare("SELECT payload FROM llm_wiki_outbox WHERE table_name = 'entries'")
            .unwrap();
        let payloads: Vec<String> = stmt
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(payloads.len(), 2, "one outbox Update per renamed row");
        for p in &payloads {
            let v: serde_json::Value = serde_json::from_str(p).unwrap();
            assert_eq!(v["area"], serde_json::json!("new"));
            assert!(v["body"].is_string(), "full-row payload");
        }

        let (count, summary): (i64, String) = conn
            .query_row(
                "SELECT COUNT(*), COALESCE(MAX(summary), '') FROM curated_agent_log
                  WHERE tool = 'wisdom_areas_rename'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(count, 1, "exactly ONE audit row (owner ruling)");
        let v: serde_json::Value = serde_json::from_str(&summary).unwrap();
        assert_eq!(v["old"], serde_json::json!("old"));
        assert_eq!(v["new"], serde_json::json!("new"));
        assert_eq!(v["rows"], serde_json::json!(2));
        assert!(v["by"].is_string());
    });
}

/// Unknown `--hub` id: nonzero exit, stderr lists ALL configured keys.
#[test]
fn unknown_hub_error_lists_configured_keys() {
    with_seeded_brain(|| {
        let _guard = fixture_areas_and_config();
        let out = run_ct(&[
            "wisdom",
            "areas",
            "rename",
            "--hub",
            "ent-nope",
            "old",
            "new",
            "--yes",
        ]);
        assert_ne!(out.status.code(), Some(0), "stderr={}", stderr(&out));
        let err = stderr(&out);
        assert!(err.contains("ent-hub"), "must list configured keys: {err}");
    });
}

/// `new` not defined by the hub's areas file: refused (nonzero), nothing
/// written.
#[test]
fn undefined_new_area_is_refused() {
    with_seeded_brain(|| {
        let _guard = fixture_areas_and_config();
        let out = run_ct(&[
            "wisdom",
            "areas",
            "rename",
            "--hub",
            "ent-hub",
            "old",
            "bogus",
            "--yes",
        ]);
        assert_ne!(out.status.code(), Some(0), "stderr={}", stderr(&out));
        assert!(stderr(&out).contains("refus"), "stderr={}", stderr(&out));
        assert_eq!(fact_area("fact-surv"), Some("old".into()));
    });
}

/// Zero matching rows: refused (documented choice — refuse never degrade).
#[test]
fn zero_matching_rows_refuses() {
    with_seeded_brain(|| {
        let _guard = fixture_areas_and_config();
        let out = run_ct(&[
            "wisdom",
            "areas",
            "rename",
            "--hub",
            "ent-hub",
            "nonexistent-area",
            "new",
            "--yes",
        ]);
        assert_ne!(out.status.code(), Some(0), "stderr={}", stderr(&out));
        assert!(stderr(&out).contains("zero"), "stderr={}", stderr(&out));
    });
}
