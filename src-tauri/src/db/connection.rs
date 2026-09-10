use crate::db::okf_ddl;
use crate::db::schema::{
    MIGRATION_V1, MIGRATION_V10, MIGRATION_V11, MIGRATION_V12, MIGRATION_V13, MIGRATION_V14,
    MIGRATION_V15, MIGRATION_V16, MIGRATION_V18, MIGRATION_V19, MIGRATION_V2, MIGRATION_V21,
    MIGRATION_V3, MIGRATION_V4, MIGRATION_V5, MIGRATION_V6, MIGRATION_V9,
};
use crate::hasher::hash_bytes;
use crate::vault::VaultConfig;
use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;

/// Outcome of one `v22_unify_documents_path` pass. Reported so the caller
/// (the V22 migration block, or its tests) can log the class-1/class-2 split
/// and a test can assert on the boundary, not just the post-state.
///
/// `rewritten` is the count of `user_doc` rows whose `path` prefix was
/// rewritten from canonical-root to configured-root. `deleted` is the count
/// of trusted-link phantom rows removed (their path was outside the canonical
/// vault root). Bounded by `SELECT COUNT(*) FROM documents WHERE tier =
/// 'user_doc'`, so neither can exceed the pre-pass count.
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub(crate) struct V22Report {
    pub rewritten: usize,
    pub deleted: usize,
}

/// The two shapes a vault root takes inside `migrate()`.
///
/// * `configured` is the root the user (or `VaultConfig`) named — the form
///   every write boundary that talks to the user wants to use.
/// * `canonical` is the same path after `canonicalize` has resolved
///   filesystem-level symlinks (macOS `/var` → `/private/var`, etc.).
///
/// V22 needs BOTH: it identifies rows by `canonical` (the form the watcher
/// has been writing, pre-fix) and rewrites them to `configured` (the form
/// the walker writes and the DB column is documented to hold). Splitting
/// the two lets V5 keep using `canonical` for its stable entity-id hash
/// while V22 introduces the rewrite, without forcing either caller to
/// recompute the canonicalization.
#[derive(Clone, Debug)]
pub struct VaultRoots {
    pub configured: String,
    pub canonical: String,
}

/// Rewrite `documents.path` from canonical to virtual form (issue #204).
///
/// For each `user_doc` row whose `path` starts with `<canonical_root>/`,
/// replace the canonical-root prefix with `<configured_root>/`. Rows whose
/// path is *outside* the canonical vault root are trusted-link phantoms (the
/// watcher's pre-fix bug; reconcile.rs:74-77 documents the column as virtual)
/// and are deleted — the walker already wrote the correct virtual-path row
/// under the same `UNIQUE(path)` constraint, or will re-ingest on next pass.
///
/// A `user_doc` row whose class-1 rewrite would COLLIDE with an existing
/// walker-written row at the same virtual path is deleted rather than
/// updated — the walker row carries the authoritative hash and synth
/// watermark, the watcher's row is a duplicate phantom of the same bytes.
/// (Plain UPDATE on collision would fail the UNIQUE(path) constraint.)
///
/// Restricted to `tier = 'user_doc'` for parity with reconcile.rs:83; wiki
/// rows have no filesystem-path semantics and must never be touched.
///
/// Idempotent: a second invocation finds zero rows matching either prefix
/// (the rewrite already landed; the delete already ran) and reports zeros.
pub(crate) fn v22_unify_documents_path(
    conn: &Connection,
    configured_root: &str,
    canonical_root: &str,
) -> Result<V22Report> {
    // Canonical/configured must be non-empty: an empty root would rewrite
    // every row's path to start with a stray `/` and silently corrupt the
    // index. Callers (the V22 block in migrate) gate on `Some(VaultRoots)`,
    // and VaultConfig never returns empty roots, so this is defensive — but
    // the alternative is a corrupt brain on a misconfigured open, which is
    // the exact failure mode V22 exists to repair.
    if configured_root.is_empty() || canonical_root.is_empty() {
        anyhow::bail!(
            "v22_unify_documents_path: empty root (configured={configured_root:?}, \
             canonical={canonical_root:?}); refusing to rewrite paths"
        );
    }

    let canonical_prefix = format!("{canonical_root}/");
    let configured_prefix = format!("{configured_root}/");
    let prefix_len = canonical_prefix.len() as i64;
    let substr_start = prefix_len + 1;

    // BEGIN IMMEDIATE: same concurrent-migration guard as V21 (a desktop app
    // and a simultaneously launching `--mcp` server can both reach this
    // block; BEGIN IMMEDIATE takes the write lock up front so exactly one
    // process runs the rewrite and the other no-ops into the version stamp).
    conn.execute_batch("BEGIN IMMEDIATE;")?;
    let result: Result<V22Report> = (|| {
        // Step 1: delete class-2 phantoms — trusted-link targets whose path
        // resolved to something OUTSIDE both the canonical vault root AND
        // the configured vault root. Checking against both roots is what
        // keeps the walker-written row (which lives at the configured_root,
        // not the canonical_root) safe: a class-1 rewrite deletes the
        // canonical-prefixed duplicate, but the virtual-path row that
        // matches `substr(path, 1, ?) = configured_prefix` must survive.
        // Must run BEFORE step 3 (the rewrite) so the rewrite doesn't
        // accidentally pull a just-modified path into the phantom branch.
        let deleted_phantoms = conn.execute(
            "DELETE FROM documents
              WHERE substr(path, 1, ?1) != ?2
                AND substr(path, 1, ?3) != ?4
                AND tier = 'user_doc'",
            rusqlite::params![
                prefix_len,
                &canonical_prefix,
                configured_prefix.len() as i64,
                &configured_prefix
            ],
        )?;

        // Step 2: delete class-1 rows that would COLLIDE with an existing
        // walker-written row at the same virtual path. The watcher row is a
        // duplicate phantom of bytes the walker already ingested, so the
        // walker row wins.
        //
        // SQLite disallows table aliases on DELETE itself, so the alias
        // sits inside the inner SELECT and the outer DELETE references the
        // candidate ids directly.
        let deleted_collisions = conn.execute(
            "DELETE FROM documents
              WHERE id IN (
                SELECT id FROM (
                  SELECT d1.id AS id FROM documents d1
                   WHERE substr(d1.path, 1, ?1) = ?2
                     AND d1.tier = 'user_doc'
                     AND EXISTS (
                       SELECT 1 FROM documents d2
                        WHERE d2.path = ?3 || substr(d1.path, ?4)
                          AND d2.tier = 'user_doc'
                          AND d2.id != d1.id
                     )
                )
              )",
            rusqlite::params![
                prefix_len,
                &canonical_prefix,
                &configured_prefix,
                substr_start
            ],
        )?;

        // Step 3: rewrite remaining class-1 rows. After steps 1+2, every
        // surviving row's rewrite target is UNIQUE, so the UPDATE cannot
        // collide.
        let rewritten = conn.execute(
            "UPDATE documents
                SET path = ?1 || substr(path, ?2)
              WHERE substr(path, 1, ?3) = ?4
                AND tier = 'user_doc'",
            rusqlite::params![
                &configured_prefix,
                substr_start,
                prefix_len,
                &canonical_prefix
            ],
        )?;

        Ok(V22Report {
            rewritten,
            deleted: deleted_collisions + deleted_phantoms,
        })
    })();
    match result {
        Ok(report) => {
            conn.execute_batch("COMMIT;")?;
            Ok(report)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK;");
            Err(e)
        }
    }
}

fn normalize_workspace_root(path: &str) -> String {
    let mut normalized = path.replace('\\', "/");
    if normalized != "/" {
        normalized = normalized.trim_end_matches('/').to_string();
        if normalized.ends_with(':') {
            normalized.push('/');
        }
        if normalized.is_empty() {
            normalized = "/".to_string();
        }
    }
    normalized
}

fn canonicalize_workspace_root(path: &str) -> String {
    std::path::Path::new(path)
        .canonicalize()
        .map(|p| normalize_workspace_root(&p.to_string_lossy()))
        .unwrap_or_else(|_| normalize_workspace_root(path))
}

