//! Phase-2 (issue #186 §2.4): one-shot re-grade of the unanchored librarian
//! evidence stock, plus export-before-purge of the doomed rows.
//!
//! Phase 1 backfilled `librarian_evidence` rows and flagged any blob without a
//! live chunk anchor `unanchored = 1`. Two things changed since: the insert-
//! time skip gate (phase-2 commit gate) means new unanchored stock is no
//! longer written, and chunking/re-anchoring may have since brought chunks
//! back that a flagged blob anchors — so the flag is now potentially stale in
//! BOTH directions. The re-grade re-evaluates every LIVE flagged row:
//!
//! - anchor is live again  → clear the flag (harmless UPDATE, always runs);
//! - still anchorless      → the row is DOOMED: it is a provenance-less fact
//!   that no retraction can ever match and no proposal can ever be
//!   re-attributed through. It is exported (full row + evidence) before the
//!   destructive phase, and purged only when the export is provably complete.
//!
//! Soft-deleted stock (`deleted_at IS NOT NULL`) is NEVER touched here —
//! the 90-day prune owns it.
//!
//! The destructive phase is gated TWICE: once on `brain_is_complete` (a
//! partial brain must not be mistaken for a brain full of orphans) and once
//! on a DISK-derived recount of the exported JSON files (spec I1) — a table-
//! level check alone cannot prove the backup actually landed. Every skip is
//! a loud WARN naming the idempotent manual recovery: `ct evidence regrade`.

use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;

/// Where the doomed-row backup lands, under the brain directory. Kept
/// distinct from V18's `repair-export-186/` so an operator can tell the two
/// repairs' exports apart at a glance.
pub const REGRADE_EXPORT_DIR: &str = "repair-export-phase2";

/// Outcome of one `regrade_unanchored` pass.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RegradeReport {
    /// Live flagged rows whose blob anchors a live chunk again — flag cleared.
    pub regraded_anchored: usize,
    /// Doomed rows exported to `<db_dir>/repair-export-phase2/<id>.json`.
    pub exported: usize,
    /// Doomed rows hard-deleted (entry + evidence + outbox Delete push).
    pub purged: usize,
    /// True when the destructive phase did not run (pathless DB, brain-
    /// incomplete, or export recount miss). Re-grades still stand.
    pub skipped_destructive: bool,
}

/// One doomed row: everything needed to export it fully and purge it safely.
#[derive(Debug, Clone)]
pub struct DoomedRow {
    pub entry_id: String,
    pub entity_id: String,
    pub title: String,
    pub body: String,
    pub source_ref: Option<String>,
    pub created_at: i64,
    pub evidence_json: String,
    pub proposal_id: String,
}

/// Export every doomed row to `<dir>/<entry_id>.json` — the FULL row plus its
/// evidence blob and proposal id. The token `source_ref` is content-free, so
/// anything less than the full row destroys the only surviving provenance
/// copy (spec C1).
///
/// Mirrors `evidence_repair::export_damaged_rows` in shape but carries the
/// extra evidence fields, so it is a dedicated exporter rather than a
/// force-fit of the V18 one.
pub fn export_doomed_rows(_conn: &Connection, doomed: &[DoomedRow], dir: &Path) -> Result<usize> {
    std::fs::create_dir_all(dir)?;
    for row in doomed {
        let payload = serde_json::json!({
            "id": row.entry_id,
            "entity_id": row.entity_id,
            "title": row.title,
            "body": row.body,
            "source_ref": row.source_ref,
            "created_at": row.created_at,
            "evidence_json": row.evidence_json,
            "proposal_id": row.proposal_id,
        });
        std::fs::write(
            dir.join(format!("{}.json", row.entry_id)),
            serde_json::to_string_pretty(&payload)?,
        )?;
    }
    Ok(doomed.len())
}

