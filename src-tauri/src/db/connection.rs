use crate::db::okf_ddl;
use crate::db::schema::{
    DELETED_SOURCES_DDL, MIGRATION_V1, MIGRATION_V10, MIGRATION_V11, MIGRATION_V12, MIGRATION_V13,
    MIGRATION_V14, MIGRATION_V15, MIGRATION_V16, MIGRATION_V18, MIGRATION_V19, MIGRATION_V2,
    MIGRATION_V21, MIGRATION_V26, MIGRATION_V3, MIGRATION_V4, MIGRATION_V5, MIGRATION_V6,
    MIGRATION_V9,
};
use crate::hasher::hash_bytes;
use crate::vault::VaultConfig;
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension};
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
/// replace the canonical-root prefix with `<configured_root>/`. A `user_doc`
/// row whose class-1 rewrite would COLLIDE with an existing walker-written
/// row at the same virtual path is deleted rather than updated — the walker
/// row carries the authoritative hash and synth watermark, the watcher's
/// row is a duplicate phantom of the same bytes. (Plain UPDATE on collision
/// would fail the UNIQUE(path) constraint.)
///
/// Orphan class-2 trusted-link phantoms — rows whose path is the canonical
/// external target of a symlinked trusted link under the configured root —
/// are deliberately NOT deleted. The original draft of this function had a
/// Step 1 that deleted every `user_doc` row whose path did not start with
/// EITHER prefix, but that predicate also catches rows from a relocated
/// vault and rows under a phantom root. Per spec D2b ("Relativization
/// failure is fail-open"), deleting rows we cannot place inside the vault
/// is the same class of unrecoverable mistake the empty-walk guard exists
/// to prevent. Step 2 above still handles the common case where both the
/// watcher phantom and the walker row exist in the vault; orphan phantoms
/// persist as dead rows until the user manually removes them.
///
/// Restricted to `tier = 'user_doc'` for parity with reconcile.rs:83; wiki
/// rows have no filesystem-path semantics and must never be touched.
///
/// Idempotent: a second invocation finds zero rows matching the canonical
/// prefix (the rewrite already landed; the collision deletes already ran)
/// and reports zeros.
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
    // SQLite's `substr()` counts UTF-8 CHARACTERS, not bytes — but
    // `String::len()` returns byte length. A non-ASCII root
    // (e.g. `/Users/josé/vault/`) would otherwise make the offsets land on
    // the wrong code-unit and Step 1 / Step 3 misclassify or rewrite from
    // the wrong offset. Use `chars().count()` for both prefixes.
    let prefix_len = canonical_prefix.chars().count() as i64;
    let substr_start = prefix_len + 1;

    // BEGIN IMMEDIATE: same concurrent-migration guard as V21 (a desktop app
    // and a simultaneously launching `--mcp` server can both reach this
    // block; BEGIN IMMEDIATE takes the write lock up front so exactly one
    // process runs the rewrite and the other no-ops into the version stamp).
    conn.execute_batch("BEGIN IMMEDIATE;")?;
    let result: Result<V22Report> = (|| {
        // V22 deliberately has no "delete class-2 phantoms" branch.
        //
        // The original draft of Step 1 deleted every `user_doc` row whose
        // `path` did not start with EITHER prefix. That predicate is a
        // strict superset of the actual phantom set — it also catches:
        //   * rows from a relocated vault (user moved `/old/vault` to
        //     `/new/vault`; every `/old/vault/…` row matches nothing), and
        //   * rows under a phantom root (`canonicalize_workspace_root`
        //     falls back to the input on error, so `canonical == configured
        //     == <non-existent string>` and every row matches nothing).
        //
        // Per spec D2b (docs/superpowers/specs/2026-09-10-watcher-walker-
        // brain-dir-exclusion-design.md, "Relativization failure is
        // fail-open"): "Deleting rows we cannot place inside the vault
        // would be the same class of unrecoverable mistake the empty-walk
        // guard exists to prevent."
        //
        // Orphan class-2 phantoms (rows whose path is the canonical external
        // target of a trusted link but no matching walker-written virtual-
        // path row exists yet) therefore persist as dead rows. Step 2
        // below still deletes the more common shape — a watcher pre-fix
        // canonical-path row whose rewrite would collide with an existing
        // walker row — because in that case both rows ARE in the vault by
        // construction and the watcher row is a known duplicate.

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
            deleted: deleted_collisions,
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
        let existing = crate::db::ddl_compat::existing_columns(conn, "llm_wiki_entries")?;
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

    // V29 (PR #336 T3 M1) column-add runs HERE — after V18 has guaranteed the
    // librarian_evidence table exists, and BEFORE the V20 block, whose
    // in-migration re-grade scan names `adjudication_hold`. Lock-free in
    // steady state (T4 delta M1); the version stamp runs later, after V28.
    ensure_v29_column(conn)?;

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
                    eprintln!(
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
        // and deletes class-1 rows whose rewrite would collide with an
        // existing walker-written row at the same virtual path. Both
        // branches restrict to `tier = 'user_doc'` for parity with
        // reconcile.rs:83. Orphan class-2 trusted-link phantoms (rows
        // whose path is outside both roots) are deliberately LEFT ALONE
        // — see the doc-comment on `v22_unify_documents_path` and spec D2b.
        //
        // Vault-root requirement (user-approved Option A for the migration):
        // refuse to run without a resolved root, log a loud FATAL, and
        // do NOT stamp 22. The watcher keeps writing the divergent shape
        // in the meantime; every subsequent open re-logs this FATAL until
        // the user resolves the root and V22 runs. Refusing to stamp is
        // deliberate — it keeps the schema below 22 as a durable
        // "recovery pending" marker, matching the V20/V21 precedent where
        // an unfinished migration leaves a trail the next open can follow.
        //
        // IMPORTANT: this branch must NOT `return Ok(())` on the rootless
        // path. Doing so skips everything below this `if` block: the V8
        // event-taxonomy migration, the 90-day `curated_agent_log` prune,
        // `CREATE INDEX IF NOT EXISTS idx_curated_agent_log_created_at`,
        // `crate::db::schema_guard::verify_llm_wiki_schema(conn)`, and
        // `warn_on_malformed_source_refs(conn)`. Every rootless open
        // (`open_in_memory`, `migrate_open_db`, `open_app_db`, and the MCP
        // server) would lose the schema guard — losing the startup check
        // that previously rejected an incompatible `llm_wiki_entries` shape.
        match vault_root.as_ref() {
            None => {
                eprintln!(
                    "[ct::repair FATAL] #204 V22 DEFERRED: vault root could not be \
                     resolved (no VaultConfig root, no CURATED_VAULT_ROOT, no \
                     --vault). V22 rewrites documents.path from canonical to \
                     configured-root form and CANNOT run without a root — guessing \
                     would silently corrupt the index. Set the vault root in \
                     config.json (or pass --vault / CURATED_VAULT_ROOT) and reopen; \
                     V22 lands on the next open."
                );
                // Skip the V22 stamp only; the rest of `migrate()` must
                // still run. Every open retries and re-warns until the
                // user resolves the root.
            }
            Some(roots) => {
                let report = v22_unify_documents_path(conn, &roots.configured, &roots.canonical)?;
                eprintln!(
                    "[ct::repair] #204 V22: rewritten={} deleted={} (canonical-path \
                     rows unified to configured-root form)",
                    report.rewritten, report.deleted
                );
                conn.execute(
                    "INSERT OR IGNORE INTO schema_version (version) VALUES (22)",
                    [],
                )?;
            }
        }
    }

    // V23 — `curated_proposal_deleted_sources` (issue #211 spec D2).
    //
    // The DDL is ungated, like the curated_agent_log index below: every open,
    // rooted or not, gets the table, so no delete path depends on V22's state.
    // The STAMP is gated on V22 having stamped. `migrate()` gates on
    // MAX(version), so stamping 23 while a rootless open has deferred V22
    // would make every later rooted open skip V22 forever, and its FATAL
    // re-warn with it.
    conn.execute_batch(DELETED_SOURCES_DDL)?;
    let stamped: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_version",
        [],
        |r| r.get(0),
    )?;
    if stamped >= 22 {
        conn.execute(
            "INSERT OR IGNORE INTO schema_version (version) VALUES (23)",
            [],
        )?;
    }

    // V24 — core-llm-wiki 7.9.0 engine migration 13 mirror: temporal fact
    // columns on `llm_wiki_entries`, `occurred_at` on `llm_wiki_events`, and
    // the librarian watermark columns on `llm_wiki_checkpoints` (all
    // nullable; no backfill — upstream reads a NULL `valid_from` as "since
    // created_at").
    //
    // The ALTERs run on every open, ungated like V23's DDL: the startup
    // schema guard below demands the full 7.9.0 column set, and the JS
    // package migration that adds these columns only runs once the frontend
    // boots — too late for CLI/MCP opens. Every name here is a hardcoded
    // literal with a plain nullable type, so the V17 snapshot-and-ALTER
    // pattern applies — one `PRAGMA table_info` per table rather than the
    // two round trips per column `add_column_if_missing` costs, since this
    // runs on every open forever. Upstream migration 13 is PRAGMA-guarded
    // the same way, so whichever side runs second no-ops.
    //
    // The STAMP is gated on V22 having stamped, exactly like V23 (reusing
    // the read above: nothing between the two gates can change the answer —
    // V23's insert only fires when `stamped >= 22` already holds). Stamping
    // 24 while a rootless open has deferred V22 would make every later
    // rooted open read MAX(version) >= 24, skip `if version < 22`, and
    // permanently skip V22's `documents.path` rewrite and its FATAL re-warn.
    apply_v24_temporal_columns(conn, stamped >= 22)?;

    // V25 — wisdom-deposit substrate (spec
    // docs/superpowers/specs/2026-10-01-wisdom-deposit-tool-surface-design.md):
    // (a) seed the agent deposit folder rules. `get_folder_mode` walks
    //     ancestors, so the supersessions/ child row is an EXPLICIT override —
    //     without it the lane would inherit `synthesize` and auto-generate
    //     standalone facts from supersession proposals (CodeRabbit finding 2
    //     on the spec PR). Auto-approve is safe here because these folders are
    //     agent-class: committed rows carry the deposit tier, never
    //     `user_stated`.
    // (b) create the deposit kick-state ledger (D3/D4 status truth source).
    // The STAMP is gated on V22 having stamped, exactly like V23/V24: a
    // rootless open defers V22, and stamping 25 would make every later
    // rooted open skip V22's FATAL re-warn permanently. Re-read MAX(version)
    // here (the `stamped` snapshot above predates this open's V22 run — on a
    // fresh rooted brain it reads 21 while V22 stamps 22 in this same call).
    let stamped_now: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_version",
        [],
        |r| r.get(0),
    )?;
    if stamped_now >= 22 {
        conn.execute_batch(
            "INSERT OR IGNORE INTO folder_rules
                 (folder_path, librarian_mode, auto_approve)
                 VALUES ('immutable-source-files/agents', 'synthesize', 1);
             INSERT OR IGNORE INTO folder_rules
                 (folder_path, librarian_mode, auto_approve)
                 VALUES ('immutable-source-files/agents/supersessions', 'summarize', 1);
             -- Task 9 (user-confirmation-queue, spec §3.7.3): the stated/ tree
             -- is the APP's capture lane — synthesize + auto-approve so a
             -- captured statement is human tier the moment the librarian
             -- finishes (the attested-source provenance arm stamps
             -- `user_stated`). Agents can NEVER deposit here: the deposit
             -- path is allow-listed to immutable-source-files/agents/ (the
             -- folder row only governs the librarian leg of the app's own
             -- kick). The three `stated/. *.jsonl` ledgers are operational
             -- state, never documents — walk_vault excludes them by name and
             -- their extension is not ingestible.
             INSERT OR IGNORE INTO folder_rules
                 (folder_path, librarian_mode, auto_approve)
                 VALUES ('stated', 'synthesize', 1);
             CREATE TABLE IF NOT EXISTS deposit_kick_state (
                 path       TEXT PRIMARY KEY,
                 state      TEXT NOT NULL
                            CHECK(state IN ('pending','chunked','ingested','failed',
                                            'no_ingest_host','queued_watcher')),
                 error      TEXT,
                 updated_ms INTEGER NOT NULL
             );",
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO schema_version (version) VALUES (25)",
            [],
        )?;
    }

    // V26 — ontology node-type gate wave-1 tables
    // (spec docs/superpowers/specs/2026-10-03-ontology-node-type-gate-and-heal-design.md
    // §2.9.1/§3): `entity_type_origin` (type-origin ledger),
    // `entity_redirects` (merge loser→survivor), `ct_entity_optouts`
    // (entity-level opt-out marker).
    //
    // Ungated DDL on every open, stamp gated on V22 having stamped — the
    // V23/V24/V25 pattern exactly. Stamping 26 while a rootless open has
    // deferred V22 would make every later rooted open read MAX(version) >= 26,
    // skip `if version < 22`, and permanently skip V22's `documents.path`
    // rewrite and its FATAL re-warn. (This ALSO caps a rootless open at 21:
    // V22's stamp gate reads MAX(version), so 26 must never land before 22.)
    rebuild_pre_r21_origin_ledger(conn)?;
    conn.execute_batch(MIGRATION_V26)?;
    if stamped_now >= 22 {
        conn.execute(
            "INSERT OR IGNORE INTO schema_version (version) VALUES (26)",
            [],
        )?;
    }

    // V27 — wisdom embed-scheme stamp (issue #265 follow-up, spec
    // docs/superpowers/specs/2026-10-07-issue265-wisdom-instruction-prefix-design.md).
    // RENUMBERED 26 → 27 at merge: main's V26 is the ontology node-type gate
    // wave-1 tables above, already merged and stamped by released builds, so
    // the scheme stamp takes the next free version. Adds
    // `llm_wiki_entries.embed_scheme TEXT NOT NULL DEFAULT 'raw'` plus the
    // explicit backfill UPDATE the spec pins (the ADD COLUMN default already
    // backfills under SQLite semantics, but the UPDATE is kept so the NULL
    // class provably never exists and `=`/`!=` filters stay two-valued), and
    // the read-scheme seed `llm_wiki_meta.wisdom_active_scheme = 'raw'`.
    //
    // Column-add runs on every open, ungated and PRAGMA-guarded exactly like
    // V24 (the same snapshot → BEGIN IMMEDIATE → re-inspect under lock
    // sequence guards concurrent desktop/`--mcp` opens against a duplicate
    // `duplicate column name` failure). The backfill and meta seed ride the
    // column-add transaction, so they run once, never on a steady-state
    // open. The STAMP is gated on V22 having stamped, like V23/V24/V25/V26: a rootless open
    // defers V22, and stamping 27 would make every later rooted open skip
    // V22's FATAL re-warn permanently. `stamped_now` above predates this
    // block only on a fresh brain where V22 stamps 22 in this same call — it
    // is re-read inside the helper after V24/V25/V26's inserts, mirroring
    // V25's re-read.
    apply_v27_embed_scheme(conn)?;
    apply_v28_two_stage_columns(conn)?;
    // V29's version stamp (T4 delta m3): the early column-add site cannot
    // stamp — a fresh brain reads MAX(version) = 19 there, under the V22
    // gate. Here the ladder has settled, so a fresh brain leaves its FIRST
    // open at 29.
    stamp_v29(conn)?;
    // V30 (PR #352 Task 1, spec 2026-11-14-areas §5): add
    // `llm_wiki_entries.area TEXT NULL` beside the settled ladder. The
    // embed_scheme precedent (V27): ungated PRAGMA-guarded column-add on
    // every open, stamp last gated on V22. NULL means whole-repo; legacy
    // rows keep NULL (no backfill).
    apply_v30_area_column(conn)?;
    // sso9 migration steps 1+2 (spec §3 PR-A steps 1+2): MERGE-refresh
    // the stored 8.x EA manifests, then re-type okf_type/edges. One backup,
    // gated on V22 + exact stamp membership; skipped (unstamped) on in-memory
    // opens.
    apply_sso9_migrations(conn)?;
    // V33 engine-9x mirror: core-llm-wiki 9.18.0
    // engine v16–v19 columns, source store, vector-removal ledger + its
    // triggers. Ungated shape on every open; stamp gated on V22. Called
    // right after `apply_sso9_migrations` so the mirror evaluates the
    // fully-retyped shape. Ordering vs the re-type is NOT a correctness
    // invariant: on deferred-V22 opens the re-type can run with the
    // triggers already present, which is harmless because the triggers
    // fire only on `deleted_at` updates or entry deletes and the re-type
    // does neither — it UPDATEs other columns of surviving rows only.
    apply_engine9_mirror(conn)?;

    // Deposit-time attribution index (user-confirmation-queue Task 1, spec
    // rev 4.1 §1/D4): the deposit file's frontmatter is authoritative; this
    // derived table is written inside the audit_and_seed_kick transaction.
    // Ungated, idempotent DDL on every open — no schema_version stamp. (The
    // ungated precedent is the V27/V28 bodies above, which also run on every
    // open; V18/V26 themselves DO stamp schema_version.) Task 2 adds its own
    // migrations/ files.
    conn.execute_batch(include_str!("migrations/0001_deposit_attribution.sql"))?;

    // Stated verification tables (user-confirmation-queue Task 2, spec rev 4.1
    // §8): derived indexes for attestations and queue denials, rebuilt from the
    // app-written stated/.attestations.jsonl / .denials.jsonl vault ledgers by
    // db::stated_ledger::rebuild_rederive (wired immediately below). Same
    // ungated, idempotent DDL shape as 0001 — no schema_version stamp.
    conn.execute_batch(include_str!("migrations/0002_stated_verification.sql"))?;

    // §8 rebuild convergence (PR #294 round-2 review, aws-cloud-agent A3):
    // `rebuild_rederive` is the ONLY writer that re-derives the Task-1/Task-2
    // derived tables and the `user_confirmed` stamps from the vault ledgers
    // (spec §8:511 defines re-derive/reset semantics for the REBUILD path
    // only — the confirm/deny/attest live writers never reset, Task-5 E2).
    // It walks the whole vault (attribution frontmatter), so it must NOT run
    // on every open: it runs ONCE per brain lifetime, stamped in
    // `llm_wiki_meta` like the V28 fresh-brain marker. A from-scratch brain
    // rebuild starts with no stamp, so the first rooted open re-derives; a
    // live brain's tables stay current through their write-time writers and
    // pay nothing at steady state.
    if let Some(roots) = vault_root.as_ref() {
        let already: Option<String> = conn
            .query_row(
                "SELECT value FROM llm_wiki_meta WHERE key = 'stated_ledger_rederived'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if already.is_none() {
            match crate::db::stated_ledger::rebuild_rederive(conn, Path::new(&roots.canonical)) {
                Ok(report) => {
                    eprintln!(
                        "[ct::repair] #294 §8 rederive: confirmations read={} applied={} \
                         attestations read={} dropped={} denials={} deposits={} (once per \
                         brain; derived state re-derived from stated/ ledgers)",
                        report.confirmations_read,
                        report.confirmations_applied,
                        report.attestations_read,
                        report.attestations_dropped,
                        report.denials_read,
                        report.deposit_attribution_rows,
                    );
                    conn.execute(
                        "INSERT OR IGNORE INTO llm_wiki_meta (key, value) \
                         VALUES ('stated_ledger_rederived', '1')",
                        [],
                    )?;
                }
                Err(e) => {
                    // Fail loud, fail deferred (the V22 shape): keep the
                    // previous derived state, skip the stamp, and re-warn on
                    // the next open until the ledger is repaired (spec §4
                    // residual: a corrupted ledger is operator-visible, not
                    // papered over).
                    eprintln!(
                        "[ct::repair FATAL] #294 §8 rederive DEFERRED: {e:#} — derived tables \
                         keep prior state; fix the ledger and reopen to retry"
                    );
                }
            }
        }
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

    // core-llm-wiki 7.7 engine migration V12, mirrored for Rust-first opens.
    crate::db::okf_ddl::apply_llm_wiki_v12_edge_index(conn)?;

    // core-llm-wiki 7.9 engine migration 13's temporal indexes, mirrored for
    // Rust-first opens (each gated on its V24 columns existing).
    crate::db::okf_ddl::apply_llm_wiki_v13_temporal_indexes(conn)?;

    // core-llm-wiki 9.18.0 helper-created objects (spec §3 PR-B step 5), mirrored
    // for Rust-first opens: created OUTSIDE the setupDatabase template (or
    // migration-only), so neither `migration_v7_sql` nor the V33 DDL re-run
    // makes them. Unstamped, every open, each gated like the engine helper.
    crate::db::okf_ddl::apply_llm_wiki_okf_v02_indexes(conn)?;
    crate::db::okf_ddl::apply_llm_wiki_v18_v19_partial_indexes(conn)?;
    crate::db::okf_ddl::apply_llm_wiki_source_store_indexes(conn)?;
    crate::db::okf_ddl::apply_llm_wiki_resurrect_trigger(conn)?;

    crate::db::schema_guard::verify_llm_wiki_schema(conn)?;

    // Startup canary: report JSON-shaped but unparseable `source_ref` values.
    // See `warn_on_malformed_source_refs` and issue #162.
    let _ = warn_on_malformed_source_refs(conn);

    Ok(())
}

/// V24 body (see the comment at its call site in `migrate()`): add the
/// core-llm-wiki 7.9.0 engine migration 13 columns, then stamp 24 when
/// `stamp` (V22 has stamped) holds. Split out so the concurrent-open guard
/// can be exercised on its own.
fn apply_v24_temporal_columns(conn: &Connection, stamp: bool) -> Result<()> {
    const V24_TEMPORAL_COLUMNS: &[(&str, &[(&str, &str)])] = &[
        (
            "llm_wiki_entries",
            &[
                ("valid_from", "INTEGER"),
                ("valid_to", "INTEGER"),
                ("superseded_by", "TEXT"),
                ("superseded_at", "INTEGER"),
            ],
        ),
        ("llm_wiki_events", &[("occurred_at", "INTEGER")]),
        (
            "llm_wiki_checkpoints",
            &[
                ("librarian_watermark_at", "INTEGER"),
                ("librarian_watermark_id", "TEXT"),
            ],
        ),
    ];

    // Concurrent-migration guard (review finding on PR #252, V21 precedent):
    // the desktop app and a simultaneously launching `--mcp` server can both
    // snapshot a column as missing, and the second ALTER would then fail its
    // open with `duplicate column name`. Any missing column takes BEGIN
    // IMMEDIATE (the loser blocks on the busy timeout set at open) and
    // re-inspects UNDER that lock, so exactly one process runs each ALTER.
    // The unlocked pre-check keeps the steady state — every column present —
    // from taking the write lock on every open.
    let v24_missing = |conn: &Connection| -> Result<Vec<(&str, &str, &str)>> {
        let mut missing = Vec::new();
        for (table, columns) in V24_TEMPORAL_COLUMNS {
            let existing = crate::db::ddl_compat::existing_columns(conn, table)?;
            for (column, declared_type) in *columns {
                if !existing.iter().any(|c| c == column) {
                    missing.push((*table, *column, *declared_type));
                }
            }
        }
        Ok(missing)
    };
    let stamp_v24 = |conn: &Connection| -> Result<()> {
        if stamp {
            conn.execute(
                "INSERT OR IGNORE INTO schema_version (version) VALUES (24)",
                [],
            )?;
        }
        Ok(())
    };
    if v24_missing(conn)?.is_empty() {
        stamp_v24(conn)?;
    } else {
        conn.execute_batch("BEGIN IMMEDIATE;")?;
        let applied = (|| -> Result<()> {
            for (table, column, declared_type) in v24_missing(conn)? {
                conn.execute(
                    &format!("ALTER TABLE {table} ADD COLUMN {column} {declared_type}"),
                    [],
                )?;
            }
            stamp_v24(conn)
        })();
        if let Err(e) = applied.and_then(|()| Ok(conn.execute_batch("COMMIT;")?)) {
            let _ = conn.execute_batch("ROLLBACK;");
            return Err(e);
        }
    }
    Ok(())
}

/// V27 body (see the comment at its call site in `migrate()`): add
/// `llm_wiki_entries.embed_scheme TEXT NOT NULL DEFAULT 'raw'` with the
/// explicit spec-pinned backfill, seed `llm_wiki_meta.wisdom_active_scheme =
/// 'raw'`, and stamp 27 when V22 has stamped. Renumbered from V26 at merge
/// (main's V26 is the ontology node-type gate wave). Split out beside
/// `apply_v24_temporal_columns` so the concurrency guard stays exercisable.
fn apply_v27_embed_scheme(conn: &Connection) -> Result<()> {
    // Unlocked pre-check keeps the steady state (column present) from taking
    // the write lock on every open, exactly like V24.
    let existing = crate::db::ddl_compat::existing_columns(conn, "llm_wiki_entries")?;
    if !existing.iter().any(|c| c == "embed_scheme") {
        conn.execute_batch("BEGIN IMMEDIATE;")?;
        let applied = (|| -> Result<()> {
            // Re-inspect UNDER the write lock: a concurrent desktop/`--mcp`
            // open may have added the column between the snapshot and the lock.
            let existing = crate::db::ddl_compat::existing_columns(conn, "llm_wiki_entries")?;
            if !existing.iter().any(|c| c == "embed_scheme") {
                conn.execute(
                    "ALTER TABLE llm_wiki_entries ADD COLUMN embed_scheme TEXT NOT NULL DEFAULT 'raw'",
                    [],
                )?;
                // The explicit backfill the spec pins. `ADD COLUMN ... NOT
                // NULL DEFAULT` already fills existing rows under SQLite
                // semantics; the UPDATE keeps the invariant independent of
                // engine behavior so the NULL class is provably empty. It
                // runs ONLY in the call that adds the column, inside the same
                // transaction: `embed_scheme` is unindexed, so an ungated
                // UPDATE would full-scan the table and take the write lock
                // on every open forever.
                conn.execute(
                    "UPDATE llm_wiki_entries SET embed_scheme = 'raw' WHERE embed_scheme IS NULL",
                    [],
                )?;
                // Seed the read-scheme meta key in the same transaction, so
                // the column and its seed land together. `INSERT OR IGNORE`
                // like every default-once seed here: an operator-installed
                // scheme is never clobbered. (A missing key still resolves
                // to `raw` in `embed_scheme::read_scheme`.)
                conn.execute(
                    "INSERT OR IGNORE INTO llm_wiki_meta (key, value) \
                     VALUES ('wisdom_active_scheme', 'raw')",
                    [],
                )?;
            }
            Ok(())
        })();
        if let Err(e) = applied.and_then(|()| Ok(conn.execute_batch("COMMIT;")?)) {
            let _ = conn.execute_batch("ROLLBACK;");
            return Err(e);
        }
    }
    // Stamp last, gated on V22 (see the call-site comment). The column add,
    // backfill and seed commit atomically above, so a crash before the stamp
    // leaves nothing half-applied; the steady state is the PRAGMA probe plus
    // this read and one INSERT OR IGNORE.
    let stamped_now: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_version",
        [],
        |r| r.get(0),
    )?;
    if stamped_now >= 22 {
        conn.execute(
            "INSERT OR IGNORE INTO schema_version (version) VALUES (27)",
            [],
        )?;
    }
    Ok(())
}

/// V28 body (spec 2026-10-09-issue271 §6/§8(c)/§9 step 1): add
/// `documents.embed_key TEXT` (model key whose vectors the doc's chunks carry;
/// NULL/stale ⇒ excluded from stage 1 — written by the §8(a) swap transaction
/// and ordinary ingests, NEVER by a migration backfill) and
/// `documents.last_indexed_hash TEXT` (hash of the bytes actually chunked and
/// embedded; written ONLY inside `mark_document_indexed`), with the ONE-SHOT
/// `last_indexed_hash` backfill. Runs AFTER every versioned migration (the V15
/// rebuild of `documents` copies an explicit column list and would drop the
/// columns otherwise). Stamp gated on V22 like V23–V27.
fn apply_v28_two_stage_columns(conn: &Connection) -> Result<()> {
    // Unlocked pre-check keeps the steady state (both columns present) from
    // taking the write lock on every open — the V24/V27 pattern.
    let existing = crate::db::ddl_compat::existing_columns(conn, "documents")?;
    let missing_embed_key = !existing.iter().any(|c| c == "embed_key");
    let missing_lih = !existing.iter().any(|c| c == "last_indexed_hash");
    let missing_cols = missing_embed_key || missing_lih;
    // Fresh-brain marker machinery (spec §8, "Fresh brain" defined): a
    // COUNT(*)=0 check cannot implement "no rows EVER" — rows leave via
    // regrade/prune/clear hard deletes — so the marker is a persistent
    // `llm_wiki_meta` row armed by an AFTER INSERT trigger on
    // `librarian_evidence` and backfilled for any brain that has rows today.
    // All probes below are read-only (no write lock taken to evaluate them);
    // the LOCK is entered only when work is actually pending:
    // - pre-V18 replica (`librarian_evidence` absent, V18 not yet run): skip
    //   everything WITHOUT error — SQLite rejects CREATE TRIGGER … ON a
    //   missing table; the machinery lands on a later open after V18's DDL.
    // - trigger missing (table present): create it (one lock, then steady).
    // - backfill pending (rows exist AND marker absent): arm the marker
    //   (one lock, then steady — nothing in production deletes the key, the
    //   clear transaction keeps `llm_wiki_meta`). An empty-evidence brain
    //   never re-locks for the backfill, so no per-open write-lock hazard
    //   (V27 precedent); `INSERT OR IGNORE` keeps the write idempotent.
    let evidence_table: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='librarian_evidence'",
        [],
        |r| r.get(0),
    )?;
    let has_evidence_table = evidence_table > 0;
    let trigger_pending =
        has_evidence_table && !trigger_exists(conn, "librarian_evidence_seen_trg")?;
    let backfill_pending = has_evidence_table && {
        let rows_exist: bool = conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM librarian_evidence)",
            [],
            |r| r.get::<_, i64>(0).map(|v| v > 0),
        )?;
        rows_exist
            && conn
                .query_row(
                    "SELECT value FROM llm_wiki_meta
                      WHERE key = 'librarian_evidence_seen'",
                    [],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
                .is_none()
    };
    if missing_cols || trigger_pending || backfill_pending {
        conn.execute_batch("BEGIN IMMEDIATE;")?;
        let applied = (|| -> Result<()> {
            // Re-inspect UNDER the write lock (concurrent desktop/`--mcp` open).
            let existing = crate::db::ddl_compat::existing_columns(conn, "documents")?;
            if !existing.iter().any(|c| c == "embed_key") {
                conn.execute("ALTER TABLE documents ADD COLUMN embed_key TEXT", [])?;
            }
            if !existing.iter().any(|c| c == "last_indexed_hash") {
                conn.execute(
                    "ALTER TABLE documents ADD COLUMN last_indexed_hash TEXT",
                    [],
                )?;
                // ONE-SHOT backfill, inside the ALTER call only (V27 precedent,
                // connection.rs:973-984): `pending` rows may already carry
                // queue.rs's pre-written NEW hash (spec §8(c) B1); pending_reindex
                // rows were staged from indexed with unchanged bytes ⇒ backfilled.
                conn.execute(
                    "UPDATE documents SET last_indexed_hash = hash
                      WHERE status IN ('indexed','pending_reindex')",
                    [],
                )?;
            }
            if has_evidence_table {
                // Idempotent both ways: `IF NOT EXISTS` + `INSERT OR IGNORE`
                // body. Concurrent opens that both entered the lock re-run
                // these harmlessly.
                conn.execute_batch(
                    "CREATE TRIGGER IF NOT EXISTS librarian_evidence_seen_trg
                       AFTER INSERT ON librarian_evidence
                     BEGIN
                       INSERT OR IGNORE INTO llm_wiki_meta (key, value)
                       VALUES ('librarian_evidence_seen', '1');
                     END;",
                )?;
                if backfill_pending {
                    conn.execute(
                        "INSERT OR IGNORE INTO llm_wiki_meta (key, value)
                         SELECT 'librarian_evidence_seen', '1'
                          WHERE EXISTS (SELECT 1 FROM librarian_evidence)",
                        [],
                    )?;
                }
            }
            Ok(())
        })();
        if let Err(e) = applied.and_then(|()| Ok(conn.execute_batch("COMMIT;")?)) {
            let _ = conn.execute_batch("ROLLBACK;");
            return Err(e);
        }
    }
    // Stamp last, gated on V22 (a rootless open must not mask a deferred V22).
    let stamped_now: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_version",
        [],
        |r| r.get(0),
    )?;
    if stamped_now >= 22 {
        conn.execute(
            "INSERT OR IGNORE INTO schema_version (version) VALUES (28)",
            [],
        )?;
    }
    Ok(())
}

