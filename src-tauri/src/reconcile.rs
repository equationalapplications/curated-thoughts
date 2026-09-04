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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::walk_vault::WalkedFile;
    use std::path::PathBuf;

    /// A document row plus `n` chunks hanging off it.
    fn seed_doc(conn: &Connection, path: &str, hash: &str, tier: &str, chunks: usize) -> i64 {
        conn.execute(
            "INSERT INTO documents (path, hash, tier, status) VALUES (?1, ?2, ?3, 'indexed')",
            rusqlite::params![path, hash, tier],
        )
        .unwrap();
        let doc_id = conn.last_insert_rowid();
        for i in 0..chunks {
            conn.execute(
                "INSERT INTO chunks (doc_id, chunk_text, position) VALUES (?1, ?2, ?3)",
                rusqlite::params![doc_id, format!("chunk {i}"), i as i64],
            )
            .unwrap();
        }
        doc_id
    }

    fn chunk_count(conn: &Connection, doc_id: i64) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM chunks WHERE doc_id = ?1",
            [doc_id],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn path_of(conn: &Connection, doc_id: i64) -> String {
        conn.query_row("SELECT path FROM documents WHERE id = ?1", [doc_id], |r| {
            r.get(0)
        })
        .unwrap()
    }

    /// Write `content` to `dir/name` and return it as a WalkedFile.
    fn walked(dir: &std::path::Path, name: &str, content: &[u8]) -> WalkedFile {
        let p = dir.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, content).unwrap();
        WalkedFile {
            virtual_path: p.clone(),
            read_path: p,
        }
    }

    fn hash_of(content: &[u8]) -> String {
        crate::db::queue::sha256_hex(content)
    }

    fn s(p: &PathBuf) -> String {
        p.to_str().unwrap().to_string()
    }

    #[test]
    fn rename_repoints_row_and_preserves_chunks() {
        // AC1 + AC2: a 100% rename keeps every chunk and reports the new path.
        let tmp = tempfile::TempDir::new().unwrap();
        let conn = crate::db::connection::open_in_memory().unwrap();
        let content = b"# moved note";

        let new = walked(tmp.path(), "procedures/note.md", content);
        let old_path = s(&tmp.path().join("note.md"));
        let doc_id = seed_doc(&conn, &old_path, &hash_of(content), "user_doc", 12);

        let out = reconcile_vault(&conn, &[new.clone()]).unwrap();

        assert_eq!(out.repointed, vec![(old_path, s(&new.virtual_path))]);
        assert!(out.deleted.is_empty());
        assert!(out.ambiguous.is_empty());
        assert_eq!(chunk_count(&conn, doc_id), 12, "chunks must ride along");
        assert_eq!(path_of(&conn, doc_id), s(&new.virtual_path));
    }

    #[test]
    fn vanished_file_is_deleted_and_chunks_cascade() {
        // AC3.
        let tmp = tempfile::TempDir::new().unwrap();
        let conn = crate::db::connection::open_in_memory().unwrap();

        let survivor = walked(tmp.path(), "kept.md", b"# kept");
        let gone_path = s(&tmp.path().join("gone.md"));
        let gone_id = seed_doc(&conn, &gone_path, &hash_of(b"# gone"), "user_doc", 5);

        let out = reconcile_vault(&conn, &[survivor]).unwrap();

        assert_eq!(out.deleted, vec![gone_path]);
        assert!(out.repointed.is_empty());
        assert_eq!(chunk_count(&conn, gone_id), 0, "chunks must cascade");
    }

    #[test]
    fn ambiguous_identical_content_is_left_alone() {
        // AC4: two vanished rows and two new paths all share one hash.
        let tmp = tempfile::TempDir::new().unwrap();
        let conn = crate::db::connection::open_in_memory().unwrap();
        let content = b"identical";
        let h = hash_of(content);

        let a = walked(tmp.path(), "new/a.md", content);
        let b = walked(tmp.path(), "new/b.md", content);
        let old_a = s(&tmp.path().join("old-a.md"));
        let old_b = s(&tmp.path().join("old-b.md"));
        let id_a = seed_doc(&conn, &old_a, &h, "user_doc", 3);
        let id_b = seed_doc(&conn, &old_b, &h, "user_doc", 3);

        let out = reconcile_vault(&conn, &[a, b]).unwrap();

        assert!(out.repointed.is_empty(), "must not guess a rename");
        assert!(out.deleted.is_empty(), "must not delete what it cannot match");
        assert_eq!(out.ambiguous.len(), 2);
        assert_eq!(path_of(&conn, id_a), old_a);
        assert_eq!(path_of(&conn, id_b), old_b);
        assert_eq!(chunk_count(&conn, id_a), 3);
        assert_eq!(chunk_count(&conn, id_b), 3);
    }

    #[test]
    fn rename_onto_an_existing_row_deletes_rather_than_colliding() {
        // AC5: documents.path is NOT NULL UNIQUE. The target already has a
        // row, so it is not an "unknown" candidate and the vanished row falls
        // through to delete -- no constraint violation.
        let tmp = tempfile::TempDir::new().unwrap();
        let conn = crate::db::connection::open_in_memory().unwrap();
        let content = b"dup content";
        let h = hash_of(content);

        let existing = walked(tmp.path(), "existing.md", content);
        seed_doc(&conn, &s(&existing.virtual_path), &h, "user_doc", 4);
        let gone_path = s(&tmp.path().join("gone.md"));
        let gone_id = seed_doc(&conn, &gone_path, &h, "user_doc", 4);

        let out = reconcile_vault(&conn, &[existing]).expect("must not violate UNIQUE");

        assert_eq!(out.deleted, vec![gone_path]);
        assert_eq!(chunk_count(&conn, gone_id), 0);
    }

    #[test]
    fn wiki_tier_rows_are_never_touched() {
        // AC6.
        let tmp = tempfile::TempDir::new().unwrap();
        let conn = crate::db::connection::open_in_memory().unwrap();

        let survivor = walked(tmp.path(), "kept.md", b"# kept");
        let wiki_path = "/not/on/disk/page.md".to_string();
        let wiki_id = seed_doc(&conn, &wiki_path, "deadbeef", "wiki", 7);

        let out = reconcile_vault(&conn, &[survivor]).unwrap();

        assert!(out.deleted.is_empty());
        assert!(out.repointed.is_empty());
        assert_eq!(path_of(&conn, wiki_id), wiki_path);
        assert_eq!(chunk_count(&conn, wiki_id), 7);
    }

    #[test]
    fn empty_walk_changes_nothing() {
        // AC7: a transient mount failure must not delete the whole index.
        let conn = crate::db::connection::open_in_memory().unwrap();
        let doc_id = seed_doc(&conn, "/vault/a.md", "aaa", "user_doc", 9);

        let out = reconcile_vault(&conn, &[]).unwrap();

        assert_eq!(out, ReconcileOutcome::default());
        assert_eq!(chunk_count(&conn, doc_id), 9);
        assert_eq!(path_of(&conn, doc_id), "/vault/a.md");
    }

    #[test]
    fn modified_in_place_file_is_untouched() {
        // AC9: same path, different hash. Not vanished, so not our business.
        let tmp = tempfile::TempDir::new().unwrap();
        let conn = crate::db::connection::open_in_memory().unwrap();

        let f = walked(tmp.path(), "note.md", b"# new content");
        let doc_id = seed_doc(&conn, &s(&f.virtual_path), "stale-hash", "user_doc", 6);

        let out = reconcile_vault(&conn, &[f.clone()]).unwrap();

        assert_eq!(out, ReconcileOutcome::default());
        assert_eq!(path_of(&conn, doc_id), s(&f.virtual_path));
        assert_eq!(chunk_count(&conn, doc_id), 6);
    }
}