/// Hard-delete every doomed row, gated on a DISK-derived completeness check
/// (spec I1): before touching anything, re-verify that the backup file for
/// EVERY doomed id actually exists under `export_dir`. A table-level
/// exported==expected count cannot distinguish a correct backup from one
/// where a write silently failed; per-id existence can. Any miss → loud WARN
/// naming the idempotent manual recovery command, delete NOTHING, and report
/// `skipped_destructive = true`.
///
/// The purge itself is ONE transaction (spec I2) through the shared
/// [`crate::db::commit::hard_delete_entries`] ceremony: per row an
/// `OutboxOperation::Delete` push, the paired `librarian_evidence` delete and
/// the entry hard-DELETE; then a single batched `purge_edges_for_hard_deleted`
/// sweep (the reviewed choice — edges anchored on a hard-deleted id can never
/// come back, and the sweep must follow the delete or the edge rows dangle,
/// #158 contract).
///
/// Returns `(purged, skipped_destructive)`.
pub fn purge_doomed_rows(
    conn: &Connection,
    doomed: &[DoomedRow],
    export_dir: &Path,
    now_ms: i64,
) -> Result<(usize, bool)> {
    let missing: Vec<&str> = doomed
        .iter()
        .map(|r| r.entry_id.as_str())
        .filter(|id| !export_dir.join(format!("{id}.json")).exists())
        .collect();
    if !missing.is_empty() {
        eprintln!(
            "[ct::repair WARN] #186 V20 re-grade SKIPPED: backup export is \
             incomplete — {} of {} doomed row(s) missing from {}: {:?}. \
             Deletion must not proceed without a complete per-row backup \
             (spec §2.5.2); investigate and invoke the idempotent \
             `ct evidence regrade` manually.",
            missing.len(),
            doomed.len(),
            export_dir.display(),
            missing
        );
        return Ok((0, true));
    }

    let tx = conn.unchecked_transaction()?;
    // The shared hard-delete ceremony (outbox Delete + entry DELETE + paired
    // evidence delete + batched edge sweep, one transaction — see
    // `commit::hard_delete_entries`): this path must never drift from the
    // other three hard-delete sites again (issue #132 class).
    let doomed_pairs: Vec<(String, String)> = doomed
        .iter()
        .map(|r| (r.entry_id.clone(), r.entity_id.clone()))
        .collect();
    crate::db::commit::hard_delete_entries(&tx, &doomed_pairs, now_ms)?;
    tx.commit()?;
    Ok((doomed.len(), false))
}