fn migrate(conn: &Connection, vault_root: Option<VaultRoots>, db_dir: Option<&Path>) -> Result<()> {
    conn.execute_batch("PRAGMA foreign_keys=ON;")?;
    conn.execute_batch(&format!(
        "BEGIN;\n{}\n{}\n{}\nCOMMIT;",
        MIGRATION_V1, MIGRATION_V2, MIGRATION_V3
    ))?;

    let version: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_version",
        [],
        |r| r.get(0),
    )?;
    if version < 4 {
        conn.execute_batch(MIGRATION_V4)?;
    }
    if version < 5 {
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V5))?;
        if let Some(roots) = vault_root.as_ref() {
            let normalized_root = &roots.canonical;
            let entity_id = format!(
                "tier_working::{}",
                &hash_bytes(normalized_root.as_bytes())[..16]
            );
            conn.execute(
                "UPDATE chunks SET entity_id = ?1 WHERE entity_id = 'tier_working'",
                [entity_id.as_str()],
            )?;
        } else {
            conn.execute(
                "UPDATE documents
                 SET status = 'pending'
                 WHERE status = 'indexed'
                   AND path NOT LIKE '%/documents/%'
                   AND path NOT LIKE '%/wiki/%'
                   AND EXISTS (
                       SELECT 1 FROM chunks c
                        WHERE c.doc_id = documents.id
                          AND c.entity_id = 'tier_working'
                   )",
                [],
            )?;
        }
    }
    if version < 6 {
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V6))?;
    }
    if version < 9 {
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V9))?;
    }
    if version < 10 {
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V10))?;
    }
    if version < 11 {
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V11))?;
    }
    if version < 7 {
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", okf_ddl::migration_v7_sql()))?;
    }
    // V12 must run AFTER the OKF V7 DDL because it touches
    // `llm_wiki_entries.deleted_at`, which V7 creates. The V7/V8 gates
    // above were intentionally ordered to land before any data-migration
    // SQL; V12 follows the same convention.
    if version < 12 {
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V12))?;
    }
    if version < 13 {
        // `documents` predates this migration, so the column add is done
        // through the additive helper rather than inside the SQL constant.
        crate::db::ddl_compat::add_column_if_missing(
            conn,
            "documents",
            "quarantined_at",
            "INTEGER",
        )?;
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V13))?;
    }
    if version < 14 {
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V14))?;
    }
    if version < 15 {
        // Rebuilds `documents` to widen the status CHECK; the PRAGMA
        // foreign_keys toggles inside must not sit in a transaction, so this
        // constant manages its own statement sequence.
        conn.execute_batch(MIGRATION_V15)?;
    }
    if version < 16 {
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V16))?;
    }
    if version < 17 {
        // core-llm-wiki@7.1.0 schema (columns added in package 6.5.0): the
        // startup schema guard below demands the full 7.1.0 column set, but
        // the JS package migration that adds these columns only runs once
        // the frontend boots — after this guard. Upgrade databases here
        // instead, mirroring the package's own PRAGMA-guarded migration v11
        // verbatim. Column names and declared types are hardcoded literals,
        // so the interpolation is safe; `add_column_if_missing` cannot be
        // reused because its identifier check rejects the multi-word
        // `NOT NULL DEFAULT 0` declaration the package DDL requires.
        let existing: Vec<String> = {
            let mut stmt = conn.prepare("PRAGMA table_info(llm_wiki_entries)")?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
            rows.filter_map(Result::ok).collect()
        };
        const V17_EMBEDDING_FAILURE_COLUMNS: &[(&str, &str)] = &[
            ("embedding_failed_at", "INTEGER"),
            ("embedding_failure_kind", "TEXT"),
            ("embedding_attempts", "INTEGER NOT NULL DEFAULT 0"),
        ];
        for (column, declared_type) in V17_EMBEDDING_FAILURE_COLUMNS {
            if !existing.iter().any(|c| c == column) {
                conn.execute(
                    &format!("ALTER TABLE llm_wiki_entries ADD COLUMN {column} {declared_type}"),
                    [],
                )?;
            }
        }
        conn.execute(
            "INSERT OR IGNORE INTO schema_version (version) VALUES (17)",
            [],
        )?;
    }
    if version < 18 {
        // DDL first, stamp last: the version stamp is written only after the
        // one-shot repair below finishes. The DDL itself is idempotent (IF
        // NOT EXISTS), making any re-entry safe. Spec §2.5.
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V18))?;

        // Finding 1 (review round 5): pre-#186 wisdom writes stamped
        // user_stated rows with the JSON sentinel
        // `{"proposal_id":null,"evidence":[]}` (or its engine-mangled
        // collapse `proposal_idnullevidence`) — a non-fixed-point ref
        // outside the librarian census, so the repair below would never
        // touch it. NULL is the "no provenance" value for manual facts;
        // new wisdom writes have used it since the same review round.
        // Exact-match values keep this idempotent.
        conn.execute(
            "UPDATE llm_wiki_entries SET source_ref = NULL
              WHERE source_type = 'user_stated'
                AND source_ref IN (
                    '{\"proposal_id\":null,\"evidence\":[]}',
                    'proposal_idnullevidence'
                )",
            [],
        )?;

        // One-shot repair of the rows the engine already mangled (#186).
        // Backup-before-mutate; deletion of unresolvable rows is gated on the
        // brain-complete assertion so a partial import can never be mistaken
        // for a brain full of orphans. Spec §2.5.
        if crate::db::evidence_repair::brain_is_complete(conn)? {
            match db_dir {
                Some(dir) => {
                    let export_dir = dir.join("repair-export-186");
                    // Fail-safe posture (review round 5, finding 4): a
                    // failure inside the backed repair arm (full disk,
                    // read-only brain dir, mid-repair SQL fault) must not
                    // abort migrate() — AppDb::open would fail and every
                    // subsequent launch would retry-and-fail forever. Export
                    // problems skip the destructive phase with a loud WARN
                    // and the damaged data survives as-is, exactly like the
                    // brain-incomplete branch; the stamp below still fires
                    // (availability outranks auto-retry — the operator can
                    // rerun the idempotent `run_evidence_repair` manually).
                    let attempt = (|| -> Result<()> {
                        let census = crate::db::evidence_repair::repair_census(conn)?;
                        let exported =
                            crate::db::evidence_repair::export_damaged_rows(conn, &export_dir)?;
                        if exported as i64 != census.damaged {
                            // Verifiable backup invariant (spec §2.5.2): orphan
                            // deletion may only run when every damaged row is
                            // provably backed up. A table-level completeness
                            // check alone cannot prove a partial import didn't
                            // drop a valid fact's anchor, but an export that
                            // misses even one damaged row is a demonstrated
                            // incomplete backup — skip the destructive phase.
                            eprintln!(
                                "[ct::repair WARN] #186 V18 repair SKIPPED: backup export \
                                 wrote {exported} rows but the census counted {} damaged \
                                 rows. Deletion must not proceed without a complete \
                                 per-row backup (spec §2.5.2); investigate \
                                 repair-export-186/ and invoke the idempotent \
                                 `run_evidence_repair` manually.",
                                census.damaged
                            );
                            return Ok(());
                        }
                        let report = crate::db::evidence_repair::run_evidence_repair(
                            conn,
                            crate::db::commit::ms_now(),
                        )?;
                        eprintln!(
                            "[ct::repair] #186 V18 repair: exported={exported} outbox={} \
                             valid_json={} proposal_id={} content_hash={} deleted={} \
                             ambiguous={}",
                            report.from_outbox,
                            report.from_valid_json,
                            report.from_proposal_id,
                            report.from_content_hash,
                            report.deleted,
                            report.ambiguous
                        );
                        Ok(())
                    })();
                    if let Err(err) = attempt {
                        eprintln!(
                            "[ct::repair WARN] #186 V18 repair SKIPPED: {err:#}. The \
                             destructive phase did not run (or stopped part-way); damaged \
                             rows survive as-is. This repair does NOT re-run automatically \
                             — after fixing the underlying cause (disk space, permissions \
                             on repair-export-186/), invoke the idempotent \
                             `run_evidence_repair` manually."
                        );
                    }
                }
                None => {
                    // In-memory / pathless database (tests, ephemeral opens):
                    // there is no brain directory to back up into, so skip the
                    // export but still run the repair — failing the migration
                    // here would defeat the whole one-shot. The DB path is
                    // unknown to this function in this branch, so the log can
                    // only say so. Errors propagate here: this branch is
                    // tests-only in practice, and a swallowed SQL fault would
                    // hide real defects from the suite.
                    eprintln!(
                        "[ct::repair WARN] #186 V18 repair: database path unavailable \
                         (in-memory or unknown); skipping backup export and running \
                         repair unbacked"
                    );
                    let report = crate::db::evidence_repair::run_evidence_repair(
                        conn,
                        crate::db::commit::ms_now(),
                    )?;
                    eprintln!(
                        "[ct::repair] #186 V18 repair (unbacked): exported=0 outbox={} \
                         valid_json={} proposal_id={} content_hash={} deleted={} ambiguous={}",
                        report.from_outbox,
                        report.from_valid_json,
                        report.from_proposal_id,
                        report.from_content_hash,
                        report.deleted,
                        report.ambiguous
                    );
                }
            }
        } else {
            eprintln!(
                "[ct::repair WARN] #186 V18 repair SKIPPED: database is not brain-complete \
                 (missing or empty chunks/documents while chunk-derived refs exist). \
                 This repair does NOT re-run automatically — the V18 schema stamp means \
                 this branch fires only once. It can be triggered later via a future \
                 migration or by manually invoking the idempotent \
                 `run_evidence_repair`."
            );
        }
        // Stamp after the repair attempt, deliberately: the brain-incomplete
        // skip, the backup-invariant skip, and a caught repair error are all
        // "skip with a loud WARN" decisions, not failures — blocking startup
        // on them would turn a data defect into an availability outage
        // (review round 5, finding 4). Manual `run_evidence_repair` remains
        // the recovery path for any skipped repair.
        conn.execute(
            "INSERT OR IGNORE INTO schema_version (version) VALUES (18)",
            [],
        )?;
    }
    if version < 19 {
        // Wrapped in an explicit transaction even though this is a single
        // statement SQLite would make atomic on its own: the boundary is
        // stated so that chunking this UPDATE, or landing another statement
        // beside it, cannot silently lose the guarantee. A partial V19 is
        // not an acceptable outcome under any future edit here.
        //
        // Stamp last, matching V18: a crash before the stamp re-runs a body
        // that is idempotent by construction (see MIGRATION_V19). Spec §2.5.
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V19))?;

        conn.execute(
            "INSERT OR IGNORE INTO schema_version (version) VALUES (19)",
            [],
        )?;
    }

    if version < 20 {
        // The Phase-2 flip itself (issue #186 §2.4): re-grade every live
        // unanchored librarian_evidence row against current chunk state.
        // Anchored rows are cleared; still-orphaned rows are exported to
        // `<db_dir>/repair-export-phase2/` and then purged — export
        // before purge is a hard gate, and the purge helper re-counts the
        // exported *.json files per doomed id before deleting anything.
        //
        // There is no SQL body: the work lives in
        // `evidence_regrade::regrade_unanchored`. The block is idempotent
        // (a settled brain has no flagged rows) but still stamp-last,
        // matching V18/V19: a crash before the stamp re-runs a body that is
        // safe to re-run. Every skip path WARNs and names the idempotent
        // manual recovery command `ct evidence regrade`.
        //
        // UNLIKE V18, a skipped destructive phase does NOT stamp (review
        // finding on PR #201): heal runs `source_ref_is_still_grounded`
        // strictly again (the Phase-2 carve-out revert), so if V20 stamped
        // while doomed rows were still live, the same session's heal would
        // soft-delete those rows BEFORE `ct evidence regrade` could export
        // them — regrade only selects `deleted_at IS NULL` stock, so the
        // documented recovery would report zeros, and the 7-day prune would
        // hard-delete the never-exported rows. Leaving the stamp unwritten
        // keeps `MAX(version) < 20` a durable "recovery pending" marker that
        // heal defers to (see `source_ref_is_still_grounded`), makes every
        // open retry the (idempotent) re-grade, and gates V21 below until
        // the brain settles — an exceptional, loudly-WARNed, self-healing
        // state rather than an availability outage.
        let mut v20_settled = true;
        let now_ms = crate::db::commit::ms_now();
        match crate::db::evidence_regrade::regrade_unanchored(conn, db_dir, now_ms) {
            Ok(report) => {
                if report.regraded_anchored > 0
                    || report.exported > 0
                    || report.purged > 0
                    || report.skipped_destructive
                {
                    println!(
                        "[ct::regrade] #186 V20: regraded_anchored={} exported={} purged={} skipped_destructive={}",
                        report.regraded_anchored,
                        report.exported,
                        report.purged,
                        report.skipped_destructive
                    );
                }
                if report.skipped_destructive {
                    v20_settled = false;
                }
            }
            Err(e) => {
                eprintln!(
                    "[ct::repair WARN] #186 V20 re-grade FAILED: {e}. The schema \
                     stamp is NOT written on an error, so the re-grade re-runs on \
                     the next open. If the failure persists, invoke the idempotent \
                     `ct evidence regrade` manually."
                );
                return Err(e);
            }
        }

        if v20_settled {
            conn.execute(
                "INSERT OR IGNORE INTO schema_version (version) VALUES (20)",
                [],
            )?;
        }
    }
    if version < 21 {
        // Human Verification Gate (hvg): add the nullable
        // `curated_proposals.reviewed_by` column. ALTER TABLE ADD COLUMN is
        // NOT idempotent (no IF NOT EXISTS), so unlike V19's
        // WHERE-convergence this body cannot be safely re-run. SQLite DDL is
        // transactional, so the ALTER and its version stamp land in ONE
        // transaction: a crash applies both or neither, and no open can ever
        // find the column present with the version still at 20 (which would
        // replay the ALTER and fail `AppDb::open` permanently with a
        // duplicate-column error). No boot loop, no silent skip.
        //
        // V20 recovery pending (review finding on PR #201): the version
        // snapshot above predates the V20 re-grade, so a skipped destructive
        // phase leaves the brain at 19 while this block would still run —
        // stamping 21 and permanently masking the unwritten 20 (every later
        // open reads MAX(version) >= 21 and never retries the re-grade, and
        // heal's version gate stops deferring). V21 is therefore deferred
        // until V20 settles; it lands on the first open after recovery.
        let v20_recovery_pending = {
            let stamped: i64 = conn.query_row(
                "SELECT COUNT(*) FROM schema_version WHERE version >= 20",
                [],
                |r| r.get(0),
            )?;
            stamped == 0
        };
        if v20_recovery_pending {
            eprintln!(
                "[ct::repair WARN] #186 V21 DEFERRED: the V20 re-grade skipped its \
                 destructive phase and left doomed rows un-purged; adding \
                 `curated_proposals.reviewed_by` now would stamp 21 and mask the \
                 unwritten 20. Complete the recovery (fix the export dir / brain \
                 completeness, then `ct evidence regrade` or the next open) and V21 \
                 lands on the following open."
            );
        } else {
            // Concurrent-migration guard (review finding on PR #201): the
            // desktop app and a simultaneously launching `--mcp` server can
            // BOTH read version=20 at the top of migrate() and enter this
            // block; the second ALTER would then fail its open with
            // `duplicate column name: reviewed_by`. BEGIN IMMEDIATE takes
            // the write lock up front (the loser blocks on it, via the busy
            // timeout set at open), and the column is re-checked UNDER that
            // lock, so exactly one process runs the ALTER and the other
            // no-ops into the idempotent stamp.
            conn.execute_batch("BEGIN IMMEDIATE;")?;
            let applied = (|| -> Result<()> {
                let has_column: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('curated_proposals')
                      WHERE name = 'reviewed_by'",
                    [],
                    |r| r.get(0),
                )?;
                if has_column == 0 {
                    conn.execute_batch(MIGRATION_V21)?;
                }
                conn.execute(
                    "INSERT OR IGNORE INTO schema_version (version) VALUES (21)",
                    [],
                )?;
                Ok(())
            })();
            match applied {
                Ok(()) => conn.execute_batch("COMMIT;")?,
                Err(e) => {
                    let _ = conn.execute_batch("ROLLBACK;");
                    return Err(e);
                }
            }
        }
    }

    if version < 22 {
        // V22 — unify `documents.path` on the configured (virtual) form
        // (issue #204, deferred from the .brain-exclusion spec's D2a).
        //
        // Pre-fix, the watcher wrote the canonical path while the walker
        // wrote the configured-root-relative path; reconcile.rs:74-77
        // documents the column as virtual, so the watcher's row was a
        // divergent phantom. V22 rewrites class-1 rows
        // (canonical-root-prefixed) to the configured-root prefix in place
        // and deletes class-2 rows (trusted-link phantoms whose path
        // resolved outside both roots). Both branches restrict to
        // `tier = 'user_doc'` for parity with reconcile.rs:83.
        //
        // Vault-root requirement (user-approved Option A for the migration):
        // refuse to run without a resolved root, log a loud FATAL, and
        // do NOT stamp 22. The watcher keeps writing the divergent shape
        // in the meantime; every subsequent open re-logs this WARN until
        // the user resolves the root and V22 runs. Refusing to stamp is
        // deliberate — it keeps the schema below 22 as a durable
        // "recovery pending" marker, matching the V20/V21 precedent where
        // an unfinished migration leaves a trail the next open can follow.
        let Some(roots) = vault_root.as_ref() else {
            eprintln!(
                "[ct::repair FATAL] #204 V22 DEFERRED: vault root could not be \
                 resolved (no VaultConfig root, no CURATED_VAULT_ROOT, no \
                 --vault). V22 rewrites documents.path from canonical to \
                 configured-root form and CANNOT run without a root — guessing \
                 would silently corrupt the index. Set the vault root in \
                 config.json (or pass --vault / CURATED_VAULT_ROOT) and reopen; \
                 V22 lands on the next open."
            );
            // Skip the stamp: every open retries and re-warns until the user
            // resolves the root. Subsequent migrations (V23+) check the
            // schema_version gate and therefore stay blocked until V22 settles
            // — matching the V20→V21 "recovery pending" pattern.
            return Ok(());
        };
        let report = v22_unify_documents_path(conn, &roots.configured, &roots.canonical)?;
        println!(
            "[ct::repair] #204 V22: rewritten={} deleted={} (canonical-path phantoms \
             unified to configured-root form)",
            report.rewritten, report.deleted
        );
        conn.execute(
            "INSERT OR IGNORE INTO schema_version (version) VALUES (22)",
            [],
        )?;
    }

    // Phase 5 data migration: fix resolution event taxonomy (run once, gated by version < 8)
    if version < 8 {
        conn.execute_batch(
            "UPDATE llm_wiki_events SET event_type = 'approved'
               WHERE event_type = 'action' AND summary LIKE 'Approved%';
             UPDATE llm_wiki_events SET event_type = 'rejected'
               WHERE event_type = 'observation' AND summary LIKE 'Rejected proposal%';",
        )?;
        // Bump schema_version to 8 so this migration runs only once
        conn.execute(
            "INSERT OR IGNORE INTO schema_version (version) VALUES (8)",
            [],
        )?;
    }

    // 90-day pruning of curated_agent_log (local-only audit trail)
    conn.execute(
        "DELETE FROM curated_agent_log WHERE created_at < unixepoch() - 90*24*60*60",
        [],
    )?;

    // Ensure index on curated_agent_log.created_at for pruning performance
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_curated_agent_log_created_at
         ON curated_agent_log(created_at);",
    )?;

    crate::db::schema_guard::verify_llm_wiki_schema(conn)?;

    // Startup canary: report JSON-shaped but unparseable `source_ref` values.
    // See `warn_on_malformed_source_refs` and issue #162.
    let _ = warn_on_malformed_source_refs(conn);

    Ok(())
}