/// V29 body (CT PR #336 Tier-3 Opus review M1): add the `librarian_evidence.adjudication_hold`
/// column (`INTEGER NOT NULL DEFAULT 0`, written only by `ct evidence
/// re-anchor`'s apply transaction).
///
/// WHY A SECOND COLUMN: the `unanchored` flag's contract across every writer
/// is `unanchored == !evidence_has_live_chunk(..)` (evidence_repair.rs,
/// bundle_apply.rs — "or it re-arms the heal-purge bait"). The heal reads the
/// flag in exactly one place (source_ref_is_still_grounded: a flagged row is
/// kept ONLY while the V20 re-grade is pending); regrade's purge scan reads it
/// unconditionally. Encoding "adjudication pending" by clearing the flag on
/// still-ungrounded rows therefore broke the invariant, exposed ambiguous rows
/// on pre-V20 brains, and protected nothing from the heal. The hold column
/// carries the adjudication state instead, and both destructive consumers
/// (regrade's purge scans, heal's keep-rule) consult it.
///
/// V29 column-add phase (called EARLY, right after the V18 block and before
/// the V20 block whose re-grade scan names `adjudication_hold`). Steady state
/// is a LOCK-FREE no-op (T4 delta M1): the unlocked pragma pre-check returns
/// before any lock is taken, exactly like V24/V28 — the BEGIN IMMEDIATE is
/// entered only when the column is actually missing, and the column is
/// re-checked UNDER the lock (the concurrent desktop + `--mcp` open race:
/// two opens can both snapshot "missing"; the loser of the write lock must
/// find the column present and no-op). The version STAMP lives in
/// [`stamp_v29`] and runs late (after V28) — at this early point a fresh
/// brain reads MAX(version) = 19 and could never clear the >= 22 gate.
pub(crate) fn ensure_v29_column(conn: &Connection) -> Result<()> {
    // Lock-free steady state (T4 delta M1): column present → read-only return.
    let has_column: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('librarian_evidence')
          WHERE name = 'adjudication_hold'",
        [],
        |r| r.get(0),
    )?;
    if has_column > 0 {
        return Ok(());
    }
    conn.execute_batch("BEGIN IMMEDIATE;")?;
    let applied = (|| -> Result<()> {
        let table_exists: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='librarian_evidence'",
            [],
            |r| r.get(0),
        )?;
        if table_exists == 0 {
            // Pre-V18 replica: skip WITHOUT error — SQLite rejects ALTER TABLE
            // on a missing table; the column lands on a later open after V18's
            // DDL (same machinery note as V28's trigger).
            return Ok(());
        }
        let has_column: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('librarian_evidence')
              WHERE name = 'adjudication_hold'",
            [],
            |r| r.get(0),
        )?;
        if has_column == 0 {
            conn.execute_batch(
                "ALTER TABLE librarian_evidence
                     ADD COLUMN adjudication_hold INTEGER NOT NULL DEFAULT 0;",
            )?;
        }
        Ok(())
    })();
    match applied {
        Ok(()) => conn.execute_batch("COMMIT;")?,
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK;");
            return Err(e);
        }
    }
    Ok(())
}

/// V29 stamp phase (called LATE, after V28 — see [`ensure_v29_column`]: at
/// the early call site a fresh brain reads MAX(version) = 19, below the V22
/// gate every stamp since V23 carries, so the stamp must wait until the
/// versioned ladder has settled). Idempotent (`INSERT OR IGNORE`).
///
/// The inner `>= 22` gate is SUBSUMED at both current call sites (T4 delta-3
/// doc nit): [`migrate`] reaches here only after the full ladder (MAX far
/// above 22), and [`ensure_v29_adjudication_hold`] already gates on
/// `MAX >= 28`. It stays as cheap standalone-call safety, not behavior.
pub(crate) fn stamp_v29(conn: &Connection) -> Result<()> {
    let stamped_now: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_version",
        [],
        |r| r.get(0),
    )?;
    if stamped_now >= 22 {
        conn.execute(
            "INSERT OR IGNORE INTO schema_version (version) VALUES (29)",
            [],
        )?;
    }
    Ok(())
}

/// V30 body (PR #352 Task 1, spec 2026-11-14-areas §5): add
/// `llm_wiki_entries.area TEXT NULL` — the area scope of a fact (NULL =
/// whole-repo; §2 Q9: legacy rows keep NULL, there is no backfill). Called
/// AFTER `stamp_v29` so the ladder is settled. The V27 embed_scheme
/// precedent: lock-free PRAGMA pre-check keeps the steady state (column
/// present) from taking the write lock; the BEGIN IMMEDIATE is entered only
/// when the column is actually missing, and the column is re-checked UNDER
/// the lock (the concurrent desktop/`--mcp` open race). The stamp is gated
/// on V22 like every stamp since V23.
fn apply_v30_area_column(conn: &Connection) -> Result<()> {
    // Lock-free pre-check keeps the steady state (column present) from
    // taking the write lock on every open.
    let has_column: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('llm_wiki_entries')
          WHERE name = 'area'",
        [],
        |r| r.get(0),
    )?;
    if has_column == 0 {
        conn.execute_batch("BEGIN IMMEDIATE;")?;
        let applied = (|| -> Result<()> {
            // Re-inspect UNDER the write lock: a concurrent desktop/`--mcp`
            // open may have added the column between the snapshot and the
            // lock (the V27 sequence).
            let has_column: i64 = conn.query_row(
                "SELECT COUNT(*) FROM pragma_table_info('llm_wiki_entries')
                  WHERE name = 'area'",
                [],
                |r| r.get(0),
            )?;
            if has_column == 0 {
                conn.execute(
                    "ALTER TABLE llm_wiki_entries ADD COLUMN area TEXT NULL",
                    [],
                )?;
            }
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
    // The stamp is NOT behind the column-add branch: the column-add can run
    // ungated on a rootless open (version still below the V22 gate — V29's
    // early column-add site, connection.rs:498, for the same reason), and an
    // early return there would leave the stamp unwritten forever once the
    // ladder settles. Stamp every open instead: INSERT OR IGNORE makes it
    // free, and the V22 gate keeps a rootless open from masking a deferred
    // V22. (V27's stamp reads MAX(version) on every open for the identical
    // reason — its column-add txn and stamp are sequential, not exclusive.)
    let stamped_now: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_version",
        [],
        |r| r.get(0),
    )?;
    if stamped_now >= 22 {
        conn.execute(
            "INSERT OR IGNORE INTO schema_version (version) VALUES (30)",
            [],
        )?;
    }
    Ok(())
}

/// Slot of the sso9 manifest MERGE-refresh step: (highest stamped literal on
/// this branch = V30) + 1. Every stamp, gate, test and log line reads this
/// const; if another lane takes the slot before merge, renumber the two
/// consts (V27 renumber precedent above).
pub(crate) const SSO9_REFRESH_VERSION: i64 = 31;
/// Slot of the sso9 okf_type/edge re-type step (plan Task 5).
pub(crate) const SSO9_RETYPE_VERSION: i64 = SSO9_REFRESH_VERSION + 1;

/// Slot of the engine-9x mirror step: (highest stamped
/// slot = `SSO9_RETYPE_VERSION` = 32) + 1. A literal, not an
/// expression, so a renumber is one visible edit here (V27 renumber
/// precedent): if another lane takes 33, bump this const — every stamp,
/// test and log line reads it. NOTE: the schema_guard fresh-brain test
/// additionally pins 31/32/33 as literals, as historical fact.
pub(crate) const ENGINE9_MIRROR_VERSION: i64 = 33;

/// Columns the 9.18.0 engine adds by `ALTER TABLE` in migrations v16/v18/v19
/// — `(table, column, declared type)`, the type copied verbatim from the
/// engine's ALTER text (`dist/index.js` of core-llm-wiki 9.18.0):
/// v16 `:2765` `ADD COLUMN source_pointer TEXT`; v18 `addPriorValidToColumn`
/// `:2852` `ADD COLUMN prior_valid_to INTEGER`, `addDerivedEventsColumn`
/// `:2863` `ADD COLUMN derived_events TEXT`; v19 `addEventsForgottenAtColumn`
/// `:2841` `ADD COLUMN forgotten_at INTEGER`. They equal the `setupDatabase`
/// template's own declarations (`LLM_WIKI_PACKAGE_DDL`), so fresh and
/// migrated brains agree on `PRAGMA table_info` (parity with the
/// verbatim template).
const ENGINE9_MIRROR_COLUMNS: &[(&str, &str, &str)] = &[
    ("llm_wiki_entries", "source_pointer", "TEXT"),
    ("llm_wiki_entries", "prior_valid_to", "INTEGER"),
    ("llm_wiki_entries", "derived_events", "TEXT"),
    ("llm_wiki_events", "forgotten_at", "INTEGER"),
];

/// `(sqlite_master type, name)` of every object `LLM_WIKI_PACKAGE_DDL`
/// creates, read from the const itself so the presence check can never drift
/// from the text V33 executes.
fn package_ddl_objects() -> Vec<(&'static str, String)> {
    const KINDS: &[(&str, &str)] = &[
        ("CREATE TABLE IF NOT EXISTS ", "table"),
        ("CREATE INDEX IF NOT EXISTS ", "index"),
        ("CREATE UNIQUE INDEX IF NOT EXISTS ", "index"),
        ("CREATE TRIGGER IF NOT EXISTS ", "trigger"),
    ];
    let ddl = okf_ddl::LLM_WIKI_PACKAGE_DDL;
    let mut out = Vec::new();
    for (kw, kind) in KINDS {
        for (at, _) in ddl.match_indices(kw) {
            let rest = &ddl[at + kw.len()..];
            let end = rest
                .find(|c: char| c.is_whitespace() || c == '(')
                .unwrap_or(rest.len());
            out.push((*kind, rest[..end].to_string()));
        }
    }
    out
}

/// Distinct tables in ENGINE9_MIRROR_COLUMNS, first-appearance order.
/// One PRAGMA table_info per DISTINCT table instead of per column.
fn grouped_mirror_tables() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for (table, _, _) in ENGINE9_MIRROR_COLUMNS {
        if !out.contains(table) {
            out.push(table);
        }
    }
    out
}

/// What the V33 step would add right now: missing mirror columns (only on
/// tables that exist — an absent table is created by the DDL re-run with the
/// columns inline) and missing package-DDL objects.
fn engine9_missing(conn: &Connection) -> Result<(Vec<String>, Vec<String>)> {
    let mut columns = Vec::new();
    for table in grouped_mirror_tables() {
        let existing = crate::db::ddl_compat::existing_columns(conn, table)?;
        for (t, column, _) in ENGINE9_MIRROR_COLUMNS {
            if *t == table && !existing.is_empty() && !existing.iter().any(|c| c == column) {
                columns.push(format!("{table}.{column}"));
            }
        }
    }
    let mut objects = Vec::new();
    for (kind, name) in package_ddl_objects() {
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = ?1 AND name = ?2",
            rusqlite::params![kind, name],
            |r| r.get(0),
        )?;
        if n == 0 {
            objects.push(format!("{kind} {name}"));
        }
    }
    Ok((columns, objects))
}

/// V33 body (spec §3 PR-B step 5): mirror core-llm-wiki 9.18.0 engine migrations v16–v19 onto brains
/// created under an older DDL — the guarded `ALTER`s for `source_pointer`,
/// `prior_valid_to`, `derived_events` (entries) and `forgotten_at` (events),
/// then the source store (`llm_wiki_source_documents`,
/// `llm_wiki_source_passages`), the vector-removal ledger
/// (`llm_wiki_vector_removals` + its pending index) and the two ledger
/// triggers, by re-executing `LLM_WIKI_PACKAGE_DDL` itself (every statement
/// `IF NOT EXISTS`) — verbatim by construction, no duplicated SQL.
///
/// Engine guards mirrored (core-llm-wiki 9.18.0 `dist/index.js`): v16
/// `:2759-2768` adds `source_pointer` only when `PRAGMA table_info` lacks it;
/// v18 `:2821-2828` / v19 `:2830-2836` the same per column (`:2849`, `:2860`,
/// `:2838`). v17 (`add_source_store`, `:2770-2819`) throws unless any
/// pre-existing `source_documents`/`source_passages` has EXACTLY its 8/6
/// columns and ZERO rows — CT therefore adds no CT-local column to, and writes
/// no row into, either table, so a later engine `setup()` passes v17. The
/// migration-only partial indexes (v18 `entries_deleted_idx`, v19
/// `events_forgotten_idx`) and the `setupDatabase` helper objects are the
/// every-open helpers, not part of this step.
///
/// Shape (V24/V30 pattern): ungated on every open — the every-open guard
/// demands the 9.18.0 shape even on rootless opens — with a lock-free
/// snapshot; only when something is missing, `BEGIN IMMEDIATE`, re-inspect
/// under the lock (the concurrent desktop/`--mcp` open race), apply,
/// `COMMIT`; any error rolls back and propagates. The stamp is written on
/// every open once `MAX(version) >= 22`, outside the add branch (V30
/// reasoning). Placed right after `apply_sso9_migrations` to
/// evaluate the mirror on the fully-retyped shape. Do NOT read a
/// re-type-vs-triggers ordering invariant into that placement: on
/// deferred-V22 opens the re-type runs with the ledger triggers already
/// present, and that is safe because the triggers are column-scoped
/// (`AFTER UPDATE OF deleted_at` / `AFTER DELETE`) while the re-type
/// UPDATEs neither column and never hard-deletes entries.
fn apply_engine9_mirror(conn: &Connection) -> Result<()> {
    let (cols, objs) = engine9_missing(conn)?;
    if !cols.is_empty() || !objs.is_empty() {
        conn.execute_batch("BEGIN IMMEDIATE;")?;
        let applied = (|| -> Result<(Vec<String>, Vec<String>)> {
            let (cols, objs) = engine9_missing(conn)?;
            for (table, column, decl) in ENGINE9_MIRROR_COLUMNS {
                if cols.iter().any(|c| *c == format!("{table}.{column}")) {
                    conn.execute(
                        &format!("ALTER TABLE {table} ADD COLUMN {column} {decl}"),
                        [],
                    )?;
                }
            }
            if !objs.is_empty() {
                conn.execute_batch(okf_ddl::LLM_WIKI_PACKAGE_DDL)?;
            }
            Ok((cols, objs))
        })();
        match applied.and_then(|(cols, objs)| {
            conn.execute_batch("COMMIT;")?;
            Ok((cols, objs))
        }) {
            Ok((cols, objs)) => {
                if !cols.is_empty() || !objs.is_empty() {
                    // stderr: under `--mcp` stdout is the JSON-RPC channel
                    // (mcp_server.rs "All tracing/logging must go to stderr").
                    eprintln!(
                        "[ct::migrate] engine9 mirror (slot {ENGINE9_MIRROR_VERSION}): added \
                         columns [{}]; objects [{}]",
                        cols.join(", "),
                        objs.join(", ")
                    );
                }
            }
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(e);
            }
        }
    }
    let stamped_now: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_version",
        [],
        |r| r.get(0),
    )?;
    if stamped_now >= 22 {
        conn.execute(
            "INSERT OR IGNORE INTO schema_version (version) VALUES (?1)",
            [ENGINE9_MIRROR_VERSION],
        )?;
    }
    Ok(())
}

/// The two slugs 9.x dropped, with the type each re-types to (spec §3).
const SSO9_RETYPE_MAP: [(&str, &str); 2] = [("handoff", "decision"), ("session_recap", "lesson")];

#[cfg(test)]
type Sso9CommitHook = Box<dyn Fn(&Connection)>;
#[cfg(test)]
type Sso9DeleteFn = fn(&Path) -> std::io::Result<()>;
#[cfg(test)]
type Sso9SnapshotCounts = (Vec<(String, String)>, Vec<i64>, Vec<(String, String)>);

#[cfg(test)]
thread_local! {
    /// Captured `[ct::migrate]` log lines (census-log / backup-reuse tests).
    static SSO9_LOG: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
    /// Test hook: runs after step 1's COMMIT, before the shape check.
    static SSO9_AFTER_REFRESH_COMMIT: std::cell::RefCell<Option<Sso9CommitHook>> =
        const { std::cell::RefCell::new(None) };
    /// Test hook: runs after step 2's COMMIT, before post-commit verification.
    static SSO9_AFTER_RETYPE_COMMIT: std::cell::RefCell<Option<Sso9CommitHook>> =
        const { std::cell::RefCell::new(None) };
    /// Test hook: replaces the backup-file deletion (deletion-failure test).
    static SSO9_DELETE_OVERRIDE: std::cell::Cell<Option<Sso9DeleteFn>> =
        const { std::cell::Cell::new(None) };
    /// Test hook: runs right before the backup reservation, so a test can
    /// plant a concurrent opener's file at the backup path (collision test).
    static SSO9_BACKUP_RACE_HOOK: std::cell::Cell<Option<fn(&Path)>> =
        const { std::cell::Cell::new(None) };
}

/// Migration log line: stderr always; captured per-thread under `cfg(test)`.
macro_rules! sso9_log {
    ($($arg:tt)*) => {{
        let line = format!($($arg)*);
        eprintln!("{line}");
        #[cfg(test)]
        SSO9_LOG.with(|l| l.borrow_mut().push(line));
    }};
}
/// Free-space margin required on top of db + wal size before the backup (S2).
const SSO9_BACKUP_MARGIN_BYTES: u64 = 64 * 1024 * 1024;
/// Tables whose row counts are compared source-vs-backup (S3 sanity check).
const SSO9_SANITY_TABLES: [&str; 3] = [
    "llm_wiki_entity_manifests",
    "llm_wiki_entries",
    "llm_wiki_edges",
];

#[cfg(test)]
thread_local! {
    /// Test seam: pin the backup timestamp so a test can pre-create the target
    /// and force a genuine `VACUUM INTO` failure.
    static SSO9_BACKUP_TS_OVERRIDE: std::cell::Cell<Option<u128>> =
        const { std::cell::Cell::new(None) };
}

fn sso9_version_stamped(conn: &Connection, version: i64) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM schema_version WHERE version = ?1",
            [version],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// S2 + S3: disk check, then a crash-consistent `VACUUM INTO` snapshot next to
/// the brain (`<stem>.bak-pre-sso9-<ts>`) and a source-vs-backup row-count
/// sanity check. Any failure is returned as `Err` (S4: the open aborts, N is
/// not stamped, the next open retries).
fn sso9_backup_before_migration(conn: &Connection, db_path: &Path) -> Result<std::path::PathBuf> {
    let dir = match db_path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let wal = std::path::PathBuf::from(format!("{}-wal", db_path.display()));
    let size = std::fs::metadata(db_path).map(|m| m.len()).unwrap_or(0)
        + std::fs::metadata(&wal).map(|m| m.len()).unwrap_or(0);
    let free = fs4::available_space(dir)
        .map_err(|e| anyhow::anyhow!("sso9 backup: cannot read free disk space: {e}"))?;
    if free < size.saturating_add(SSO9_BACKUP_MARGIN_BYTES) {
        anyhow::bail!(
            "sso9 backup: insufficient free disk space ({free} bytes free, \
             need db+wal {size} bytes plus {SSO9_BACKUP_MARGIN_BYTES} margin)"
        );
    }
    let stem = db_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("brain.db");
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis();
    #[cfg(test)]
    let ts = SSO9_BACKUP_TS_OVERRIDE.with(|c| c.get()).unwrap_or(ts);
    let backup = dir.join(format!("{stem}.bak-pre-sso9-{ts}"));
    #[cfg(test)]
    if let Some(h) = SSO9_BACKUP_RACE_HOOK.with(|c| c.get()) {
        h(&backup);
    }
    // Impl-review (owxT, rOZU follow-up): reserve the destination ATOMICALLY
    // — an exclusive create of an EMPTY file, which `VACUUM INTO` accepts as
    // its target. A FAILED reservation means another opener already holds
    // this exact millisecond slot: proceeding would run `VACUUM INTO` into
    // the winner's (still empty) file, and this opener's eventual failure-
    // cleanup could then delete the winner's COMPLETED backup (`reserved`
    // was true only for the winner). Bail instead — the open aborts, and
    // the next open retries with a fresh timestamp.
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&backup)
        .map_err(|e| {
            // Impl-review (slxp): only an existing file is a lost race with
            // another opener; any other I/O failure (permissions, a full or
            // unwritable directory) must say so, or the operator chases a
            // phantom collision while reopens keep failing for the real
            // reason. Both keep the open aborted (S4).
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                anyhow::anyhow!(
                    "sso9 backup: cannot exclusively reserve {} ({e}) — another opener \
                     holds this slot; open aborted, the next open retries with a fresh timestamp",
                    backup.display()
                )
            } else {
                anyhow::anyhow!(
                    "sso9 backup: cannot create the reservation at {} ({e}); open aborted, \
                     slot not stamped",
                    backup.display()
                )
            }
        })?;
    let result = (|| -> Result<String> {
        conn.execute("VACUUM INTO ?1", [backup.to_string_lossy().as_ref()])?;
        let bk = Connection::open_with_flags(&backup, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let mut summary = Vec::new();
        for table in SSO9_SANITY_TABLES {
            let sql = format!("SELECT COUNT(*) FROM {table}");
            let src: i64 = conn.query_row(&sql, [], |r| r.get(0))?;
            let dst: i64 = bk.query_row(&sql, [], |r| r.get(0))?;
            if src != dst {
                anyhow::bail!("sso9 backup sanity: {table} has {src} rows, backup has {dst}");
            }
            summary.push(format!("{table}={src}/{dst}"));
        }
        Ok(summary.join(" "))
    })();
    match result {
        Ok(summary) => {
            sso9_log!(
                "[ct::migrate] sso9 backup ok: path={} sanity(source/backup) {summary}",
                backup.display()
            );
            Ok(backup)
        }
        Err(e) => {
            // We own the reservation (an exclusive create succeeded above) —
            // always remove our own failed attempt's file.
            let _ = std::fs::remove_file(&backup);
            Err(e.context("sso9 backup failed; open aborted, slot not stamped"))
        }
    }
}

fn sso9_read_manifests(conn: &Connection) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare("SELECT entity_id, manifest_json FROM llm_wiki_entity_manifests")?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// S5 dry pass: classify every stored manifest without writing; log the census.
fn sso9_census(conn: &Connection) -> Result<()> {
    use crate::db::entity_gate::{plan_sso9_manifest_refresh, RefreshPlan};
    let (mut total, mut eligible, mut untouched, mut malformed) = (0, 0, 0, 0);
    for (_id, json) in sso9_read_manifests(conn)? {
        total += 1;
        match plan_sso9_manifest_refresh(&json)? {
            RefreshPlan::Refresh { .. } => eligible += 1,
            RefreshPlan::Untouched(_) => untouched += 1,
            RefreshPlan::Malformed(_) => malformed += 1,
        }
    }
    sso9_log!(
        "[ct::migrate] sso9 manifest census (dry pass, slot {SSO9_REFRESH_VERSION}): \
         total={total} eligible={eligible} foreign_or_untouched={untouched} malformed={malformed}"
    );
    Ok(())
}

/// S6 + S7: the step body. `BEGIN IMMEDIATE`; re-check the slot unstamped
/// under the lock; MERGE-refresh every eligible manifest with a compare-and-
/// swap UPDATE; stamp; `COMMIT` (any error → `ROLLBACK`). Idempotent: a
/// refreshed manifest no longer passes the old-17 eligibility, so a re-run
/// changes nothing. Returns `(entity_id, new_json)` per refreshed manifest.
fn run_sso9_refresh_txn(conn: &Connection) -> Result<Vec<(String, String)>> {
    use crate::db::entity_gate::{plan_sso9_manifest_refresh, RefreshPlan};
    conn.execute_batch("BEGIN IMMEDIATE;")?;
    let applied = (|| -> Result<Vec<(String, String)>> {
        if sso9_version_stamped(conn, SSO9_REFRESH_VERSION)? {
            return Ok(Vec::new());
        }
        let mut refreshed = Vec::new();
        for (entity_id, old_json) in sso9_read_manifests(conn)? {
            if let RefreshPlan::Refresh { new_json } = plan_sso9_manifest_refresh(&old_json)? {
                // Impl-review (owxz): fold the ensure planner's edits into the
                // SAME transaction — the manifest and its required ensure
                // edits (document/process + fallback) must commit together, or
                // the stamp would skip the ensure forever on the next open.
                let final_json =
                    crate::db::entity_gate::fold_sso9_ensure(conn, &entity_id, &new_json)?;
                let changed = conn.execute(
                    "UPDATE llm_wiki_entity_manifests SET manifest_json = ?1
                      WHERE entity_id = ?2 AND manifest_json = ?3",
                    rusqlite::params![final_json, entity_id, old_json],
                )?;
                if changed != 1 {
                    anyhow::bail!(
                        "sso9 refresh: compare-and-swap changed {changed} rows for {entity_id}"
                    );
                }
                refreshed.push((entity_id, final_json));
            }
        }
        conn.execute(
            "INSERT OR IGNORE INTO schema_version (version) VALUES (?1)",
            [SSO9_REFRESH_VERSION],
        )?;
        Ok(refreshed)
    })();
    match applied {
        Ok(refreshed) => match conn.execute_batch("COMMIT;") {
            // Impl-review (owxc): a failed COMMIT (e.g. SQLITE_BUSY on a
            // rollback-journal reader) must not leave the transaction open —
            // the next BEGIN IMMEDIATE inside the same handle would fail.
            Err(commit_err) => {
                let _ = conn.execute_batch("ROLLBACK;");
                Err(commit_err.into())
            }
            Ok(()) => Ok(refreshed),
        },
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK;");
            Err(e)
        }
    }
}

/// One edge row as the re-type step sees it.
#[derive(Debug, Clone)]
struct Sso9Edge {
    id: String,
    entity_id: String,
    source_id: String,
    target_id: String,
    edge_type: String,
}

/// Every edge disposition, computed BEFORE any write (plan Task 5 §5b order
/// rule: the Q4 resolver reads the OLD `entity_type`/`okf_type`).
#[derive(Debug, Default)]
struct Sso9EdgePlan {
    /// Step 2: `handoffFor` → `about` (source in the snapshot, target type
    /// `software_application` only — a `statement`-typed target is not a
    /// declared `about` shape, U-review finding 1).
    remap: Vec<Sso9Edge>,
    /// Step 3: `handoffFor` with a `service` target.
    service_drop: Vec<Sso9Edge>,
    /// Step 4: parent-family edges touching a re-typed row with an
    /// undeclared re-typed shape.
    parent_drop: Vec<Sso9Edge>,
    /// Owner ruling (Kurt, 2026-11-25, the t93c open question): the subset
    /// of parent-family edges whose verdict was `UnresolvedEndpoint` — KEPT,
    /// never dropped (the live gate climbs by owning entity and never reads
    /// okf_type NULLness, so these edges are admitted at runtime; the
    /// success path deletes the backup, making a wrong drop irreversible).
    /// The count is logged, not acted on.
    kept_unresolved_endpoint: usize,
    /// Impl-review (owxi): parent-family edges whose POST-re-type shape is
    /// DECLARED in the entity's refreshed manifest — kept, never dropped.
    /// The NULL-endpoint keeps land here too (ruling above), counted
    /// separately by `kept_unresolved_endpoint`.
    kept_declared_parent: Vec<Sso9Edge>,
    /// Keep set: `about` → `organization` from a re-typed source.
    kept_about_org: usize,
    /// U-review finding 7: `about` from a re-typed source to a target that
    /// becomes a DECLARED statement shape (software_application, area) —
    /// kept, and counted separately from undispositioned.
    kept_declared_about: usize,
    /// Step 5 / Q5: outside the spec's keep/drop lists — untouched, counted.
    undispositioned: usize,
    /// Census inputs.
    handoff_for_by_target: std::collections::BTreeMap<String, usize>,
    touching_by_type: std::collections::BTreeMap<String, usize>,
}

/// Step-2/3/4 dispositioned edges, captured by a pre-write SELECT, for the
/// post-commit verification (c).
#[derive(Debug, Default, Clone)]
struct Sso9Dispositions {
    /// Step 2: the (edge id, entity_id, source_id, target_id) tuple.
    remapped: Vec<(String, String, String, String)>,
    /// Steps 3 + 4: dropped edge ids.
    dropped: Vec<String>,
}

fn sso9_in_slugs_sql() -> String {
    let list: Vec<String> = SSO9_RETYPE_MAP.iter().map(|(f, _)| format!("'{f}'")).collect();
    list.join(",")
}

/// Step 1 of the writes: the re-typed id set — entry ids AND curated-entity
/// ids carrying a dropped slug (tombstones included in both).
fn sso9_snapshot_ids(conn: &Connection) -> Result<std::collections::HashSet<String>> {
    let slugs = sso9_in_slugs_sql();
    let sql = format!(
        "SELECT id FROM llm_wiki_entries WHERE okf_type IN ({slugs})
         UNION SELECT id FROM curated_entities WHERE entity_type IN ({slugs})"
    );
    let mut stmt = conn.prepare(&sql)?;
    let ids = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<std::collections::HashSet<_>>>()?;
    Ok(ids)
}