/// One-shot re-grade of the live unanchored stock (issue #186 §2.4).
///
/// See the module docs for the full contract. Idempotent: a second pass on a
/// settled brain finds no flagged rows and returns a default report without
/// touching the export directory.
pub fn regrade_unanchored(
    conn: &Connection,
    db_dir: Option<&Path>,
    now_ms: i64,
) -> Result<RegradeReport> {
    let mut report = RegradeReport::default();

    // 1. Live flagged rows only — soft-deleted stock is prune's business.
    let mut stmt = conn.prepare(
        "SELECT le.entry_id, le.evidence_json
           FROM librarian_evidence le
           JOIN llm_wiki_entries e ON e.id = le.entry_id
          WHERE le.unanchored = 1 AND e.deleted_at IS NULL",
    )?;
    let flagged: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    // 2. Re-grade each against CURRENT chunk state.
    let mut doomed: Vec<DoomedRow> = Vec::new();
    for (entry_id, evidence_json) in flagged {
        if crate::db::commit::evidence_has_live_chunk(conn, &evidence_json)? {
            conn.execute(
                "UPDATE librarian_evidence SET unanchored = 0 WHERE entry_id = ?1",
                [&entry_id],
            )?;
            report.regraded_anchored += 1;
        } else {
            let mut row = conn.query_row(
                "SELECT entity_id, title, body, source_ref, created_at
                   FROM llm_wiki_entries WHERE id = ?1",
                [&entry_id],
                |r| {
                    Ok(DoomedRow {
                        entry_id: entry_id.clone(),
                        entity_id: r.get(0)?,
                        title: r.get(1)?,
                        body: r.get(2)?,
                        source_ref: r.get(3)?,
                        created_at: r.get(4)?,
                        evidence_json: evidence_json.clone(),
                        proposal_id: String::new(),
                    })
                },
            )?;
            row.proposal_id = crate::db::commit::proposal_id_from_evidence_json(&evidence_json)
                .unwrap_or_default();
            doomed.push(row);
        }
    }

    // 3. Nothing doomed → done (idempotency early-out).
    if doomed.is_empty() {
        return Ok(report);
    }

    // 4. Export (backed path only). fs/backup failures take the V18 fail-safe
    //    posture (V18 review round 5, finding 4): WARN + skip the destructive
    //    phase + let migrate() stamp — blocking startup on a backup-path defect
    //    would turn a data defect into an availability outage. SQL errors still
    //    propagate (genuine DB faults fail loud and re-run on next open).
    match db_dir {
        Some(dir) => {
            let export_dir = dir.join(REGRADE_EXPORT_DIR);
            match export_doomed_rows(conn, &doomed, &export_dir) {
                Ok(n) => report.exported = n,
                Err(e) => {
                    eprintln!(
                        "[ct::repair WARN] #186 V20 re-grade SKIPPED: backup export \
                         failed: {e:#}. The destructive phase did not run; doomed rows \
                         survive as-is. This gate does NOT re-run automatically — fix \
                         the cause (disk space, permissions on {}/) and invoke the \
                         idempotent `ct evidence regrade` manually.",
                        export_dir.display()
                    );
                    report.skipped_destructive = true;
                    return Ok(report);
                }
            }
        }
        None => {
            eprintln!(
                "[ct::repair WARN] #186 V20 re-grade: database path unavailable \
                 (in-memory or unknown); re-grade ran unbacked and the destructive \
                 phase is SKIPPED — invoke the idempotent `ct evidence regrade` \
                 manually on the file-backed brain."
            );
            report.skipped_destructive = true;
            return Ok(report);
        }
    }

    // 5. Brain-completeness gate: a partial brain must not be mistaken for a
    //    brain full of orphans. Re-grades stand; the doomed rows stay.
    if !crate::db::evidence_repair::brain_is_complete(conn)? {
        eprintln!(
            "[ct::repair WARN] #186 V20 re-grade SKIPPED: database is not \
             brain-complete; destructive phase skipped — invoke the idempotent \
             `ct evidence regrade` manually after investigating."
        );
        report.skipped_destructive = true;
        return Ok(report);
    }

    // 6. Purge — its disk-derived recount gate owns the final completeness
    //    check (spec I1). Unreachable with db_dir = None (step 4 early-returns
    //    above); the else arm keeps that invariant loud instead of silently
    //    mis-reporting skipped_destructive = false.
    let Some(dir) = db_dir else {
        eprintln!(
            "[ct::repair WARN] #186 V20 re-grade: database path became unavailable \
             before the purge; destructive phase SKIPPED — invoke the idempotent \
             `ct evidence regrade` manually."
        );
        report.skipped_destructive = true;
        return Ok(report);
    };
    let export_dir = dir.join(REGRADE_EXPORT_DIR);
    let (purged, skipped) = purge_doomed_rows(conn, &doomed, &export_dir, now_ms)?;
    report.purged = purged;
    report.skipped_destructive = skipped;

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::open_in_memory;

    const ANCHOR_HASH: &str = "feedface00000000000000000000000000000000000000000000000000000000";

    /// Insert a documents+chunks pair so `evidence_has_live_chunk` can find
    /// the anchor, then seed one flagged librarian fact.
    fn seed_flagged(conn: &Connection, entry_id: &str, anchored: bool, deleted_at: Option<i64>) {
        if anchored {
            conn.execute(
                "INSERT OR IGNORE INTO documents (path, hash, tier, status)
                 VALUES ('/v/notes.md', 'h', 'user_doc', 'indexed')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT OR IGNORE INTO chunks (doc_id, chunk_text, position, entity_id, content_hash)
                 VALUES (1, 'c', 0, 'ent', ?1)",
                [ANCHOR_HASH],
            )
            .unwrap();
        }
        let evidence = if anchored {
            format!(
                r#"{{"evidence":[{{"chunk_id":1,"content_hash":"{ANCHOR_HASH}"}}],"proposal_id":"prop_{entry_id}"}}"#
            )
        } else {
            format!(r#"{{"evidence":[],"proposal_id":"prop_{entry_id}"}}"#)
        };
        conn.execute(
            "INSERT INTO llm_wiki_entries (id, entity_id, title, body, tags, confidence,
                 source_type, source_ref, created_at, updated_at, deleted_at)
             VALUES (?1, 'ent', 't', 'b', '[]', 'inferred', 'librarian_inferred',
                     NULL, 1, 1, ?2)",
            rusqlite::params![entry_id, deleted_at],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO librarian_evidence (entry_id, proposal_id, evidence_json,
                 unanchored, created_at)
             VALUES (?1, ?2, ?3, 1, 1)",
            rusqlite::params![entry_id, format!("prop_{entry_id}"), evidence],
        )
        .unwrap();
    }

    #[test]
    fn regrade_clears_anchored_exports_and_purges_doomed() {
        let conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO documents (path, hash, tier, status)
             VALUES ('/v/notes.md', 'h', 'user_doc', 'indexed')",
            [],
        )
        .unwrap();
        seed_flagged(&conn, "fact_anchored_a", true, None);
        seed_flagged(&conn, "fact_anchored_b", true, None);
        seed_flagged(&conn, "fact_doomed_a", false, None);
        seed_flagged(&conn, "fact_doomed_b", false, None);
        seed_flagged(&conn, "fact_soft_a", false, Some(999));
        seed_flagged(&conn, "fact_soft_b", false, Some(999));
        seed_flagged(&conn, "fact_soft_c", false, Some(999));

        let dir = tempfile::TempDir::new().unwrap();
        let report = regrade_unanchored(&conn, Some(dir.path()), 1_000).unwrap();

        assert_eq!(report.regraded_anchored, 2);
        assert_eq!(report.exported, 2);
        assert_eq!(report.purged, 2);
        assert!(!report.skipped_destructive);

        // Anchored ones cleared.
        for id in ["fact_anchored_a", "fact_anchored_b"] {
            let unanchored: i64 = conn
                .query_row(
                    "SELECT unanchored FROM librarian_evidence WHERE entry_id = ?1",
                    [id],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(unanchored, 0, "{id} must be re-graded to anchored");
        }

        // Doomed ones GONE: entry hard-deleted, evidence gone, outbox Delete
        // pushed, no dangling edges.
        for id in ["fact_doomed_a", "fact_doomed_b"] {
            let entries: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM llm_wiki_entries WHERE id = ?1",
                    [id],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(entries, 0, "{id} entry must be hard-deleted");
            let evidence: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM librarian_evidence WHERE entry_id = ?1",
                    [id],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(evidence, 0, "{id} evidence must be gone");
            let deletes: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM llm_wiki_outbox
                      WHERE record_id = ?1 AND operation = 'DELETE'",
                    [id],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(deletes, 1, "{id} must have exactly one outbox Delete");
        }

        // Soft-deleted flagged rows UNTOUCHED — prune owns them.
        let soft: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM librarian_evidence le
                  JOIN llm_wiki_entries e ON e.id = le.entry_id
                 WHERE le.unanchored = 1 AND e.deleted_at IS NOT NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(soft, 3, "soft-deleted stock must be untouched");

        // Export dir holds exactly the doomed rows, as FULL-row JSON.
        let export_dir = dir.path().join(REGRADE_EXPORT_DIR);
        let mut names: Vec<String> = std::fs::read_dir(&export_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "fact_doomed_a.json".to_string(),
                "fact_doomed_b.json".to_string()
            ]
        );
        let raw = std::fs::read_to_string(export_dir.join("fact_doomed_a.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
        for key in [
            "id",
            "entity_id",
            "title",
            "body",
            "source_ref",
            "created_at",
            "evidence_json",
            "proposal_id",
        ] {
            assert!(
                parsed.get(key).is_some(),
                "export must carry `{key}` (full-row backup, spec C1)"
            );
        }
        assert_eq!(parsed["proposal_id"], "prop_fact_doomed_a");
    }

    #[test]
    fn regrade_export_miss_gate_skips_purge_but_regrades_stand() {
        let conn = open_in_memory().unwrap();
        seed_flagged(&conn, "fact_doomed_a", false, None);
        seed_flagged(&conn, "fact_doomed_b", false, None);

        // Brain-completeness requires a documents row for the chunks to hang
        // off; nothing is anchored here so no chunks were seeded.
        conn.execute(
            "INSERT INTO documents (path, hash, tier, status)
             VALUES ('/v/notes.md', 'h', 'user_doc', 'indexed')",
            [],
        )
        .unwrap();

        let dir = tempfile::TempDir::new().unwrap();
        let export_dir = dir.path().join(REGRADE_EXPORT_DIR);
        let doomed = vec![
            DoomedRow {
                entry_id: "fact_doomed_a".into(),
                entity_id: "ent".into(),
                title: "t".into(),
                body: "b".into(),
                source_ref: None,
                created_at: 1,
                evidence_json: r#"{"evidence":[],"proposal_id":"prop_a"}"#.into(),
                proposal_id: "prop_a".into(),
            },
            DoomedRow {
                entry_id: "fact_doomed_b".into(),
                entity_id: "ent".into(),
                title: "t".into(),
                body: "b".into(),
                source_ref: None,
                created_at: 1,
                evidence_json: r#"{"evidence":[],"proposal_id":"prop_b"}"#.into(),
                proposal_id: "prop_b".into(),
            },
        ];
        assert_eq!(export_doomed_rows(&conn, &doomed, &export_dir).unwrap(), 2);
        // Simulate a partial backup: one file vanishes after export.
        std::fs::remove_file(export_dir.join("fact_doomed_b.json")).unwrap();

        let (purged, skipped) = purge_doomed_rows(&conn, &doomed, &export_dir, 1_000).unwrap();
        assert_eq!(purged, 0, "a missed backup file must delete NOTHING");
        assert!(skipped);
        // Survivors on disk and in the DB.
        assert!(export_dir.join("fact_doomed_a.json").exists());
        let entries: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_entries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(entries, 2, "no rows may be purged behind a partial backup");
    }

    #[test]
    fn regrade_pathless_db_runs_regrade_skips_purge() {
        let conn = open_in_memory().unwrap();
        seed_flagged(&conn, "fact_doomed", false, None);
        seed_flagged(&conn, "fact_anchored", true, None);

        let report = regrade_unanchored(&conn, None, 1_000).unwrap();
        assert_eq!(report.regraded_anchored, 1);
        assert_eq!(report.exported, 0);
        assert_eq!(report.purged, 0);
        assert!(report.skipped_destructive);
        let still: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_entries WHERE id = 'fact_doomed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(still, 1, "pathless DB must never destroy rows");
    }

    #[test]
    fn regrade_is_idempotent() {
        let conn = open_in_memory().unwrap();
        seed_flagged(&conn, "fact_doomed", false, None);
        seed_flagged(&conn, "fact_anchored", true, None);
        let dir = tempfile::TempDir::new().unwrap();

        let first = regrade_unanchored(&conn, Some(dir.path()), 1_000).unwrap();
        assert_eq!(first.purged, 1);
        let entries_before: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_entries", [], |r| r.get(0))
            .unwrap();

        let second = regrade_unanchored(&conn, Some(dir.path()), 2_000).unwrap();
        assert_eq!(
            second,
            RegradeReport::default(),
            "second pass must be a no-op"
        );
        let entries_after: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_entries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(entries_before, entries_after);
    }

    #[test]
    fn regrade_brain_incomplete_skips_destructive_phase() {
        let conn = open_in_memory().unwrap();
        // A chunk-derived source_ref with NO chunks table content is exactly
        // the "brain incomplete" signal brain_is_complete() detects — a
        // partial import that dropped its chunks.
        conn.execute(
            "INSERT INTO llm_wiki_entries
                (id, entity_id, title, body, tags, confidence, source_type,
                 source_ref, created_at, updated_at)
             VALUES ('trap', 'ent', 't', 'b', '[]', 'inferred',
                     'librarian_inferred',
                     'evidencechunk_id12content_hashdeadbeefquotehello', 1, 1)",
            [],
        )
        .unwrap();
        seed_flagged(&conn, "fact_doomed", false, None);

        let dir = tempfile::TempDir::new().unwrap();
        let report = regrade_unanchored(&conn, Some(dir.path()), 1_000).unwrap();
        assert_eq!(report.regraded_anchored, 0);
        assert_eq!(report.purged, 0);
        assert!(report.skipped_destructive);
        let still: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_entries WHERE id = 'fact_doomed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(still, 1, "brain-incomplete must keep the doomed row");
    }

    #[test]
    fn regrade_purges_dangling_edges_of_doomed_rows() {
        let conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO documents (path, hash, tier, status)
             VALUES ('/v/notes.md', 'h', 'user_doc', 'indexed')",
            [],
        )
        .unwrap();
        seed_flagged(&conn, "fact_doomed", false, None);
        seed_flagged(&conn, "fact_other", true, None);
        conn.execute(
            "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id,
                 edge_type, created_at)
             VALUES ('edge-1', 'ent', 'fact_doomed', 'fact_other', 'supports', 1)",
            [],
        )
        .unwrap();

        let dir = tempfile::TempDir::new().unwrap();
        let report = regrade_unanchored(&conn, Some(dir.path()), 1_000).unwrap();
        assert_eq!(report.purged, 1);
        let edges: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            edges, 0,
            "edges anchored on a hard-deleted id must be swept"
        );
    }
}