/// Report `source_ref` values that look like JSON but will not parse.
///
/// Issue #162: every evidence blob written on one brain between 2026-08-29
/// and 2026-09-01 was stored with JSON punctuation stripped. Nothing caught
/// it at write time — `source_ref_is_still_grounded` treats an unparseable
/// ref as "still grounded" by design (PR #99), so heal silently no-oped, and
/// `tier_backfill`'s `json_valid` guard silently skipped the rows.
///
/// The leading-`{` test keeps legitimate plain-path refs out of the count;
/// only a ref that claims to be JSON and is not is a defect.
///
/// Returns the count so callers and tests can assert on it. Never fails the
/// connection — a diagnostic must not be able to prevent startup.
pub(crate) fn warn_on_malformed_source_refs(conn: &Connection) -> usize {
    let result: rusqlite::Result<i64> = conn.query_row(
        "SELECT COUNT(*) FROM llm_wiki_entries
          WHERE source_ref IS NOT NULL
            AND substr(source_ref, 1, 1) = '{'
            AND NOT json_valid(source_ref)",
        [],
        |r| r.get(0),
    );

    match result {
        Ok(0) => 0,
        Ok(n) => {
            // Surface in EVERY build: in mcp-server builds tracing routes to
            // the structured log; elsewhere eprintln! guarantees the canary
            // is at least visible on stderr instead of silently doing nothing
            // (a silent canary defeated its purpose during the #162 incident).
            #[cfg(feature = "mcp-server")]
            tracing::warn!(
                malformed_source_refs = n,
                "found entries whose source_ref looks like JSON but will not parse; \
                 evidence provenance is unrecoverable for these rows and heal will \
                 silently skip them (see issue #162)"
            );
            #[cfg(not(feature = "mcp-server"))]
            eprintln!(
                "[curated-thoughts] WARNING: {n} source_ref value(s) look like JSON \
                 but will not parse; evidence provenance is unrecoverable for these \
                 rows and heal will silently skip them (see issue #162)"
            );
            n as usize
        }
        Err(e) => {
            #[cfg(feature = "mcp-server")]
            tracing::warn!(error = %e, "source_ref canary query failed; skipping the check");
            #[cfg(not(feature = "mcp-server"))]
            eprintln!("[curated-thoughts] WARNING: source_ref canary query failed ({e}); skipping the check");
            0
        }
    }
}

#[allow(dead_code)]
pub struct AppDb(pub Connection);

impl AppDb {
    /// Open the brain database, deriving the config path from the canonical
    /// resolver (honors `CURATED_BRAIN_DB` / `CURATED_BRAIN_CONFIG`).
    /// All new callers should use [`AppDb::open_with_config`] directly so the
    /// config path is explicit; this thin wrapper preserves the historical
    /// single-arg API while routing through the unified resolver.
    pub fn open(path: &Path) -> Result<Self> {
        let paths = crate::retrieval::resolve_brain_paths();
        Self::open_with_config(path, &paths.config_path)
    }

    /// Open the brain database, resolving the vault root from an explicit
    /// config path. Callers that honor split `CURATED_BRAIN_DB` /
    /// `CURATED_BRAIN_CONFIG` environments must use this instead of [`AppDb::open`],
    /// which derives config.json from the database's parent directory.
    pub fn open_with_config(path: &Path, config_path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout = 5000;")?;
        let vault_roots = VaultConfig::new(config_path.as_ref().to_path_buf())
            .vault_root()
            .unwrap_or(None)
            .map(|root| {
                let configured = root.to_string_lossy().to_string();
                let canonical = canonicalize_workspace_root(&configured);
                VaultRoots { configured, canonical }
            });
        migrate(&conn, vault_roots.clone(), path.parent())?;
        if let Some(root) = vault_roots.as_ref() {
            let vault_path = std::path::Path::new(&root.canonical);
            if vault_path.is_dir() {
                let _ = crate::db::okf_migration::run_okf_migration(&conn, vault_path);
            }
        }
        Ok(AppDb(conn))
    }
}

/// Bring an ALREADY-OPEN database up to the current schema.
///
/// Exists for connections that must not create the database file (the `--mcp`
/// server opens with `SQLITE_OPEN_READ_WRITE` and no `CREATE`), which
/// therefore cannot route through [`AppDb::open_with_config`]. Runs the same
/// migration ladder, minus the vault-root-dependent OKF step: `vault_root` is
/// `None`, matching [`open_app_db`].
pub fn migrate_open_db(conn: &Connection, db_dir: Option<&Path>) -> Result<()> {
    migrate(conn, None, db_dir)
}

pub fn open_in_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    migrate(&conn, None::<VaultRoots>, None)?;
    Ok(conn)
}