/// Q4 resolver: `llm_wiki_entries.okf_type` when the id names an entry, else
/// `curated_entities.entity_type`, else unresolved (`None`). Returns the
/// stored value VERBATIM (rOZe: the parent-shape check must map the re-type
/// over the EXACT stored spelling to predict what the SQL's exact-slug
/// updates write; slx2: trimming here would mispredict a value like
/// `'handoff '` that the SQL does not match. Callers normalize as needed —
/// the classification call sites trim+lowercase for slug comparison).
fn sso9_resolve_type(
    conn: &Connection,
    cache: &mut std::collections::HashMap<String, Option<String>>,
    id: &str,
) -> Result<Option<String>> {
    if let Some(hit) = cache.get(id) {
        return Ok(hit.clone());
    }
    let entry: Option<Option<String>> = conn
        .query_row(
            "SELECT okf_type FROM llm_wiki_entries WHERE id = ?1",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    let resolved = match entry {
        Some(t) => t,
        None => conn
            .query_row(
                "SELECT entity_type FROM curated_entities WHERE id = ?1",
                [id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten(),
    }
    .map(|t| t.to_string());
    cache.insert(id.to_string(), resolved.clone());
    Ok(resolved)
}

/// Impl-review (owxi): the entity's applicable manifest's declared edge
/// triples, keyed by entity id (`None` = no applicable manifest — every shape
/// counts undeclared). Step 1 has committed before the re-type runs, so a
/// stored manifest IS the refreshed one. Applicability follows the gate's own
/// two-lookup cascade (rOZa: `resolve_strict_edge_vocabulary_with`) — the
/// entity's own strict manifest wins; WITHOUT one, a curated entity inherits
/// the canonical `tier_fact` partition manifest (the typical production
/// case). Non-strict rows fall through to the partition leg (the gate's
/// leg semantics); an unreadable APPLICABLE (partition) manifest aborts
/// the re-type (slyA, refined t93k: a malformed OWNER row falls through
/// exactly like the gate, because the mode is unreadable along with the
/// shapes).
type Sso9DeclaredEdgeSet = crate::db::entity_gate::ManifestDeclaredEdges;
type Sso9DeclaredEdgeCache = std::collections::HashMap<String, Option<Sso9DeclaredEdgeSet>>;

fn sso9_declared_edges(
    conn: &Connection,
    cache: &mut Sso9DeclaredEdgeCache,
    entity_id: &str,
) -> Result<Option<Sso9DeclaredEdgeSet>> {
    if let Some(hit) = cache.get(entity_id) {
        return Ok(hit.clone());
    }
    let mut set = None;
    for (leg, lookup) in [entity_id, "tier_fact"].into_iter().enumerate() {
        match crate::wiki_graph::wiki_get_ontology(conn, lookup) {
            Ok(o) if o.mode == "strict" => {
                set = o.manifest.as_ref().map(crate::db::entity_gate::manifest_declared_edges);
                break;
            }
            // Present but not strict (or no row): fall through to the next
            // lookup id — the gate's leg semantics.
            Ok(_) => {}
            // Impl-review (slyA, refined by the fresh round's t93k finding):
            // an unreadable manifest must not classify shapes as undeclared
            // — the re-type DELETES on "undeclared" and the backup is
            // removed after its checks pass. But the gate's own Err leg
            // FALLS THROUGH, because `wiki_get_ontology` parses the JSON
            // before it reads the mode: a malformed owner row is
            // indistinguishable from a non-strict one, and for the gate the
            // PARTITION manifest then governs. Mirror that exactly — an
            // owner-row read error falls through to `tier_fact`; only the
            // APPLICABLE manifest's leg (the partition fallback) aborts the
            // re-type, keeping the backup for retry (operator repairs).
            Err(e) if leg == 0 => {
                sso9_log!(
                    "[ct::migrate WARN] sso9 retype: unreadable manifest for {lookup} \
                     ({e}); the partition fallback governs, mirroring the gate"
                );
            }
            Err(e) => anyhow::bail!(
                "sso9 retype: cannot read the manifest for {lookup} — an unreadable \
                 applicable manifest must not classify shapes as undeclared: {e}"
            ),
        }
    }
    cache.insert(entity_id.to_string(), set.clone());
    Ok(set)
}

/// Verdict of the declared-shape consult. `UnresolvedEndpoint` is KEPT per
/// the owner's ruling (Kurt, 2026-11-25, spec erratum E6): NULL type
/// information does not establish that a stored edge's shape is undeclared,
/// so the migration must not delete it — the count is reported in the
/// plan/log instead of a drop.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Sso9ShapeVerdict {
    Declared,
    Undeclared,
    UnresolvedEndpoint,
}

/// Consult the edge's POST-re-type shape (types resolved VERBATIM from the
/// OLD rows, then mapped through `SSO9_RETYPE_MAP`) is declared in the
/// owning entity's applicable manifest — or when the entity is UNGATED
/// (no applicable strict manifest on either leg, or a strict manifest whose
/// edge vocabulary is empty): the gate admits every shape there
/// (`ontology_leg`'s "ungated means most permissive" contract), so the
/// re-type must not delete edges the live gate admits.
fn sso9_parent_shape_declared(
    conn: &Connection,
    declared_cache: &mut Sso9DeclaredEdgeCache,
    type_cache: &mut std::collections::HashMap<String, Option<String>>,
    e: &Sso9Edge,
) -> Result<Sso9ShapeVerdict> {
    // The applicable-partition manifest read (inside sso9_declared_edges)
    // runs FIRST: a corrupt APPLICABLE partition aborts fail-closed (E7)
    // before any keep verdict can fire.
    let declared = sso9_declared_edges(conn, declared_cache, &e.entity_id)?;
    let post_retype = |t: Option<String>| -> Option<String> {
        t.map(|x| {
            SSO9_RETYPE_MAP
                .iter()
                .find(|(from, _)| *from == x)
                .map(|(_, to)| to.to_string())
                .unwrap_or(x)
        })
    };
    let src = post_retype(sso9_resolve_type(conn, type_cache, &e.source_id)?);
    let tgt = post_retype(sso9_resolve_type(conn, type_cache, &e.target_id)?);
    // An unresolvable endpoint (row absent or okf_type/entity_type NULL —
    // 1,436/2,420 live rows carry NULL okf_type): NULL type information does
    // NOT establish that the stored shape is undeclared — the live gate
    // climbs endpoints by owning entity and never reads okf_type NULLness.
    // RULING (Kurt, 2026-11-25; spec erratum E6): KEEP and count on EVERY
    // path — gated, ungated, and empty-vocabulary alike — never drop (the
    // migration's success path deletes the backup, so a wrong drop is
    // irreversible). This preserves STORED edges; it does not admit new
    // ones (a genuinely undeclared shape or a missing endpoint row still
    // drops/rejects at runtime).
    if src.is_none() || tgt.is_none() {
        return Ok(Sso9ShapeVerdict::UnresolvedEndpoint);
    }
    let Some(declared) = declared else {
        // Impl-review (T1.5 rung 2 MAJOR 1): ungated — keep. RATIFIED (spec
        // erratum E5, owner-authorized 2026-11-25): the justification is NOT
        // the Rust edge gate (it checks edge-type NAMES only, never endpoint
        // shapes); the criterion is spec change-6's shape-level declaration,
        // with ungated entities — where no shape can be declared — exempt
        // from the drop by ruling.
        return Ok(Sso9ShapeVerdict::Declared);
    };
    if declared.is_empty() {
        // Strict manifest with an EMPTY edge vocabulary: `ontology_leg`
        // returns Strict(None) and disarms the gate ("a strict manifest that
        // declares no edge types" is treated as most permissive) — mirror it.
        return Ok(Sso9ShapeVerdict::Declared);
    }
    if declared.contains(
        &crate::db::entity_gate::NodeVocabulary::key(&e.edge_type),
        &crate::db::entity_gate::NodeVocabulary::key(&src.unwrap()),
        &crate::db::entity_gate::NodeVocabulary::key(&tgt.unwrap()),
    ) {
        Ok(Sso9ShapeVerdict::Declared)
    } else {
        Ok(Sso9ShapeVerdict::Undeclared)
    }
}

/// Classify every relevant edge (read-only). The captured sets ARE this
/// pre-write SELECT's result (plan S2).
fn sso9_plan_edges(
    conn: &Connection,
    snapshot: &std::collections::HashSet<String>,
) -> Result<Sso9EdgePlan> {
    let mut plan = Sso9EdgePlan::default();
    let mut cache = std::collections::HashMap::new();
    let mut declared_cache: Sso9DeclaredEdgeCache = std::collections::HashMap::new();
    let mut stmt = conn
        .prepare("SELECT id, entity_id, source_id, target_id, edge_type FROM llm_wiki_edges ORDER BY id")?;
    let edges = stmt
        .query_map([], |r| {
            Ok(Sso9Edge {
                id: r.get(0)?,
                entity_id: r.get(1)?,
                source_id: r.get(2)?,
                target_id: r.get(3)?,
                edge_type: r.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for e in edges {
        let src_in = snapshot.contains(&e.source_id);
        let touches = src_in || snapshot.contains(&e.target_id);
        if touches {
            *plan.touching_by_type.entry(e.edge_type.clone()).or_default() += 1;
        }
        if e.edge_type == "handoffFor" {
            let tt = sso9_resolve_type(conn, &mut cache, &e.target_id)?
                .map(|t| t.trim().to_lowercase());
            *plan
                .handoff_for_by_target
                .entry(tt.clone().unwrap_or_else(|| "unresolved".into()))
                .or_default() += 1;
            match tt.as_deref() {
                Some("service") => plan.service_drop.push(e),
                // U-review finding 1: a `statement`-typed target is NOT a
                // declared `about` shape (about: statement→area/
                // software_application/organization) — such edges fall to
                // `undispositioned` rather than minting an undeclared shape.
                Some("software_application") if src_in => plan.remap.push(e),
                _ => plan.undispositioned += 1,
            }
        } else if touches {
            match e.edge_type.as_str() {
                // Impl-review (owxi): the drop rule is "undeclared post-re-type
                // shape" — consult the entity's refreshed manifest before
                // dropping; a custom manifest may still declare the re-typed
                // shape (e.g. author(decision, person)).
                "author" | "publisher" | "supersedes" => {
                    match sso9_parent_shape_declared(conn, &mut declared_cache, &mut cache, &e)? {
                        Sso9ShapeVerdict::Declared => plan.kept_declared_parent.push(e),
                        Sso9ShapeVerdict::Undeclared => plan.parent_drop.push(e),
                        Sso9ShapeVerdict::UnresolvedEndpoint => {
                            plan.kept_declared_parent.push(e);
                            plan.kept_unresolved_endpoint += 1;
                        }
                    }
                }
                "about" if src_in => {
                    let tt = sso9_resolve_type(conn, &mut cache, &e.target_id)?
                        .map(|t| t.trim().to_lowercase());
                    match tt.as_deref() {
                        Some("person") | Some("place") | Some("event") => {
                            // Impl-review (T1.5 rung 3 BLOCKER 1): same rule
                            // as the parent-family arms — consult the
                            // applicable manifest (ungated/declared keeps)
                            // instead of a hard-coded drop. The ratified
                            // default brain still drops here (its refreshed
                            // manifest declares no about(decision|lesson,
                            // person|place|event) shape); ungated and
                            // custom-declared brains keep edges the live
                            // gate admits, and the success path deletes the
                            // backup, so a wrong drop is irreversible.
                            match sso9_parent_shape_declared(
                                conn,
                                &mut declared_cache,
                                &mut cache,
                                &e,
                            )? {
                                Sso9ShapeVerdict::Declared => {
                                    plan.kept_declared_parent.push(e);
                                }
                                Sso9ShapeVerdict::Undeclared => plan.parent_drop.push(e),
                                Sso9ShapeVerdict::UnresolvedEndpoint => {
                                    plan.kept_declared_parent.push(e);
                                    plan.kept_unresolved_endpoint += 1;
                                }
                            }
                        }
                        Some("organization") => plan.kept_about_org += 1,
                        // U-review finding 7: after the re-type these become
                        // DECLARED statement→sa/area shapes — not unhandled.
                        Some("software_application") | Some("area") => {
                            plan.kept_declared_about += 1
                        }
                        _ => plan.undispositioned += 1,
                    }
                }
                _ => plan.undispositioned += 1,
            }
        }
    }
    Ok(plan)
}

fn sso9_fmt_map(m: &std::collections::BTreeMap<String, usize>) -> String {
    let parts: Vec<String> = m.iter().map(|(k, v)| format!("{k}:{v}")).collect();
    format!("[{}]", parts.join(","))
}

/// S5 dry pass for step 2: every census count, one log line, no writes.
fn sso9_retype_census(conn: &Connection) -> Result<()> {
    let slugs = sso9_in_slugs_sql();
    let mut parts: Vec<String> = Vec::new();
    for (table, col, label) in [
        ("llm_wiki_entries", "okf_type", "entries"),
        ("llm_wiki_tasks", "okf_type", "tasks"),
    ] {
        for (slug, _) in SSO9_RETYPE_MAP {
            for (state, pred) in [("live", "deleted_at IS NULL"), ("tombstoned", "deleted_at IS NOT NULL")] {
                let n: i64 = conn.query_row(
                    &format!("SELECT COUNT(*) FROM {table} WHERE {col} = ?1 AND {pred}"),
                    [slug],
                    |r| r.get(0),
                )?;
                parts.push(format!("{label}_{slug}_{state}={n}"));
            }
        }
        let n: i64 = conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM {table}
                  WHERE lower(trim({col})) IN ({slugs}) AND {col} NOT IN ({slugs})"
            ),
            [],
            |r| r.get(0),
        )?;
        parts.push(format!("case_variant_{label}={n}"));
    }
    let n: i64 = conn.query_row(
        &format!(
            "SELECT COUNT(*) FROM curated_entities
              WHERE lower(trim(entity_type)) IN ({slugs}) AND entity_type NOT IN ({slugs})"
        ),
        [],
        |r| r.get(0),
    )?;
    parts.push(format!("case_variant_entities={n}"));
    for (slug, _) in SSO9_RETYPE_MAP {
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM curated_entities WHERE entity_type = ?1",
            [slug],
            |r| r.get(0),
        )?;
        parts.push(format!("entities_{slug}={n}"));
    }
    let snapshot = sso9_snapshot_ids(conn)?;
    let plan = sso9_plan_edges(conn, &snapshot)?;
    parts.push(format!("handoffFor_by_target={}", sso9_fmt_map(&plan.handoff_for_by_target)));
    parts.push(format!("edges_touching_by_type={}", sso9_fmt_map(&plan.touching_by_type)));
    parts.push(format!(
        "planned_remap={} planned_service_drop={} planned_parent_family_drop={} \
         kept_about_organization={} kept_declared_about={} kept_declared_parent={} \
         kept_unresolved_endpoint={} undispositioned={}",
        plan.remap.len(),
        plan.service_drop.len(),
        plan.parent_drop.len(),
        plan.kept_about_org,
        plan.kept_declared_about,
        // CHv: exclusive of the NULL-endpoint keeps, so this field matches
        // the write-log line exactly (the inclusive total is the sum).
        plan.kept_declared_parent.len() - plan.kept_unresolved_endpoint,
        plan.kept_unresolved_endpoint,
        plan.undispositioned
    ));
    let pending_types: i64 = conn.query_row(
        "SELECT COUNT(*) FROM curated_proposal_items
          WHERE status = 'pending'
            AND (payload LIKE '%\"handoff\"%' OR payload LIKE '%\"session_recap\"%')",
        [],
        |r| r.get(0),
    )?;
    let pending_edges: i64 = conn.query_row(
        "SELECT COUNT(*) FROM curated_proposal_items
          WHERE status = 'pending' AND payload LIKE '%handoffFor%'",
        [],
        |r| r.get(0),
    )?;
    parts.push(format!(
        "pending_proposal_items_with_dropped_types={pending_types} \
         pending_proposal_items_with_handoffFor={pending_edges}"
    ));
    sso9_log!(
        "[ct::migrate] sso9 retype census (dry pass, slot {SSO9_RETYPE_VERSION}): {}",
        parts.join(" ")
    );
    Ok(())
}

/// Step 2 body (plan Task 5 §5b). One `BEGIN IMMEDIATE`; stamps N+1 only
/// after N is stamped (re-checked under the lock). Returns `None` when it
/// did not run (already stamped, or N unstamped), else the captured edge
/// dispositions for the post-commit verification.
fn run_sso9_retype_txn(conn: &Connection, backup: &Path) -> Result<Option<Sso9Dispositions>> {
    conn.execute_batch("BEGIN IMMEDIATE;")?;
    let applied = (|| -> Result<Option<Sso9Dispositions>> {
        if sso9_version_stamped(conn, SSO9_RETYPE_VERSION)? {
            return Ok(None);
        }
        if !sso9_version_stamped(conn, SSO9_REFRESH_VERSION)? {
            sso9_log!(
                "[ct::migrate] sso9 retype (slot {SSO9_RETYPE_VERSION}) not run: \
                 refresh slot {SSO9_REFRESH_VERSION} is not stamped"
            );
            return Ok(None);
        }
        // 1. snapshot + every disposition, BEFORE any rewrite.
        let snapshot = sso9_snapshot_ids(conn)?;
        let plan = sso9_plan_edges(conn, &snapshot)?;
        // Impl-review (owxv, rOZt/rOZh follow-up): the backup is a point-in-
        // time snapshot; the plan is captured under this write lock, but a
        // write committed between the backup and this lock is invisible to
        // the backup while the plan sees it. The previous COUNT-equality
        // guard had two holes: it missed EQUAL-COUNT drift (a late writer
        // that removed one unrelated edge and added a service-target
        // `handoffFor` passed the check, and the re-type then deleted a row
        // the backup never held), and it armed only when a drop was planned
        // (a remap-only pass could still destroy a late row in the dedupe
        // DELETE). The precise invariant: EVERY row this transaction will
        // destroy (remap losers, service drops, parent drops) must still
        // exist in the backup with identical content. Any missing or changed
        // planned row aborts the retype unapplied — backup preserved, and
        // the next open takes a fresh backup and re-captures the plan.
        // (Idempotent replays with nothing left to destroy skip the check —
        // the direct-call tests rely on that.)
        let destructive: Vec<&Sso9Edge> = plan
            .remap
            .iter()
            .chain(plan.service_drop.iter())
            .chain(plan.parent_drop.iter())
            .collect();
        if !destructive.is_empty() {
            let bk = Connection::open_with_flags(
                backup,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .map_err(|e| {
                anyhow::anyhow!(
                    "sso9 retype: cannot open the pre-migration backup {}: {e}",
                    backup.display()
                )
            })?;
            for e in &destructive {
                let in_backup: Option<(String, String, String, String)> = bk
                    .query_row(
                        "SELECT entity_id, source_id, target_id, edge_type
                          FROM llm_wiki_edges WHERE id = ?1",
                        [&e.id],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                    )
                    .optional()?;
                let expected = (
                    e.entity_id.clone(),
                    e.source_id.clone(),
                    e.target_id.clone(),
                    e.edge_type.clone(),
                );
                if in_backup.as_ref() != Some(&expected) {
                    anyhow::bail!(
                        "sso9 retype: planned edge {} ({}/{}/{}) is missing from or changed \
                         since the pre-migration backup — a late write slipped between the \
                         backup and the re-type; aborting before any delete, backup preserved, \
                         retype retried on the next open",
                        e.id,
                        e.entity_id,
                        e.source_id,
                        e.target_id
                    );
                }
            }
        }
        let disp = Sso9Dispositions {
            remapped: plan
                .remap
                .iter()
                .map(|e| (e.id.clone(), e.entity_id.clone(), e.source_id.clone(), e.target_id.clone()))
                .collect(),
            dropped: plan
                .service_drop
                .iter()
                .chain(plan.parent_drop.iter())
                .map(|e| e.id.clone())
                .collect(),
        };
        // 2. handoffFor → about (UPDATE OR IGNORE), then dedupe the losers
        //    with the same per-id predicate.
        let (mut remapped, mut deduped) = (0usize, 0usize);
        for e in &plan.remap {
            remapped += conn.execute(
                "UPDATE OR IGNORE llm_wiki_edges SET edge_type = 'about'
                  WHERE id = ?1 AND edge_type = 'handoffFor'",
                [&e.id],
            )?;
            deduped += conn.execute(
                "DELETE FROM llm_wiki_edges WHERE id = ?1 AND edge_type = 'handoffFor'",
                [&e.id],
            )?;
        }
        sso9_log!(
            "[ct::migrate] sso9 retype: handoffFor->about remapped={remapped} \
             deduped={deduped} (pre-existing about rows kept)"
        );
        // 3. service targets: drop.
        let mut service_dropped = 0usize;
        for e in &plan.service_drop {
            service_dropped +=
                conn.execute("DELETE FROM llm_wiki_edges WHERE id = ?1", [&e.id])?;
        }
        sso9_log!("[ct::migrate] sso9 retype: handoffFor->service dropped={service_dropped}");
        // 4. parent-family edges with an undeclared re-typed shape: drop.
        let mut by_type: std::collections::BTreeMap<String, usize> = Default::default();
        for e in &plan.parent_drop {
            let n = conn.execute("DELETE FROM llm_wiki_edges WHERE id = ?1", [&e.id])?;
            *by_type.entry(e.edge_type.clone()).or_default() += n;
        }
        sso9_log!(
            "[ct::migrate] sso9 retype: parent-family edges dropped by type={} \
             (about->organization kept={}, declared about->sa/area kept={}, \
             parent edges kept as declared post-retype={}, \
             unresolved-endpoint parent edges kept={})",
            sso9_fmt_map(&by_type),
            plan.kept_about_org,
            plan.kept_declared_about,
            plan.kept_declared_parent.len() - plan.kept_unresolved_endpoint,
            plan.kept_unresolved_endpoint
        );
        // 5. everything else: untouched, counted.
        sso9_log!(
            "[ct::migrate] sso9 retype: undispositioned={} (left untouched)",
            plan.undispositioned
        );
        // 6-8. the type rewrites, tombstones included, AFTER the edge steps.
        for (from, to) in SSO9_RETYPE_MAP {
            let e = conn.execute(
                "UPDATE llm_wiki_entries SET okf_type = ?2 WHERE okf_type = ?1",
                [from, to],
            )?;
            let t = conn.execute(
                "UPDATE llm_wiki_tasks SET okf_type = ?2 WHERE okf_type = ?1",
                [from, to],
            )?;
            let c = conn.execute(
                "UPDATE curated_entities SET entity_type = ?2 WHERE entity_type = ?1",
                [from, to],
            )?;
            sso9_log!(
                "[ct::migrate] sso9 retype: {from}->{to} entries={e} tasks={t} curated_entities={c}"
            );
        }
        // 9. stamp.
        conn.execute(
            "INSERT OR IGNORE INTO schema_version (version) VALUES (?1)",
            [SSO9_RETYPE_VERSION],
        )?;
        Ok(Some(disp))
    })();
    match applied {
        Ok(d) => match conn.execute_batch("COMMIT;") {
            // Impl-review (owxc): a failed COMMIT must not leave the
            // transaction open — see run_sso9_refresh_txn.
            Err(commit_err) => {
                let _ = conn.execute_batch("ROLLBACK;");
                Err(commit_err.into())
            }
            Ok(()) => Ok(d),
        },
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK;");
            Err(e)
        }
    }
}

/// R1 shape check: EVERY refreshed row's stored manifest equals the
/// planner's captured `new_json` (parsed-Value compare, F6; T1.5 rung 1
/// nit 1 — all rows, not just the first). `Ok(None)` = passes (also
/// vacuously when nothing was refreshed). `Ok(Some(why))` = failed.
fn sso9_shape_check(conn: &Connection, refreshed: &[(String, String)]) -> Result<Option<String>> {
    // Impl-review (T1.5 rung 1 nit 1): verify EVERY refreshed row, not just
    // the first — the check's whole purpose is post-commit drift detection,
    // and a tamper or write in a second refreshed manifest previously went
    // unseen. One SELECT per refreshed row (the refresh touches a handful).
    let mut failures = Vec::new();
    for (entity_id, new_json) in refreshed {
        let stored: Option<String> = conn
            .query_row(
                "SELECT manifest_json FROM llm_wiki_entity_manifests WHERE entity_id = ?1",
                [entity_id],
                |r| r.get(0),
            )
            .optional()?;
        let ok = match stored {
            Some(stored) => {
                let a = serde_json::from_str::<serde_json::Value>(&stored);
                let b = serde_json::from_str::<serde_json::Value>(new_json);
                matches!((a, b), (Ok(a), Ok(b)) if a == b)
            }
            None => false,
        };
        if !ok {
            failures.push(format!(
                "shape check: stored manifest for {entity_id} differs from the planned refresh"
            ));
        }
    }
    Ok(if failures.is_empty() {
        None
    } else {
        Some(failures.join("; "))
    })
}

/// Post-commit verification (plan Task 5 step 10): returns the failed checks.
/// (a) stamps, (b) no stale slugs, (c) per-class dispositioned-edge outcomes.
fn sso9_verify(
    conn: &Connection,
    shape_failure: Option<String>,
    disp: Option<&Sso9Dispositions>,
) -> Vec<String> {
    let mut failures = Vec::new();
    if let Some(f) = shape_failure {
        failures.push(f);
    }
    let q = |sql: &str, p: &[&dyn rusqlite::ToSql]| -> rusqlite::Result<bool> {
        conn.query_row(sql, p, |_| Ok(())).optional().map(|o| o.is_some())
    };
    // (a) STAMPS
    for v in [SSO9_REFRESH_VERSION, SSO9_RETYPE_VERSION] {
        match sso9_version_stamped(conn, v) {
            Ok(true) => {}
            Ok(false) => failures.push(format!("(a) stamps: slot {v} is not stamped")),
            Err(e) => failures.push(format!("(a) stamps: query failed: {e}")),
        }
    }
    // (b) NO STALE SLUGS (exact match; case variants deliberately left alone)
    let slugs = sso9_in_slugs_sql();
    for (table, col) in [
        ("llm_wiki_entries", "okf_type"),
        ("llm_wiki_tasks", "okf_type"),
        ("curated_entities", "entity_type"),
    ] {
        match conn.query_row(
            &format!("SELECT COUNT(*) FROM {table} WHERE {col} IN ({slugs})"),
            [],
            |r| r.get::<_, i64>(0),
        ) {
            Ok(0) => {}
            Ok(n) => failures.push(format!("(b) stale slugs: {n} row(s) in {table}")),
            Err(e) => failures.push(format!("(b) stale slugs: query on {table} failed: {e}")),
        }
    }
    // (c) DISPOSITIONED-EDGE OUTCOMES
    if let Some(d) = disp {
        for (id, entity, src, tgt) in &d.remapped {
            let row: rusqlite::Result<Option<(String, String, String, String)>> = conn
                .query_row(
                    "SELECT edge_type, entity_id, source_id, target_id
                      FROM llm_wiki_edges WHERE id = ?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional();
            let ok = match row {
                // Impl-review (owxn): a surviving row must ALSO match the
                // captured (entity, source, target) — same id + type with a
                // moved endpoint would otherwise pass verification while the
                // original relationship is missing.
                Ok(Some((t, e2, s2, t2))) => {
                    t == "about" && &e2 == entity && &s2 == src && &t2 == tgt
                }
                Ok(None) => q(
                    "SELECT 1 FROM llm_wiki_edges
                      WHERE entity_id = ?1 AND source_id = ?2 AND target_id = ?3
                        AND edge_type = 'about'",
                    &[entity, src, tgt],
                )
                .unwrap_or(false),
                Err(_) => false,
            };
            if !ok {
                failures.push(format!(
                    "(c) step-2 re-mapped edge {id} is neither `about` nor gone with an `about` \
                     on ({entity}, {src}, {tgt})"
                ));
            }
        }
        for id in &d.dropped {
            if q("SELECT 1 FROM llm_wiki_edges WHERE id = ?1", &[id]).unwrap_or(true) {
                failures.push(format!("(c) dropped edge {id} is still present"));
            }
        }
    }
    failures
}

fn sso9_delete_backup(path: &Path) -> std::io::Result<()> {
    #[cfg(test)]
    if let Some(f) = SSO9_DELETE_OVERRIDE.with(|c| c.get()) {
        return f(path);
    }
    std::fs::remove_file(path)
}

/// Stale sibling backups the success path keeps after pruning (each is a
/// crash-recovery copy a failed open left behind for its retry).
const SSO9_BACKUP_KEEP: usize = 3;

/// Impl-review (T1.5 rung 2 MINOR 2): a persistently failing open used to
/// accumulate one full-DB backup per launch — the success path only ever
/// removed its own file. After a verified success, prune stale
/// `<stem>.bak-pre-sso9-<ts>` siblings, keeping the newest
/// `SSO9_BACKUP_KEEP` (concurrent openers' fresh backups sort newest and
/// stay). Only the success path prunes: a failing open keeps everything for
/// its retry.
fn sso9_prune_stale_backups(own: &Path) {
    let Some(dir) = own.parent() else { return };
    let Some(name) = own.file_name().and_then(|s| s.to_str().map(str::to_string)) else { return };
    let Some(prefix) = name.rsplit_once('-').map(|(p, _)| p.to_string()) else { return };
    let mut stale: Vec<(u128, std::path::PathBuf)> = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let Some(n) = entry.file_name().to_str().map(str::to_string) else { continue };
        if n == name {
            continue;
        }
        if let Some(ts) = n
            .strip_prefix(prefix.as_str())
            .and_then(|r| r.strip_prefix('-'))
            .and_then(|ts| ts.parse::<u128>().ok())
        {
            stale.push((ts, entry.path()));
        }
    }
    stale.sort_by_key(|(ts, _)| *ts);
    let excess = stale.len().saturating_sub(SSO9_BACKUP_KEEP);
    for (_, path) in stale.into_iter().take(excess) {
        match std::fs::remove_file(&path) {
            Ok(()) => sso9_log!(
                "[ct::migrate] sso9 stale backup pruned: path={}",
                path.display()
            ),
            Err(e) => sso9_log!(
                "[ct::migrate WARN] sso9 stale backup not pruned: path={} error={e}",
                path.display()
            ),
        }
    }
}

#[cfg(test)]
fn sso9_run_hook(
    slot: &'static std::thread::LocalKey<std::cell::RefCell<Option<Sso9CommitHook>>>,
    conn: &Connection,
) {
    slot.with(|h| {
        if let Some(f) = h.borrow().as_ref() {
            f(conn);
        }
    });
}

/// sso9 migration steps 1 + 2 (plan Tasks 4 + 5): one backup per call when
/// either slot is unstamped; step 1 (manifest MERGE refresh) then step 2
/// (okf_type + edge re-type); post-commit verification; backup deletion when
/// no unstamped step remains and every check passed.
fn apply_sso9_migrations(conn: &Connection) -> Result<()> {
    let stamped_max: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_version",
        [],
        |r| r.get(0),
    )?;
    if stamped_max < 22 {
        sso9_log!(
            "[ct::migrate] sso9 migration (slots {SSO9_REFRESH_VERSION}/{SSO9_RETYPE_VERSION}) \
             skipped: V22 not stamped (deferred open)"
        );
        return Ok(());
    }
    // Exact membership, never MAX-based (a later slot landing first must not
    // skip these steps forever).
    let refresh_pending = !sso9_version_stamped(conn, SSO9_REFRESH_VERSION)?;
    let retype_pending = !sso9_version_stamped(conn, SSO9_RETYPE_VERSION)?;
    if !refresh_pending && !retype_pending {
        return Ok(());
    }
    // S1: path probe. In-memory / unknown → WARN, skip backup AND migration,
    // do not stamp.
    let db_path = match conn.path() {
        Some(p) if !p.is_empty() => std::path::PathBuf::from(p),
        _ => {
            sso9_log!(
                "[ct::migrate WARN] sso9 migration (slots {SSO9_REFRESH_VERSION}/\
                 {SSO9_RETYPE_VERSION}) skipped: database path unavailable (in-memory or \
                 unknown); the steps do not run unbacked and are not stamped"
            );
            return Ok(());
        }
    };
    // S2 + S3 + S4: ONE backup per call; a failure propagates and aborts.
    let backup = sso9_backup_before_migration(conn, &db_path)?;

    let mut shape_failure = None;
    let mut refreshed: Vec<(String, String)> = Vec::new();
    if refresh_pending {
        // S5: dry pass, then S6/S7: the transactional body.
        sso9_census(conn)?;
        refreshed = run_sso9_refresh_txn(conn)?;
        sso9_log!(
            "[ct::migrate] sso9 manifest refresh committed (slot {SSO9_REFRESH_VERSION}): \
             refreshed={} entities",
            refreshed.len()
        );
        #[cfg(test)]
        sso9_run_hook(&SSO9_AFTER_REFRESH_COMMIT, conn);
        // R1: the shape check sits BETWEEN the COMMIT and the ensure loop.
        shape_failure = sso9_shape_check(conn, &refreshed)?;
        for (entity_id, _) in &refreshed {
            // The ensure opens its own transaction, so it runs only after
            // COMMIT. Best-effort: a per-row failure is logged, never fatal.
            match crate::db::entity_gate::ensure_manifest_vocabulary(conn, entity_id) {
                Ok(outcome) => sso9_log!("[ct::migrate] sso9 ensure {entity_id}: {outcome:?}"),
                Err(e) => sso9_log!("[ct::migrate WARN] sso9 ensure {entity_id} failed: {e:#}"),
            }
        }
    }

    let mut dispositions = None;
    if retype_pending && sso9_version_stamped(conn, SSO9_REFRESH_VERSION)? {
        sso9_retype_census(conn)?;
        dispositions = run_sso9_retype_txn(conn, &backup)?;
        sso9_log!(
            "[ct::migrate] sso9 retype committed (slot {SSO9_RETYPE_VERSION}): ran={}",
            dispositions.is_some()
        );
        #[cfg(test)]
        sso9_run_hook(&SSO9_AFTER_RETYPE_COMMIT, conn);
    }

    if dispositions.is_some() {
        // CodeRabbit round on fa887299 (CH0): the R1 shape check predates
        // the re-type transaction, so a manifest row restored to its
        // pre-refresh JSON after the re-type commit would pass every other
        // verification (stamps, stale slugs, dispositioned edges) while the
        // stored manifest drifted from the planned refresh — and the backup
        // would be deleted on top of the drift. Re-run the comparison AFTER
        // the re-type commit and BEFORE the backup decision; fail-closed:
        // verification failure keeps the backup (CRITICAL, retried next
        // open). Vacuous when the refresh ran in a previous open (nothing
        // captured to compare).
        if let Some(why) = sso9_shape_check(conn, &refreshed)? {
            shape_failure = Some(match shape_failure {
                Some(prev) => format!("{prev}; {why}"),
                None => why,
            });
        }
    }

    let failures = sso9_verify(conn, shape_failure, dispositions.as_ref());
    if failures.is_empty() {
        match sso9_delete_backup(&backup) {
            Ok(()) => {
                sso9_log!(
                    "[ct::migrate] sso9 verification passed; backup deleted: path={}",
                    backup.display()
                );
                sso9_prune_stale_backups(&backup);
            }
            Err(e) => sso9_log!(
                "[ct::migrate WARN] sso9 verification passed but the backup could not be \
                 deleted (kept): path={} error={e}",
                backup.display()
            ),
        }
    } else {
        sso9_log!(
            "[ct::migrate CRITICAL] sso9 post-commit verification FAILED; backup KEPT at {}; \
             failed checks: {}",
            backup.display(),
            failures.join(" | ")
        );
    }
    Ok(())
}

/// One-shot V29 ensure for direct callers that hold a raw connection to a
/// brain the app has not opened yet (the `ct` evidence commands' migrate-first
/// path, T4 M3): column-add + stamp in one call. The stamp only lands when
/// the versioned ladder has settled (MAX >= 28, T4 delta-2 m4): on a brain
/// parked mid-ladder (e.g. V25, waiting for a rooted open) the column is
/// added — the evidence scans need it — but `ct status` keeps reporting the
/// unsettled ladder truthfully until the next full migrate stamps the rest.
pub fn ensure_v29_adjudication_hold(conn: &Connection) -> Result<()> {
    // T4 delta-3 m3: on a pre-V18 brain neither `schema_version` nor
    // `librarian_evidence` exists yet — there is NOTHING to rescue, and the
    // raw rusqlite error ("no such table: schema_version") gave the operator
    // no path forward. Name the situation and the remedy instead.
    let sv_table: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='schema_version'",
        [],
        |r| r.get(0),
    )?;
    if sv_table == 0 {
        anyhow::bail!(
            "brain schema is pre-V18 (no schema_version table): nothing to \
             re-anchor yet. Open the brain once in the app (or run a full \
             migrate) to build the V18 evidence schema, then retry."
        );
    }
    ensure_v29_column(conn)?;
    let stamped_now: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_version",
        [],
        |r| r.get(0),
    )?;
    if stamped_now >= 28 {
        stamp_v29(conn)?;
    }
    Ok(())
}

