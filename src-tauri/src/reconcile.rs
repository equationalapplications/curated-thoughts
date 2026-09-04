//! Reconcile the `documents` table against what is actually on disk.
//!
//! Spec: docs/superpowers/specs/2026-09-04-ingest-integrity-wave-design.md §4
//!
//! The live filesystem watcher handles moves correctly while it is running: a
//! `Remove` event deletes the `documents` row and chunks cascade. The gap this
//! module closes is the *offline* move -- the app is closed, a file is
//! `git mv`'d, and on the next run the walker discovers the new path as a new
//! document while the old row survives forever, still owning every chunk.
//!
//! Re-pointing rather than re-ingesting is deliberate: `chunks.doc_id`
//! references `documents.id`, so a single `UPDATE documents SET path` leaves
//! every chunk and embedding attached and costs no embedding work. Deleting
//! and re-ingesting would pay the full embedding cost of the moved content and
//! leave a recall gap until the sweep caught up. Vault reorganizations move
//! many files at once, so that cost is not hypothetical.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use rusqlite::Connection;

use crate::walk_vault::WalkedFile;

/// What a reconciliation pass changed. Returned rather than only logged so the
/// caller can report it and tests can assert on it.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ReconcileOutcome {
    /// `(old_path, new_path)` for each row whose path was re-pointed.
    pub repointed: Vec<(String, String)>,
    /// Paths whose rows were deleted because the file is gone with no
    /// content-identical replacement.
    pub deleted: Vec<String>,
    /// Vanished paths left untouched because the match was not unambiguous.
    pub ambiguous: Vec<String>,
}

/// Diff `documents` against `walked` and apply renames and deletions.
///
/// Only `tier = 'user_doc'` rows participate. Wiki-tier rows are not all
/// filesystem-backed and must never be reconciled against a vault walk.
pub fn reconcile_vault(conn: &Connection, walked: &[WalkedFile]) -> Result<ReconcileOutcome> {
    let mut outcome = ReconcileOutcome::default();

    // An empty walk means a misconfigured or unmounted vault root, not an
    // empty vault. Reconciling against it would delete the entire index --
    // a transient mount failure must never be able to do that.
    if walked.is_empty() {
        eprintln!("[reconcile] walk returned no files; skipping reconciliation");
        return Ok(outcome);
    }

    // `documents.path` stores the VIRTUAL path (tools/src/cmds.rs:217).
    // Comparing against `read_path` would report every symlinked file as
    // vanished and delete it.
    let walked_paths: HashSet<String> = walked
        .iter()
        .filter_map(|f| f.virtual_path.to_str().map(str::to_string))
        .collect();

    let db_rows: Vec<(String, String)> = {
        let mut stmt = conn.prepare("SELECT path, hash FROM documents WHERE tier = 'user_doc'")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };

    let db_paths: HashSet<&str> = db_rows.iter().map(|(p, _)| p.as_str()).collect();

    let vanished: Vec<&(String, String)> = db_rows
        .iter()
        .filter(|(p, _)| !walked_paths.contains(p))
        .collect();

    if vanished.is_empty() {
        return Ok(outcome);
    }

    // Hash only paths the database has never seen. Re-hashing the whole vault
    // on every ingest would be the dominant cost of this pass.
    //
    // Because a path already present in `documents` is skipped here, a rename
    // whose target already has its own row simply finds no candidate and falls
    // through to the delete arm below. That is why there is no separate
    // UNIQUE-collision guard: the collision is unreachable by construction.
    let mut unknown_by_hash: HashMap<String, Vec<String>> = HashMap::new();
    for f in walked {
        let Some(vp) = f.virtual_path.to_str() else {
            eprintln!(
                "[reconcile] skipping non-UTF-8 path: {}",
                f.virtual_path.display()
            );
            continue;
        };
        if db_paths.contains(vp) {
            continue;
        }
        let bytes = match std::fs::read(&f.read_path) {
            Ok(b) => b,
            Err(e) => {
                // Unreadable candidates simply cannot participate in rename
                // detection. Not fatal -- the ingest loop will report it.
                eprintln!("[reconcile] skipping unreadable {}: {e}", f.read_path.display());
                continue;
            }
        };
        unknown_by_hash
            .entry(crate::db::queue::sha256_hex(&bytes))
            .or_default()
            .push(vp.to_string());
    }

    // A hash claimed by more than one vanished row is as ambiguous as one
    // claimed by more than one candidate.
    let mut vanished_per_hash: HashMap<&str, usize> = HashMap::new();
    for (_, h) in &vanished {
        *vanished_per_hash.entry(h.as_str()).or_insert(0) += 1;
    }

    let tx = conn.unchecked_transaction()?;
    for (old_path, hash) in &vanished {
        let unique_source = vanished_per_hash.get(hash.as_str()).copied().unwrap_or(0) == 1;
        match unknown_by_hash.get(hash.as_str()) {
            Some(candidates) if candidates.len() == 1 && unique_source => {
                let new_path = &candidates[0];
                tx.execute(
                    "UPDATE documents SET path = ?1 WHERE path = ?2",
                    rusqlite::params![new_path, old_path],
                )?;
                outcome.repointed.push((old_path.clone(), new_path.clone()));
            }
            Some(_) => {
                // Never guess which of several identical-content files is
                // "the" rename. Changing nothing is always recoverable.
                outcome.ambiguous.push(old_path.clone());
            }
            None => {
                tx.execute(
                    "DELETE FROM documents WHERE path = ?1",
                    rusqlite::params![old_path],
                )?;
                outcome.deleted.push(old_path.clone());
            }
        }
    }
    tx.commit()?;

    Ok(outcome)
}