/// Open (and migrate) a brain database at an arbitrary path. Intended for
/// tests that need an on-disk database file — production code must use
/// [`AppDb::open_with_config`] so the config-derived vault root is honored.
/// The `config` argument is accepted for API symmetry with `AppDb::open` and
/// is currently unused.
#[allow(dead_code)]
pub fn open_app_db(path: &Path, _config: Option<&Path>) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout = 5000;")?;
    migrate(&conn, None::<VaultRoots>, path.parent())?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_db_initializes_with_schema_version() {
        let conn = open_in_memory().unwrap();
        let max_version: i64 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .unwrap();
        // Bumped from 17 to 18 by MIGRATION_V18, which adds the CT-owned
        // `librarian_evidence` table (issue #186 spec §2.1).
        // Bumped from 18 to 19 by MIGRATION_V19, which repairs the mixed
        // seconds/milliseconds units in `llm_wiki_edges.created_at`
        // (issue #191 spec §2.5).
        // Bumped from 21 to 22 by MIGRATION_V22, which rewrites
        // `documents.path` from canonical to configured (virtual) form
        // (issue #204, formerly spec D2a). `open_in_memory` runs with no
        // vault root, so V22 refuses to stamp and this assertion holds at
        // 21; the bound is exercised by the integration test that calls
        // `migrate(Some(VaultRoots))` directly.
        assert_eq!(max_version, 21, "open_in_memory has no vault root, so V22 refuses to stamp and the schema caps at 21");
    }

    /// `--mcp` has no other migration point: its read connection is read-only
    /// and its lazy RW connection opens the file bare, so without an explicit
    /// migration a brain.db left at an older version keeps failing every write
    /// that touches a migration-added column (V21 `reviewed_by` is the first).
    /// `migrate_open_db` is that point — it upgrades an already-open handle.
    #[test]
    fn migrate_open_db_upgrades_an_already_open_connection() {
        let conn = open_in_memory().unwrap();
        // Rewind to the pre-V21 shape (see the V17 rewind above for why the
        // column must go with the stamp: V21's ALTER is not idempotent).
        conn.execute_batch(
            "ALTER TABLE curated_proposals DROP COLUMN reviewed_by;
             DELETE FROM schema_version WHERE version >= 21;",
        )
        .unwrap();
        let before: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('curated_proposals') WHERE name='reviewed_by'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(before, 0, "test precondition: the column is gone");

        migrate_open_db(&conn, None).unwrap();

        let after: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('curated_proposals') WHERE name='reviewed_by'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(after, 1, "migrate_open_db must bring the schema forward");
    }

    /// Human Verification Gate (hvg): MIGRATION_V21 adds the nullable
    /// `reviewed_by` column to `curated_proposals`. Existing rows keep NULL
    /// — only resolutions made after this migration carry a reviewer.
    #[test]
    fn v21_adds_reviewed_by_column_null_on_existing_rows() {
        let conn = open_in_memory().unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('curated_proposals') WHERE name='reviewed_by'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "reviewed_by column must exist after migration");
        // Seed one proposal row, assert its reviewed_by is NULL: the ALTER
        // appends the column, so every pre-migration row must read back as
        // unreviewed rather than failing the SELECT or inventing a value.
        conn.execute(
            "INSERT INTO curated_proposals (id, kind, model, status, created_at)
             VALUES ('p1','new_entity','m','pending', 1)",
            [],
        )
        .unwrap();
        let rb: Option<String> = conn
            .query_row(
                "SELECT reviewed_by FROM curated_proposals WHERE id='p1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(rb.is_none());
    }

    /// Upgraded-DB path for the core-llm-wiki@7.1.0 bump: a database created
    /// before the package gained the embedding-failure marker columns must
    /// open successfully. The Rust schema guard rejects the old shape, and
    /// the JS package migration that adds the columns only runs after the
    /// frontend boots — so the V17 gate has to add them first.
    #[test]
    fn migration_v17_adds_package_embedding_failure_columns() {
        let conn = open_in_memory().unwrap();

        // Rewind to the pre-7.1 shape: drop the three package columns and
        // remove the V17..V21 stamps (the DELETE pulls the whole ladder, so
        // migrate() re-runs every gate from 17 up). The rewind must drop
        // `reviewed_by` as well: V21's ALTER is deliberately NOT idempotent
        // (stamp-last design), so re-running it against the surviving column
        // would fail with a duplicate-column error instead of re-applying.
        // A pre-existing row proves the added columns backfill their DDL
        // defaults.
        conn.execute(
            "INSERT INTO llm_wiki_entries (id, entity_id, title, body, created_at, updated_at)
             VALUES ('e1', 'ent1', 't', 'b', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute_batch(
            "ALTER TABLE llm_wiki_entries DROP COLUMN embedding_failed_at;
             ALTER TABLE llm_wiki_entries DROP COLUMN embedding_failure_kind;
             ALTER TABLE llm_wiki_entries DROP COLUMN embedding_attempts;
             ALTER TABLE curated_proposals DROP COLUMN reviewed_by;
             DELETE FROM schema_version WHERE version >= 17;",
        )
        .unwrap();

        // Precondition: the guard alone would reject this shape.
        let guard_err = crate::db::schema_guard::verify_llm_wiki_schema(&conn)
            .expect_err("guard must reject a pre-7.1 entries table");
        assert!(guard_err.to_string().contains("missing columns"));

        migrate(&conn, None, None).expect("migrate must upgrade a pre-7.1 database");

        let post_columns: Vec<String> = conn
            .prepare("PRAGMA table_info(llm_wiki_entries)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        for column in [
            "embedding_failed_at",
            "embedding_failure_kind",
            "embedding_attempts",
        ] {
            assert!(
                post_columns.iter().any(|c| c == column),
                "{column} must exist after migrate()"
            );
        }

        // The default backfill matches the package DDL: existing rows read 0,
        // not NULL.
        let attempts: Option<i64> = conn
            .query_row(
                "SELECT embedding_attempts FROM llm_wiki_entries WHERE id = 'e1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(attempts, Some(0));

        crate::db::schema_guard::verify_llm_wiki_schema(&conn)
            .expect("guard must accept the upgraded database");

        let post_version: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(post_version >= 17, "schema_version must reach >= 17");
    }

    /// V15 widens the `documents.status` CHECK so the deferred-reindex
    /// staging writes are accepted. Before it, every
    /// `UPDATE documents SET status = 'pending_reindex'` failed with a
    /// constraint violation and the deferred rechunk was silently dropped.
    #[test]
    fn v15_documents_status_accepts_pending_reindex() {
        let conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO documents (path, hash, tier, status)
             VALUES ('/a.md', 'h', 'user_doc', 'indexed')",
            [],
        )
        .unwrap();

        let n = conn
            .execute(
                "UPDATE documents SET status = 'pending_reindex'
                   WHERE path = '/a.md' AND status = 'indexed'",
                [],
            )
            .expect("pending_reindex must satisfy the status CHECK");
        assert_eq!(n, 1);

        let status: String = conn
            .query_row(
                "SELECT status FROM documents WHERE path = '/a.md'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "pending_reindex");

        // The CHECK still rejects genuinely invalid values.
        assert!(
            conn.execute(
                "UPDATE documents SET status = 'bogus' WHERE path = '/a.md'",
                [],
            )
            .is_err(),
            "the widened CHECK must still reject unknown statuses"
        );
    }

    /// The V15 rebuild must carry every column forward — including the V11
    /// synthesis watermark and the V13 quarantine stamp — and recreate the
    /// partial index that was dropped with the old table.
    #[test]
    fn v15_rebuild_preserves_columns_rows_and_the_dirty_index() {
        let conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO documents
                (path, hash, tier, status, synth_hash, synth_model, synth_at, quarantined_at)
             VALUES ('/keep.md', 'h1', 'user_doc', 'indexed', 'sh', 'sm', 42, 7)",
            [],
        )
        .unwrap();

        let (hash, sh, sm, sa, q): (String, String, String, i64, i64) = conn
            .query_row(
                "SELECT hash, synth_hash, synth_model, synth_at, quarantined_at
                   FROM documents WHERE path = '/keep.md'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!(
            (hash.as_str(), sh.as_str(), sm.as_str(), sa, q),
            ("h1", "sh", "sm", 42, 7)
        );

        let idx: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                  WHERE type='index' AND name='idx_documents_dirty'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(idx, 1, "idx_documents_dirty must survive the rebuild");
    }

    /// Fresh-DB path: `open_in_memory` applies every migration, so the
    /// watermark columns and the dirty-doc partial index must exist.
    #[test]
    fn migration_v11_fresh_db_adds_watermark_columns_and_dirty_index() {
        let conn = open_in_memory().unwrap();
        for column in &["synth_hash", "synth_model", "synth_at"] {
            let has_column: bool = conn
                .prepare("PRAGMA table_info(documents)")
                .unwrap()
                .query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .filter_map(Result::ok)
                .any(|name| name == *column);
            assert!(
                has_column,
                "documents.{column} must exist on a fresh database"
            );
        }
        let index_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master
                 WHERE type='index' AND name='idx_documents_dirty'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(index_count, 1, "idx_documents_dirty must exist");
    }

    /// V12 idempotent unit-contract: every seconds-valued `deleted_at` row is
    /// promoted to ms on the first run, and a second run on the same data is
    /// a no-op. Pin the boundary against the 11-zeros off-by-one bug from
    /// spec review: rows that already pass `SEC_VS_MS_THRESHOLD` (i.e. were
    /// written in ms by the post-fix heal writers, or by `commit.rs:733` /
    /// `facts.rs:232`) must NOT be multiplied.
    #[test]
    fn migration_v12_promotes_seconds_and_is_idempotent() {
        use crate::db::schema::SEC_VS_MS_THRESHOLD;

        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        // Build the schema to V11 so we have an `llm_wiki_entries` table to
        // mutate before V12 fires.
        conn.execute_batch(&format!(
            "BEGIN;\n{}\n{}\n{}\nCOMMIT;",
            MIGRATION_V1, MIGRATION_V2, MIGRATION_V3
        ))
        .unwrap();
        conn.execute_batch(MIGRATION_V4).unwrap();
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V5))
            .unwrap();
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V6))
            .unwrap();
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", okf_ddl::migration_v7_sql()))
            .unwrap();
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V9))
            .unwrap();
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V10))
            .unwrap();
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V11))
            .unwrap();

        // Seed three rows: one in seconds (the bug), one already in ms
        // (anything from `commit.rs:733` or `facts.rs:232`), one null.
        let insert = |deleted_at: Option<i64>| -> i64 {
            conn.execute(
                "INSERT INTO llm_wiki_entries
                    (id, entity_id, title, body, tags, confidence, source_type,
                     created_at, updated_at, deleted_at)
                 VALUES ('f' || hex(randomblob(6)), 'e', 't', 'b', '[]', 'inferred',
                         'librarian_inferred', 1, 1, ?1)",
                rusqlite::params![deleted_at],
            )
            .unwrap();
            conn.last_insert_rowid()
        };
        let secs_id = insert(Some(1_750_000_000)); // seconds — must be promoted
        let ms_id = insert(Some(SEC_VS_MS_THRESHOLD + 1)); // already ms — must NOT change
        let null_id = insert(None); // untouched

        // Pre-V12: confirm the seeded values.
        assert_eq!(
            conn.query_row(
                "SELECT deleted_at FROM llm_wiki_entries WHERE rowid = ?1",
                [secs_id],
                |r| r.get::<_, i64>(0),
            )
            .unwrap(),
            1_750_000_000,
            "seeded seconds-valued row must read back as seconds pre-V12"
        );

        // Fire V12.
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V12))
            .unwrap();

        let read = |id: i64| -> Option<i64> {
            conn.query_row(
                "SELECT deleted_at FROM llm_wiki_entries WHERE rowid = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(
            read(secs_id),
            Some(1_750_000_000 * 1000),
            "seconds-valued row must be promoted to milliseconds"
        );
        assert_eq!(
            read(ms_id),
            Some(SEC_VS_MS_THRESHOLD + 1),
            "already-ms row above threshold must NOT be multiplied"
        );
        assert_eq!(read(null_id), None, "NULL row must stay NULL");

        // Idempotency: re-running V12 must change nothing.
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V12))
            .unwrap();
        assert_eq!(read(secs_id), Some(1_750_000_000 * 1000));
        assert_eq!(read(ms_id), Some(SEC_VS_MS_THRESHOLD + 1));
        assert_eq!(read(null_id), None);

        // Pin the post-condition: zero rows below the threshold (the live-DB
        // smoke gate).
        let below: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_entries
                  WHERE deleted_at IS NOT NULL
                    AND deleted_at < ?1",
                [SEC_VS_MS_THRESHOLD],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(below, 0, "no row may remain below SEC_VS_MS_THRESHOLD");
    }

    /// Upgraded-DB path: simulate a pre-V11 database (schema_version = 10,
    /// no watermark columns), run the production `migrate` gate, and assert
    /// the columns appear.
    #[test]
    fn migration_v11_upgrades_v10_database() {
        // Pre-seed a V10 database: V1..V6 + OKF V7 DDL + data migration to 8 + V9 + V10.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        conn.execute_batch(&format!(
            "BEGIN;\n{}\n{}\n{}\n{}\n{}\nCOMMIT;",
            MIGRATION_V1, MIGRATION_V2, MIGRATION_V3, MIGRATION_V4, MIGRATION_V5
        ))
        .unwrap();
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V6))
            .unwrap();
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", okf_ddl::migration_v7_sql()))
            .unwrap();
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V9))
            .unwrap();
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V10))
            .unwrap();

        let pre_has_synth_hash: bool = conn
            .prepare("PRAGMA table_info(documents)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(Result::ok)
            .any(|name| name == "synth_hash");
        assert!(
            !pre_has_synth_hash,
            "test precondition: no synth_hash before migrate()"
        );

        migrate(&conn, None, None).expect("migrate must succeed upgrading a V10 DB");

        let post_has_synth_hash: bool = conn
            .prepare("PRAGMA table_info(documents)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(Result::ok)
            .any(|name| name == "synth_hash");
        assert!(post_has_synth_hash, "synth_hash must exist after migrate()");
        let post_version: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(post_version >= 11, "schema_version must reach >= 11");
    }

    /// Backfill correctness: a doc whose latest ingest run succeeded gets
    /// synth_hash = hash and synth_model = 'pre-watermark'; a doc with no
    /// ingest history (or whose latest run failed) stays NULL (dirty).
    #[test]
    fn migration_v11_backfills_only_docs_with_indexed_latest_run() {
        // Build a V10-state database by hand so we control ingest history.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        conn.execute_batch(&format!(
            "BEGIN;\n{}\n{}\n{}\n{}\n{}\nCOMMIT;",
            MIGRATION_V1, MIGRATION_V2, MIGRATION_V3, MIGRATION_V4, MIGRATION_V5
        ))
        .unwrap();
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V6))
            .unwrap();
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", okf_ddl::migration_v7_sql()))
            .unwrap();
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V9))
            .unwrap();
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V10))
            .unwrap();

        let insert_doc = |path: &str, hash: &str| -> i64 {
            conn.execute(
                "INSERT INTO documents (path, hash, tier, status) VALUES (?1, ?2, 'user_doc', 'indexed')",
                [path, hash],
            )
            .unwrap();
            conn.last_insert_rowid()
        };
        // Indexed-latest doc (older error, then indexed).
        let indexed_id = insert_doc("/v/a.md", "hash-a");
        conn.execute(
            "INSERT INTO ingest_runs (doc_id, run_at, outcome) VALUES (?1, 100, 'error')",
            [indexed_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ingest_runs (doc_id, run_at, outcome) VALUES (?1, 200, 'indexed')",
            [indexed_id],
        )
        .unwrap();
        // Error-latest doc.
        let errored_id = insert_doc("/v/b.md", "hash-b");
        conn.execute(
            "INSERT INTO ingest_runs (doc_id, run_at, outcome) VALUES (?1, 100, 'indexed')",
            [errored_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ingest_runs (doc_id, run_at, outcome) VALUES (?1, 300, 'error')",
            [errored_id],
        )
        .unwrap();
        // No-history doc.
        let _no_history_id = insert_doc("/v/c.md", "hash-c");

        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V11))
            .unwrap();

        let (a_hash, a_model): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT synth_hash, synth_model FROM documents WHERE id = ?1",
                [indexed_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            a_hash.as_deref(),
            Some("hash-a"),
            "indexed-latest doc must be backfilled with its hash"
        );
        assert_eq!(a_model.as_deref(), Some("pre-watermark"));

        {
            let id = errored_id;
            let (h, m): (Option<String>, Option<String>) = conn
                .query_row(
                    "SELECT synth_hash, synth_model FROM documents WHERE id = ?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert!(
                h.is_none(),
                "doc {id} without an indexed latest run must stay dirty"
            );
            assert!(m.is_none());
        }
        let dirty_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM documents WHERE synth_hash IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            dirty_count, 2,
            "exactly the error-latest and no-history docs stay dirty"
        );
    }

    #[test]
    fn test_v7_okf_and_curated_tables_exist() {
        let conn = open_in_memory().unwrap();
        for table in &[
            "llm_wiki_entries",
            "llm_wiki_outbox",
            "llm_wiki_meta",
            "llm_wiki_edges",
            "curated_entities",
            "curated_proposals",
            "curated_proposal_items",
            "curated_proposal_sources",
            "curated_agent_log",
        ] {
            let count: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "table '{}' not found in schema", table);
        }
    }

    #[test]
    fn test_all_tables_exist() {
        let conn = open_in_memory().unwrap();
        for table in &[
            "documents",
            "chunks",
            "wiki_pages",
            "folder_rules",
            "curated_relationships",
        ] {
            let count: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "table '{}' not found in schema", table);
        }
    }

    #[test]
    fn test_embeddings_table_exists() {
        let conn = open_in_memory().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='embeddings'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn migration_v5_adds_defined_symbol_and_entity_id_columns() {
        let conn = open_in_memory().unwrap();
        let doc_id = crate::db::queries::upsert_document(&conn, "/x/b.md", "h2").unwrap();
        let chunk = crate::chunker::Chunk {
            text: "body".into(),
            start_line: 1,
            end_line: 1,
            symbol_name: Some("MyStruct".into()),
            defined_symbol: Some("mystruct".into()),
            strategy: crate::chunker::ChunkStrategyTag::AstSymbolRust,
        };
        let id =
            crate::db::queries::insert_chunk(&conn, doc_id, &chunk, 0, "tier_fact", "").unwrap();
        let (def_sym, eid): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT defined_symbol, entity_id FROM chunks WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(def_sym.as_deref(), Some("mystruct"));
        assert_eq!(eid.as_deref(), Some("tier_fact"));
    }

    #[test]
    fn migration_v5_backfills_entity_id_from_document_path_prefix() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        conn.execute_batch(&format!(
            "BEGIN;\n{}\n{}\n{}\n{}\nCOMMIT;",
            MIGRATION_V1, MIGRATION_V2, MIGRATION_V3, MIGRATION_V4
        ))
        .unwrap();

        conn.execute(
            "INSERT INTO documents (path, hash, tier, status) VALUES (?1, ?2, ?3, 'indexed')",
            ["/vault/documents/doc.md", "h1", "user_doc"],
        )
        .unwrap();
        let doc_id: i64 = conn
            .query_row(
                "SELECT id FROM documents WHERE path = ?1",
                ["/vault/documents/doc.md"],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute(
            "INSERT INTO chunks (doc_id, chunk_text, position, start_line, end_line, symbol_name, strategy) VALUES (?1, ?2, ?3, 1, 1, NULL, 'prose')",
            rusqlite::params![doc_id, "body", 0],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO documents (path, hash, tier, status) VALUES (?1, ?2, ?3, 'indexed')",
            ["/vault/src/init.rs", "h2", "user_doc"],
        )
        .unwrap();
        let working_doc_id: i64 = conn
            .query_row(
                "SELECT id FROM documents WHERE path = ?1",
                ["/vault/src/init.rs"],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute(
            "INSERT INTO chunks (doc_id, chunk_text, position, start_line, end_line, symbol_name, strategy) VALUES (?1, ?2, ?3, 1, 1, NULL, 'prose')",
            rusqlite::params![working_doc_id, "body", 0],
        )
        .unwrap();

        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V5))
            .unwrap();

        let entity_id_fact: String = conn
            .query_row(
                "SELECT entity_id FROM chunks WHERE doc_id = ?1",
                [doc_id],
                |r| r.get(0),
            )
            .unwrap();
        let entity_id_working: String = conn
            .query_row(
                "SELECT entity_id FROM chunks WHERE doc_id = ?1",
                [working_doc_id],
                |r| r.get(0),
            )
            .unwrap();

        assert_eq!(entity_id_fact, "tier_fact");
        assert_eq!(entity_id_working, "tier_working");
    }

    #[test]
    fn migration_v5_backfills_working_chunks_with_vault_root_hash() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config_path = tmp.path().join("config.json");
        let cfg = VaultConfig::new(config_path.clone());
        cfg.set_vault_path("/vault").unwrap();

        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        conn.execute_batch(&format!(
            "BEGIN;\n{}\n{}\n{}\n{}\nCOMMIT;",
            MIGRATION_V1, MIGRATION_V2, MIGRATION_V3, MIGRATION_V4
        ))
        .unwrap();

        conn.execute(
            "INSERT INTO documents (path, hash, tier, status) VALUES (?1, ?2, ?3, 'indexed')",
            ["/vault/src/init.rs", "h2", "user_doc"],
        )
        .unwrap();
        let working_doc_id: i64 = conn
            .query_row(
                "SELECT id FROM documents WHERE path = ?1",
                ["/vault/src/init.rs"],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute(
            "INSERT INTO chunks (doc_id, chunk_text, position, start_line, end_line, symbol_name, strategy) VALUES (?1, ?2, ?3, 1, 1, NULL, 'prose')",
            rusqlite::params![working_doc_id, "body", 0],
        )
        .unwrap();

        migrate(
            &conn,
            Some(VaultRoots {
                configured: "/vault".to_string(),
                canonical: "/vault".to_string(),
            }),
            None,
        )
        .unwrap();

        let entity_id_working: String = conn
            .query_row(
                "SELECT entity_id FROM chunks WHERE doc_id = ?1",
                [working_doc_id],
                |r| r.get(0),
            )
            .unwrap();

        let expected = format!(
            "tier_working::{}",
            &hash_bytes("/vault".replace('\\', "/").trim_end_matches('/').as_bytes())[..16]
        );

        assert_eq!(entity_id_working, expected);
    }

    #[test]
    fn migration_v4_chunk_columns_roundtrip() {
        let conn = open_in_memory().unwrap();
        let doc_id = crate::db::queries::upsert_document(&conn, "/x/a.md", "h1").unwrap();
        let chunk = crate::chunker::Chunk {
            text: "body".into(),
            start_line: 3,
            end_line: 7,
            symbol_name: Some("foo".into()),
            defined_symbol: None,
            strategy: crate::chunker::ChunkStrategyTag::Declarative,
        };
        let id =
            crate::db::queries::insert_chunk(&conn, doc_id, &chunk, 0, "tier_working", "").unwrap();
        let (sl, el, sym, strat): (i64, i64, Option<String>, String) = conn
            .query_row(
                "SELECT start_line, end_line, symbol_name, strategy FROM chunks WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(sl, 3);
        assert_eq!(el, 7);
        assert_eq!(sym.as_deref(), Some("foo"));
        assert_eq!(strat, "declarative");
    }

    /// Regression test for the Phase 9 chunk-id resolution migration.
    ///
    /// Phase 5 already bumped `schema_version` to 8 on every released DB.
    /// Gating the new migration on `version < 7` made the gate unreachable
    /// on any production database — `ALTER TABLE chunks ADD COLUMN
    /// content_hash` never ran, and `insert_chunk` crashed at runtime
    /// (column missing). The fix renames the gate to `version < 9`.
    ///
    /// This test pre-seeds a connection with `schema_version = 8` and
    /// the OKF V7 DDL (the state of a Phase 5 production DB), runs the
    /// production migration gate (`migrate`), and asserts the
    /// `content_hash` column exists.
    #[test]
    fn migration_v13_creates_watchdog_tables_and_quarantine_column() {
        let conn = open_in_memory().unwrap();

        for table in ["pipeline_heartbeat", "pipeline_stalls", "stall_strikes"] {
            let found: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(found, 1, "missing table {table}");
        }

        // documents gains a nullable quarantine timestamp.
        conn.execute_batch(
            "INSERT INTO documents (path, hash, tier, status, quarantined_at)
         VALUES ('/tmp/a.md', 'h', 'user_doc', 'pending', 123);",
        )
        .unwrap();
        let q: Option<i64> = conn
            .query_row(
                "SELECT quarantined_at FROM documents WHERE path = '/tmp/a.md'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(q, Some(123));

        // Heartbeat is a single-row table seeded at migration time.
        let hb: i64 = conn
            .query_row("SELECT COUNT(*) FROM pipeline_heartbeat", [], |r| r.get(0))
            .unwrap();
        assert_eq!(hb, 1);
    }

    #[test]
    fn migration_v13_is_idempotent() {
        let conn = open_in_memory().unwrap();
        // Re-running the migration body must not error or duplicate the seed row.
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V13))
            .unwrap();
        let hb: i64 = conn
            .query_row("SELECT COUNT(*) FROM pipeline_heartbeat", [], |r| r.get(0))
            .unwrap();
        assert_eq!(hb, 1);
    }

    #[test]
    fn v16_adds_tier_column_with_check_constraint() {
        let conn = open_in_memory().unwrap();

        // Column exists and accepts the three legal states.
        conn.execute(
            "INSERT INTO llm_wiki_entries (id, entity_id, title, body, created_at, updated_at, tier)
             VALUES ('a', 'ent_1', 'A', '', 0, 0, 'fact'),
                    ('b', 'ent_1', 'B', '', 0, 0, 'wisdom'),
                    ('c', 'ent_1', 'C', '', 0, 0, NULL)",
            [],
        )
        .unwrap();

        // The CHECK is the floor: an out-of-vocabulary tier carries no prompt
        // semantics and matches no filter, so the database refuses it.
        let bad = conn.execute(
            "INSERT INTO llm_wiki_entries (id, entity_id, title, body, created_at, updated_at, tier)
             VALUES ('d', 'ent_1', 'D', '', 0, 0, 'anchor')",
            [],
        );
        assert!(
            bad.is_err(),
            "CHECK must reject a tier outside fact/wisdom/NULL"
        );
    }

    #[test]
    fn v16_admits_existing_rows_unchanged() {
        // Every pre-migration row is NULL, so the CHECK needs no data pass.
        let conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_entries (id, entity_id, title, body, created_at, updated_at)
             VALUES ('a', 'ent_1', 'A', '', 0, 0)",
            [],
        )
        .unwrap();
        let tier: Option<String> = conn
            .query_row(
                "SELECT tier FROM llm_wiki_entries WHERE id = 'a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tier, None);
    }

    #[test]
    fn migration_v9_adds_content_hash_column_to_phase5_database() {
        // Simulate a Phase 5 production database: V1..V6 + OKF V7 DDL,
        // and `schema_version` is at 8 (set by Phase 5's data migration).
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        conn.execute_batch(&format!(
            "BEGIN;\n{}\n{}\n{}\n{}\n{}\nCOMMIT;",
            MIGRATION_V1, MIGRATION_V2, MIGRATION_V3, MIGRATION_V4, MIGRATION_V5
        ))
        .unwrap();
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V6))
            .unwrap();
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", okf_ddl::migration_v7_sql()))
            .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO schema_version (version) VALUES (8)",
            [],
        )
        .unwrap();

        // Sanity: confirm the pre-seeded state matches a Phase 5 production DB.
        let pre_version: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            pre_version, 8,
            "test precondition: schema_version must be 8 before migrate()"
        );

        // Pre-fix bug: gating on `version < 7` skipped the ALTER TABLE,
        // leaving `chunks` without `content_hash`. With the V9 gate the
        // column is added.
        migrate(&conn, None, None).expect("migrate must succeed on Phase 5 DB");

        let has_content_hash: bool = conn
            .prepare("PRAGMA table_info(chunks)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(Result::ok)
            .any(|name| name == "content_hash");
        assert!(
            has_content_hash,
            "chunks.content_hash must exist after migrate() on a Phase 5 DB"
        );

        // Post-migration schema_version should be at least 9 (V9 bumped it).
        let post_version: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            post_version >= 9,
            "schema_version must reach >= 9 after migrate(), got {post_version}"
        );
    }

    /// Seed an `llm_wiki_entries` row with an explicit `source_ref`.
    /// Mirrors the V12 test's minimal column set (without `deleted_at`) so the
    /// canary tests can mix valid, malformed, and missing refs without
    /// touching every column.
    fn seed_entry_with_source_ref(conn: &Connection, id: &str, source_ref: &str) {
        conn.execute(
            "INSERT INTO llm_wiki_entries
                (id, entity_id, title, body, tags, confidence, source_type,
                 source_ref, created_at, updated_at)
             VALUES (?1, 'e', 't', 'b', '[]', 'inferred', 'librarian_inferred',
                     ?2, 1, 1)",
            rusqlite::params![id, source_ref],
        )
        .unwrap();
    }

    fn seed_entry_with_source_ref_null(conn: &Connection, id: &str) {
        conn.execute(
            "INSERT INTO llm_wiki_entries
                (id, entity_id, title, body, tags, confidence, source_type,
                 source_ref, created_at, updated_at)
             VALUES (?1, 'e', 't', 'b', '[]', 'inferred', 'librarian_inferred',
                     NULL, 1, 1)",
            [id],
        )
        .unwrap();
    }

    /// Hand-build the schema to the V16 state (everything except the V17
    /// package columns and the V18 #186 step), mirroring the other
    /// hand-built-migration tests. Stamping 16 lets migrate() run its V17 and
    /// V18 gates against a brain that predates the fix.
    fn build_pre_v18_brain(conn: &Connection) {
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        conn.execute_batch(&format!(
            "BEGIN;\n{}\n{}\n{}\nCOMMIT;",
            MIGRATION_V1, MIGRATION_V2, MIGRATION_V3
        ))
        .unwrap();
        conn.execute_batch(MIGRATION_V4).unwrap();
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V5))
            .unwrap();
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V6))
            .unwrap();
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", okf_ddl::migration_v7_sql()))
            .unwrap();
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V9))
            .unwrap();
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V10))
            .unwrap();
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V11))
            .unwrap();
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V12))
            .unwrap();
        crate::db::ddl_compat::add_column_if_missing(
            conn,
            "documents",
            "quarantined_at",
            "INTEGER",
        )
        .unwrap();
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V13))
            .unwrap();
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V14))
            .unwrap();
        conn.execute_batch(MIGRATION_V15).unwrap();
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V16))
            .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO schema_version (version) VALUES (16)",
            [],
        )
        .unwrap();
    }

    /// Seed the pre-V18 damage: a mangleable librarian row whose ref is still
    /// valid JSON with a live chunk anchor, plus a user_stated row carrying
    /// the old manual sentinel, on a brain-complete chunks/documents pair.
    fn seed_v18_damage(conn: &Connection) -> String {
        conn.execute(
            "INSERT INTO documents (path, hash, tier, status)
             VALUES ('/v/notes.md', 'h', 'user_doc', 'indexed')",
            [],
        )
        .unwrap();
        let doc_id = conn.last_insert_rowid();
        let hash64 = format!("{}{}", "feedface00", "0".repeat(54));
        conn.execute(
            "INSERT INTO chunks (doc_id, chunk_text, position, start_line, end_line,
                 strategy, entity_id, content_hash)
             VALUES (?1, 'c', 0, 1, 1, 'prose', 'ent', ?2)",
            rusqlite::params![doc_id, hash64],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_entries
                (id, entity_id, title, body, tags, confidence, source_type,
                 source_ref, created_at, updated_at)
             VALUES ('fact_v18', 'ent', 't', 'b', '[]', 'inferred',
                     'librarian_inferred', ?1, 1, 1)",
            [format!(
                r#"{{"evidence":[{{"chunk_id":1,"content_hash":"{hash64}"}}],"proposal_id":"prop_v18"}}"#
            )],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_entries
                (id, entity_id, title, body, tags, confidence, source_type,
                 source_ref, created_at, updated_at)
             VALUES ('fact_manual', 'ent', 't', 'b', '[]', 'confirmed',
                     'user_stated', '{\"proposal_id\":null,\"evidence\":[]}', 1, 1)",
            [],
        )
        .unwrap();
        hash64
    }

    /// Review round 5, finding 7: the production FILE-BACKED V18 path —
    /// backup export, the exported==census.damaged invariant, the user_stated
    /// sentinel normalization, the stamp — had zero coverage; every
    /// always-run test reached migrate() with db_dir=None.
    #[test]
    fn v18_file_backed_repair_exports_backs_up_and_tokens_damaged_rows() {
        let tmp = tempfile::TempDir::new().unwrap();
        let conn = Connection::open(tmp.path().join("brain.db")).unwrap();
        build_pre_v18_brain(&conn);
        seed_v18_damage(&conn);

        migrate(&conn, None, Some(tmp.path())).expect("file-backed V18 migration must succeed");

        // Backup invariant: one JSON file per damaged row under
        // repair-export-186/, written BEFORE any mutation.
        let export =
            std::fs::read_to_string(tmp.path().join("repair-export-186").join("fact_v18.json"))
                .expect("damaged row must be backed up before mutation");
        assert!(export.contains("prop_v18"));

        // The damaged row got the token and its paired evidence row.
        let ref_after: String = conn
            .query_row(
                "SELECT source_ref FROM llm_wiki_entries WHERE id = 'fact_v18'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            ref_after,
            crate::db::commit::librarian_source_ref_token("fact_v18")
        );
        let (pid, unanchored): (String, i64) = conn
            .query_row(
                "SELECT proposal_id, unanchored FROM librarian_evidence
                  WHERE entry_id = 'fact_v18'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(pid, "prop_v18");
        assert_eq!(unanchored, 0, "the seeded chunk anchor is live");

        // Finding 1: the user_stated sentinel is normalized to NULL.
        let manual_ref: Option<String> = conn
            .query_row(
                "SELECT source_ref FROM llm_wiki_entries WHERE id = 'fact_manual'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(manual_ref, None, "manual sentinel must be nulled by V18");

        let version: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            version, 21,
            "the V18 stamp must land after the repair (V19/V20/V21 stamps now follow)"
        );
    }

    /// One-open V18→V20 (issue #186 §2.4): migrate() run over a hand-built
    /// pre-V18 brain must land ALL of the V18 repair, the V20 re-grade and
    /// all three stamps in the SAME pass. The seeded unanchored fact anchors
    /// a live chunk, so the re-grade must CLEAR its flag (not purge it) —
    /// proving the re-grade really ran between the V18 repair and the V20
    /// stamp. This test lives here because migrate() and the fixtures are
    /// private.
    #[test]
    fn v18_through_v20_regrade_clears_live_anchor_in_same_pass() {
        let tmp = tempfile::TempDir::new().unwrap();
        let conn = Connection::open(tmp.path().join("brain.db")).unwrap();
        build_pre_v18_brain(&conn);
        let hash64 = seed_v18_damage(&conn);

        // A second, genuinely unanchored live fact: the re-grade must purge
        // it (export then hard delete) while the anchored fact survives.
        // librarian_evidence does not exist at V16 — MIGRATION_V18 creates
        // it. Pre-create the table and stamp 18 so migrate() skips the V18
        // gate (which would otherwise REPAIR fact_v18's mangled ref, i.e.
        // un-flag nothing and interleave two repairs in one pass) and runs
        // only the V20 re-grade. The orphan's evidence row is flagged
        // unanchored=1 with an empty evidence array; fact_v18's healthy
        // V18-shaped row is seeded by seed_v18_damage's INSERT OR REPLACE
        // (which now has a table to land in).
        conn.execute_batch(crate::db::schema::MIGRATION_V18)
            .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO schema_version (version) VALUES (18)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_entries
                (id, entity_id, title, body, tags, confidence, source_type,
                 source_ref, created_at, updated_at)
             VALUES ('fact_orphan', 'ent', 'orphan', 'b', '[]', 'inferred',
                     'librarian_inferred',
                     'librarian-orphan00000000000000000000000000000', 1, 1)",
            [],
        )
        .unwrap();
        // The orphan's flagged evidence row (empty evidence array → truly
        // unanchored → purge after export).
        conn.execute(
            "INSERT INTO librarian_evidence (entry_id, proposal_id, evidence_json,
                 unanchored, created_at)
             VALUES ('fact_orphan', 'prop_orphan',
                 '{\"evidence\":[],\"proposal_id\":\"prop_orphan\"}', 1, 1)",
            [],
        )
        .unwrap();
        // Give the anchored fact its V18-shaped evidence row (flagged 1; the
        // re-grade — not the V18 repair, which we are skipping — must clear
        // it). The chunk anchor from seed_v18_damage is live, and the row is
        // built in Rust so json_valid() sees exactly the intended blob.
        let evidence_json = format!(
            r#"{{"evidence":[{{"chunk_id":1,"content_hash":"{hash64}"}}],"proposal_id":"prop_v18"}}"#
        );
        conn.execute(
            "INSERT INTO librarian_evidence (entry_id, proposal_id, evidence_json,
                 unanchored, created_at)
             VALUES ('fact_v18', 'prop_v18', ?1, 1, 1)",
            [evidence_json],
        )
        .unwrap();

        migrate(&conn, None, Some(tmp.path())).unwrap();

        // Anchored fact: the re-grade CLEARED its flag (row + evidence
        // survive). (V18 gate is skipped in this fixture, so the token
        // normalization is out of scope here — it is covered by
        // v18_file_backed_repair_exports_backs_up_and_tokens_damaged_rows.)
        let (pid, unanchored): (String, i64) = conn
            .query_row(
                "SELECT le.proposal_id, le.unanchored
                   FROM llm_wiki_entries e
                   JOIN librarian_evidence le ON le.entry_id = e.id
                  WHERE e.id = 'fact_v18'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(unanchored, 0, "live chunk anchor must survive the re-grade");
        assert_eq!(pid, "prop_v18");

        // Orphaned fact: purged (hard delete), with its evidence and any
        // dangling edges swept, and a backup JSON on disk.
        let gone: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_entries WHERE id = 'fact_orphan'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(gone, 0, "still-orphaned row must be purged by V20");
        let ev_gone: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM librarian_evidence WHERE entry_id = 'fact_orphan'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(ev_gone, 0, "evidence must go with the entry");
        let backup = tmp
            .path()
            .join(crate::db::evidence_regrade::REGRADE_EXPORT_DIR)
            .join("fact_orphan.json");
        assert!(backup.exists(), "export-before-purge must leave a backup");

        // The same-pass stamp contract: ALL of 18, 19 and 20 present.
        let version: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(version, 21, "one open must stamp through V21");
    }

    /// Review round 5, finding 4: a file-IO failure inside the backed repair
    /// arm must not abort migrate() — AppDb::open would fail on every launch
    /// (boot loop). Fail-safe posture: skip the destructive phase with a loud
    /// WARN, damaged data survives as-is, and the stamp still lands.
    #[test]
    fn v18_file_backed_repair_survives_an_unwritable_export_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        let conn = Connection::open(tmp.path().join("brain.db")).unwrap();
        build_pre_v18_brain(&conn);
        seed_v18_damage(&conn);
        // Block the export directory with a regular FILE so
        // create_dir_all/export fails with a non-DB error (full-disk proxy).
        std::fs::write(tmp.path().join("repair-export-186"), "not a dir").unwrap();

        migrate(&conn, None, Some(tmp.path()))
            .expect("migrate must survive an unwritable export dir (fail-safe)");

        // The destructive phase was skipped: the damaged row survives intact.
        let ref_after: String = conn
            .query_row(
                "SELECT source_ref FROM llm_wiki_entries WHERE id = 'fact_v18'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            ref_after.starts_with('{'),
            "damaged row must survive untouched when the backup export fails"
        );
        let evidence_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM librarian_evidence WHERE entry_id = 'fact_v18'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(evidence_rows, 0);

        // And the stamp still lands — no boot loop on the next launch.
        let version: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(version, 21);
    }

    /// V20 test parity with the V18 fail-safe above (final review F1): a
    /// file-backed brain with a live doomed (flagged, unanchored) row whose
    /// `repair-export-phase2/` export path is blocked by a regular FILE must
    /// NOT abort migrate() — the export fs error takes the fail-safe posture
    /// (WARN + skip destructive phase, AppDb::open still succeeds) — and the
    /// doomed row survives for the manual `ct evidence regrade`.
    ///
    /// Unlike V18, the stamp does NOT land on a skip (PR #201 review): heal
    /// grounds strictly again post-revert, so a stamp here would let the same
    /// session's heal soft-delete the doomed rows before the manual regrade
    /// could export them (regrade only sees `deleted_at IS NULL` stock). The
    /// unwritten 20 is the durable "recovery pending" marker heal defers to,
    /// and V21 is deferred behind it.
    #[test]
    fn v20_file_backed_regrade_survives_an_unwritable_export_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        let conn = Connection::open(tmp.path().join("brain.db")).unwrap();
        build_pre_v18_brain(&conn);
        seed_v18_damage(&conn);
        // Same fixture strategy as the same-pass test: librarian_evidence
        // does not exist at V16 - MIGRATION_V18 creates it. Pre-create the
        // table and stamp 18 so migrate() runs only the V20 re-grade, then
        // seed one TRULY doomed row (empty evidence array, flagged) that V20
        // must export and purge - unless the export is blocked.
        conn.execute_batch(crate::db::schema::MIGRATION_V18)
            .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO schema_version (version) VALUES (18)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_entries
                (id, entity_id, title, body, tags, confidence, source_type,
                 source_ref, created_at, updated_at)
             VALUES ('fact_orphan', 'ent', 'orphan', 'b', '[]', 'inferred',
                     'librarian_inferred',
                     'librarian-abababababababababababababababab', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO librarian_evidence (entry_id, proposal_id, evidence_json,
                 unanchored, created_at)
             VALUES ('fact_orphan', 'prop_orphan',
                 '{\"evidence\":[],\"proposal_id\":\"prop_orphan\"}', 1, 1)",
            [],
        )
        .unwrap();
        // Block the V20 export directory with a regular FILE so
        // create_dir_all/export fails with a non-DB error (full-disk proxy).
        std::fs::write(tmp.path().join("repair-export-phase2"), "not a dir").unwrap();

        migrate(&conn, None, Some(tmp.path()))
            .expect("migrate must survive an unwritable V20 export dir (fail-safe)");

        // The destructive phase was skipped: the doomed row survives intact
        // (entry + flagged evidence), waiting for the manual
        // `ct evidence regrade`.
        let survived: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_entries WHERE id = 'fact_orphan'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            survived, 1,
            "doomed row must survive when the backup export fails"
        );
        let still_flagged: i64 = conn
            .query_row(
                "SELECT unanchored FROM librarian_evidence WHERE entry_id = 'fact_orphan'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            still_flagged, 1,
            "doomed row must remain flagged when the destructive phase is skipped"
        );

        // And the stamp does NOT land — the unwritten 20 is the durable
        // "recovery pending" marker: heal defers the doomed row, every open
        // retries the idempotent re-grade, and V21 stays deferred behind it.
        let version: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            version, 19,
            "a skipped V20 destructive phase must not stamp"
        );

        // V21 is deferred with it: stamping 21 here would mask the unwritten
        // 20 forever (every later open reads MAX(version) >= 21).
        let has_reviewed_by: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('curated_proposals')
                  WHERE name = 'reviewed_by'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(has_reviewed_by, 0, "V21 must wait for the V20 recovery");

        // Heal defers the still-flagged row while recovery is pending — this
        // is the exact soft-delete that defeated export-before-purge when the
        // skip path stamped anyway (PR #201 review finding 1).
        let token: String = conn
            .query_row(
                "SELECT source_ref FROM llm_wiki_entries WHERE id = 'fact_orphan'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            crate::db::commit::source_ref_is_still_grounded(&conn, &token),
            "heal must treat a flagged row as grounded while the V20 recovery is pending"
        );

        // Recovery unblocks the ladder: unblock the export dir, re-run the
        // re-grade (what `ct evidence regrade` / the next open does), and the
        // following migrate() stamps 20 AND lands V21.
        std::fs::remove_file(tmp.path().join("repair-export-phase2")).unwrap();
        let now_ms = crate::db::commit::ms_now();
        let report =
            crate::db::evidence_regrade::regrade_unanchored(&conn, Some(tmp.path()), now_ms)
                .unwrap();
        assert_eq!(
            report.purged, 1,
            "recovered re-grade must purge the doomed row"
        );
        assert!(!report.skipped_destructive);

        migrate(&conn, None, Some(tmp.path())).expect("migrate must succeed after the recovery");
        let version: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(version, 21, "recovery settles V20 and lands V21");
    }

    #[test]
    fn canary_counts_only_json_shaped_malformed_refs() {
        let conn = open_in_memory().unwrap();

        // Valid JSON — must not be counted.
        seed_entry_with_source_ref(&conn, "e1", r#"{"proposal_id":null,"evidence":[]}"#);
        // Legitimate plain path — must not be counted (does not start with `{`).
        seed_entry_with_source_ref(&conn, "e2", "documents/notes.md");
        // NULL — must not be counted.
        seed_entry_with_source_ref_null(&conn, "e3");
        // JSON-shaped but unparseable — the #162 corruption signature.
        seed_entry_with_source_ref(&conn, "e4", "{evidencechunk_id3261quote hi");

        assert_eq!(warn_on_malformed_source_refs(&conn), 1);
    }

    #[test]
    fn canary_is_zero_on_a_clean_brain() {
        let conn = open_in_memory().unwrap();
        seed_entry_with_source_ref(&conn, "e1", r#"{"proposal_id":null,"evidence":[]}"#);
        assert_eq!(warn_on_malformed_source_refs(&conn), 0);
    }

    // ---- #204 V22: unify documents.path on the virtual (configured-root)
    // form. The watcher previously staged rows keyed by the CANONICAL path
    // while the walker stored the VIRTUAL (configured-root-relative) path,
    // and reconcile.rs:74-77 documents the column as virtual — so the
    // watcher's row was a divergent phantom. V22 rewrites class-1 rows
    // (canonical-root-prefixed) to the configured-root prefix in place and
    // deletes class-2 rows (trusted-link phantoms whose path is outside the
    // canonical vault root). Both branches are restricted to tier='user_doc'
    // for parity with reconcile.rs:83.

    /// Class 1: a `user_doc` row whose path starts with `<canonical_root>/`
    /// has its prefix rewritten to `<configured_root>/` in place. The row's
    /// `id`, chunks, and synth watermark stay attached (no re-embedding).
    #[test]
    fn v22_rewrites_canonical_prefixed_user_doc_row_to_configured_prefix() {
        let conn = open_in_memory().unwrap();
        // macOS /var → /private/var is the only divergence most users hit;
        // use a different prefix on each side so the rewrite is observable
        // without filesystem canonicalize quirks.
        let canonical_root = "/private/var/vault";
        let configured_root = "/Users/kurt/vault";
        let canonical_path = format!("{canonical_root}/notes.md");

        // Seed a row in the pre-fix canonical-path shape. `open_in_memory()`
        // has already stamped schema_version=22 over this conn, so we rewind
        // to v21 first (V22 only fires when MAX(version) < 22; for the unit
        // boundary this lets us invoke the helper directly).
        conn.execute(
            "DELETE FROM schema_version WHERE version >= 22",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO documents (path, hash, tier, status) \
             VALUES (?1, 'h-class1', 'user_doc', 'indexed')",
            rusqlite::params![&canonical_path],
        )
        .unwrap();

        let report = v22_unify_documents_path(&conn, configured_root, canonical_root).unwrap();
        assert_eq!(
            report.rewritten, 1,
            "exactly one class-1 row should have been rewritten"
        );
        assert_eq!(
            report.deleted, 0,
            "no class-2 rows were seeded, so none should be deleted"
        );

        let actual: String = conn
            .query_row(
                "SELECT path FROM documents WHERE hash = 'h-class1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            actual,
            format!("{configured_root}/notes.md"),
            "canonical-root-prefixed path must rewrite to configured-root form"
        );

        // The row's id must survive — chunks cascade on row deletion, and a
        // re-insert would force re-embedding. Preserving the id is the whole
        // point of UPDATE over DELETE+INSERT.
        let id: i64 = conn
            .query_row(
                "SELECT id FROM documents WHERE hash = 'h-class1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(id > 0, "rewritten row must keep its row id");
    }

    /// Class 2: a `user_doc` row whose path is OUTSIDE the canonical vault
    /// root (the trusted-link phantom the watcher staged in the bug) must
    /// be deleted, not rewritten. The walker has either already written the
    /// correct virtual-path row or will re-ingest on next pass; the
    /// canonical-path duplicate is what D2a flagged as the divergent shape.
    #[test]
    fn v22_deletes_class2_phantom_rows_outside_canonical_root() {
        let conn = open_in_memory().unwrap();
        let canonical_root = "/private/var/vault";
        let configured_root = "/Users/kurt/vault";
        // Path under an external target reached via a trusted-link under
        // documents/ — the watcher's pre-fix bug shape.
        let phantom_path = "/external/target/specs/x.md";

        conn.execute(
            "DELETE FROM schema_version WHERE version >= 22",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO documents (path, hash, tier, status) \
             VALUES (?1, 'h-phantom', 'user_doc', 'indexed')",
            rusqlite::params![phantom_path],
        )
        .unwrap();

        let report = v22_unify_documents_path(&conn, configured_root, canonical_root).unwrap();
        assert_eq!(report.deleted, 1, "trusted-link phantom must be deleted");
        assert_eq!(report.rewritten, 0, "no class-1 row was seeded");

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM documents WHERE hash = 'h-phantom'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "the phantom row must be gone after V22");
    }

    /// Collision case: a class-1 row whose rewrite would land on a path
    /// already occupied by a walker-written row. The watcher row is the
    /// duplicate phantom — same bytes, same target — so the walker row wins
    /// and the watcher row is deleted (the plain UPDATE would fail
    /// UNIQUE(path)). This is the case that fires on macOS where the vault
    /// root canonicalizes to /private/var/... and the watcher writes that
    /// form while the walker writes /var/... for the same file.
    #[test]
    fn v22_deletes_class1_row_that_collides_with_walker_written_row() {
        let conn = open_in_memory().unwrap();
        let canonical_root = "/private/var/vault";
        let configured_root = "/Users/kurt/vault";
        let canonical_path = format!("{canonical_root}/notes.md");
        let configured_path = format!("{configured_root}/notes.md");

        conn.execute(
            "DELETE FROM schema_version WHERE version >= 22",
            [],
        )
        .unwrap();
        // Watcher row (canonical path, the pre-fix bug shape).
        conn.execute(
            "INSERT INTO documents (path, hash, tier, status) \
             VALUES (?1, 'h-watcher', 'user_doc', 'pending')",
            rusqlite::params![&canonical_path],
        )
        .unwrap();
        // Walker row (virtual path, already correct).
        conn.execute(
            "INSERT INTO documents (path, hash, tier, status) \
             VALUES (?1, 'h-walker', 'user_doc', 'indexed')",
            rusqlite::params![&configured_path],
        )
        .unwrap();

        let report = v22_unify_documents_path(&conn, configured_root, canonical_root).unwrap();
        assert_eq!(
            report.rewritten, 0,
            "the class-1 row's target is taken by the walker row, so it must \
             be deleted, not rewritten"
        );
        assert_eq!(
            report.deleted, 1,
            "exactly one row (the watcher duplicate) must be deleted"
        );

        let remaining: String = conn
            .query_row(
                "SELECT path FROM documents WHERE hash = 'h-walker'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            remaining, configured_path,
            "the walker row's authoritative path and hash must survive"
        );

        let watcher_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM documents WHERE hash = 'h-watcher'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            watcher_count, 0,
            "the watcher's duplicate phantom must be deleted by the collision branch"
        );
    }

    /// `tier = 'wiki'` rows must survive V22 untouched. Wiki entries have no
    /// filesystem-path semantics (the engine creates them from chunks; they
    /// can carry a `path` for cross-referencing, but never a canonical-root
    /// form the watcher stages), and reconcile.rs:83 only operates on
    /// `user_doc`. Touching wiki rows here would broaden the blast radius
    /// past the bug's actual scope.
    #[test]
    fn v22_leaves_wiki_tier_rows_alone() {
        let conn = open_in_memory().unwrap();
        let canonical_root = "/private/var/vault";
        let configured_root = "/Users/kurt/vault";
        let wiki_path_under_canonical = format!("{canonical_root}/facts/engine-derives-this.md");

        conn.execute(
            "DELETE FROM schema_version WHERE version >= 22",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO documents (path, hash, tier, status) \
             VALUES (?1, 'h-wiki', 'wiki', 'indexed')",
            rusqlite::params![&wiki_path_under_canonical],
        )
        .unwrap();

        let report = v22_unify_documents_path(&conn, configured_root, canonical_root).unwrap();
        assert_eq!(
            report.rewritten + report.deleted,
            0,
            "wiki-tier rows must not be rewritten or deleted"
        );

        let wiki_path_actual: String = conn
            .query_row(
                "SELECT path FROM documents WHERE hash = 'h-wiki'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            wiki_path_actual, wiki_path_under_canonical,
            "wiki-tier rows must keep their original path"
        );
    }

    /// A second invocation finds zero rows in any of the three branches
    /// and reports zeros. This is what makes V22 safe to re-run: a crash
    /// after the rewrite but before the version stamp re-enters the body,
    /// and a re-application must produce the same end state without
    /// doubling deletes or rewriting an already-configured row back to its
    /// canonical form.
    #[test]
    fn v22_is_idempotent_on_second_invocation() {
        let conn = open_in_memory().unwrap();
        let canonical_root = "/private/var/vault";
        let configured_root = "/Users/kurt/vault";
        let seeded_path = format!("{canonical_root}/notes.md");

        conn.execute(
            "DELETE FROM schema_version WHERE version >= 22",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO documents (path, hash, tier, status) \
             VALUES (?1, 'h-idem', 'user_doc', 'indexed')",
            rusqlite::params![&seeded_path],
        )
        .unwrap();

        let first = v22_unify_documents_path(&conn, configured_root, canonical_root).unwrap();
        assert_eq!(
            first.rewritten, 1,
            "first run must rewrite the canonical-prefixed row"
        );

        let second = v22_unify_documents_path(&conn, configured_root, canonical_root).unwrap();
        assert_eq!(
            second.rewritten, 0,
            "second run must rewrite nothing — the row is already configured"
        );
        assert_eq!(
            second.deleted, 0,
            "second run must delete nothing — class-2 phantoms already gone"
        );

        // Final state: the row sits at the configured path and has not been
        // flipped back to the canonical form or duplicated.
        let final_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM documents WHERE hash = 'h-idem'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            final_count, 1,
            "exactly one row must survive — duplicates are the bug V22 fixes"
        );
        let final_path: String = conn
            .query_row(
                "SELECT path FROM documents WHERE hash = 'h-idem'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            final_path,
            format!("{configured_root}/notes.md"),
            "the row must remain at the configured-root form"
        );
    }

    /// V22 is reached through `migrate()`: when `vault_root` is `Some`, the
    /// migration block fires and stamps schema_version=22. When it is
    /// `None`, the migration refuses to stamp (loud fatal WARN, see the
    /// next test) and the schema stays below 22. This test pins the Some
    /// path end-to-end.
    #[test]
    fn migrate_with_resolvable_vault_root_stamps_v22_and_rewrites_paths() {
        let conn = open_in_memory().unwrap();
        // open_in_memory() runs every migration including V22 with no root
        // — rewind to v21 so the migrate() call below actually exercises V22.
        conn.execute(
            "DELETE FROM schema_version WHERE version >= 22",
            [],
        )
        .unwrap();
        let canonical_root = "/private/var/vault";
        let configured_root = "/Users/kurt/vault";
        // Seed a row in the pre-fix canonical-path shape.
        conn.execute(
            "INSERT INTO documents (path, hash, tier, status) \
             VALUES (?1, 'h-migrate', 'user_doc', 'indexed')",
            rusqlite::params![format!("{canonical_root}/notes.md")],
        )
        .unwrap();

        migrate(
            &conn,
            Some(VaultRoots {
                configured: configured_root.to_string(),
                canonical: canonical_root.to_string(),
            }),
            None,
        )
        .expect("migrate with resolvable vault root must succeed");

        let version: i64 = conn
            .query_row(
                "SELECT MAX(version) FROM schema_version",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(version, 22, "V22 must be stamped when the migration runs");

        let rewritten_path: String = conn
            .query_row(
                "SELECT path FROM documents WHERE hash = 'h-migrate'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            rewritten_path,
            format!("{configured_root}/notes.md"),
            "migrate(Some(VaultRoots)) must rewrite the seeded canonical path"
        );
    }

    /// V22 with `vault_root = None` is a user-approved "loud fatal, refuse
    /// to run, do not stamp" — mirroring the V20→V21 recovery-pending
    /// pattern. The schema must stay below 22 so subsequent migrations
    /// (none yet, but the gate exists) do not silently run past an
    /// unfinished step. The seeded canonical-path row is left untouched.
    #[test]
    fn migrate_without_resolvable_vault_root_refuses_v22_and_does_not_stamp() {
        let conn = open_in_memory().unwrap();
        // open_in_memory() ran V1-V21 (the cap without a root). Rewind past
        // 21 to make the migrate(None) call actually reach V22.
        conn.execute(
            "DELETE FROM schema_version WHERE version >= 21",
            [],
        )
        .unwrap();
        let canonical_root = "/private/var/vault";
        let seeded_path = format!("{canonical_root}/notes.md");
        conn.execute(
            "INSERT INTO documents (path, hash, tier, status) \
             VALUES (?1, 'h-defer', 'user_doc', 'indexed')",
            rusqlite::params![&seeded_path],
        )
        .unwrap();

        migrate(&conn, None::<VaultRoots>, None).expect("migrate must succeed even when V22 refuses");

        let version: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            version < 22,
            "V22 must NOT stamp when the vault root is unresolvable (got version={version})"
        );

        // The seeded row is left in its canonical shape — V22 did not run.
        let still_canonical: String = conn
            .query_row(
                "SELECT path FROM documents WHERE hash = 'h-defer'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            still_canonical, seeded_path,
            "a deferred V22 must not modify the seeded row"
        );
    }
}