/// Read-only probe for CLI callers (T4 delta-3 m5): true when
/// `librarian_evidence.adjudication_hold` exists. Lets a read-only command
/// take the write lock ONLY when the V29 column is actually missing, instead
/// of ensuring unconditionally (steady-state dry runs stay fully read-only —
/// the same unlocked-pre-check → lock-only-when-pending house pattern V29
/// itself uses). A missing `librarian_evidence` table reports `false`, so the
/// caller's ensure path surfaces the pre-V18 remedy via
/// [`ensure_v29_adjudication_hold`]'s clear error.
pub fn v29_hold_column_present(conn: &Connection) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('librarian_evidence')
          WHERE name = 'adjudication_hold'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// True when a trigger of the given name exists (any table). Used to decide
/// whether the `librarian_evidence_seen_trg` creation still needs to run on a
/// steady-state open.
fn trigger_exists(conn: &Connection, name: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='trigger' AND name = ?1",
        [name],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// Best-effort ensure failure logging (r8-m6). The open NEVER fails when
/// the ensure cannot run on a read-only or contended database; this just
/// surfaces the cause so a later write failure is not a mystery.
fn log_ensure_failure(e: &anyhow::Error) {
    #[cfg(feature = "mcp-server")]
    tracing::warn!(error = %e, "manifest vocabulary ensure skipped or failed at open");
    #[cfg(not(feature = "mcp-server"))]
    eprintln!(
        "[curated-thoughts] WARNING: manifest vocabulary ensure skipped or failed at open: {e}"
    );
}

/// Loud okf_migration abort logging (spec §2.5 r6-M2). The migration's
/// failure used to be discarded by the production `let _ = run_okf_migration(...)`
/// call in `AppDb::open_with_config`; now the cause is logged to stderr AND
/// written into the `okf_migration_diagnostic` `llm_wiki_meta` row so the
/// next heal pass can surface it. The line carries the spec-referenced
/// phrases a grep-friendly tool can pick up.
fn log_okf_migration_failure(e: &anyhow::Error) {
    #[cfg(feature = "mcp-server")]
    tracing::error!(
        error = %e,
        "okf_migration aborted at open — see `ct heal` for the diagnostic; \
         retry once the cause named in the error is fixed (spec §2.5 r6-M2)"
    );
    #[cfg(not(feature = "mcp-server"))]
    eprintln!(
        "[curated-thoughts] ERROR: okf_migration aborted at open: {e:#}\n  \
         next heal run will surface the diagnostic under `ontology.okf_migration_diagnostic`;\n  \
         retry succeeds once the cause named above is fixed (spec §2.5 r6-M2)"
    );
}

/// Drop a pre-r21 `entity_type_origin` so `MIGRATION_V26` re-creates it in
/// the r21 shape (spec R2.4.6: nullable `original_type`, new `reason`).
///
/// V26 never shipped in a release and nothing wrote the ledger before the
/// r21 revision, so a dev brain that ran the first V26 DDL holds an EMPTY
/// table: dropping it loses nothing. A non-empty pre-r21 table means some
/// writer existed that this code does not know about — fail the open loudly
/// rather than guess a `reason` for its rows.
///
/// Same concurrency shape as `apply_v24_temporal_columns`: the unlocked
/// pre-check keeps the steady state lock-free, and the rebuild re-inspects
/// under BEGIN IMMEDIATE so a racing `--mcp` open cannot drop a table the
/// winner just re-created.
fn rebuild_pre_r21_origin_ledger(conn: &Connection) -> Result<()> {
    // An absent table reads as no columns: nothing to rebuild.
    let needs_rebuild = |conn: &Connection| -> Result<bool> {
        let existing = crate::db::ddl_compat::existing_columns(conn, "entity_type_origin")?;
        Ok(!existing.is_empty() && !existing.iter().any(|c| c == "reason"))
    };
    if !needs_rebuild(conn)? {
        return Ok(());
    }
    conn.execute_batch("BEGIN IMMEDIATE;")?;
    let rebuilt = (|| -> Result<()> {
        if !needs_rebuild(conn)? {
            return Ok(());
        }
        let rows: i64 =
            conn.query_row("SELECT COUNT(*) FROM entity_type_origin", [], |r| r.get(0))?;
        if rows > 0 {
            anyhow::bail!(
                "entity_type_origin has {rows} row(s) in the pre-r21 shape (no `reason` \
                 column); refusing to guess a reason — inspect and clear the table by hand"
            );
        }
        conn.execute_batch("DROP TABLE entity_type_origin;")?;
        Ok(())
    })();
    if let Err(e) = rebuilt.and_then(|()| Ok(conn.execute_batch("COMMIT;")?)) {
        let _ = conn.execute_batch("ROLLBACK;");
        return Err(e);
    }
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
        let mut conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout = 5000;")?;
        let vault_roots = VaultConfig::new(config_path.as_ref().to_path_buf())
            .vault_root()
            .unwrap_or(None)
            .map(|root| {
                let configured = root.to_string_lossy().to_string();
                let canonical = canonicalize_workspace_root(&configured);
                VaultRoots {
                    configured,
                    canonical,
                }
            });
        migrate(&conn, vault_roots.clone(), path.parent())?;
        // Order pinned by spec plan-p10-m4:
        //   migrate → ensure_manifest_vocabulary → run_okf_migration.
        // ensure MUST come BEFORE okf_migration so a fresh install's
        // `tier_fact` row gets the document/process entries + fallback
        // written before okf_migration's gate call (Task 3) reads it —
        // an upgraded brain with a pending conversion + pre-wave-1
        // manifest hitting the gate without a fallback aborts at open.
        // Best-effort (r8-m6): log and never fail the open.
        if let Err(e) = crate::db::entity_gate::ensure_all_manifest_vocabularies(&conn) {
            log_ensure_failure(&e);
        }
        if let Some(root) = vault_roots.as_ref() {
            let vault_path = std::path::Path::new(&root.canonical);
            if vault_path.is_dir() {
                if let Err(e) = crate::db::okf_migration::run_okf_migration(&mut conn, vault_path) {
                    // Spec §2.5 r6-M2: the abort is no longer silent.
                    // `run_okf_migration` already recorded the cause into the
                    // `okf_migration_diagnostic` `llm_wiki_meta` row; this
                    // makes the open-time failure VISIBLE to the operator so
                    // a later write failure is not a mystery.
                    log_okf_migration_failure(&e);
                }
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
/// Best-effort schema bring-up for non-app hosts (MCP sidecar, `ct` wisdom
/// commands): open the brain DB read-write, tolerate locks, migrate, and warn
/// (not fail) on a read-only or contended database. Reads still work when this
/// fails; the warning names the cause so a later write failure is not a
/// mystery.
pub fn migrate_brain_db(db_path: &std::path::Path) {
    if !db_path.exists() {
        // Openers report the missing file with the actionable env-var hint;
        // don't pre-empt it with a second, vaguer message.
        return;
    }
    let opened =
        rusqlite::Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE);
    let conn = match opened {
        Ok(conn) => conn,
        Err(e) => {
            eprintln!(
                "curated-thoughts: schema check skipped, {} not writable ({e}); \
                 reads work, writes may fail on a missing column",
                db_path.display()
            );
            return;
        }
    };
    // Tolerate the desktop app or librarian holding the write lock.
    let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
    if let Err(e) = migrate_open_db(&conn, db_path.parent()) {
        eprintln!(
            "curated-thoughts: schema migration failed ({e}); \
             reads work, writes may fail on a missing column"
        );
    }
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
        // V23 (issue #211) creates `curated_proposal_deleted_sources` on
        // every open but stamps only once V22 has, so a rootless open still
        // caps at 21.
        // V24 (core-llm-wiki 7.9.0 adoption) mirrors engine migration 13's
        // temporal/watermark columns on every open but, like V23, stamps
        // only once V22 has — so this still caps at 21.
        // V26 (ontology node-type gate wave 1) creates the three CT-owned
        // tables (`entity_type_origin`, `entity_redirects`,
        // `ct_entity_optouts`) on every open but, like V23–V25, stamps only
        // once V22 has — still capped at 21 here.
        assert_eq!(
            max_version, 21,
            "open_in_memory has no vault root, so V22 refuses to stamp and the schema caps at 21"
        );
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

    /// Upgraded-DB path for the core-llm-wiki@7.9.0 bump: a database created
    /// before engine migration 13 (temporal fact columns, event
    /// `occurred_at`, librarian watermark columns) must open successfully.
    /// The Rust schema guard rejects the old shape, and the JS package
    /// migration that adds the columns only runs after the frontend boots —
    /// so the V24 gate has to add the columns first, and the temporal-index
    /// mirror has to recreate the two partial indexes.
    #[test]
    fn migration_v24_adds_engine_migration_13_columns() {
        let conn = open_in_memory().unwrap();

        // Rewind to the pre-7.9 shape: drop the seven package columns (the
        // two temporal indexes first — SQLite refuses to drop an indexed
        // column), plus the V21 column whose non-idempotent ALTER re-runs
        // when the DELETE pulls the whole stamp ladder from 17 up. A
        // pre-existing row proves the added columns backfill NULL.
        conn.execute(
            "INSERT INTO llm_wiki_entries (id, entity_id, title, body, created_at, updated_at)
             VALUES ('e1', 'ent1', 't', 'b', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute_batch(
            "DROP INDEX IF EXISTS llm_wiki_entries_superseded_idx;
             DROP INDEX IF EXISTS llm_wiki_entries_temporal_idx;
             ALTER TABLE llm_wiki_entries DROP COLUMN valid_from;
             ALTER TABLE llm_wiki_entries DROP COLUMN valid_to;
             ALTER TABLE llm_wiki_entries DROP COLUMN superseded_by;
             ALTER TABLE llm_wiki_entries DROP COLUMN superseded_at;
             ALTER TABLE llm_wiki_events DROP COLUMN occurred_at;
             ALTER TABLE llm_wiki_checkpoints DROP COLUMN librarian_watermark_at;
             ALTER TABLE llm_wiki_checkpoints DROP COLUMN librarian_watermark_id;
             ALTER TABLE curated_proposals DROP COLUMN reviewed_by;
             DELETE FROM schema_version WHERE version >= 17;",
        )
        .unwrap();

        // Precondition: the guard alone would reject this shape.
        let guard_err = crate::db::schema_guard::verify_llm_wiki_schema(&conn)
            .expect_err("guard must reject a pre-7.9 entries table");
        assert!(guard_err.to_string().contains("missing columns"));

        migrate(&conn, None, None).expect("migrate must upgrade a pre-7.9 database");

        for (table, column) in [
            ("llm_wiki_entries", "valid_from"),
            ("llm_wiki_entries", "valid_to"),
            ("llm_wiki_entries", "superseded_by"),
            ("llm_wiki_entries", "superseded_at"),
            ("llm_wiki_events", "occurred_at"),
            ("llm_wiki_checkpoints", "librarian_watermark_at"),
            ("llm_wiki_checkpoints", "librarian_watermark_id"),
        ] {
            let n: i64 = conn
                .query_row(
                    &format!(
                        "SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = '{column}'"
                    ),
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "{table}.{column} must exist after migrate()");
        }

        // The temporal-index mirror must recreate both partial indexes on
        // the Rust-only path (the engine's createTemporalIndexesIfColumnsExist
        // would otherwise only run at first frontend boot).
        for index in [
            "llm_wiki_entries_superseded_idx",
            "llm_wiki_entries_temporal_idx",
        ] {
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?1",
                    [index],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "{index} must exist after migrate()");
        }

        // No backfill: existing rows read NULL, which upstream defines as
        // "valid since created_at".
        let valid_from: Option<i64> = conn
            .query_row(
                "SELECT valid_from FROM llm_wiki_entries WHERE id = 'e1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(valid_from, None);

        crate::db::schema_guard::verify_llm_wiki_schema(&conn)
            .expect("guard must accept the upgraded database");

        // Spec §6: the upgrade is a no-op on re-open — a second migrate()
        // re-runs the ungated V24 ALTERs against the now-existing columns
        // and must succeed (guards against a future bare-ALTER regression,
        // the exact non-idempotent-ALTER mistake V21 made).
        migrate(&conn, None, None).expect("second migrate() must be a no-op");
        crate::db::schema_guard::verify_llm_wiki_schema(&conn)
            .expect("guard must still accept the re-opened database");
    }

    /// V29 restore guard (PR #336 T4 delta m6): a brain rolled back to
    /// pre-V29 state (column dropped, stamp forgotten) must come back with
    /// the column present on the next migrate — and a second migrate must be
    /// a no-op (the bare-ALTER regression guard every column migration
    /// carries since V21).
    #[test]
    fn v29_column_is_restored_on_a_pre_v29_brain() {
        let conn = open_in_memory().unwrap();
        conn.execute_batch(
            "ALTER TABLE librarian_evidence DROP COLUMN adjudication_hold;
             DELETE FROM schema_version WHERE version = 29;",
        )
        .unwrap();

        migrate(&conn, None, None).expect("migrate must restore the V29 column");
        let has: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('librarian_evidence')
                  WHERE name = 'adjudication_hold'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(has, 1, "migrate must restore the adjudication_hold column");

        migrate(&conn, None, None).expect("second migrate must be a no-op");
    }

    /// V29 stamp placement (PR #336 T4 delta m3): the early column-add site
    /// cannot stamp (a fresh brain reads MAX(version) = 19 there, below the
    /// V22 gate), so the stamp runs after V28 — a settled brain must leave
    /// migrate() stamped at 29.
    #[test]
    fn v29_stamp_lands_after_the_ladder_settles() {
        let conn = open_in_memory().unwrap();
        // Simulate a rooted brain that has settled V22 (the in-memory test
        // env is rootless, so V22 defers and sits at 21 otherwise).
        conn.execute(
            "INSERT OR IGNORE INTO schema_version (version) VALUES (22)",
            [],
        )
        .unwrap();
        migrate(&conn, None, None).unwrap();
        let v: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            v >= 29,
            "a brain settled at V22+ must stamp 29 on migrate, got MAX={v}"
        );
    }

    /// T4 delta-3 MAJOR: the CLI one-shot on a brain parked MID-LADDER (the
    /// `ct status`-truthful contract, see [`ensure_v29_adjudication_hold`]).
    /// The column must be ADDED — both evidence scans name it — while the
    /// version stamp stays EXACTLY where the ladder left it (below 29) until
    /// a full migrate settles the rest.
    #[test]
    fn ensure_v29_one_shot_adds_column_but_leaves_mid_ladder_stamp() {
        let conn = open_in_memory().unwrap();
        // Park the brain mid-ladder: simulate a rooted brain that settled
        // V22 (the in-memory env is rootless, so V22 defers otherwise) and
        // rewind past V25 — the exact mid-ladder state the doc names — then
        // drop the V29 column to complete the pre-V29 replica.
        conn.execute(
            "INSERT OR IGNORE INTO schema_version (version) VALUES (22)",
            [],
        )
        .unwrap();
        conn.execute_batch("DELETE FROM schema_version WHERE version > 25;")
            .unwrap();
        conn.execute_batch("ALTER TABLE librarian_evidence DROP COLUMN adjudication_hold;")
            .unwrap();
        let stamp_before: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            stamp_before < 28,
            "test setup must park the brain below the stamp gate, got MAX={stamp_before}"
        );

        ensure_v29_adjudication_hold(&conn).expect("one-shot ensure must succeed");

        let has: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('librarian_evidence')
                  WHERE name = 'adjudication_hold'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(has, 1, "one-shot must add the adjudication_hold column");
        let stamp_after: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            stamp_after, stamp_before,
            "mid-ladder stamp must stay truthful (no V29 stamp below the gate)"
        );

        // The full ladder then settles the stamp on the next migrate.
        migrate(&conn, None, None).unwrap();
        let settled: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            settled >= 29,
            "full migrate must stamp 29, got MAX={settled}"
        );
    }

    /// Concurrent-open guard for V24 (review finding on PR #252): the desktop
    /// app and an `--mcp` server opening the same pre-7.9 brain at once must
    /// both succeed. Without BEGIN IMMEDIATE plus the under-lock re-inspection,
    /// both snapshot the columns as missing and the loser's ALTER fails with
    /// `duplicate column name`. Drives the V24 body directly: the rest of
    /// `migrate()` has its own, separate concurrent-open behavior.
    #[test]
    fn migration_v24_concurrent_opens_do_not_race_on_alter() {
        use std::sync::{Arc, Barrier};

        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("brain.db");
        let open = |path: &std::path::Path| {
            let conn = Connection::open(path).unwrap();
            conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout = 5000;")
                .unwrap();
            conn
        };

        {
            let conn = open(&path);
            migrate(&conn, None, None).unwrap();
            conn.execute_batch(
                "DROP INDEX IF EXISTS llm_wiki_entries_superseded_idx;
                 DROP INDEX IF EXISTS llm_wiki_entries_temporal_idx;
                 ALTER TABLE llm_wiki_entries DROP COLUMN valid_from;
                 ALTER TABLE llm_wiki_entries DROP COLUMN valid_to;
                 ALTER TABLE llm_wiki_entries DROP COLUMN superseded_by;
                 ALTER TABLE llm_wiki_entries DROP COLUMN superseded_at;
                 ALTER TABLE llm_wiki_events DROP COLUMN occurred_at;
                 ALTER TABLE llm_wiki_checkpoints DROP COLUMN librarian_watermark_at;
                 ALTER TABLE llm_wiki_checkpoints DROP COLUMN librarian_watermark_id;",
            )
            .unwrap();
        }

        const OPENERS: usize = 4;
        let barrier = Arc::new(Barrier::new(OPENERS));
        let handles: Vec<_> = (0..OPENERS)
            .map(|_| {
                let barrier = Arc::clone(&barrier);
                let path = path.clone();
                std::thread::spawn(move || {
                    let conn = open(&path);
                    barrier.wait();
                    apply_v24_temporal_columns(&conn, false).map_err(|e| e.to_string())
                })
            })
            .collect();
        for handle in handles {
            handle
                .join()
                .unwrap()
                .expect("every concurrent open must migrate cleanly");
        }

        let conn = open(&path);
        crate::db::schema_guard::verify_llm_wiki_schema(&conn)
            .expect("guard must accept the concurrently upgraded database");
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

    /// V28 — two-stage retrieval columns (spec 2026-10-09-issue271 §6/§8(c)):
    /// `documents.embed_key` and `documents.last_indexed_hash` exist on every
    /// open, and the `last_indexed_hash` backfill is ONE-SHOT (rides the ALTER
    /// call's transaction only). embed_key gets NO migration backfill.
    #[test]
    fn v28_adds_embed_key_and_last_indexed_hash_with_one_shot_backfill() {
        let conn = open_in_memory().unwrap();
        // open_in_memory runs rootless, so V22+ stay unstamped (cap 21). Rewind
        // and re-migrate with a resolvable root so the stamp ladder completes
        // (same pattern as migrate_with_resolvable_vault_root_stamps_v22...):
        // both columns exist and stamp 28 is present.
        conn.execute("DELETE FROM schema_version WHERE version >= 22", [])
            .unwrap();
        migrate(
            &conn,
            Some(VaultRoots {
                configured: "/Users/kurt/vault".to_string(),
                canonical: "/private/var/vault".to_string(),
            }),
            None,
        )
        .unwrap();
        let cols = crate::db::ddl_compat::existing_columns(&conn, "documents").unwrap();
        assert!(cols.iter().any(|c| c == "embed_key"));
        assert!(cols.iter().any(|c| c == "last_indexed_hash"));
        let v: i64 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, ENGINE9_MIRROR_VERSION);

        // One-shot backfill semantics: an indexed row was backfilled from `hash`;
        // embed_key got NO migration backfill (NULL until a writer stamps it).
        conn.execute(
            "INSERT INTO documents (path, hash, status, tier) VALUES ('/v/a.md','h1','indexed','user_doc')",
            [],
        )
        .unwrap();
        // simulate a pre-upgrade pair by clearing the backfill on one row:
        conn.execute("UPDATE documents SET last_indexed_hash = NULL", [])
            .unwrap();
        // Re-open is required to re-run migrate; use migrate_open_db directly:
        crate::db::connection::migrate_open_db(&conn, None).unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM documents WHERE last_indexed_hash IS NOT NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            n, 0,
            "backfill must be ONE-SHOT: a second migrate must NOT re-run it"
        );
    }

    /// embed_key has NO migration backfill — it is written by the §8(a) swap
    /// transaction and ordinary ingests only (spec §6); a migration that
    /// guessed a key would corrupt the stage-1 coverage check.
    #[test]
    fn v28_does_not_backfill_embed_key() {
        let conn = open_in_memory().unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM documents WHERE embed_key IS NOT NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            n, 0,
            "embed_key has NO migration backfill (writers only, spec §6)"
        );
    }

    /// Pre-V15 brain upgraded on open (spec §9 step 1): a `documents` table
    /// built by the REAL V1–V14 chain (the shape `MIGRATION_V15`'s rebuild
    /// copies from — no SQL dump fixture exists under tests/fixtures/), then
    /// pushed through the full migrate() ladder. The row survives the V15
    /// rebuild and its `last_indexed_hash` is backfilled from `hash` because
    /// status='indexed'.
    #[test]
    fn pre_v15_fixture_gains_v28_columns_on_open() {
        let conn = Connection::open_in_memory().unwrap();
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
            &conn,
            "documents",
            "quarantined_at",
            "INTEGER",
        )
        .unwrap();
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V13))
            .unwrap();
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V14))
            .unwrap();

        conn.execute(
            "INSERT INTO documents (path, hash, tier, status)
             VALUES ('/v/old.md', 'h-old', 'user_doc', 'indexed')",
            [],
        )
        .unwrap();

        crate::db::connection::migrate_open_db(&conn, None).unwrap();
        let cols = crate::db::ddl_compat::existing_columns(&conn, "documents").unwrap();
        assert!(cols.iter().any(|c| c == "embed_key"));
        assert!(cols.iter().any(|c| c == "last_indexed_hash"));
        // The pre-V15 row survived the rebuild chain and was backfilled (status='indexed').
        let lih: Option<String> = conn
            .query_row(
                "SELECT last_indexed_hash FROM documents WHERE path='/v/old.md'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(lih.as_deref(), Some("h-old"));
    }

    /// V28 — `librarian_evidence_seen` fresh-brain marker (spec
    /// 2026-10-09-issue271 §8, "Fresh brain" defined): an `AFTER INSERT`
    /// trigger on `librarian_evidence` arms the marker on every live insert,
    /// and the migration backfills it once for any brain that has rows
    /// today. A COUNT(*)=0 check cannot implement "no rows EVER" (rows leave
    /// via regrade/prune/clear hard deletes), hence the persistent marker.
    #[test]
    fn evidence_seen_marker_armed_by_trigger_and_backfill() {
        let conn = open_in_memory().unwrap();

        // Trigger exists after migration (V18's DDL ran earlier in the chain).
        let trg: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                  WHERE type='trigger' AND name='librarian_evidence_seen_trg'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(trg, 1, "the AFTER INSERT trigger must exist after open");

        // A live evidence insert arms the marker (trigger path).
        conn.execute(
            "INSERT INTO llm_wiki_entries (id, entity_id, title, body, created_at, updated_at)
             VALUES ('e_seen', 'ent_seen', 't', 'b', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO librarian_evidence (entry_id, proposal_id, evidence_json, created_at)
             VALUES ('e_seen', 'prop_seen', '{\"evidence\":[],\"proposal_id\":\"prop_seen\"}', 1)",
            [],
        )
        .unwrap();
        let v: String = conn
            .query_row(
                "SELECT value FROM llm_wiki_meta WHERE key = 'librarian_evidence_seen'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v, "1");

        // Backfill arm: a brain with rows but a missing marker gets it on the
        // next migrate — the one-shot backfill rides the ALTER-call
        // transaction (V27 precedent) and `INSERT OR IGNORE` keeps it
        // idempotent against the already-armed value.
        conn.execute(
            "DELETE FROM llm_wiki_meta WHERE key = 'librarian_evidence_seen'",
            [],
        )
        .unwrap();
        crate::db::connection::migrate_open_db(&conn, None).unwrap();
        let backfilled: Option<String> = conn
            .query_row(
                "SELECT value FROM llm_wiki_meta WHERE key = 'librarian_evidence_seen'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .unwrap();
        assert_eq!(
            backfilled.as_deref(),
            Some("1"),
            "the migration backfill must arm the marker for a brain with evidence rows"
        );
    }

    /// Pre-V18 replica upgrade (spec §8 Trigger placement): `librarian_evidence`
    /// does not exist yet, so migrate() must NOT fail creating the trigger —
    /// the marker machinery is skipped and lands on a later open once V18's
    /// DDL has run. Marker absent = fresh per definition.
    #[test]
    fn pre_v18_fixture_open_succeeds_and_arms_marker_on_upgrade() {
        let tmp = tempfile::TempDir::new().unwrap();
        let conn = Connection::open(tmp.path().join("brain.db")).unwrap();
        build_pre_v18_brain(&conn);

        // First open: V18's DDL runs inside the same migrate pass, but the
        // marker machinery must be harmless either way — no trigger failure,
        // no marker (no evidence rows ever existed on this brain).
        migrate(&conn, None, Some(tmp.path())).unwrap();
        // The trigger count is informational here (the after-open phase
        // below re-checks the machinery); the underscore prefix keeps the
        // scoped `cargo check --tests` gate warning-free (Task 15's gate
        // run surfaced this pre-existing warning from 2bedb35d).
        let _trg: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                  WHERE type='trigger' AND name='librarian_evidence_seen_trg'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let marker: Option<String> = conn
            .query_row(
                "SELECT value FROM llm_wiki_meta WHERE key = 'librarian_evidence_seen'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .unwrap();
        assert_eq!(marker, None, "no evidence rows ⇒ the marker stays absent");

        // Now the brain gains V18's evidence table and live rows (as a real
        // replica upgrade would), and the NEXT open must arm the machinery:
        // trigger created (table now exists) and the one-shot backfill fires
        // because rows exist today.
        conn.execute_batch(crate::db::schema::MIGRATION_V18)
            .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_entries (id, entity_id, title, body, created_at, updated_at)
             VALUES ('e_replica', 'ent_replica', 't', 'b', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO librarian_evidence (entry_id, proposal_id, evidence_json, created_at)
             VALUES ('e_replica', 'prop_replica',
                     '{\"evidence\":[],\"proposal_id\":\"prop_replica\"}', 1)",
            [],
        )
        .unwrap();

        crate::db::connection::migrate_open_db(&conn, None).unwrap();
        let trg: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                  WHERE type='trigger' AND name='librarian_evidence_seen_trg'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(trg, 1, "the trigger lands once librarian_evidence exists");
        let v: String = conn
            .query_row(
                "SELECT value FROM llm_wiki_meta WHERE key = 'librarian_evidence_seen'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v, "1", "the backfill arms the marker for existing rows");
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

    /// §8 rebuild convergence wiring (PR #294 round-2 review): the first
    /// ROOTED open of a fresh brain re-derives derived state from the stated/
    /// ledgers — an `llm_wiki_entries` row matching a `.confirmations.jsonl`
    /// line's content hash is re-stamped `user_confirmed` — and the one-shot
    /// is stamped in `llm_wiki_meta` so steady-state opens re-run nothing.
    #[test]
    fn fresh_rooted_open_rederives_derived_state_once_per_brain() {
        let tmp = tempfile::TempDir::new().unwrap();
        let vault = tmp.path().join("vault");
        std::fs::create_dir_all(vault.join("stated")).unwrap();
        let roots = VaultRoots {
            configured: vault.to_string_lossy().into_owned(),
            canonical: vault.to_string_lossy().into_owned(),
        };

        // migrate #1: creates the full schema (OKF DDL + 0001/0002) and runs
        // the once-per-brain rederive against an empty ledger.
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn, Some(roots.clone()), None).unwrap();

        // Rebuild scenario: a fact re-ingested as librarian_inferred (fresh
        // brain) plus a durable confirmation ledger line for it.
        let title = "Confirmed fact";
        let body = "The body.";
        let hash = crate::db::stated_ledger::confirmation_content_hash(title, body);
        conn.execute(
            "INSERT INTO llm_wiki_entries (id, entity_id, title, body, tags, confidence, \
             source_type, created_at, updated_at) \
             VALUES ('fact_x', 'ent_x', ?1, ?2, '[]', 'confirmed', 'librarian_inferred', 1, 1)",
            rusqlite::params![title, body],
        )
        .unwrap();
        std::fs::write(
            vault.join("stated/.confirmations.jsonl"),
            format!(
                "{{\"fact_id\":\"fact_x\",\"content_hash\":\"{hash}\",\
                 \"by\":\"kurt\",\"at\":\"2026-10-22T00:00:00Z\"}}\n"
            ),
        )
        .unwrap();
        // Fresh-brain condition: no rederive stamp (a rebuilt .brain has none).
        conn.execute(
            "DELETE FROM llm_wiki_meta WHERE key = 'stated_ledger_rederived'",
            [],
        )
        .unwrap();

        // migrate #2: the rooted open re-derives — the row is re-stamped and
        // the one-shot marker is (re)written.
        migrate(&conn, Some(roots.clone()), None).unwrap();
        let stamped: String = conn
            .query_row(
                "SELECT source_type FROM llm_wiki_entries WHERE id = 'fact_x'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            stamped, "user_confirmed",
            "§8: rebuild re-derives the stamp"
        );
        let marker: String = conn
            .query_row(
                "SELECT value FROM llm_wiki_meta WHERE key = 'stated_ledger_rederived'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(marker, "1");

        // migrate #3: steady state — the marker gates, so a drifted row is
        // NOT re-derived (live writers own the steady state; the walk costs
        // nothing on re-open).
        conn.execute(
            "UPDATE llm_wiki_entries SET source_type = 'librarian_inferred' WHERE id = 'fact_x'",
            [],
        )
        .unwrap();
        migrate(&conn, Some(roots), None).unwrap();
        let after: String = conn
            .query_row(
                "SELECT source_type FROM llm_wiki_entries WHERE id = 'fact_x'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            after, "librarian_inferred",
            "one-shot: a stamped brain must not re-run the rederive on open"
        );
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
        // Task 10: a live unanchored-but-GROUNDED companion keeps regrade's
        // batch PARTIAL — `fact_orphan` alone would be regrade's ENTIRE
        // selection (L=1, |doomed|=1) and the rev-34 breaker refuses the
        // recovered pass outright. The companion counts toward L but anchors
        // to the live chunk `seed_v18_damage` plants, so the recount keeps it.
        conn.execute(
            "INSERT INTO llm_wiki_entries
                (id, entity_id, title, body, tags, confidence, source_type,
                 source_ref, created_at, updated_at)
             VALUES ('fact_companion', 'ent', 'companion', 'b', '[]', 'inferred',
                     'librarian_inferred',
                     'librarian-cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO librarian_evidence (entry_id, proposal_id, evidence_json,
                 unanchored, created_at)
             VALUES ('fact_companion', 'prop_companion', ?1, 1, 1)",
            [{
                let hash64 = "feedface00".to_string() + &"0".repeat(54);
                format!(
                    r#"{{"evidence":[{{"chunk_id":1,"content_hash":"{hash64}"}}],"proposal_id":"prop_companion"}}"#
                )
            }],
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
        conn.execute("DELETE FROM schema_version WHERE version >= 22", [])
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

    /// Class 2 orphan phantom (the trusted-link target the watcher staged
    /// pre-fix when the canonical form resolved outside both roots) MUST
    /// be left alone, not deleted. V22 deliberately has no "delete phantoms"
    /// branch (the original draft's Step 1 also caught relocated-vault rows
    /// and rows under a phantom root — D2b fail-open forbids the destructive
    /// alternative).
    ///
    /// The phantom persists as a dead row: the walker will overwrite the
    /// walker-written row at `<configured_root>/documents/specs/x.md` via
    /// `UNIQUE(path)` on its next pass, but the external-canonical phantom
    /// has no UNIQUE-partner and survives until manually removed. Step 2
    /// covers the common case where BOTH the watcher phantom and the walker
    /// row exist (see `v22_deletes_class1_row_that_collides_with_walker_written_row`).
    #[test]
    fn v22_leaves_class2_phantom_row_alone_for_reconcile_or_manual_cleanup() {
        let conn = open_in_memory().unwrap();
        let canonical_root = "/private/var/vault";
        let configured_root = "/Users/kurt/vault";
        // Path under an external target reached via a trusted-link under
        // documents/ — the watcher's pre-fix bug shape.
        let phantom_path = "/external/target/specs/x.md";

        conn.execute("DELETE FROM schema_version WHERE version >= 22", [])
            .unwrap();
        conn.execute(
            "INSERT INTO documents (path, hash, tier, status) \
             VALUES (?1, 'h-phantom', 'user_doc', 'indexed')",
            rusqlite::params![phantom_path],
        )
        .unwrap();

        let report = v22_unify_documents_path(&conn, configured_root, canonical_root).unwrap();
        assert_eq!(
            report.deleted, 0,
            "V22 deliberately does not delete class-2 phantoms (D2b fail-open)"
        );
        assert_eq!(report.rewritten, 0, "no class-1 row was seeded");

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM documents WHERE hash = 'h-phantom'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 1,
            "the phantom row must be preserved after V22; D2b forbids deleting \
             rows that cannot be relativized against either root"
        );

        let surviving_path: String = conn
            .query_row(
                "SELECT path FROM documents WHERE hash = 'h-phantom'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            surviving_path, phantom_path,
            "the phantom's stored path must be untouched (no rewrite, no delete)"
        );
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

        conn.execute("DELETE FROM schema_version WHERE version >= 22", [])
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

    /// Spec D2b row-side regression test for V22: a `user_doc` row from a
    /// PRIOR vault location (the user moved their vault from
    /// `/old/vault` to the currently-resolved roots and reopened before
    /// re-ingesting) must be LEFT ALONE — not deleted, not rewritten, and
    /// not cascaded into. Its attached chunks must also survive.
    ///
    /// D2b fail-open: "Deleting rows we cannot place inside the vault would
    /// be the same class of unrecoverable mistake the empty-walk guard
    /// exists to prevent." V22's original Step 1 deleted every `user_doc`
    /// row whose path didn't match either prefix, which would nuke these
    /// rows and their chunks. V22 has no Step 1 any more.
    #[test]
    fn v22_preserves_relocated_user_doc_row_with_chunk() {
        let conn = open_in_memory().unwrap();
        let canonical_root = "/private/var/vault";
        let configured_root = "/Users/kurt/vault";
        // Pre-relocation path: outside both current roots, no way to
        // distinguish from a class-2 phantom by string alone. The fail-open
        // rule says leave it.
        let prior_vault_path = "/old/vault/notes.md";

        conn.execute("DELETE FROM schema_version WHERE version >= 22", [])
            .unwrap();
        conn.execute(
            "INSERT INTO documents (path, hash, tier, status) \
             VALUES (?1, 'h-relocated', 'user_doc', 'indexed')",
            rusqlite::params![&prior_vault_path],
        )
        .unwrap();
        let doc_id: i64 = conn
            .query_row(
                "SELECT id FROM documents WHERE hash = 'h-relocated'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        // Attach a chunk so the cascade behavior is observable — if V22
        // touched the row, the FK ON DELETE CASCADE would nuke this chunk.
        conn.execute(
            "INSERT INTO chunks (doc_id, chunk_text, position) \
             VALUES (?1, 'preserved', 0)",
            rusqlite::params![doc_id],
        )
        .unwrap();

        let report = v22_unify_documents_path(&conn, configured_root, canonical_root).unwrap();
        assert_eq!(
            report.rewritten, 0,
            "no canonical-prefixed row was seeded; nothing to rewrite"
        );
        assert_eq!(
            report.deleted, 0,
            "D2b fail-open: rows outside both roots must NOT be deleted by V22"
        );

        // The document row survives intact, path unchanged.
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM documents WHERE id = ?1",
                [doc_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "the relocated row must survive V22 untouched");
        let surviving_path: String = conn
            .query_row("SELECT path FROM documents WHERE id = ?1", [doc_id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            surviving_path, prior_vault_path,
            "the relocated row's stored path must NOT be rewritten"
        );

        // The chunk must still be attached (no cascade fired).
        let chunk_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chunks WHERE doc_id = ?1",
                [doc_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            chunk_count, 1,
            "the relocated row's chunk must NOT cascade-delete — the row was \
             preserved, so its children must be too"
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

        conn.execute("DELETE FROM schema_version WHERE version >= 22", [])
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

        conn.execute("DELETE FROM schema_version WHERE version >= 22", [])
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
        conn.execute("DELETE FROM schema_version WHERE version >= 22", [])
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
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            version, ENGINE9_MIRROR_VERSION,
            "V22 then V23 then V24 then V25 then V26 then V27 then V28 then V29 \
             then V30 then V33 (engine9 mirror, V22-gated; sso9 31/32 skip \
             in-memory) must be stamped when the migration runs"
        );

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

    /// V25 — wisdom-deposit substrate (spec 2026-10-01-wisdom-deposit-tool-surface):
    /// agent folder rules seeded (synthesize+auto_approve, with an EXPLICIT
    /// summarize child override on supersessions/ — get_folder_mode walks
    /// ancestors, so an absent child row would inherit synthesize and
    /// auto-generate facts from supersession proposals), and the
    /// deposit_kick_state ledger exists. Stamps only past a settled V22.

    #[test]
    fn v25_seeds_agent_folder_rules_and_kick_state_table() {
        use crate::librarian::get_folder_mode;
        let conn = open_in_memory().unwrap();
        conn.execute("DELETE FROM schema_version WHERE version >= 22", [])
            .unwrap();
        migrate(
            &conn,
            Some(VaultRoots {
                configured: "/Users/kurt/vault".to_string(),
                canonical: "/private/var/vault".to_string(),
            }),
            None,
        )
        .unwrap();

        let version: i64 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            version, ENGINE9_MIRROR_VERSION,
            "rooted open stamps 26 (ontology gate wave), 27 (embed scheme), \
             28 (two-stage columns), 29 (adjudication hold), 30 (area) and \
             33 (engine9 mirror; sso9 31/32 skip in-memory); MAX must be 33",
        );

        let agents: String = conn
            .query_row(
                "SELECT librarian_mode || ':' || auto_approve FROM folder_rules
                  WHERE folder_path = 'immutable-source-files/agents'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(agents, "synthesize:1");

        let supersessions: String = conn
            .query_row(
                "SELECT librarian_mode || ':' || auto_approve FROM folder_rules
                  WHERE folder_path = 'immutable-source-files/agents/supersessions'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(supersessions, "summarize:1");

        // The child override wins the ancestor walk — this is the whole
        // point of the second row (CodeRabbit finding 2 on the spec PR).
        let (mode, auto) =
            get_folder_mode(&conn, "immutable-source-files/agents/supersessions/x.md");
        assert_eq!(
            (mode.as_str(), auto),
            ("summarize", true),
            "supersessions/ must override the parent synthesize rule"
        );

        // kick-state ledger exists and accepts the state vocabulary
        conn.execute(
            "INSERT INTO deposit_kick_state (path, state, error, updated_ms)
             VALUES ('immutable-source-files/agents/a.md', 'no_ingest_host', NULL, 0)",
            [],
        )
        .unwrap();
    }

    /// Task 9 (user-confirmation-queue, spec §3.7.3): the stated/ tree gets a
    /// folder rule — synthesize + auto-approve — so a captured statement is
    /// human tier the moment the librarian finishes (the attested-source arm
    /// stamps `user_stated`). The rule is seeded on EVERY open (INSERT OR
    /// IGNORE in the V25 block), so existing brains get it without a new
    /// schema_version stamp (the task's no-DDL constraint).
    #[test]
    fn v25_block_seeds_stated_folder_rule() {
        use crate::librarian::get_folder_mode;
        // Rooted re-migrate: the V25 block (folder rules + kick-state ledger)
        // only runs past a settled V22, so the open must carry a vault root
        // — same setup as v25_seeds_agent_folder_rules_and_kick_state_table.
        let conn = open_in_memory().unwrap();
        conn.execute("DELETE FROM schema_version WHERE version >= 22", [])
            .unwrap();
        migrate(
            &conn,
            Some(VaultRoots {
                configured: "/Users/kurt/vault".to_string(),
                canonical: "/private/var/vault".to_string(),
            }),
            None,
        )
        .unwrap();
        let (mode, auto) = get_folder_mode(&conn, "stated/kurt/2026/some-statement.md");
        assert_eq!(
            (mode.as_str(), auto),
            ("synthesize", true),
            "stated/ must be synthesize + auto-approve for the app capture lane"
        );
    }

    /// RED (F2 consumer, B1 round 2): `documents.path` is now the canonical
    /// ABSOLUTE key in every regime, while V25's `folder_rules` rows are
    /// vault-relative — so the librarian's literal ancestor walk must
    /// relativize an absolute source path against the configured vault root
    /// before matching, or the kick regime loses its synthesize +
    /// auto-approve lane (V25's stated intent).
    #[test]
    fn get_folder_mode_matches_v25_rules_for_absolute_document_paths() {
        use crate::librarian::get_folder_mode;
        let tmp = tempfile::TempDir::new().unwrap();
        let vault = tmp.path().join("vault");
        std::fs::create_dir_all(vault.join("stated")).unwrap();
        let brain = tmp.path().join("brain");
        std::fs::create_dir_all(&brain).unwrap();
        std::fs::write(
            brain.join("config.json"),
            serde_json::json!({ "vault_path": vault.to_string_lossy() }).to_string(),
        )
        .unwrap();
        // File-backed ROOTED open: V22 settles and V25 seeds the
        // vault-relative folder rules, and the connection carries its brain
        // path so get_folder_mode resolves the vault root from the
        // config.json beside it (never the live user-config resolver —
        // issue #178 guard).
        let db = crate::db::connection::AppDb::open_with_config(
            &brain.join("brain.db"),
            brain.join("config.json"),
        )
        .unwrap();
        let conn = db.0;
        // `documents.path` keys are produced by the canonical-root family
        // (`virtual_path_key_at_root`), so the absolute source path the
        // librarian sees is the CANONICAL spelling of the file even though
        // the config.json root spelling above is the raw tempdir form
        // (macOS: /var/... is a symlink to /private/var/...). Derive the
        // input the same way production does.
        let abs = crate::db::queue::virtual_path_key_at_root(
            &vault,
            std::path::Path::new("stated/kurt/2026/note.md"),
        )
        .unwrap();
        let (mode, auto) = get_folder_mode(&conn, &abs);
        assert_eq!(
            (mode.as_str(), auto),
            ("synthesize", true),
            "RED (F2): an absolute documents.path no longer matches V25's \
             vault-relative folder rules — the kick lane loses synthesize+auto"
        );
    }

    /// Rootless opens must not stamp V25 past a deferred V22 (same masking
    /// hazard V23/V24 document).
    #[test]
    fn v25_never_stamps_past_deferred_v22() {
        let conn = open_in_memory().unwrap();
        conn.execute("DELETE FROM schema_version WHERE version >= 22", [])
            .unwrap();
        migrate(&conn, None::<VaultRoots>, None).unwrap();
        let version: i64 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert!(
            version < 22,
            "V22 deferred means nothing past it stamps (got {version})"
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
        conn.execute("DELETE FROM schema_version WHERE version >= 21", [])
            .unwrap();
        let canonical_root = "/private/var/vault";
        let seeded_path = format!("{canonical_root}/notes.md");
        conn.execute(
            "INSERT INTO documents (path, hash, tier, status) \
             VALUES (?1, 'h-defer', 'user_doc', 'indexed')",
            rusqlite::params![&seeded_path],
        )
        .unwrap();

        migrate(&conn, None::<VaultRoots>, None)
            .expect("migrate must succeed even when V22 refuses");

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

    /// Issue #211 spec D2: the deleted-sources table exists on EVERY open,
    /// including rootless ones, without stamping 23 past a deferred V22.
    #[test]
    fn deleted_sources_table_exists_on_rootless_open_without_stamping_23() {
        let conn = open_in_memory().unwrap();
        let count = |kind: &str, name: &str| -> i64 {
            conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = ?1 AND name = ?2",
                rusqlite::params![kind, name],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(count("table", "curated_proposal_deleted_sources"), 1);
        assert_eq!(
            count("index", "idx_curated_proposal_deleted_sources_hash"),
            1
        );

        // Idempotent: a second pass must not fail on the existing table.
        migrate(&conn, None::<VaultRoots>, None).expect("second migrate must succeed");

        let v23: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_version WHERE version = 23",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            v23, 0,
            "a rootless open must not stamp 23 while V22 is deferred"
        );
    }

    /// Issue #211 spec D2 / Principle 4 regression: stamping 23 on a
    /// rootless open would make every later rooted open read MAX = 23, skip
    /// `if version < 22`, and never unify paths.
    #[test]
    fn v23_never_stamps_past_deferred_v22_so_a_later_rooted_open_still_unifies() {
        // open_in_memory() is rootless: V22 deferred, MAX(version) = 21.
        let conn = open_in_memory().unwrap();
        let canonical_root = "/private/var/vault";
        let configured_root = "/Users/kurt/vault";
        conn.execute(
            "INSERT INTO documents (path, hash, tier, status) \
             VALUES (?1, 'h-v23', 'user_doc', 'indexed')",
            rusqlite::params![format!("{canonical_root}/notes.md")],
        )
        .unwrap();

        migrate(&conn, None::<VaultRoots>, None).expect("rootless migrate");
        let max_version = |c: &Connection| -> i64 {
            c.query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(max_version(&conn), 21, "rootless opens cap at 21");

        migrate(
            &conn,
            Some(VaultRoots {
                configured: configured_root.to_string(),
                canonical: canonical_root.to_string(),
            }),
            None,
        )
        .expect("rooted migrate");

        let path: String = conn
            .query_row("SELECT path FROM documents WHERE hash = 'h-v23'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            path,
            format!("{configured_root}/notes.md"),
            "V22 must still run on the first rooted open"
        );
        assert_eq!(
            max_version(&conn),
            ENGINE9_MIRROR_VERSION,
            "rooted open stamps 22..29, V30 and V33 (the latter two gated on V22)"
        );
    }

    fn edge_index_names(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare(
                "SELECT name FROM sqlite_master
                 WHERE type = 'index' AND tbl_name = 'llm_wiki_edges'
                   AND name NOT LIKE 'sqlite_autoindex%'
                 ORDER BY name",
            )
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<String>>>()
            .unwrap()
    }

    #[test]
    fn llm_wiki_v12_edge_index_replaces_legacy_index_on_open() {
        let conn = open_in_memory().unwrap();
        // Simulate a pre-7.7 brain: legacy single-column index only.
        conn.execute_batch(
            "DROP INDEX IF EXISTS llm_wiki_edges_entity_id_idx;
             CREATE INDEX IF NOT EXISTS llm_wiki_edges_entity_idx ON llm_wiki_edges(entity_id);",
        )
        .unwrap();
        assert_eq!(edge_index_names(&conn), vec!["llm_wiki_edges_entity_idx"]);

        migrate_open_db(&conn, None).unwrap();
        assert_eq!(
            edge_index_names(&conn),
            vec!["llm_wiki_edges_entity_id_idx"]
        );

        // Idempotent: a second open changes nothing.
        migrate_open_db(&conn, None).unwrap();
        assert_eq!(
            edge_index_names(&conn),
            vec!["llm_wiki_edges_entity_id_idx"]
        );
    }

    /// Spec plan-p10-m4: `open_with_config` runs the open in this exact order:
    /// `migrate` → `ensure_manifest_vocabulary` → `run_okf_migration`.
    /// A fresh brain with a pre-wave-1 manifest (no `fallback_node_type`) is
    /// the matrix case that fails the open if the order is wrong (Task 3's
    /// gate call would abort without a fallback). We can't drive
    /// `open_with_config` end-to-end in a unit test (it needs a real
    /// `VaultConfig`), so this pins the order by checking the post-`migrate`
    /// state, then asserting `ensure_all_manifest_vocabularies` runs
    /// successfully and that the ensure's memo is recorded BEFORE the
    /// `okf_migration` call site is reached. Concretely: after migrate, an
    /// EA-subset manifest without `fallback_node_type` exists; running
    /// `ensure_all_manifest_vocabularies` produces an `Ensured` outcome with
    /// `fallback_set: true` and a recorded memo, matching what
    /// `open_with_config` will see.
    #[test]
    fn open_with_config_runs_migrate_then_ensure_then_okf_migration_in_order() {
        // 1. Open an in-memory brain and run migrate().
        let conn = open_in_memory().unwrap();

        // 2. Plant a pre-wave-1 EA-subset manifest WITHOUT fallback_node_type.
        let types: Vec<serde_json::Value> = crate::db::entity_gate::EA_SEED_TYPES
            .iter()
            .map(|s| serde_json::json!({"type": s}))
            .collect();
        let manifest = serde_json::json!({
            "node_types": types,
            "edge_types": [],
        });
        let manifest_json = serde_json::to_string(&manifest).unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_entity_manifests (entity_id, mode, manifest_json, updated_at)
             VALUES (?1, 'a', ?2, 1)",
            rusqlite::params!["tier_fact", manifest_json],
        )
        .unwrap();

        // 3. Run ensure_all_manifest_vocabularies — what open_with_config does
        //    between migrate and run_okf_migration.
        let summary = crate::db::entity_gate::ensure_all_manifest_vocabularies(&conn)
            .expect("ensure must succeed on a fresh brain with a clean manifest");
        assert_eq!(summary.visited, 1, "exactly one manifest row was visited");
        assert_eq!(
            summary.extended, 1,
            "the document/process entries were added"
        );
        assert_eq!(
            summary.fallbacks_set, 1,
            "the fallback_node_type key was written"
        );

        // 4. Verify the post-state matches what Task 3's gate call sees.
        let stored: String = conn
            .query_row(
                "SELECT manifest_json FROM llm_wiki_entity_manifests WHERE entity_id = 'tier_fact'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&stored).unwrap();
        let slugs: Vec<String> = parsed["node_types"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v["type"].as_str().map(String::from))
            .collect();
        assert!(slugs.iter().any(|s| s == "document"));
        assert!(slugs.iter().any(|s| s == "process"));
        assert_eq!(
            parsed["fallback_node_type"].as_str(),
            Some("project"),
            "ensure wrote the per-manifest fallback before run_okf_migration ran"
        );

        // 5. The memo was recorded (r11-m4: only after commit).
        let memo_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM manifest_ensure_memo WHERE entity_id = 'tier_fact'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(memo_count, 1, "ensure must record its memo post-commit");
    }

    // ---- sso9 manifest MERGE refresh (spec §3 PR-A, MERGE-refresh step) ----

    const SSO8_MANIFEST: &str = include_str!("testdata/schema-software-org-8.0.2.manifest.json");

    fn sso9_backups(dir: &Path) -> Vec<std::path::PathBuf> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("brain.db.bak-pre-sso9-"))
            })
            .collect()
    }

    /// Full migrate with a real vault root, so V22 stamps (the sso9 step is
    /// gated on V22; a rootless open defers it).
    fn sso9_migrate(conn: &Connection, dir: &Path) -> Result<()> {
        let vault = dir.join("vault");
        std::fs::create_dir_all(&vault).unwrap();
        let root = vault.to_string_lossy().to_string();
        let roots = VaultRoots {
            configured: root.clone(),
            canonical: canonicalize_workspace_root(&root),
        };
        migrate(conn, Some(roots), Some(dir))
    }

    fn sso9_stamped(conn: &Connection) -> bool {
        sso9_version_stamped(conn, SSO9_REFRESH_VERSION).unwrap()
    }

    fn sso9_clear_log() {
        SSO9_LOG.with(|l| l.borrow_mut().clear());
    }

    fn sso9_log_lines() -> Vec<String> {
        SSO9_LOG.with(|l| l.borrow().clone())
    }

    /// `(table, source, backup)` parsed from the `sso9 backup ok` log line.
    fn sso9_backup_log_counts() -> Vec<(String, String, String)> {
        let line = sso9_log_lines()
            .into_iter()
            .find(|l| l.contains("sso9 backup ok:"))
            .expect("backup log line");
        let tail = line.split("sanity(source/backup)").nth(1).unwrap().to_string();
        tail.split_whitespace()
            .map(|tok| {
                let (t, c) = tok.split_once('=').unwrap();
                let (a, b) = c.split_once('/').unwrap();
                (t.to_string(), a.to_string(), b.to_string())
            })
            .collect()
    }

    fn sso9_snapshot(conn: &Connection) -> Sso9SnapshotCounts {
        let manifests = {
            let mut s = conn
                .prepare("SELECT entity_id, manifest_json FROM llm_wiki_entity_manifests ORDER BY entity_id")
                .unwrap();
            s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };
        let stamps = {
            let mut s = conn
                .prepare("SELECT version FROM schema_version ORDER BY version")
                .unwrap();
            s.query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect()
        };
        let memo = {
            let mut s = conn
                .prepare("SELECT entity_id, manifest_hash FROM manifest_ensure_memo ORDER BY entity_id, manifest_hash")
                .unwrap();
            s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };
        (manifests, stamps, memo)
    }

    /// A tempfile brain migrated once, then rewound to the pre-step state: the
    /// slot unstamped, an 8.x tier_fact + a customised 8.x entity + a foreign
    /// + a malformed manifest planted, and the first backup removed.
    fn sso9_rewound_brain() -> (tempfile::TempDir, Connection) {
        let tmp = tempfile::tempdir().unwrap();
        let conn = Connection::open(tmp.path().join("brain.db")).unwrap();
        sso9_migrate(&conn, tmp.path()).unwrap();
        for b in sso9_backups(tmp.path()) {
            std::fs::remove_file(b).unwrap();
        }
        let mut custom: serde_json::Value = serde_json::from_str(SSO8_MANIFEST).unwrap();
        custom["node_types"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"type": "gadget"}));
        custom["fallback_node_type"] = serde_json::json!("project");
        for (id, json) in [
            ("tier_fact", SSO8_MANIFEST.to_string()),
            ("ent_custom", custom.to_string()),
            (
                "ent_foreign",
                r#"{"node_types":[{"type":"widget"}],"fallback_node_type":"widget"}"#.to_string(),
            ),
            ("ent_bad", "{not json".to_string()),
        ] {
            conn.execute(
                "INSERT OR REPLACE INTO llm_wiki_entity_manifests (entity_id, mode, manifest_json, updated_at)
                 VALUES (?1, 'strict', ?2, 1)",
                rusqlite::params![id, json],
            )
            .unwrap();
        }
        conn.execute(
            "DELETE FROM schema_version WHERE version = ?1",
            [SSO9_REFRESH_VERSION],
        )
        .unwrap();
        (tmp, conn)
    }

    fn manifest_value(conn: &Connection, id: &str) -> serde_json::Value {
        let s: String = conn
            .query_row(
                "SELECT manifest_json FROM llm_wiki_entity_manifests WHERE entity_id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        serde_json::from_str(&s).unwrap()
    }

    fn type_keys(v: &serde_json::Value) -> Vec<String> {
        v["node_types"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["type"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn sso9_refresh_slot_is_one_past_the_highest_prior_stamp() {
        assert_eq!(SSO9_REFRESH_VERSION, 31);
        assert_eq!(SSO9_RETYPE_VERSION, SSO9_REFRESH_VERSION + 1);
        // 30 = highest stamped literal elsewhere in this file (V30 area column).
        assert_eq!(SSO9_REFRESH_VERSION, 30 + 1);
        let conn = open_in_memory().unwrap();
        assert!(!sso9_stamped(&conn), "in-memory leaves the sso9 slot unstamped");
    }

    #[test]
    fn sso9_refresh_merges_8x_manifests_stamps_and_backs_up() {
        let (tmp, conn) = sso9_rewound_brain();
        sso9_clear_log();
        sso9_migrate(&conn, tmp.path()).unwrap();
        assert!(sso9_stamped(&conn));
        // P1/step 11: on the success path the backup is deleted (Task 5 step 10);
        // its row counts are proven from the migration log line, and its
        // existence is asserted only on the verification-failure path (5c).
        assert!(sso9_backups(tmp.path()).is_empty(), "backup deleted after a verified pair");
        let counts = sso9_backup_log_counts();
        assert_eq!(counts.len(), SSO9_SANITY_TABLES.len());
        for (t, a, b) in &counts {
            assert_eq!(a, b, "{t} source/backup row counts match in the log line");
        }
        // tier_fact (incl.) refreshed AND ensured: 9.x types + document/process + fallback.
        let tf = manifest_value(&conn, "tier_fact");
        let keys = type_keys(&tf);
        for k in ["decision", "lesson", "area", "document", "process"] {
            assert!(keys.contains(&k.to_string()), "tier_fact missing {k}");
        }
        assert!(!keys.contains(&"handoff".to_string()));
        assert!(tf["fallback_node_type"].is_string());
        // Customised keeps extras + its own fallback.
        let cu = manifest_value(&conn, "ent_custom");
        assert!(type_keys(&cu).contains(&"gadget".to_string()));
        assert_eq!(cu["fallback_node_type"], "project");
        // Foreign + malformed untouched.
        assert_eq!(
            manifest_value(&conn, "ent_foreign")["node_types"][0]["type"],
            "widget"
        );
        let bad: String = conn
            .query_row(
                "SELECT manifest_json FROM llm_wiki_entity_manifests WHERE entity_id = 'ent_bad'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(bad, "{not json");
    }

    #[test]
    fn sso9_refresh_in_memory_leaves_slot_unstamped_and_manifests_unchanged() {
        let conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_entity_manifests (entity_id, mode, manifest_json, updated_at)
             VALUES ('tier_fact', 'strict', ?1, 1)",
            [SSO8_MANIFEST],
        )
        .unwrap();
        let before = sso9_snapshot(&conn);
        apply_sso9_migrations(&conn).unwrap();
        assert!(!sso9_stamped(&conn));
        assert_eq!(sso9_snapshot(&conn), before);
    }

    #[test]
    fn sso9_refresh_backup_failure_aborts_migrate_without_stamping() {
        let (tmp, conn) = sso9_rewound_brain();
        // Force a backup failure: pin the backup timestamp and pre-create the
        // target — the exclusive reservation refuses an existing file, so the
        // step bails before `VACUUM INTO` ever runs (rOZU: a lost reservation
        // means another opener holds the slot; proceeding could destroy the
        // winner's completed backup). A read-only dir cannot be used —
        // earlier steps need the journal.
        SSO9_BACKUP_TS_OVERRIDE.with(|c| c.set(Some(42)));
        let squatter = tmp.path().join("brain.db.bak-pre-sso9-42");
        std::fs::write(&squatter, b"squatter").unwrap();
        let before = sso9_snapshot(&conn);
        let result = sso9_migrate(&conn, tmp.path());
        SSO9_BACKUP_TS_OVERRIDE.with(|c| c.set(None));
        let err = result.expect_err("backup failure must abort migrate()");
        assert!(
            format!("{err:#}").contains("sso9 backup"),
            "failure must come from the backup step: {err:#}"
        );
        assert!(!sso9_stamped(&conn));
        assert_eq!(sso9_snapshot(&conn), before);
        // Only the squatter remains, untouched by the failure cleanup.
        assert_eq!(sso9_backups(tmp.path()), vec![squatter.clone()]);
        assert_eq!(std::fs::read(&squatter).unwrap(), b"squatter");
        // The next open retries and succeeds.
        sso9_migrate(&conn, tmp.path()).unwrap();
        assert!(sso9_stamped(&conn));
    }

    #[test]
    fn sso9_refresh_is_idempotent_across_migrate_calls() {
        let (tmp, conn) = sso9_rewound_brain();
        sso9_migrate(&conn, tmp.path()).unwrap();
        let first = sso9_snapshot(&conn);
        sso9_clear_log();
        sso9_migrate(&conn, tmp.path()).unwrap();
        assert_eq!(sso9_snapshot(&conn), first);
        assert_eq!(sso9_backups(tmp.path()).len(), 0, "no backup left; stamped slots take none");
        assert!(
            sso9_log_lines().iter().all(|l| !l.contains("sso9 backup ok")),
            "a fully stamped pair takes no new backup"
        );
    }

    #[test]
    fn sso9_refresh_body_is_idempotent_with_the_stamp_bypassed() {
        let (tmp, conn) = sso9_rewound_brain();
        sso9_migrate(&conn, tmp.path()).unwrap();
        let first = sso9_snapshot(&conn);
        // Bypass the stamp check: drop the stamp and run the BODY again.
        conn.execute(
            "DELETE FROM schema_version WHERE version = ?1",
            [SSO9_REFRESH_VERSION],
        )
        .unwrap();
        let refreshed = run_sso9_refresh_txn(&conn).unwrap();
        assert!(refreshed.is_empty(), "no row may change on a second body run");
        assert_eq!(sso9_snapshot(&conn), first);
    }

    #[test]
    fn sso9_refresh_logs_backup_path_sanity_and_census() {
        // Smoke: the dry pass classifies without writing and does not error.
        let (_tmp, conn) = sso9_rewound_brain();
        let before = sso9_snapshot(&conn);
        sso9_census(&conn).unwrap();
        assert_eq!(sso9_snapshot(&conn), before, "dry pass must not write");
    }

    // ---- sso9 re-type + edge dispositions (spec §3 PR-A, re-type step) ----
    //
    // F11/N4 trigger-coverage evidence (core-llm-wiki 8.0.2 `setupDatabase`):
    // at capture time the only triggers were the three FTS triggers on
    // `llm_wiki_entries` (`entries_ai` AFTER INSERT, `entries_au` AFTER
    // UPDATE OF id, entity_id, title, body, tags, deleted_at, `entries_ad`
    // AFTER DELETE). None fire on `okf_type`; there are NO triggers on
    // `llm_wiki_edges`, `llm_wiki_tasks` or `curated_entities`. NOTE
    // (engine-9x PR-B): fresh brains now ALSO carry the three ledger
    // triggers (enqueue AFTER UPDATE OF deleted_at, hard_delete AFTER
    // DELETE, resurrect AFTER UPDATE OF deleted_at) — all fire only on
    // deleted_at updates or entry deletes, never on `okf_type`, which is
    // the invariant this evidence supports.
    // `sso9_retype_has_no_triggers_on_retyped_columns` re-checks this
    // against the live Rust fixture.

    const SSO9_SEED: &str = r#"
        INSERT INTO llm_wiki_entries (id, entity_id, title, body, created_at, updated_at, okf_type, deleted_at, ontology_checked_at, heal_checked_at) VALUES
          ('h1','ent1','t','b',1,77,'handoff',NULL,88,99),
          ('h_tomb','ent1','t','b',1,77,'handoff',5,88,99),
          ('s1','ent1','t','b',1,77,'session_recap',NULL,88,99),
          ('s_tomb','ent1','t','b',1,77,'session_recap',5,88,99),
          ('hv','ent1','t','b',1,77,'Handoff',NULL,88,99),
          ('sw1','ent1','t','b',1,77,'software_application',NULL,88,99),
          ('svc1','ent1','t','b',1,77,'service',NULL,88,99),
          ('org1','ent1','t','b',1,77,'organization',NULL,88,99),
          ('per1','ent1','t','b',1,77,'person',NULL,88,99),
          ('pl1','ent1','t','b',1,77,'place',NULL,88,99),
          ('ev1','ent1','t','b',1,77,'event',NULL,88,99),
          ('w1','ent1','t','b',1,77,'creativework',NULL,88,99);
        INSERT INTO llm_wiki_tasks (id, entity_id, description, created_at, updated_at, okf_type, deleted_at) VALUES
          ('t_h','ent1','d',1,77,'handoff',NULL),
          ('t_s','ent1','d',1,77,'session_recap',5),
          ('t_c','ent1','d',1,77,'Session_Recap',NULL);
        INSERT INTO curated_entities (id, name, entity_type, created_at, updated_at, deleted_at) VALUES
          ('ce_h','n','handoff',1,77,NULL),
          ('ce_s','n','session_recap',1,77,5),
          ('ce_cv','n','HANDOFF',1,77,NULL),
          ('ce_concept','n','concept',1,77,NULL),
          ('ce_process','n','process',1,77,NULL),
          ('ce_repo','n','repository',1,77,NULL),
          ('ce_job','n','job',1,77,NULL),
          ('ce_sw','n','software_application',1,77,NULL);
        INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at) VALUES
          ('e_coll_hf','ent1','h1','sw1','handoffFor',1),
          ('e_coll_ab','ent1','h1','sw1','about',1),
          ('e_remap','ent1','s1','sw1','handoffFor',1),
          ('e_svc','ent1','h1','svc1','handoffFor',1),
          ('e_ce_src','ent1','ce_h','ce_sw','handoffFor',1),
          ('e_author','ent1','h1','per1','author',1),
          ('e_publisher','ent1','s1','org1','publisher',1),
          ('e_about_person','ent1','h1','per1','about',1),
          ('e_about_place','ent1','h1','pl1','about',1),
          ('e_about_event','ent1','s1','ev1','about',1),
          ('e_supersedes','ent1','h_tomb','h1','supersedes',1),
          ('e_about_org','ent1','s1','org1','about',1),
          ('e_author_ce_tgt','ent1','w1','ce_h','author',1),
          ('e_undisp','ent1','h1','org1','handoffFor',1),
          ('e_related','ent1','h1','sw1','relatedTo',1),
          ('e_unrelated','ent1','sw1','org1','knows',1);
        INSERT INTO llm_wiki_outbox (id, entity_id, table_name, record_id, operation, payload, created_at)
          VALUES ('ob1','ent1','llm_wiki_entries','h1','upsert','{"okf_type":"handoff"}',3);
        INSERT INTO curated_proposals (id, kind, model, status, created_at)
          VALUES ('p1','new_entity','m','pending',1);
        INSERT INTO curated_proposal_items (id, proposal_id, item_type, payload, status) VALUES
          ('pi1','p1','fact_add','{"type":"handoff"}','pending'),
          ('pi2','p1','edge_add','{"edge_type":"handoffFor"}','pending');
    "#;

    /// A tempfile brain at the 8.0.2-era state: BOTH sso9 slots unstamped,
    /// the 8.x manifests planted, and the Task 5 data seeded.
    fn sso9_retype_brain() -> (tempfile::TempDir, Connection) {
        let (tmp, conn) = sso9_rewound_brain();
        conn.execute(
            "DELETE FROM schema_version WHERE version = ?1",
            [SSO9_RETYPE_VERSION],
        )
        .unwrap();
        conn.execute_batch(SSO9_SEED).unwrap();
        (tmp, conn)
    }

    /// Seed + migrate, returning the captured log.
    fn sso9_run_retype() -> (tempfile::TempDir, Connection, Vec<String>) {
        let (tmp, conn) = sso9_retype_brain();
        sso9_clear_log();
        sso9_migrate(&conn, tmp.path()).unwrap();
        let log = sso9_log_lines();
        (tmp, conn, log)
    }

    fn q_str(conn: &Connection, sql: &str, id: &str) -> Option<String> {
        conn.query_row(sql, [id], |r| r.get::<_, Option<String>>(0))
            .optional()
            .unwrap()
            .flatten()
    }

    fn okf(conn: &Connection, table: &str, id: &str) -> Option<String> {
        q_str(conn, &format!("SELECT okf_type FROM {table} WHERE id = ?1"), id)
    }

    fn etype(conn: &Connection, id: &str) -> Option<String> {
        q_str(conn, "SELECT edge_type FROM llm_wiki_edges WHERE id = ?1", id)
    }

    fn ce_type(conn: &Connection, id: &str) -> Option<String> {
        q_str(conn, "SELECT entity_type FROM curated_entities WHERE id = ?1", id)
    }

    /// Every column the step may or may not change, for idempotency checks.
    fn sso9_data_snapshot(conn: &Connection) -> Vec<Vec<String>> {
        let mut out = Vec::new();
        for sql in [
            "SELECT id, ifnull(okf_type,''), ifnull(deleted_at,''), updated_at, ifnull(ontology_checked_at,''), ifnull(heal_checked_at,'') FROM llm_wiki_entries ORDER BY id",
            "SELECT id, ifnull(okf_type,''), ifnull(deleted_at,''), updated_at, '', '' FROM llm_wiki_tasks ORDER BY id",
            "SELECT id, entity_type, ifnull(deleted_at,''), updated_at, '', '' FROM curated_entities ORDER BY id",
            "SELECT id, entity_id, source_id, target_id, edge_type, created_at FROM llm_wiki_edges ORDER BY id",
            "SELECT id, entity_id, table_name, record_id, payload, created_at FROM llm_wiki_outbox ORDER BY id",
            "SELECT id, proposal_id, item_type, payload, status, '' FROM curated_proposal_items ORDER BY id",
        ] {
            let mut stmt = conn.prepare(sql).unwrap();
            let rows = stmt
                .query_map([], |r| {
                    (0..6)
                        .map(|i| r.get::<_, rusqlite::types::Value>(i).map(|v| format!("{v:?}")))
                        .collect::<rusqlite::Result<Vec<_>>>()
                })
                .unwrap();
            out.extend(rows.map(|r| r.unwrap()));
        }
        out
    }

    #[test]
    fn sso9_retype_rewrites_entries_and_maps_handoff_for_to_about() {
        let (_tmp, conn, _log) = sso9_run_retype();
        assert_eq!(okf(&conn, "llm_wiki_entries", "h1").as_deref(), Some("decision"));
        assert_eq!(okf(&conn, "llm_wiki_entries", "s1").as_deref(), Some("lesson"));
        assert_eq!(etype(&conn, "e_remap").as_deref(), Some("about"));
        assert_eq!(etype(&conn, "e_ce_src").as_deref(), Some("about"));
        // Case variants are counted, never rewritten.
        assert_eq!(okf(&conn, "llm_wiki_entries", "hv").as_deref(), Some("Handoff"));
        assert!(sso9_version_stamped(&conn, SSO9_RETYPE_VERSION).unwrap());
    }

    #[test]
    fn sso9_retype_rewrites_tombstoned_entries() {
        let (_tmp, conn, _log) = sso9_run_retype();
        assert_eq!(okf(&conn, "llm_wiki_entries", "h_tomb").as_deref(), Some("decision"));
        assert_eq!(okf(&conn, "llm_wiki_entries", "s_tomb").as_deref(), Some("lesson"));
    }

    #[test]
    fn sso9_retype_rewrites_tasks_tombstones_included() {
        let (_tmp, conn, _log) = sso9_run_retype();
        assert_eq!(okf(&conn, "llm_wiki_tasks", "t_h").as_deref(), Some("decision"));
        assert_eq!(okf(&conn, "llm_wiki_tasks", "t_s").as_deref(), Some("lesson"));
        assert_eq!(okf(&conn, "llm_wiki_tasks", "t_c").as_deref(), Some("Session_Recap"));
    }

    #[test]
    fn sso9_retype_collision_keeps_the_existing_about_and_logs_the_dedupe() {
        let (_tmp, conn, log) = sso9_run_retype();
        assert_eq!(etype(&conn, "e_coll_ab").as_deref(), Some("about"), "pre-existing about survives");
        assert_eq!(etype(&conn, "e_coll_hf"), None, "the handoffFor loser is gone");
        assert!(
            log.iter().any(|l| l.contains("remapped=2") && l.contains("deduped=1")),
            "dedupe count logged: {log:#?}"
        );
    }

    #[test]
    fn sso9_retype_drops_service_targets_with_a_count() {
        let (_tmp, conn, log) = sso9_run_retype();
        assert_eq!(etype(&conn, "e_svc"), None);
        assert!(log.iter().any(|l| l.contains("handoffFor->service dropped=1")), "{log:#?}");
    }

    #[test]
    fn sso9_retype_drops_each_parent_family_class_and_keeps_about_organization() {
        let (_tmp, conn, log) = sso9_run_retype();
        for id in [
            "e_author",
            "e_publisher",
            "e_about_person",
            "e_about_place",
            "e_about_event",
            "e_supersedes",
        ] {
            assert_eq!(etype(&conn, id), None, "{id} must be dropped");
        }
        assert_eq!(etype(&conn, "e_about_org").as_deref(), Some("about"), "about->organization kept");
        assert!(
            log.iter().any(|l| l.contains("dropped by type=[about:3,author:2,publisher:1,supersedes:1]")),
            "per-type counts logged: {log:#?}"
        );
    }

    #[test]
    fn sso9_retype_curated_entity_source_endpoint_becomes_about() {
        let (_tmp, conn, _log) = sso9_run_retype();
        // F1/N3: handoffFor from a curated_entities row typed `handoff` to a
        // `software_application` target -> `about` (single expected result).
        assert_eq!(etype(&conn, "e_ce_src").as_deref(), Some("about"));
        assert_eq!(ce_type(&conn, "ce_h").as_deref(), Some("decision"));
    }

    #[test]
    fn sso9_retype_curated_entity_target_parent_family_edge_is_dropped() {
        let (_tmp, conn, _log) = sso9_run_retype();
        // N3: `author` (creativework family) whose TARGET is a re-typed
        // curated_entities row is dropped (counted in author:2 above).
        assert_eq!(etype(&conn, "e_author_ce_tgt"), None);
    }

    #[test]
    fn sso9_retype_leaves_undispositioned_edges_and_unrelated_edges_alone() {
        let (_tmp, conn, log) = sso9_run_retype();
        for id in ["e_undisp", "e_related", "e_unrelated", "e_coll_ab"] {
            assert!(etype(&conn, id).is_some(), "{id} must be untouched");
        }
        assert_eq!(etype(&conn, "e_undisp").as_deref(), Some("handoffFor"));
        assert!(log.iter().any(|l| l.contains("declared about->sa/area kept=1")), "{log:#?}");
        assert!(log.iter().any(|l| l.contains("undispositioned=2 (left untouched)")), "{log:#?}");
    }

    #[test]
    fn sso9_retype_leaves_updated_at_checked_at_and_outbox_byte_identical() {
        let (tmp, conn) = sso9_retype_brain();
        sso9_migrate(&conn, tmp.path()).unwrap();
        for (table, id) in [("llm_wiki_entries", "h1"), ("llm_wiki_entries", "h_tomb"), ("llm_wiki_entries", "s1")] {
            let (u, o, h): (i64, i64, i64) = conn
                .query_row(
                    &format!("SELECT updated_at, ontology_checked_at, heal_checked_at FROM {table} WHERE id = ?1"),
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .unwrap();
            assert_eq!((u, o, h), (77, 88, 99), "{id}");
        }
        for table in ["llm_wiki_tasks", "curated_entities"] {
            let n: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {table} WHERE updated_at <> 77"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 0, "{table}.updated_at untouched");
        }
        let (ob, ca): (String, i64) = conn
            .query_row(
                "SELECT payload, created_at FROM llm_wiki_outbox WHERE id = 'ob1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((ob.as_str(), ca), (r#"{"okf_type":"handoff"}"#, 3), "outbox untouched");
        let obn: i64 = conn
            .query_row("SELECT COUNT(*) FROM llm_wiki_outbox", [], |r| r.get(0))
            .unwrap();
        assert_eq!(obn, 1, "no outbox rows added");
        // Pending proposal payloads are counted, never rewritten.
        let pl: String = conn
            .query_row("SELECT payload FROM curated_proposal_items WHERE id = 'pi1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(pl, r#"{"type":"handoff"}"#);
    }

    #[test]
    fn sso9_retype_has_no_triggers_on_retyped_columns() {
        let (_tmp, conn, _log) = sso9_run_retype();
        let mut stmt = conn
            .prepare(
                "SELECT name, tbl_name, sql FROM sqlite_master WHERE type = 'trigger'
                  AND tbl_name IN ('llm_wiki_entries','llm_wiki_edges','llm_wiki_tasks','curated_entities')",
            )
            .unwrap();
        let triggers: Vec<(String, String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        for (name, tbl, sql) in &triggers {
            assert!(
                tbl == "llm_wiki_entries" && !sql.contains("okf_type"),
                "trigger {name} on {tbl} must not fire on okf_type/entity_type: {sql}"
            );
            let upper = sql.to_uppercase();
            assert!(
                !upper.contains("AFTER UPDATE ON") && !upper.contains("BEFORE UPDATE ON"),
                "unscoped UPDATE trigger {name} would fire on the re-type: {sql}"
            );
        }
    }

    #[test]
    fn sso9_retype_entity_types_rewrite_only_the_two_dropped_slugs() {
        let (_tmp, conn, log) = sso9_run_retype();
        assert_eq!(ce_type(&conn, "ce_h").as_deref(), Some("decision"));
        assert_eq!(ce_type(&conn, "ce_s").as_deref(), Some("lesson"), "tombstone included");
        for (id, t) in [
            ("ce_concept", "concept"),
            ("ce_process", "process"),
            ("ce_repo", "repository"),
            ("ce_job", "job"),
            ("ce_cv", "HANDOFF"),
        ] {
            assert_eq!(ce_type(&conn, id).as_deref(), Some(t), "{id} must be untouched");
        }
        assert!(
            log.iter().any(|l| l.contains("handoff->decision") && l.contains("curated_entities=1")),
            "{log:#?}"
        );
        let census = log.iter().find(|l| l.contains("retype census")).unwrap();
        assert!(census.contains("entities_handoff=1") && census.contains("entities_session_recap=1"));
    }

    #[test]
    fn sso9_retype_does_not_run_when_the_refresh_slot_is_unstamped() {
        let (_tmp, conn) = sso9_retype_brain();
        conn.execute("DELETE FROM schema_version WHERE version = ?1", [SSO9_REFRESH_VERSION]).unwrap();
        let before = sso9_data_snapshot(&conn);
        assert!(
            run_sso9_retype_txn(&conn, Path::new("no-backup-here"))
                .unwrap()
                .is_none()
        );
        assert!(!sso9_version_stamped(&conn, SSO9_RETYPE_VERSION).unwrap());
        assert_eq!(sso9_data_snapshot(&conn), before);
    }

    #[test]
    fn sso9_retype_is_idempotent_across_migrate_calls() {
        let (tmp, conn, _log) = sso9_run_retype();
        let data = sso9_data_snapshot(&conn);
        let stamps = sso9_snapshot(&conn).1;
        sso9_clear_log();
        sso9_migrate(&conn, tmp.path()).unwrap();
        assert_eq!(sso9_data_snapshot(&conn), data, "second run changes zero rows");
        assert_eq!(sso9_snapshot(&conn).1, stamps, "second run stamps nothing new");
        assert!(sso9_log_lines().iter().all(|l| !l.contains("backup ok")));
    }

    #[test]
    fn sso9_retype_body_is_idempotent_with_the_stamp_bypassed() {
        let (_tmp, conn, _log) = sso9_run_retype();
        let data = sso9_data_snapshot(&conn);
        conn.execute("DELETE FROM schema_version WHERE version = ?1", [SSO9_RETYPE_VERSION]).unwrap();
        let disp = run_sso9_retype_txn(&conn, Path::new("no-backup-here"))
            .unwrap()
            .expect("body ran");
        assert!(disp.remapped.is_empty() && disp.dropped.is_empty());
        assert_eq!(sso9_data_snapshot(&conn), data, "zero rows change on a second body run");
        assert!(sso9_version_stamped(&conn, SSO9_RETYPE_VERSION).unwrap());
    }

    #[test]
    fn sso9_retype_keeps_parent_edges_the_refreshed_manifest_still_declares() {
        let (tmp, conn) = sso9_retype_brain();
        // Impl-review (owxi): ent1's manifest declares the POST-re-type shape
        // author(decision, person); the refresh preserves the custom
        // declaration, so the re-type must KEEP the author edge on the
        // re-typed row. The stock-refreshed manifest declares no such shape,
        // so the same edge still drops there (standard-fixture tests).
        let mut m: serde_json::Value = serde_json::from_str(SSO8_MANIFEST).unwrap();
        m["edge_types"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "type": "author", "source_type": "decision", "target_type": "person"
            }));
        conn.execute(
            "INSERT OR REPLACE INTO llm_wiki_entity_manifests (entity_id, mode, manifest_json, updated_at)
             VALUES ('ent1', 'strict', ?1, 1)",
            [&m.to_string()],
        )
        .unwrap();
        sso9_clear_log();
        sso9_migrate(&conn, tmp.path()).unwrap();
        assert_eq!(
            etype(&conn, "e_author").as_deref(),
            Some("author"),
            "declared post-re-type parent edge kept"
        );
        let log = sso9_log_lines();
        assert!(
            log.iter()
                .any(|l| l.contains("parent edges kept as declared post-retype=1")),
            "{log:#?}"
        );
        // The undeclared publisher edge on the same brain still drops.
        assert_eq!(etype(&conn, "e_publisher"), None);
    }

    #[test]
    fn sso9_ensure_fold_failure_rolls_back_the_refresh_slot() {
        let (tmp, conn) = sso9_rewound_brain();
        // Impl-review (owxz): break the ensure's durable memo write. With the
        // fold, the refresh transaction must roll back WHOLE — the stamp and
        // the ensure edits commit together or not at all (pre-fold, the stamp
        // landed and the ensure failure was only logged, leaving the manifest
        // unensured forever). The memo table is SCHEMA-mangled (not dropped):
        // migrate's own table ensure only recreates a MISSING table, so the
        // mangled one survives into the step and record_ensure_memo fails.
        let ddl: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name = 'manifest_ensure_memo'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute_batch(
            "DROP TABLE manifest_ensure_memo;
             CREATE TABLE manifest_ensure_memo (entity_id TEXT PRIMARY KEY, recorded_at TEXT);",
        )
        .unwrap();
        let before: String = conn
            .query_row(
                "SELECT manifest_json FROM llm_wiki_entity_manifests WHERE entity_id = 'tier_fact'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let result = sso9_migrate(&conn, tmp.path());
        let err = result.expect_err("a failed ensure fold must abort migrate()");
        assert!(!sso9_stamped(&conn), "refresh slot must NOT be stamped: {err:#}");
        let after: String = conn
            .query_row(
                "SELECT manifest_json FROM llm_wiki_entity_manifests WHERE entity_id = 'tier_fact'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(after, before, "the manifest refresh rolls back with the fold");
        assert_eq!(sso9_backups(tmp.path()).len(), 1, "backup kept for the retry");
        // Recreate the memo table; the next open retries and succeeds.
        conn.execute_batch(&format!("DROP TABLE manifest_ensure_memo; {};", ddl))
            .unwrap();
        sso9_migrate(&conn, tmp.path()).unwrap();
        assert!(sso9_stamped(&conn));
    }

    #[test]
    fn sso9_retype_late_edge_write_between_backup_and_delete_aborts_the_retype() {
        let (tmp, conn) = sso9_retype_brain();
        // Impl-review (owxv): a concurrent writer commits a service-target
        // handoffFor edge AFTER the backup snapshot — V32 must not delete a
        // row the retained backup never held. The re-type aborts unapplied.
        SSO9_AFTER_REFRESH_COMMIT.with(|h| {
            *h.borrow_mut() = Some(Box::new(|c: &Connection| {
                c.execute(
                    "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
                     VALUES ('e_late','ent1','s1','svc1','handoffFor',1)",
                    [],
                )
                .unwrap();
            }))
        });
        sso9_clear_log();
        let result = sso9_migrate(&conn, tmp.path());
        SSO9_AFTER_REFRESH_COMMIT.with(|h| *h.borrow_mut() = None);
        let err = result.expect_err("a late edge write must abort the re-type before any delete");
        assert!(
            format!("{err:#}").contains("between the backup and the re-type"),
            "{err:#}"
        );
        assert!(
            !sso9_version_stamped(&conn, SSO9_RETYPE_VERSION).unwrap(),
            "re-type slot unstamped"
        );
        assert!(
            sso9_version_stamped(&conn, SSO9_REFRESH_VERSION).unwrap(),
            "the refresh committed before the hook fired"
        );
        assert_eq!(sso9_backups(tmp.path()).len(), 1, "backup preserved");
        assert!(etype(&conn, "e_svc").is_some(), "no delete happened");
        // The next open retries cleanly with a fresh backup.
        sso9_migrate(&conn, tmp.path()).unwrap();
        assert!(sso9_version_stamped(&conn, SSO9_RETYPE_VERSION).unwrap());
        assert_eq!(etype(&conn, "e_svc"), None, "retype drops the service edge");
    }

    #[test]
    fn sso9_retype_equal_count_late_swap_aborts_the_retype() {
        let (tmp, conn) = sso9_retype_brain();
        // Impl-review (rOZt): count equality missed EQUAL-COUNT drift. A late
        // writer that removes one unrelated edge and adds a service-target
        // `handoffFor` between the backup and the write lock keeps the counts
        // equal; the count guard passed and the re-type then deleted a row
        // the retained backup never held. The per-row backup check must
        // abort instead.
        SSO9_AFTER_REFRESH_COMMIT.with(|h| {
            *h.borrow_mut() = Some(Box::new(|c: &Connection| {
                c.execute("DELETE FROM llm_wiki_edges WHERE id = 'e_unrelated'", [])
                    .unwrap();
                c.execute(
                    "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
                     VALUES ('e_late_swap','ent1','s1','svc1','handoffFor',1)",
                    [],
                )
                .unwrap();
            }))
        });
        sso9_clear_log();
        let result = sso9_migrate(&conn, tmp.path());
        SSO9_AFTER_REFRESH_COMMIT.with(|h| *h.borrow_mut() = None);
        let err = result.expect_err("an equal-count late swap must abort the re-type");
        assert!(
            format!("{err:#}").contains("between the backup and the re-type"),
            "{err:#}"
        );
        assert!(
            !sso9_version_stamped(&conn, SSO9_RETYPE_VERSION).unwrap(),
            "re-type slot unstamped"
        );
        assert_eq!(sso9_backups(tmp.path()).len(), 1, "backup preserved");
        assert!(etype(&conn, "e_svc").is_some(), "no delete happened");
        // The next open retries cleanly with a fresh backup.
        sso9_migrate(&conn, tmp.path()).unwrap();
        assert!(sso9_version_stamped(&conn, SSO9_RETYPE_VERSION).unwrap());
        assert_eq!(
            etype(&conn, "e_late_swap"),
            None,
            "the fresh backup covers the late row"
        );
    }

    #[test]
    fn sso9_retype_remap_only_late_write_aborts_the_retype() {
        let (tmp, conn) = sso9_retype_brain();
        // Impl-review (rOZh): the old count guard armed only when a DROP was
        // planned. A remap-eligible `handoffFor` committed between the backup
        // and the write lock reaches the remap loop with no drops planned —
        // its UPDATE OR IGNORE loses to the pre-existing `about` row on the
        // same tuple and the dedupe DELETE destroys a row the backup never
        // held. Strip the fixture's drop-class edges so the plan carries
        // remap rows ONLY (the guard-disarm precondition), then commit the
        // late write in the window.
        conn.execute_batch(
            "DELETE FROM llm_wiki_edges WHERE edge_type IN ('author','publisher','supersedes');
             DELETE FROM llm_wiki_edges WHERE edge_type = 'handoffFor'
               AND id NOT IN ('e_coll_hf','e_remap');
             DELETE FROM llm_wiki_edges WHERE edge_type = 'about'
               AND target_id NOT IN ('sw1','org1');
             INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
               VALUES ('e_pre_tomb','ent1','h_tomb','sw1','about',1);",
        )
        .unwrap();
        SSO9_AFTER_REFRESH_COMMIT.with(|h| {
            *h.borrow_mut() = Some(Box::new(|c: &Connection| {
                c.execute(
                    "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
                     VALUES ('e_late_remap','ent1','h_tomb','sw1','handoffFor',1)",
                    [],
                )
                .unwrap();
            }))
        });
        sso9_clear_log();
        let result = sso9_migrate(&conn, tmp.path());
        SSO9_AFTER_REFRESH_COMMIT.with(|h| *h.borrow_mut() = None);
        let err = result.expect_err("a late remap row must abort before any delete");
        assert!(
            format!("{err:#}").contains("between the backup and the re-type"),
            "{err:#}"
        );
        assert!(
            !sso9_version_stamped(&conn, SSO9_RETYPE_VERSION).unwrap(),
            "re-type slot unstamped"
        );
        assert_eq!(sso9_backups(tmp.path()).len(), 1, "backup preserved");
        assert!(
            etype(&conn, "e_remap").is_some(),
            "no delete happened"
        );
        // The next open retries cleanly with a fresh backup that holds the
        // late row: the remap loses to the pre-existing about row and the
        // dedupe DELETE runs against a row the backup now covers.
        sso9_migrate(&conn, tmp.path()).unwrap();
        assert!(sso9_version_stamped(&conn, SSO9_RETYPE_VERSION).unwrap());
        assert_eq!(etype(&conn, "e_late_remap"), None, "remapped row deduped");
        assert_eq!(
            etype(&conn, "e_pre_tomb").as_deref(),
            Some("about"),
            "the pre-existing about row is kept"
        );
    }

    #[test]
    fn sso9_retype_keeps_parent_edges_declared_by_the_inherited_partition_manifest() {
        let (tmp, conn) = sso9_retype_brain();
        // Impl-review (rOZa): ent1 has NO manifest row of its own — the
        // production-typical case. The gate's two-lookup cascade
        // (`resolve_strict_edge_vocabulary_with`) inherits the canonical
        // `tier_fact` partition manifest; the re-type's declared-shape check
        // must consult the same cascade, or a shape the inherited manifest
        // declares is read as undeclared and a legal edge is deleted.
        let mut m: serde_json::Value = serde_json::from_str(SSO8_MANIFEST).unwrap();
        m["edge_types"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "type": "author", "source_type": "decision", "target_type": "person"
            }));
        let no_ent1_row: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_entity_manifests WHERE entity_id = 'ent1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            no_ent1_row, 0,
            "fixture: ent1 must have no manifest row of its own"
        );
        conn.execute(
            "UPDATE llm_wiki_entity_manifests SET manifest_json = ?1 WHERE entity_id = 'tier_fact'",
            [&m.to_string()],
        )
        .unwrap();
        sso9_clear_log();
        sso9_migrate(&conn, tmp.path()).unwrap();
        assert_eq!(
            etype(&conn, "e_author").as_deref(),
            Some("author"),
            "inherited-declared parent edge kept"
        );
        assert_eq!(
            etype(&conn, "e_publisher"),
            None,
            "genuinely undeclared shapes still drop"
        );
    }

    #[test]
    fn sso9_retype_parent_shape_mixed_case_target_is_predicted_as_stored() {
        let (tmp, conn) = sso9_retype_brain();
        // Impl-review (rOZe): the declared-shape check must predict what the
        // SQL actually writes. The re-type updates EXACT slugs (`WHERE
        // okf_type = 'handoff'`), so the mixed-case `Handoff` row (`hv`) is
        // NOT retyped — an edge landing on it stays (…, handoff) after the
        // migration and must be adjudicated by its STORED type, not by the
        // lowered-and-mapped one. The exact-slug retype scope itself is
        // pinned (`entity_types_rewrite_only_the_two_dropped_slugs`) and
        // untouched here.
        let mut m: serde_json::Value = serde_json::from_str(SSO8_MANIFEST).unwrap();
        m["edge_types"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "type": "supersedes", "source_type": "decision", "target_type": "decision"
            }));
        conn.execute(
            "INSERT OR REPLACE INTO llm_wiki_entity_manifests (entity_id, mode, manifest_json, updated_at)
             VALUES ('ent1', 'strict', ?1, 1)",
            [&m.to_string()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
             VALUES ('e_sup_hv','ent1','h1','hv','supersedes',1)",
            [],
        )
        .unwrap();
        sso9_clear_log();
        sso9_migrate(&conn, tmp.path()).unwrap();
        assert_eq!(
            etype(&conn, "e_sup_hv"),
            None,
            "the target stays stored-typed 'Handoff' (exact-slug SQL) — the shape is undeclared"
        );
        // The lowercase twin keeps adjudicating as declared (unchanged).
        assert_eq!(etype(&conn, "e_supersedes").as_deref(), Some("supersedes"));
    }

    #[test]
    fn sso9_shape_check_covers_every_refreshed_row() {
        let (_tmp, conn) = sso9_rewound_brain();
        // Impl-review (T1.5 rung 1 nit 1): the shape check verifies the
        // COMMIT against drift; inspecting only `refreshed.first()` left
        // every other refreshed manifest unchecked — drift or a tamper in
        // row two went unseen. The first row must pass while the second
        // row's drift is caught.
        let planned = vec![
            (
                "tier_fact".to_string(),
                r#"{"node_types":[],"edge_types":[]}"#.to_string(),
            ),
            (
                "tier_wisdom".to_string(),
                r#"{"node_types":["person"]}"#.to_string(),
            ),
        ];
        conn.execute(
            "INSERT OR REPLACE INTO llm_wiki_entity_manifests (entity_id, mode, manifest_json, updated_at)
             VALUES ('tier_fact', 'strict', '{\"node_types\":[],\"edge_types\":[]}', 1)",
            [],
        )
        .unwrap();
        // Stored content DIFFERS from the planned `new_json` for row two.
        conn.execute(
            "INSERT OR REPLACE INTO llm_wiki_entity_manifests (entity_id, mode, manifest_json, updated_at)
             VALUES ('tier_wisdom', 'strict', '{\"node_types\":[\"person\",\"drifted\"]}', 1)",
            [],
        )
        .unwrap();
        let failure = sso9_shape_check(&conn, &planned).unwrap();
        assert!(
            failure
                .as_deref()
                .map(|f| f.contains("tier_wisdom"))
                .unwrap_or(false),
            "the second refreshed row's drift must be caught: {failure:?}"
        );
        // The passing first row contributes no failure text.
        assert!(
            failure
                .as_deref()
                .map(|f| !f.contains("tier_fact"))
                .unwrap_or(false),
            "the matching first row must not be flagged: {failure:?}"
        );
    }

    #[test]
    fn sso9_retype_parent_shape_trailing_space_target_is_predicted_as_stored() {
        let (tmp, conn) = sso9_retype_brain();
        // Impl-review (slx2): the resolver must return the stored value
        // VERBATIM. The SQL re-type matches exact slugs, so a stored `handoff `
        // (trailing space) is NOT retyped — its post-migration key stays
        // `handoff`. Mapping the TRIMMED value predicted `decision` and would
        // drop an edge whose stored shape the manifest still declares.
        let mut m: serde_json::Value = serde_json::from_str(SSO8_MANIFEST).unwrap();
        m["edge_types"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "type": "supersedes", "source_type": "decision", "target_type": "handoff"
            }));
        conn.execute(
            "INSERT OR REPLACE INTO llm_wiki_entity_manifests (entity_id, mode, manifest_json, updated_at)
             VALUES ('ent1', 'strict', ?1, 1)",
            [&m.to_string()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_entries (id, entity_id, title, body, created_at, updated_at, okf_type, deleted_at, ontology_checked_at, heal_checked_at)
             VALUES ('hsp','ent1','t','b',1,77,'handoff ',NULL,88,99)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
             VALUES ('e_sup_sp','ent1','h1','hsp','supersedes',1)",
            [],
        )
        .unwrap();
        sso9_clear_log();
        sso9_migrate(&conn, tmp.path()).unwrap();
        assert_eq!(
            etype(&conn, "e_sup_sp").as_deref(),
            Some("supersedes"),
            "the target stays stored-typed 'handoff ' (exact-slug SQL) — key 'handoff' is declared"
        );
    }

    #[test]
    fn sso9_retype_unreadable_partition_manifest_aborts_the_retype() {
        let (tmp, conn) = sso9_retype_brain();
        // Impl-review (slyA, refined by the fresh round's t93k finding): an
        // unreadable APPLICABLE manifest must not classify shapes as
        // undeclared — the re-type deletes on "undeclared" and the backup is
        // removed after its checks pass. A malformed OWNER row is
        // indistinguishable from a non-strict one (the JSON is parsed before
        // the mode is read), so the gate's own Err leg falls through and the
        // PARTITION manifest governs — mirror that. Corrupt ent1's manifest:
        // the re-type must SUCCEED with tier_fact governing; then corrupt
        // tier_fact itself: the re-type must ABORT, not delete.
        conn.execute(
            "INSERT INTO llm_wiki_entity_manifests (entity_id, mode, manifest_json, updated_at)
             VALUES ('ent1', 'strict', 'not json at all', 1)",
            [],
        )
        .unwrap();
        sso9_clear_log();
        sso9_migrate(&conn, tmp.path()).unwrap();
        assert!(sso9_version_stamped(&conn, SSO9_RETYPE_VERSION).unwrap());
        assert_eq!(
            etype(&conn, "e_author"),
            None,
            "the partition manifest governs a malformed owner row (gate-mirrored)"
        );
        // Now corrupt the APPLICABLE partition manifest and re-open the
        // migration (slots already stamped — clear them to force a retry).
        conn.execute(
            "UPDATE llm_wiki_entity_manifests SET manifest_json = 'still not json' WHERE entity_id = 'tier_fact'",
            [],
        )
        .unwrap();
        conn.execute(
            "DELETE FROM schema_version WHERE version = ?1 OR version = ?2",
            [SSO9_REFRESH_VERSION, SSO9_RETYPE_VERSION],
        )
        .unwrap();
        // Phase 1 dropped every parent-family edge AND retyped every
        // handoff/session_recap row (the snapshot is empty) — re-seed a
        // handoff-typed entry with an author edge so the plan actually
        // consults the (now corrupt) applicable manifest.
        conn.execute(
            "INSERT INTO llm_wiki_entries (id, entity_id, title, body, created_at, updated_at, okf_type, deleted_at, ontology_checked_at, heal_checked_at)
             VALUES ('h2','ent1','t','b',1,77,'handoff',NULL,88,99)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
             VALUES ('e_author2','ent1','h2','per1','author',1)",
            [],
        )
        .unwrap();
        sso9_clear_log();
        let result = sso9_migrate(&conn, tmp.path());
        let err = result.expect_err("an unreadable partition manifest must abort the re-type");
        assert!(
            format!("{err:#}")
                .contains("cannot read the manifest for tier_fact"),
            "{err:#}"
        );
        assert!(
            !sso9_version_stamped(&conn, SSO9_RETYPE_VERSION).unwrap(),
            "re-type slot unstamped"
        );
        assert_eq!(sso9_backups(tmp.path()).len(), 1, "backup preserved");
    }

    #[test]
    fn sso9_retype_ungated_entity_keeps_parent_family_edges() {
        let (tmp, conn) = sso9_retype_brain();
        // Impl-review (T1.5 rung 2 MAJOR 1): an entity with NO applicable
        // strict manifest on either leg is UNGATED — the gate admits every
        // shape on such a brain (`ontology_leg`'s "ungated means most
        // permissive" contract), and the success path deletes the backup, so
        // the re-type must not delete parent-family edges the live gate
        // admits. Remove the tier_fact partition manifest (ent1 has no
        // manifest row of its own in the fixture).
        conn.execute(
            "DELETE FROM llm_wiki_entity_manifests WHERE entity_id = 'tier_fact'",
            [],
        )
        .unwrap();
        sso9_clear_log();
        sso9_migrate(&conn, tmp.path()).unwrap();
        assert_eq!(
            etype(&conn, "e_author").as_deref(),
            Some("author"),
            "ungated brain keeps parent-family edges"
        );
        assert_eq!(
            etype(&conn, "e_publisher").as_deref(),
            Some("publisher"),
            "ungated brain keeps parent-family edges"
        );
        // Impl-review (T1.5 rung 3 BLOCKER 1): the about arm gets the same
        // ungated consult — a person-target about edge is kept on an ungated
        // brain (the ratified default brain still drops it: its refreshed
        // manifest declares no about(decision|lesson, person) shape).
        assert_eq!(
            etype(&conn, "e_about_person").as_deref(),
            Some("about"),
            "ungated brain keeps about edges to person endpoints too"
        );
        // The handoffFor dispositions are NOT manifest-gated and still run.
        assert_eq!(
            etype(&conn, "e_svc"),
            None,
            "service drops are unconditional, not gated by manifests"
        );
    }

    #[test]
    fn sso9_retype_null_endpoint_parent_edges_are_kept_and_counted() {
        let (tmp, conn) = sso9_retype_brain();
        // Owner ruling (Kurt, 2026-11-25, the t93c open question): the live
        // gate climbs endpoints by OWNING ENTITY and never reads okf_type
        // NULLness, so an edge whose endpoint type resolves to nothing is
        // still ADMITTED at runtime — the re-type must KEEP it and log the
        // count, not drop it (the success path deletes the backup, so a
        // wrong drop is irreversible). Fixture: an author edge from a
        // snapshot row to a NULL-okf_type entry, under the stock strict
        // partition manifest — had the types resolved, the shape would be
        // UNDECLARED there (author(decision, person)); the keep below is
        // the NULL ruling, not the declared-shape rule. (The `about` arm
        // shares this consult but cannot produce a NULL verdict today: a
        // NULL target falls to `undispositioned` before the consult, and a
        // snapshot source always resolves.)
        conn.execute_batch(
            "INSERT INTO llm_wiki_entries (id, entity_id, title, body, created_at, updated_at, okf_type, deleted_at, ontology_checked_at, heal_checked_at) VALUES
               ('null_e','ent1','t','b',1,77,NULL,NULL,88,99);
             INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at) VALUES
               ('e_null_author','ent1','h1','null_e','author',1);",
        )
        .unwrap();
        sso9_clear_log();
        sso9_migrate(&conn, tmp.path()).unwrap();
        assert_eq!(
            etype(&conn, "e_null_author").as_deref(),
            Some("author"),
            "author edge with a NULL-typed endpoint must survive the re-type"
        );
        let log = sso9_log_lines();
        assert!(
            log.iter()
                .any(|l| l.contains("unresolved-endpoint parent edges kept=1")),
            "the kept NULL-endpoint count must be logged: {log:#?}"
        );
        // The undeclared publisher edge on the same brain still drops —
        // the ruling widens NOTHING except the unresolved-endpoint class.
        assert_eq!(etype(&conn, "e_publisher"), None);
    }

    #[test]
    fn sso9_retype_null_endpoint_ungated_keeps_are_counted_too() {
        let (tmp, conn) = sso9_retype_brain();
        // CodeRabbit round on fa887299 (CHk): on an UNGATED brain the
        // consult used to return `Declared` before resolving endpoint
        // types, so a NULL-endpoint parent edge was kept WITHOUT the E6
        // count — the ruling says keep AND log the count, on every path.
        // The ungated/empty-vocabulary keep outcome itself must not change,
        // and the E7 applicable-partition Err abort must still fire first.
        conn.execute(
            "DELETE FROM llm_wiki_entity_manifests WHERE entity_id = 'tier_fact'",
            [],
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO llm_wiki_entries (id, entity_id, title, body, created_at, updated_at, okf_type, deleted_at, ontology_checked_at, heal_checked_at) VALUES
               ('null_e','ent1','t','b',1,77,NULL,NULL,88,99);
             INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at) VALUES
               ('e_null_author','ent1','h1','null_e','author',1);",
        )
        .unwrap();
        sso9_clear_log();
        sso9_migrate(&conn, tmp.path()).unwrap();
        assert_eq!(
            etype(&conn, "e_null_author").as_deref(),
            Some("author"),
            "ungated brain still keeps the NULL-endpoint edge"
        );
        let log = sso9_log_lines();
        assert!(
            log.iter()
                .any(|l| l.contains("unresolved-endpoint parent edges kept=1")),
            "the E6 count must cover ungated keeps too: {log:#?}"
        );
    }

    #[test]
    fn sso9_retype_manifest_restored_after_retype_commit_keeps_the_backup() {
        let (tmp, conn) = sso9_retype_brain();
        // CodeRabbit round on fa887299 (CH0): the shape check ran once,
        // between the refresh COMMIT and the re-type; a manifest row
        // restored to its pre-refresh JSON after the re-type commit would
        // pass the remaining verification (stamps, stale slugs,
        // dispositioned edges) and the backup would be deleted while the
        // stored manifest differs from the planned refresh. Re-run the
        // comparison after the re-type commit and keep the backup on drift.
        let before: String = conn
            .query_row(
                "SELECT manifest_json FROM llm_wiki_entity_manifests WHERE entity_id = 'tier_fact'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        SSO9_AFTER_RETYPE_COMMIT.with(|h| {
            *h.borrow_mut() = Some(Box::new(move |c: &Connection| {
                c.execute(
                    "UPDATE llm_wiki_entity_manifests SET manifest_json = ?1
                     WHERE entity_id = 'tier_fact'",
                    [&before],
                )
                .unwrap();
            }));
        });
        sso9_clear_log();
        let result = sso9_migrate(&conn, tmp.path());
        SSO9_AFTER_RETYPE_COMMIT.with(|h| *h.borrow_mut() = None);
        // Verification failure is not fatal: CRITICAL log, backup KEPT.
        result.expect("migrate() completes; verification failure keeps the backup");
        assert_eq!(
            sso9_backups(tmp.path()).len(),
            1,
            "backup kept when the post-retype manifest re-check fails"
        );
        let log = sso9_log_lines();
        assert!(
            log.iter().any(|l| l.contains("verification FAILED")),
            "{log:#?}"
        );
        assert!(
            log.iter().all(|l| !l.contains("backup deleted")),
            "no deletion on manifest drift: {log:#?}"
        );
    }

    #[test]
    fn sso9_retype_strict_empty_edge_vocabulary_keeps_parent_family_edges() {
        let (tmp, conn) = sso9_retype_brain();
        // Impl-review (T1.5 rung 2 MAJOR 1, empty-vocabulary leg): a strict
        // manifest with an EMPTY edge_types array disarms the gate
        // (`ontology_leg` returns Strict(None)) — the re-type must keep the
        // parent-family edges instead of reading the empty set as
        // "everything undeclared". The manifest is deliberately NOT eligible
        // for the refresh (missing the old-17 node slugs) so it is stored
        // verbatim and the empty set survives to the re-type.
        let m = serde_json::json!({
            "node_types": ["person"],
            "edge_types": [],
        });
        conn.execute(
            "INSERT OR REPLACE INTO llm_wiki_entity_manifests (entity_id, mode, manifest_json, updated_at)
             VALUES ('ent1', 'strict', ?1, 1)",
            [&m.to_string()],
        )
        .unwrap();
        sso9_clear_log();
        sso9_migrate(&conn, tmp.path()).unwrap();
        assert_eq!(
            etype(&conn, "e_author").as_deref(),
            Some("author"),
            "a gate-disarming manifest keeps parent-family edges"
        );
    }

    #[test]
    fn sso9_success_prunes_stale_backups_keeping_the_newest_three() {
        let (tmp, conn) = sso9_rewound_brain();
        // Impl-review (T1.5 rung 2 MINOR 2): a persistently failing open
        // accumulated one full-DB backup per launch — the success path only
        // ever removed its own file. It now prunes stale siblings, keeping
        // the newest three (each may be a crash-recovery copy).
        for ts in [10, 11, 12, 13, 14] {
            std::fs::write(tmp.path().join(format!("brain.db.bak-pre-sso9-{ts}")), b"x")
                .unwrap();
        }
        sso9_migrate(&conn, tmp.path()).unwrap();
        let left = sso9_backups(tmp.path());
        assert_eq!(left.len(), 3, "keep the newest three stale backups: {left:?}");
        let names: Vec<String> = left
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .collect();
        assert!(
            names.iter().all(|n| n.ends_with("-12") || n.ends_with("-13") || n.ends_with("-14")),
            "the OLDEST stale backups are the pruned ones: {names:?}"
        );
    }

    #[test]
    fn sso9_retype_parent_declared_shape_keeps_the_edge() {
        let (tmp, conn) = sso9_retype_brain();
        // Impl-review (T3 MAJOR 1): the manifest declares statement shapes on
        // the ABSTRACT parent — `decision` carries `parent_type: statement`
        // and the statement description says source-`statement` rows "apply
        // to every kind" — and spec change-6 itself keeps about->organization
        // as "a valid statement->organization shape". A custom manifest that
        // declares about(statement, person) therefore DECLARES the re-typed
        // (about, decision, person) shape, and the edge must be KEPT — exact
        // triple matching missed the parent convention and deleted the edge.
        let mut m: serde_json::Value = serde_json::from_str(SSO8_MANIFEST).unwrap();
        m["edge_types"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "type": "about", "source_type": "statement", "target_type": "person"
            }));
        conn.execute(
            "INSERT OR REPLACE INTO llm_wiki_entity_manifests (entity_id, mode, manifest_json, updated_at)
             VALUES ('ent1', 'strict', ?1, 1)",
            [&m.to_string()],
        )
        .unwrap();
        sso9_clear_log();
        sso9_migrate(&conn, tmp.path()).unwrap();
        assert_eq!(
            etype(&conn, "e_about_person").as_deref(),
            Some("about"),
            "parent-declared shape about(statement, person) keeps the re-typed edge"
        );
    }

    #[test]
    fn sso9_verification_c_surviving_remap_with_a_moved_target_fires() {
        // Impl-review (owxn): the surviving row must match the captured
        // (entity, source, target), not just its type.
        sso9_assert_retype_verification_fires(
            |c| {
                c.execute(
                    // (ent1, s1, pl1, about) is collision-free in the fixture
                    // (e_about_place is h1->pl1; e_about_org is s1->org1).
                    "UPDATE llm_wiki_edges SET target_id = 'pl1' WHERE id = 'e_remap'",
                    [],
                )
                .unwrap();
            },
            "(c) step-2 re-mapped edge e_remap",
        );
    }

    #[test]
    fn sso9_backup_collision_never_deletes_the_other_openers_file() {
        let (tmp, conn) = sso9_rewound_brain();
        // Impl-review (owxT): a concurrent opener wins the race to the same
        // millisecond — their backup file exists by the time this opener
        // reaches the backup step. This opener must fail WITHOUT deleting it.
        SSO9_BACKUP_TS_OVERRIDE.with(|c| c.set(Some(42)));
        let other = tmp.path().join("brain.db.bak-pre-sso9-42");
        SSO9_BACKUP_RACE_HOOK.with(|c| {
            c.set(Some(|p: &Path| {
                std::fs::write(p, b"other-openers-snapshot").unwrap();
            }))
        });
        let before = sso9_snapshot(&conn);
        let result = sso9_migrate(&conn, tmp.path());
        SSO9_BACKUP_RACE_HOOK.with(|c| c.set(None));
        SSO9_BACKUP_TS_OVERRIDE.with(|c| c.set(None));
        let err = result.expect_err("the lost race must abort the migration");
        assert!(format!("{err:#}").contains("sso9 backup"), "{err:#}");
        assert_eq!(
            std::fs::read(&other).unwrap(),
            b"other-openers-snapshot",
            "the other opener's backup is untouched"
        );
        assert_eq!(sso9_snapshot(&conn), before);
    }

    #[test]
    fn sso9_retype_census_log_carries_every_count() {
        let (_tmp, conn) = sso9_retype_brain();
        sso9_clear_log();
        let before = sso9_data_snapshot(&conn);
        sso9_retype_census(&conn).unwrap();
        assert_eq!(sso9_data_snapshot(&conn), before, "dry pass must not write");
        let log = sso9_log_lines();
        let line = log.iter().find(|l| l.contains("retype census")).unwrap();
        for want in [
            "entries_handoff_live=1",
            "entries_handoff_tombstoned=1",
            "entries_session_recap_live=1",
            "entries_session_recap_tombstoned=1",
            "tasks_handoff_live=1",
            "tasks_handoff_tombstoned=0",
            "tasks_session_recap_live=0",
            "tasks_session_recap_tombstoned=1",
            "case_variant_entries=1",
            "case_variant_tasks=1",
            "case_variant_entities=1",
            "entities_handoff=1",
            "entities_session_recap=1",
            "handoffFor_by_target=[organization:1,service:1,software_application:3]",
            "edges_touching_by_type=[about:5,author:2,handoffFor:5,publisher:1,relatedTo:1,supersedes:1]",
            "planned_remap=3",
            "planned_service_drop=1",
            "planned_parent_family_drop=7",
            "kept_about_organization=1",
            "kept_declared_about=1",
            "undispositioned=2",
            "pending_proposal_items_with_dropped_types=1",
            "pending_proposal_items_with_handoffFor=1",
        ] {
            assert!(line.contains(want), "census line missing `{want}`: {line}");
        }
    }

    #[test]
    fn sso9_retype_backup_is_gone_after_both_steps_and_the_log_says_so() {
        let (tmp, _conn, log) = sso9_run_retype();
        assert!(sso9_backups(tmp.path()).is_empty());
        assert!(log.iter().any(|l| l.contains("backup deleted: path=")), "{log:#?}");
    }

    #[test]
    fn sso9_retype_one_migrate_call_creates_exactly_one_backup() {
        // Proven from the LOG LINES only (the post-migrate listing is empty).
        let (_tmp, _conn, log) = sso9_run_retype();
        assert_eq!(log.iter().filter(|l| l.contains("sso9 backup ok:")).count(), 1, "{log:#?}");
        assert!(log.iter().any(|l| l.contains("manifest refresh committed")));
        assert!(log.iter().any(|l| l.contains("sso9 retype committed")));
    }

    #[test]
    fn sso9_retype_deletion_failure_is_logged_and_never_fatal() {
        let (tmp, conn) = sso9_retype_brain();
        SSO9_DELETE_OVERRIDE.with(|c| {
            c.set(Some(|_p: &Path| Err(std::io::Error::other("injected delete failure"))))
        });
        sso9_clear_log();
        let result = sso9_migrate(&conn, tmp.path());
        SSO9_DELETE_OVERRIDE.with(|c| c.set(None));
        result.expect("the open still succeeds");
        assert!(sso9_version_stamped(&conn, SSO9_RETYPE_VERSION).unwrap());
        assert!(
            sso9_log_lines().iter().any(|l| l.contains("could not be deleted") && l.contains("injected delete failure")),
            "{:#?}",
            sso9_log_lines()
        );
        assert_eq!(sso9_backups(tmp.path()).len(), 1, "the undeleted backup remains");
    }

    /// Runs the full migrate with `hook` firing after the re-type COMMIT and
    /// asserts the verification-failure contract: backup KEPT, CRITICAL line
    /// names the path and the failed check, the open still succeeds.
    fn sso9_assert_retype_verification_fires(hook: impl Fn(&Connection) + 'static, marker: &str) {
        let (tmp, conn) = sso9_retype_brain();
        SSO9_AFTER_RETYPE_COMMIT.with(|h| *h.borrow_mut() = Some(Box::new(hook)));
        sso9_clear_log();
        let result = sso9_migrate(&conn, tmp.path());
        SSO9_AFTER_RETYPE_COMMIT.with(|h| *h.borrow_mut() = None);
        result.expect("a verification failure is not propagated");
        let backups = sso9_backups(tmp.path());
        assert_eq!(backups.len(), 1, "backup KEPT on the verification-failure path");
        let name = backups[0].file_name().unwrap().to_string_lossy().to_string();
        let log = sso9_log_lines();
        let crit = log
            .iter()
            .find(|l| l.contains("CRITICAL"))
            .unwrap_or_else(|| panic!("no CRITICAL line: {log:#?}"));
        assert!(crit.contains(&name), "CRITICAL names the backup path: {crit}");
        assert!(crit.contains(marker), "CRITICAL names `{marker}`: {crit}");
        assert!(log.iter().all(|l| !l.contains("backup deleted")), "no deletion on failure");
        // The kept backup's row counts are proven from the log line.
        for (t, a, b) in sso9_backup_log_counts() {
            assert_eq!(a, b, "{t}");
        }
    }

    // (a) STAMPS is EXEMPT from a firing test (S4): the stamps commit in the
    // same transaction as the data, so a missed stamp is a rollback, not a
    // verifiable post-state.

    #[test]
    fn sso9_verification_b_stale_slug_after_commit_fires() {
        sso9_assert_retype_verification_fires(
            |c| {
                c.execute(
                    "INSERT INTO llm_wiki_entries (id, entity_id, title, body, created_at, updated_at, okf_type)
                     VALUES ('late','ent1','t','b',1,1,'handoff')",
                    [],
                )
                .unwrap();
            },
            "(b) stale slugs",
        );
    }

    #[test]
    fn sso9_verification_c_dropped_edge_reappearing_fires() {
        sso9_assert_retype_verification_fires(
            |c| {
                c.execute(
                    "INSERT INTO llm_wiki_edges (id, entity_id, source_id, target_id, edge_type, created_at)
                     VALUES ('e_svc','ent1','h1','svc1','handoffFor',1)",
                    [],
                )
                .unwrap();
            },
            "(c) dropped edge e_svc",
        );
    }

    #[test]
    fn sso9_verification_c_step2_gone_without_an_about_on_the_tuple_fires() {
        // T1 tuple-proof branch: the captured collision id (e_coll_hf) is GONE
        // by the dedupe; deleting the surviving `about` on its tuple leaves
        // GONE with no proof.
        sso9_assert_retype_verification_fires(
            |c| {
                c.execute(
                    "DELETE FROM llm_wiki_edges WHERE entity_id = 'ent1' AND source_id = 'h1'
                       AND target_id = 'sw1' AND edge_type = 'about'",
                    [],
                )
                .unwrap();
            },
            "(c) step-2 re-mapped edge e_coll_hf",
        );
    }

    #[test]
    fn sso9_verification_c_step2b_present_but_wrong_type_fires() {
        // V1: a successfully re-mapped captured row set BACK to handoffFor.
        sso9_assert_retype_verification_fires(
            |c| {
                c.execute("UPDATE llm_wiki_edges SET edge_type = 'handoffFor' WHERE id = 'e_remap'", [])
                    .unwrap();
            },
            "(c) step-2 re-mapped edge e_remap",
        );
    }

    #[test]
    fn sso9_verification_shape_check_tamper_between_commit_and_check_fires() {
        let (tmp, conn) = sso9_retype_brain();
        SSO9_AFTER_REFRESH_COMMIT.with(|h| {
            *h.borrow_mut() = Some(Box::new(|c: &Connection| {
                c.execute(
                    "UPDATE llm_wiki_entity_manifests SET manifest_json = '{\"node_types\":[],\"edge_types\":[]}'
                      WHERE entity_id IN ('tier_fact','ent_custom')",
                    [],
                )
                .unwrap();
            }))
        });
        sso9_clear_log();
        let result = sso9_migrate(&conn, tmp.path());
        SSO9_AFTER_REFRESH_COMMIT.with(|h| *h.borrow_mut() = None);
        result.expect("not propagated");
        let backups = sso9_backups(tmp.path());
        assert_eq!(backups.len(), 1, "backup KEPT");
        let name = backups[0].file_name().unwrap().to_string_lossy().to_string();
        let log = sso9_log_lines();
        let crit = log.iter().find(|l| l.contains("CRITICAL")).expect("CRITICAL line");
        assert!(crit.contains(&name) && crit.contains("shape check"), "{crit}");
    }

    #[test]
    fn sso9_alias_end_to_end_handoff_admits_as_decision_after_migration() {
        let (_tmp, conn, _log) = sso9_run_retype();
        let manifest = crate::wiki_graph::wiki_get_ontology(&conn, "tier_fact")
            .unwrap()
            .manifest
            .expect("tier_fact manifest");
        let vocab = crate::db::entity_gate::NodeVocabulary::from_manifest(&manifest);
        assert_eq!(
            crate::db::entity_gate::admit_label_landing(&vocab, "handoff").as_deref(),
            Some("decision")
        );
        assert_eq!(
            crate::db::entity_gate::admit_label_landing(&vocab, "session_recap").as_deref(),
            Some("lesson")
        );
    }

    // ── V33 engine-mirror step tests ─────────────────────
    //
    // Every test works on a tempdir COPY of the base-capture fixture brain (a V32-stamped
    // brain created under the core-llm-wiki 8.0.2 DDL); the checked-in file is
    // never migrated.

    /// Raw (no migrate) open of a tempdir copy of the 8.0.2 fixture brain.
    fn engine9_fixture_copy() -> (tempfile::TempDir, Connection) {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/db/testdata/ct-brain-v32-core-8.0.2.db");
        let tmp = tempfile::tempdir().unwrap();
        let dst = tmp.path().join("brain.db");
        std::fs::copy(&src, &dst).expect("copy 8.0.2 fixture brain");
        let conn = Connection::open(&dst).expect("raw open of fixture copy");
        (tmp, conn)
    }

    /// Fixture copy brought through CT's existing-DB open path.
    fn engine9_migrated_fixture() -> (tempfile::TempDir, Connection) {
        let (tmp, conn) = engine9_fixture_copy();
        migrate_open_db(&conn, Some(tmp.path())).expect("8.0.2 fixture migrates");
        (tmp, conn)
    }

    fn engine9_master_count(conn: &Connection, kind: &str, name: &str) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = ?1 AND name = ?2",
            rusqlite::params![kind, name],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn engine9_v33_stamped(conn: &Connection) -> bool {
        conn.query_row(
            "SELECT COUNT(*) FROM schema_version WHERE version = ?1",
            [ENGINE9_MIRROR_VERSION],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
            == 1
    }

    fn engine9_seed_entry(conn: &Connection, id: &str) {
        conn.execute(
            "INSERT INTO llm_wiki_entries (id, entity_id, title, body, created_at, updated_at)
             VALUES (?1, 'ent_e9', 't', 'b', 1, 1)",
            [id],
        )
        .unwrap();
    }

    fn engine9_ledger_rows(conn: &Connection, fact_id: &str) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM llm_wiki_vector_removals WHERE fact_id = ?1",
            [fact_id],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn engine9_mirror_slot_is_one_past_the_sso9_retype_slot() {
        assert_eq!(ENGINE9_MIRROR_VERSION, 33);
        assert_eq!(ENGINE9_MIRROR_VERSION, SSO9_RETYPE_VERSION + 1);
    }

    /// Spec §5 test 2 (Rust half): an existing 8.0.2-shape brain passes the
    /// 9.18.0 guard after CT's existing-DB open path, and V33 is stamped.
    #[test]
    fn engine9_existing_8_0_2_brain_migrates_and_passes_9_18_guard() {
        let (_tmp, conn) = engine9_migrated_fixture();
        crate::db::schema_guard::verify_llm_wiki_schema(&conn)
            .expect("migrated 8.0.2 brain matches the 9.18.0 guard");
        assert!(engine9_v33_stamped(&conn), "V22-stamped brain stamps V33");
        // v17 invariants: CT adds no column to, and writes no row into, the
        // source store.
        for t in ["llm_wiki_source_documents", "llm_wiki_source_passages"] {
            let n: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {t}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 0, "{t} holds no rows after migrate (engine v17 pre-condition)");
        }
        let docs = crate::db::ddl_compat::existing_columns(&conn, "llm_wiki_source_documents").unwrap();
        assert_eq!(
            docs,
            ["id", "entity_id", "source_ref", "source_hash", "text", "text_hash", "char_count", "created_at"]
        );
        let passages = crate::db::ddl_compat::existing_columns(&conn, "llm_wiki_source_passages").unwrap();
        assert_eq!(
            passages,
            ["id", "entity_id", "document_id", "seq", "start_offset", "end_offset"]
        );
    }

    /// Spec §5 test 3 (round trip / forward incompatibility): after migrate,
    /// the 8.0.2 expectations bail with "unexpected columns" naming the new
    /// entries columns; on an unmigrated copy they pass.
    #[test]
    fn engine9_migrated_brain_fails_the_8_0_2_guard_with_unexpected_columns() {
        let old = crate::db::schema_guard::core_llm_wiki_8_0_2_expectations();
        let (_raw_tmp, raw) = engine9_fixture_copy();
        crate::db::schema_guard::verify_schema_with(&raw, &old)
            .expect("8.0.2 expectations pass on the unmigrated copy");

        let (_tmp, conn) = engine9_migrated_fixture();
        let err = crate::db::schema_guard::verify_schema_with(&conn, &old)
            .expect_err("an 8.0.2 binary must refuse the migrated brain")
            .to_string();
        assert!(err.contains("unexpected columns"), "got: {err}");
        for col in ["source_pointer", "prior_valid_to", "derived_events"] {
            assert!(err.contains(col), "bail must name {col}: {err}");
        }
    }

    /// Spec §5 test 5 positive control: the ledger triggers survive
    /// migration and fire as the engine defines them.
    #[test]
    fn engine9_ledger_triggers_enqueue_on_soft_delete_and_marked_hard_delete_only() {
        let (_tmp, conn) = engine9_migrated_fixture();
        for id in ["e9_soft", "e9_hard", "e9_marked"] {
            engine9_seed_entry(&conn, id);
        }
        // Soft delete → exactly one ledger row.
        conn.execute(
            "UPDATE llm_wiki_entries SET deleted_at = 5 WHERE id = 'e9_soft'",
            [],
        )
        .unwrap();
        assert_eq!(engine9_ledger_rows(&conn, "e9_soft"), 1);
        // Hard delete, no external_index_in_use marker → nothing enqueued.
        let total = |c: &Connection| -> i64 {
            c.query_row("SELECT COUNT(*) FROM llm_wiki_vector_removals", [], |r| r.get(0))
                .unwrap()
        };
        let before = total(&conn);
        conn.execute("DELETE FROM llm_wiki_entries WHERE id = 'e9_hard'", [])
            .unwrap();
        assert_eq!(total(&conn), before, "unmarked hard delete enqueues nothing");
        // Positive control: with the marker set, a live hard delete
        // enqueues exactly one row — proves the hard-delete trigger exists.
        conn.execute(
            "INSERT OR REPLACE INTO llm_wiki_meta (key, value) VALUES ('external_index_in_use', '1')",
            [],
        )
        .unwrap();
        conn.execute("DELETE FROM llm_wiki_entries WHERE id = 'e9_marked'", [])
            .unwrap();
        assert_eq!(engine9_ledger_rows(&conn, "e9_marked"), 1);
        assert_eq!(total(&conn), before + 1);
    }

    #[test]
    fn engine9_mirror_is_idempotent_across_reopens() {
        let (tmp, conn) = engine9_migrated_fixture();
        let snapshot = |c: &Connection| -> Engine9Snapshot {
            let mut s = c
                .prepare("SELECT type, name, sql FROM sqlite_master ORDER BY type, name")
                .unwrap();
            let master = s
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .unwrap()
                .map(|r| r.unwrap())
                .collect();
            let mut s = c.prepare("SELECT version FROM schema_version ORDER BY version").unwrap();
            let stamps = s.query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect();
            let ledger = c
                .query_row("SELECT COUNT(*) FROM llm_wiki_vector_removals", [], |r| r.get(0))
                .unwrap();
            (master, stamps, ledger)
        };
        let first = snapshot(&conn);
        migrate_open_db(&conn, Some(tmp.path())).expect("second migrate succeeds");
        assert_eq!(snapshot(&conn), first, "second pass changes nothing");
        // The step body itself is idempotent with everything present.
        apply_engine9_mirror(&conn).unwrap();
        assert_eq!(snapshot(&conn), first);
    }

    /// Stamp gating: a rootless open (V22 deferred) still gets the
    /// columns, tables, and ledger triggers — the every-open guard demands
    /// them — but never the V33 stamp.
    #[test]
    fn engine9_rootless_open_gets_shape_without_stamping_v33() {
        let conn = open_in_memory().unwrap();
        let max: i64 = conn
            .query_row("SELECT COALESCE(MAX(version), 0) FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert!(max < 22, "precondition: rootless open defers V22 (got V{max})");
        assert!(!engine9_v33_stamped(&conn), "V33 must not stamp past a deferred V22");
        for t in [
            "llm_wiki_source_documents",
            "llm_wiki_source_passages",
            "llm_wiki_vector_removals",
        ] {
            assert_eq!(engine9_master_count(&conn, "table", t), 1, "{t}");
        }
        for t in ["llm_wiki_vector_removals_enqueue", "llm_wiki_vector_removals_hard_delete"] {
            assert_eq!(engine9_master_count(&conn, "trigger", t), 1, "{t}");
        }
        crate::db::schema_guard::verify_llm_wiki_schema(&conn).unwrap();
    }

    /// The step adds what an 8.0.2-shape brain lacks even with the stamp
    /// present (stamp is not a skip gate) — exercised on a raw fixture copy
    /// pre-stamped with V33 (a raw copy is V32-stamped, so
    /// inserting 33 first is what actually pins "stamp is not a skip gate").
    #[test]
    fn engine9_mirror_body_adds_columns_tables_and_triggers_on_raw_fixture() {
        let (_tmp, conn) = engine9_fixture_copy();
        assert_eq!(engine9_master_count(&conn, "table", "llm_wiki_vector_removals"), 0);
        conn.execute(
            &format!(
                "INSERT INTO schema_version (version) VALUES ({ENGINE9_MIRROR_VERSION});"
            ),
            [],
        )
        .unwrap();
        apply_engine9_mirror(&conn).unwrap();
        let entries = crate::db::ddl_compat::existing_columns(&conn, "llm_wiki_entries").unwrap();
        for col in ["source_pointer", "prior_valid_to", "derived_events"] {
            assert!(entries.iter().any(|c| c == col), "entries.{col}");
        }
        let events = crate::db::ddl_compat::existing_columns(&conn, "llm_wiki_events").unwrap();
        assert!(events.iter().any(|c| c == "forgotten_at"));
        assert_eq!(engine9_master_count(&conn, "index", "llm_wiki_vector_removals_pending"), 1);
        assert_eq!(engine9_master_count(&conn, "trigger", "llm_wiki_vector_removals_enqueue"), 1);
        assert_eq!(engine9_master_count(&conn, "trigger", "llm_wiki_vector_removals_hard_delete"), 1);
        assert!(engine9_v33_stamped(&conn), "V32 fixture is past V22 → stamps");
    }

    /// Desktop-upgrade proof (#376): an engine-generated 8.0.2 brain that
    /// already carries FTS-at-rest text goes through the V33 chain. The
    /// guarded ALTERs must not error, rows must survive, and the
    /// engine-owned FTS objects must be untouched by the mirror.
    #[test]
    fn engine9_mirror_over_fts_carrying_8_0_2_brain_preserves_data_and_fts() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/db/testdata/ct-brain-v33-src-8.0.2-fts.db");
        let tmp = tempfile::tempdir().unwrap();
        let dst = tmp.path().join("brain.db");
        std::fs::copy(&src, &dst).expect("copy FTS fixture brain");
        let conn = Connection::open(&dst).expect("raw open of FTS fixture copy");
        // The fixture is engine-only (no CT-owned tables); the chain's
        // stamp step needs CT's version table, which a real desktop brain has.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER PRIMARY KEY);
             INSERT OR IGNORE INTO schema_version (version) VALUES (32);",
        )
        .unwrap();

        let count = |c: &Connection, t: &str| -> i64 {
            c.query_row(&format!("SELECT COUNT(*) FROM {t}"), [], |r| r.get(0)).unwrap()
        };
        let fts_objects = |c: &Connection| -> Vec<(String, String, Option<String>)> {
            let mut stmt = c
                .prepare(
                    "SELECT type, name, sql FROM sqlite_master \
                     WHERE name LIKE '%fts%' ORDER BY type, name",
                )
                .unwrap();
            let rows = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .unwrap()
                .collect::<std::result::Result<Vec<_>, _>>()
                .unwrap();
            rows
        };
        let entries_before = count(&conn, "llm_wiki_entries");
        let content_before = count(&conn, "llm_wiki_entries_fts_content");
        let fts_before = fts_objects(&conn);
        assert!(entries_before >= 24, "fixture seeds >= 8 entities x 3 entries");
        assert_eq!(content_before, entries_before, "FTS at-rest text present");
        assert!(!fts_before.is_empty());

        apply_engine9_mirror(&conn).expect("V33 chain over FTS-carrying brain");
        // Second run: every guarded ALTER now targets an existing column.
        apply_engine9_mirror(&conn).expect("V33 chain is idempotent");

        assert_eq!(count(&conn, "llm_wiki_entries"), entries_before);
        assert_eq!(count(&conn, "llm_wiki_entries_fts_content"), content_before);
        assert_eq!(fts_objects(&conn), fts_before, "mirror must not touch engine FTS");
        let entries = crate::db::ddl_compat::existing_columns(&conn, "llm_wiki_entries").unwrap();
        for col in ["source_pointer", "prior_valid_to", "derived_events"] {
            assert!(entries.iter().any(|c| c == col), "entries.{col}");
        }
    }

    /// V33's snapshot → BEGIN IMMEDIATE → re-inspect → apply
    /// sequence is the only guard against the desktop/`--mcp` double-open
    /// race. Four concurrent opens race `apply_engine9_mirror` on a raw
    /// V32 fixture; every open must succeed (serialized by the write lock)
    /// and the result must satisfy the 9.18.0 guard. The
    /// COMMIT-failure rollback arm added in `827a4b2d` is not
    /// deterministically forceable here (it needs a write lock stolen
    /// between BEGIN and COMMIT); the concurrent serialization below is
    /// the exercisable half.
    #[test]
    fn engine9_mirror_concurrent_opens_serialize_and_do_not_race() {
        use std::sync::{Arc, Barrier};

        let (tmp, _) = engine9_fixture_copy();
        let path = tmp.path().join("brain.db");
        const OPENERS: usize = 4;
        let barrier = Arc::new(Barrier::new(OPENERS));
        let handles: Vec<_> = (0..OPENERS)
            .map(|_| {
                let barrier = Arc::clone(&barrier);
                let path = path.clone();
                std::thread::spawn(move || {
                    let conn = Connection::open(&path).unwrap();
                    conn.execute_batch("PRAGMA busy_timeout = 5000;").unwrap();
                    barrier.wait();
                    apply_engine9_mirror(&conn).map_err(|e| e.to_string())
                })
            })
            .collect();
        for handle in handles {
            handle
                .join()
                .unwrap()
                .expect("every concurrent open must mirror cleanly");
        }
        let conn = Connection::open(&path).unwrap();
        crate::db::schema_guard::verify_llm_wiki_schema(&conn)
            .expect("guard must accept the concurrently upgraded database");
        for t in [
            "llm_wiki_source_documents",
            "llm_wiki_source_passages",
            "llm_wiki_vector_removals",
        ] {
            assert_eq!(engine9_master_count(&conn, "table", t), 1, "{t} created once");
        }
    }

    // ── helper objects + fresh/migrated parity tests ─────

    /// Objects the 9.18.0 engine creates OUTSIDE the `setupDatabase` template
    /// (helpers called at the end of `setupDatabase`, plus the v18/v19
    /// migration-only partial indexes) — `(sqlite_master type, name)`.
    const ENGINE9_HELPER_OBJECTS: &[(&str, &str)] = &[
        ("index", "llm_wiki_entries_lifecycle_status_idx"),
        ("index", "llm_wiki_entries_stale_after_idx"),
        ("index", "llm_wiki_entries_last_verified_at_idx"),
        ("index", "llm_wiki_tasks_lifecycle_status_idx"),
        ("index", "llm_wiki_tasks_stale_after_idx"),
        ("index", "llm_wiki_tasks_last_verified_at_idx"),
        ("index", "llm_wiki_entries_superseded_idx"),
        ("index", "llm_wiki_entries_temporal_idx"),
        ("index", "llm_wiki_idx_source_passages_doc"),
        ("index", "llm_wiki_idx_source_passages_entity"),
        ("index", "llm_wiki_idx_source_documents_ref"),
        ("index", "llm_wiki_idx_source_documents_hash"),
        ("index", "llm_wiki_entries_deleted_idx"),
        ("index", "llm_wiki_events_forgotten_idx"),
        ("index", "llm_wiki_events_related_forgotten_idx"),
        ("index", "llm_wiki_events_related_live_idx"),
        ("trigger", "llm_wiki_vector_removals_resurrect"),
    ];

    /// Issue #378 D2: the anti-join partial index is gated on BOTH columns —
    /// present on a v19 shape, absent (and no error) on a pre-v19 shape.
    #[test]
    fn related_forgotten_idx_is_guarded_on_both_columns() {
        let idx_count = |c: &Connection| -> i64 {
            c.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='llm_wiki_events_related_forgotten_idx'",
                [],
                |r| r.get(0),
            )
            .unwrap()
        };
        let (_tmp, conn) = engine9_migrated_fixture();
        assert_eq!(idx_count(&conn), 1, "v19 shape carries the index");
        crate::db::test_support::drop_forgotten_column(&conn);
        crate::db::okf_ddl::apply_llm_wiki_v18_v19_partial_indexes(&conn).unwrap();
        assert_eq!(idx_count(&conn), 0, "pre-v19 shape must not get the index");
    }

    /// #401: the survival guard's LIVE half (`forgotten_at IS NULL`) is served
    /// by its own partial index, gated on both columns like the forgotten idx.
    #[test]
    fn related_live_idx_is_guarded_on_both_columns() {
        let idx_count = |c: &Connection| -> i64 {
            c.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='llm_wiki_events_related_live_idx'",
                [],
                |r| r.get(0),
            )
            .unwrap()
        };
        let (_tmp, conn) = engine9_migrated_fixture();
        assert_eq!(idx_count(&conn), 1, "v19 shape carries the index");
        // r1-B3 + r5 minor 4: ONE source of truth for the drop order — the
        // shared helper (drops all three indexes, then the column; SQLite
        // refuses ALTER TABLE … DROP COLUMN while a partial index's WHERE
        // clause uses it).
        crate::db::test_support::drop_forgotten_column(&conn);
        assert_eq!(idx_count(&conn), 0, "pre-v19 shape carries no index");
        crate::db::okf_ddl::apply_llm_wiki_v18_v19_partial_indexes(&conn).unwrap();
        assert_eq!(idx_count(&conn), 0, "pre-v19 shape must not get the index");
    }

    /// T3 r1 MINOR 1: the live index's guard has TWO column halves. Dropping
    /// `forgotten_at` is covered above via the shared helper; this arm drops
    /// `related_entry_id` and asserts the re-apply creates NEITHER
    /// `related_entry_id`-serving index (both are nested inside the
    /// `related_entry_id` guard in `apply_llm_wiki_v18_v19_partial_indexes`).
    #[test]
    fn related_live_idx_requires_related_entry_id_column() {
        let idx_count = |name: &str, c: &Connection| -> i64 {
            c.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?1",
                [name],
                |r| r.get(0),
            )
            .unwrap()
        };
        let (_tmp, conn) = engine9_migrated_fixture();
        conn.execute("DROP INDEX IF EXISTS llm_wiki_events_related_live_idx", [])
            .unwrap();
        conn.execute(
            "DROP INDEX IF EXISTS llm_wiki_events_related_forgotten_idx",
            [],
        )
        .unwrap();
        conn.execute(
            "ALTER TABLE llm_wiki_events DROP COLUMN related_entry_id",
            [],
        )
        .unwrap();
        crate::db::okf_ddl::apply_llm_wiki_v18_v19_partial_indexes(&conn).unwrap();
        assert_eq!(
            idx_count("llm_wiki_events_related_live_idx", &conn),
            0,
            "no related_entry_id column → live index must not be created"
        );
        assert_eq!(
            idx_count("llm_wiki_events_related_forgotten_idx", &conn),
            0,
            "no related_entry_id column → forgotten-related index must not be created"
        );
    }

    /// Spec test-plan item 5 (T3 r1 MINOR 2): EXPLAIN QUERY PLAN on the
    /// survival guard's LIVE half — the correlated subquery the entry arm
    /// emits — must be served by `llm_wiki_events_related_live_idx`, so a
    /// future arm-SQL or DDL change that loses the index fails a test
    /// instead of silently regressing to a SCAN.
    #[test]
    fn eqp_live_subquery_uses_related_live_idx() {
        let (_tmp, conn) = engine9_migrated_fixture();
        let mut stmt = conn
            .prepare(
                "EXPLAIN QUERY PLAN SELECT COUNT(*) FROM llm_wiki_entries e \
                 WHERE EXISTS (SELECT 1 FROM llm_wiki_events le \
                 WHERE le.related_entry_id = e.id AND le.forgotten_at IS NULL)",
            )
            .unwrap();
        let mut saw_live_idx = false;
        let mut rows = stmt.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            let detail: String = row.get(3).unwrap();
            if detail.contains("llm_wiki_events_related_live_idx") {
                saw_live_idx = true;
            }
        }
        assert!(
            saw_live_idx,
            "the arm's live subquery must be served by llm_wiki_events_related_live_idx"
        );
    }

    /// A fresh ROOTED file brain through CT's real open path.
    fn engine9_fresh_rooted_brain() -> (tempfile::TempDir, AppDb) {
        let tmp = tempfile::tempdir().unwrap();
        let vault = tmp.path().join("vault");
        std::fs::create_dir_all(&vault).unwrap();
        let config = tmp.path().join("config.json");
        std::fs::write(
            &config,
            // serde_json serialization (Windows-path escaping — see the
            // schema_guard twin of this fixture helper).
            serde_json::json!({ "vault_path": vault.to_str().unwrap() }).to_string(),
        )
        .unwrap();
        let db = AppDb::open_with_config(&tmp.path().join("brain.db"), &config)
            .expect("fresh rooted brain opens");
        (tmp, db)
    }

    type Engine9Columns = std::collections::BTreeMap<String, (String, i64, Option<String>, i64)>;
    type Engine9Snapshot = (Vec<(String, String, Option<String>)>, Vec<i64>, i64);
    type Engine9MasterMap = std::collections::BTreeMap<(String, String), Option<String>>;
    type Engine9Shape = (
        Engine9MasterMap,
        std::collections::BTreeMap<String, Engine9Columns>,
    );

    /// `type → name → normalized sql` for every named schema object, plus
    /// `table → name-keyed PRAGMA table_info (cid excluded)`.
    fn engine9_schema_shape(conn: &Connection) -> Engine9Shape {
        let mut s = conn
            .prepare("SELECT type, name, sql FROM sqlite_master WHERE type IN ('table','index','trigger')")
            .unwrap();
        let master: Engine9MasterMap = s
            .query_map([], |r| {
                Ok((
                    (r.get::<_, String>(0)?, r.get::<_, String>(1)?),
                    r.get::<_, Option<String>>(2)?
                        .map(|sql| crate::db::ddl_compat::normalize_ddl_statement(&sql)),
                ))
            })
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        let mut cols = std::collections::BTreeMap::new();
        for (kind, name) in master.keys() {
            if kind != "table" {
                continue;
            }
            let mut p = conn
                .prepare(&format!("SELECT name, type, \"notnull\", dflt_value, pk FROM pragma_table_info('{name}')"))
                .unwrap();
            let info: Engine9Columns = p
                .query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))))
                .unwrap()
                .map(|r| r.unwrap())
                .collect();
            cols.insert(name.clone(), info);
        }
        (master, cols)
    }

    /// Spec §5 test 6, parity definition per erratum E8: a fresh rooted brain
    /// and a migrated 8.0.2 fixture brain have equal table/index/trigger
    /// NAME sets; equal normalized index + trigger SQL; equal name-keyed
    /// `PRAGMA table_info` (cid excluded — ALTER appends after CT-local
    /// columns while the fresh template carries them inline); and equal
    /// normalized table SQL for every table PR-B does not ALTER.
    #[test]
    fn engine9_fresh_and_migrated_brains_have_schema_parity() {
        let (_ftmp, fresh) = engine9_fresh_rooted_brain();
        let (_mtmp, migrated) = engine9_migrated_fixture();
        let (fm, fc) = engine9_schema_shape(&fresh.0);
        let (mm, mc) = engine9_schema_shape(&migrated);

        let names = |m: &std::collections::BTreeMap<(String, String), Option<String>>| {
            m.keys().cloned().collect::<std::collections::BTreeSet<_>>()
        };
        let (fnames, mnames) = (names(&fm), names(&mm));
        assert_eq!(
            fnames, mnames,
            "object name sets differ — fresh-only: {:?}; migrated-only: {:?}",
            fnames.difference(&mnames).collect::<Vec<_>>(),
            mnames.difference(&fnames).collect::<Vec<_>>()
        );
        const ALTERED: &[&str] = &["llm_wiki_entries", "llm_wiki_events"];
        for (key, fsql) in &fm {
            let (kind, name) = key;
            if kind == "table" && ALTERED.contains(&name.as_str()) {
                continue;
            }
            assert_eq!(fsql, &mm[key], "{kind} {name}: normalized SQL differs");
        }
        assert_eq!(fc, mc, "name-keyed table_info differs");
    }

    /// Parity alone is vacuous for objects NEITHER path creates: both brains
    /// must carry every engine helper object by exact prefixed name.
    #[test]
    fn engine9_helper_objects_present_on_fresh_and_migrated_brains() {
        let (_ftmp, fresh) = engine9_fresh_rooted_brain();
        let (_mtmp, migrated) = engine9_migrated_fixture();
        for (label, conn) in [("fresh", &fresh.0), ("migrated", &migrated)] {
            let missing: Vec<_> = ENGINE9_HELPER_OBJECTS
                .iter()
                .filter(|(kind, name)| engine9_master_count(conn, kind, name) != 1)
                .collect();
            assert!(missing.is_empty(), "{label} brain lacks helper objects: {missing:?}");
        }
    }

    /// The resurrect trigger mirrors the engine's level ladder: a brain with
    /// the embed-marker columns gets the level-2 body; a pre-existing
    /// lower-level trigger is replaced, an equal one is left alone.
    #[test]
    fn engine9_resurrect_trigger_is_level_2_and_upgrades_a_lower_level() {
        let (_tmp, conn) = engine9_migrated_fixture();
        let sql = |c: &Connection| -> String {
            c.query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'trigger' AND name = 'llm_wiki_vector_removals_resurrect'",
                [],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert!(sql(&conn).contains("embedding_failed_at"), "level 2 on a CT brain");
        conn.execute_batch(
            "DROP TRIGGER llm_wiki_vector_removals_resurrect;
             CREATE TRIGGER llm_wiki_vector_removals_resurrect
             AFTER UPDATE OF deleted_at ON llm_wiki_entries
             WHEN OLD.deleted_at IS NOT NULL AND NEW.deleted_at IS NULL
             BEGIN DELETE FROM llm_wiki_vector_removals WHERE fact_id = NEW.id; END;",
        )
        .unwrap();
        crate::db::okf_ddl::apply_llm_wiki_resurrect_trigger(&conn).unwrap();
        let upgraded = sql(&conn);
        assert!(upgraded.contains("embedding_failed_at"), "level 0 → 2: {upgraded}");
        crate::db::okf_ddl::apply_llm_wiki_resurrect_trigger(&conn).unwrap();
        assert_eq!(sql(&conn), upgraded, "equal level is left alone");

        // Behaviour: un-deleting a fact drops its (unconfirmed) ledger row
        // and keeps its embedding.
        engine9_seed_entry(&conn, "e9_res");
        conn.execute(
            "UPDATE llm_wiki_entries SET embedding = 'v', deleted_at = 5 WHERE id = 'e9_res'",
            [],
        )
        .unwrap();
        assert_eq!(engine9_ledger_rows(&conn, "e9_res"), 1);
        conn.execute("UPDATE llm_wiki_entries SET deleted_at = NULL WHERE id = 'e9_res'", [])
            .unwrap();
        assert_eq!(engine9_ledger_rows(&conn, "e9_res"), 0);
        let emb: Option<String> = conn
            .query_row("SELECT embedding FROM llm_wiki_entries WHERE id = 'e9_res'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(emb.as_deref(), Some("v"), "unconfirmed removal keeps the vector");
    }

    // ── schema dump for the TS setup() proof tests ───────

    /// Render the migrated 8.0.2 fixture's `llm_wiki_*` objects as the dump
    /// `src/__tests__/engine9-setup-compat.test.ts` loads. One `-- object:`
    /// header per object so the TS side can order tables before indexes and
    /// triggers without splitting SQL on `;`.
    fn engine9_migrated_schema_dump(conn: &Connection) -> String {
        let mut s = conn
            .prepare(
                "SELECT type, name, sql FROM sqlite_master WHERE name LIKE 'llm\\_wiki\\_%' ESCAPE '\\' \
                 AND sql IS NOT NULL ORDER BY type, name",
            )
            .unwrap();
        let rows: Vec<(String, String, String)> = s
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        let mut out = String::from(
            "-- GENERATED by connection.rs engine9_migrated_schema_fixture_is_current; do not edit.\n\
             -- The llm_wiki_* schema of the 8.0.2 fixture brain after CT's migrate (V33 + helpers).\n\
             -- Regenerate: CT_REGEN_FIXTURES=1 cargo test --manifest-path src-tauri/Cargo.toml \
             --lib --features test-utils engine9_migrated_schema_fixture_is_current\n",
        );
        for (kind, name, sql) in rows {
            out.push_str(&format!("\n-- object: {kind} {name}\n{sql};\n"));
        }
        out
    }

    /// Drift gate for `testdata/ct-engine9-migrated.llm_wiki.schema.sql`: the
    /// TS-side engine `setup()` proof runs over exactly what the Rust chain
    /// produces. `CT_REGEN_FIXTURES=1` rewrites the file.
    #[test]
    fn engine9_migrated_schema_fixture_is_current() {
        let (_tmp, conn) = engine9_migrated_fixture();
        let dump = engine9_migrated_schema_dump(&conn);
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/db/testdata/ct-engine9-migrated.llm_wiki.schema.sql");
        if std::env::var("CT_REGEN_FIXTURES").as_deref() == Ok("1") {
            std::fs::write(&path, &dump).unwrap();
        }
        let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            on_disk == dump,
            "{} is stale — regenerate with CT_REGEN_FIXTURES=1 (see file header)",
            path.display()
        );
    }

    // ── CT's raw delete paths vs the ledger tests ────────
    //
    // Spec §3 step 6: the raw delete sites stay raw; the
    // obligation is that they never bypass the ledger triggers. Each path is
    // driven over a migrated fixture copy seeded with a soft-deleted fact (a
    // pre-existing ledger row) and a live fact in entity `ent_t9`.
    //
    // Engine semantics (the hard_delete trigger, `LLM_WIKI_PACKAGE_DDL`):
    // without the `external_index_in_use` marker nothing is owed, so a live
    // hard delete enqueues nothing and a hard-deleted soft-deleted fact's own
    // pending row is dropped; with the marker, pending rows survive the hard
    // delete and each live hard-deleted fact enqueues exactly one row.

    fn t9_seed(conn: &Connection, marker: bool) {
        if marker {
            conn.execute(
                "INSERT OR REPLACE INTO llm_wiki_meta (key, value) VALUES ('external_index_in_use', '1')",
                [],
            )
            .unwrap();
        }
        for id in ["t9_soft", "t9_live"] {
            conn.execute(
                "INSERT INTO llm_wiki_entries (id, entity_id, title, body, created_at, updated_at)
                 VALUES (?1, 'ent_t9', 't', 'b', 1, 1)",
                [id],
            )
            .unwrap();
        }
        conn.execute("UPDATE llm_wiki_entries SET deleted_at = 5 WHERE id = 't9_soft'", [])
            .unwrap();
        assert_eq!(engine9_ledger_rows(conn, "t9_soft"), 1, "seed: soft delete enqueued");
        assert_eq!(engine9_ledger_rows(conn, "t9_live"), 0);
    }

    /// After a path hard-deleted both seeded facts: the ledger reflects the
    /// engine trigger exactly — nothing bypassed, nothing CT-written.
    fn t9_assert_ledger(conn: &Connection, marker: bool, path: &str) {
        let gone: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_entries WHERE id IN ('t9_soft','t9_live')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(gone, 0, "{path}: both facts hard-deleted");
        let (soft, live) = (
            engine9_ledger_rows(conn, "t9_soft"),
            engine9_ledger_rows(conn, "t9_live"),
        );
        if marker {
            assert_eq!(soft, 1, "{path}: pre-existing ledger row survives the hard delete");
            assert_eq!(live, 1, "{path}: live hard delete enqueues exactly one row");
        } else {
            assert_eq!(soft, 0, "{path}: unmarked — engine drops the row nothing can owe");
            assert_eq!(live, 0, "{path}: unmarked live hard delete enqueues nothing");
        }
    }

    #[test]
    fn t9_commit_hard_delete_entries_does_not_bypass_the_ledger() {
        for marker in [false, true] {
            let (_tmp, conn) = engine9_migrated_fixture();
            t9_seed(&conn, marker);
            crate::db::commit::hard_delete_entries(
                &conn,
                &[
                    ("t9_soft".into(), "ent_t9".into()),
                    ("t9_live".into(), "ent_t9".into()),
                ],
                1,
            )
            .unwrap();
            t9_assert_ledger(&conn, marker, "commit::hard_delete_entries");
        }
    }

    #[test]
    fn t9_clear_vault_tables_does_not_bypass_the_ledger() {
        for marker in [false, true] {
            let (_tmp, mut conn) = engine9_migrated_fixture();
            t9_seed(&conn, marker);
            crate::db::queries::clear_vault_tables(&mut conn, 1).unwrap();
            t9_assert_ledger(&conn, marker, "queries::clear_vault_tables");
        }
    }
}
