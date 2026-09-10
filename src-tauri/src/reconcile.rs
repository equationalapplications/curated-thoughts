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
use std::path::Path;

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
///
/// `vault_root` is the as-configured vault root. It is required, not
/// derived: the excluded-row predicate must relativize each row's absolute
/// path before matching, because `EXCLUDED_DIRS` names occur in ordinary
/// ancestor directories and a raw absolute-path check on a vault at
/// `<tmp>/target/wiki/` would match `target` on every row and delete the
/// entire index (spec D1).
pub fn reconcile_vault(
    conn: &Connection,
    walked: &[WalkedFile],
    vault_root: &Path,
) -> Result<ReconcileOutcome> {
    let mut outcome = ReconcileOutcome::default();

    // An empty walk means a misconfigured or unmounted vault root, not an
    // empty vault. Reconciling against it would delete the entire index --
    // a transient mount failure must never be able to do that.
    //
    // The one carve-out is `.brain`: CT owns that directory, writes into it
    // itself (`pipeline/mod.rs:484`), and no CT code path has ever
    // legitimately ingested from it, so such a row is illegitimate whether
    // or not the vault is mounted. The scope is deliberately NARROWER than
    // the non-empty pre-pass: for `node_modules/`, `target/`, `.git/` etc.
    // an empty walk is not proof the row should be absent, and deleting
    // them on an unmounted vault is exactly the disaster this guard exists
    // to prevent (spec item 4).
    if walked.is_empty() {
        eprintln!("[reconcile] walk returned no files; skipping reconciliation");
        return purge_brain_rows(conn, vault_root);
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
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };

    let db_paths: HashSet<&str> = db_rows.iter().map(|(p, _)| p.as_str()).collect();

    let vanished: Vec<&(String, String)> = db_rows
        .iter()
        .filter(|(p, _)| !walked_paths.contains(p))
        .collect();

    // Spec item 3a -- partition BEFORE rename detection.
    //
    // Rows under an excluded directory must never reach the hash-matching
    // arms. `.brain/errors.log` is routinely rotated or truncated to 0
    // bytes, and every empty file shares one sha256: with one empty
    // `.brain/errors.log` row and one newly added empty `inbox/todo.md`,
    // the unique-hash branch would repoint the .brain row at the user's
    // file, leaving chunks attached to the wrong content and the real
    // file's own row blocked by the UNIQUE path. With two empty candidates
    // the row would sit in `ambiguous` forever.
    //
    // Scope here is intentionally BROAD -- all of EXCLUDED_DIRS. A
    // successful (non-empty) walk IS proof such rows should be absent. The
    // narrow `.brain`-only scope applies only to the empty-walk branch
    // above, where an empty walk may mean a transient mount failure.
    //
    // Spec D2b: a row that cannot be relativized against the vault root
    // is left ALONE, never deleted. `abs_path_is_excluded_in_vault`
    // fail-opens to `false` for such rows (we have no proof they lived
    // under the vault), and the partition would otherwise drop them into
    // `remaining` where the no-candidate delete arm below would still
    // delete them. Filter them out up front so neither arm can touch them.
    let (excluded, remaining): (Vec<_>, Vec<_>) = vanished
        .into_iter()
        .filter(|(p, _)| {
            crate::walk_vault::relativize_to_vault(Path::new(p), vault_root).is_some()
        })
        .partition(|(p, _)| {
            crate::walk_vault::abs_path_is_excluded_in_vault(Path::new(p), vault_root)
        });

    if excluded.is_empty() && remaining.is_empty() {
        return Ok(outcome);
    }

    // Hash only paths the database has never seen. Re-hashing the whole vault
    // on every ingest would be the dominant cost of this pass.
    //
    // Because a path already present in `documents` is skipped here, a rename
    // whose target already has its own row simply finds no candidate and falls
    // through to the delete arm below. That is why there is no separate
    // UNIQUE-collision guard: the collision is unreachable by construction.
    //
    // Done BEFORE the transaction opens so the write lock is not held across
    // filesystem I/O, and skipped entirely when the pre-pass consumed every
    // vanished row.
    //
    // Excluded rows need no filtering out of this map: it is built solely
    // from WALKED paths absent from `documents`, and `.brain` (and every
    // other EXCLUDED_DIRS name) is pruned by `collect_files`, so an excluded
    // path can never appear here.
    let mut unknown_by_hash: HashMap<String, Vec<String>> = HashMap::new();
    if !remaining.is_empty() {
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
                    eprintln!(
                        "[reconcile] skipping unreadable {}: {e}",
                        f.read_path.display()
                    );
                    continue;
                }
            };
            unknown_by_hash
                .entry(crate::db::queue::sha256_hex(&bytes))
                .or_default()
                .push(vp.to_string());
        }
    }

    // A hash claimed by more than one vanished row is as ambiguous as one
    // claimed by more than one candidate. Excluded rows are absent from this
    // accounting, so they cannot perturb another row's uniqueness verdict.
    let mut vanished_per_hash: HashMap<&str, usize> = HashMap::new();
    for (_, h) in &remaining {
        *vanished_per_hash.entry(h.as_str()).or_insert(0) += 1;
    }

    // ONE transaction covers both the pre-pass and the rename/delete match,
    // so a mid-loop rusqlite error rolls the whole pass back. It is opened
    // above the "nothing left to match" return below: when the pre-pass
    // consumed every vanished row, its deletes must still commit.
    let tx = conn.unchecked_transaction()?;

    for (old_path, _) in &excluded {
        tx.execute(
            "DELETE FROM documents WHERE path = ?1",
            rusqlite::params![old_path],
        )?;
        outcome.deleted.push((*old_path).clone());
    }

    for (old_path, hash) in &remaining {
        let unique_source = vanished_per_hash.get(hash.as_str()).copied().unwrap_or(0) == 1;
        match unknown_by_hash.get(hash.as_str()) {
            Some(candidates) if candidates.len() == 1 && unique_source => {
                let new_path = &candidates[0];
                tx.execute(
                    "UPDATE documents SET path = ?1 WHERE path = ?2",
                    rusqlite::params![new_path, old_path],
                )?;
                outcome
                    .repointed
                    .push(((*old_path).clone(), new_path.clone()));
            }
            Some(_) => {
                // Never guess which of several identical-content files is
                // "the" rename. Changing nothing is always recoverable.
                outcome.ambiguous.push((*old_path).clone());
            }
            None => {
                tx.execute(
                    "DELETE FROM documents WHERE path = ?1",
                    rusqlite::params![old_path],
                )?;
                outcome.deleted.push((*old_path).clone());
            }
        }
    }
    tx.commit()?;

    Ok(outcome)
}

/// Delete `user_doc` rows whose vault-relative path contains a `.brain`
/// component, and nothing else. Used only by the empty-walk branch.
///
/// Wrapped in a transaction matching the main path so a mid-loop rusqlite
/// error rolls back rather than leaving a half-deleted index. Chunk cleanup
/// relies on `chunks.doc_id ON DELETE CASCADE`, which fires only with
/// `PRAGMA foreign_keys=ON` — set in `db/connection.rs:35` for every
/// connection opened through the standard path.
fn purge_brain_rows(conn: &Connection, vault_root: &Path) -> Result<ReconcileOutcome> {
    let mut outcome = ReconcileOutcome::default();

    let rows: Vec<String> = {
        let mut stmt = conn.prepare("SELECT path FROM documents WHERE tier = 'user_doc'")?;
        let r = stmt.query_map([], |r| r.get::<_, String>(0))?;
        r.collect::<std::result::Result<Vec<_>, _>>()?
    };

    let doomed: Vec<String> = rows
        .into_iter()
        .filter(|p| crate::walk_vault::abs_path_has_brain_in_vault(Path::new(p), vault_root))
        .collect();

    if doomed.is_empty() {
        return Ok(outcome);
    }

    let tx = conn.unchecked_transaction()?;
    for path in &doomed {
        tx.execute(
            "DELETE FROM documents WHERE path = ?1",
            rusqlite::params![path],
        )?;
        outcome.deleted.push(path.clone());
    }
    tx.commit()?;

    eprintln!(
        "[reconcile] empty walk: purged {} .brain row(s); all other rows preserved",
        outcome.deleted.len()
    );
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

        let out = reconcile_vault(&conn, &[new.clone()], tmp.path()).unwrap();

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

        let out = reconcile_vault(&conn, &[survivor], tmp.path()).unwrap();

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

        let out = reconcile_vault(&conn, &[a, b], tmp.path()).unwrap();

        assert!(out.repointed.is_empty(), "must not guess a rename");
        assert!(
            out.deleted.is_empty(),
            "must not delete what it cannot match"
        );
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

        let out = reconcile_vault(&conn, &[existing], tmp.path()).expect("must not violate UNIQUE");

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

        let out = reconcile_vault(&conn, &[survivor], tmp.path()).unwrap();

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

        let out = reconcile_vault(&conn, &[], Path::new("/vault")).unwrap();

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

        let out = reconcile_vault(&conn, &[f.clone()], tmp.path()).unwrap();

        assert_eq!(out, ReconcileOutcome::default());
        assert_eq!(path_of(&conn, doc_id), s(&f.virtual_path));
        assert_eq!(chunk_count(&conn, doc_id), 6);
    }

    /// Spec item 3a: THE bug this pre-pass exists for. Every empty file
    /// shares one sha256, so a truncated .brain/errors.log row would be
    /// repointed at a newly added empty user file by the unique-hash branch.
    #[test]
    fn excluded_row_is_deleted_not_repointed_on_empty_hash_collision() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let conn = crate::db::connection::open_in_memory().unwrap();

        let brain_path = root.join(".brain").join("errors.log");
        let brain_id = seed_doc(&conn, &s(&brain_path), &hash_of(b""), "user_doc", 3);

        // A newly added empty user file, not yet in the DB.
        let todo = walked(&root, "inbox/todo.md", b"");

        let out = reconcile_vault(&conn, &[todo.clone()], &root).unwrap();

        assert!(
            out.repointed.is_empty(),
            "the .brain row was repointed at a user file: {:?}",
            out.repointed
        );
        assert!(
            out.ambiguous.is_empty(),
            "the .brain row must be deleted, not parked as ambiguous"
        );
        assert!(out.deleted.contains(&s(&brain_path)));
        assert_eq!(chunk_count(&conn, brain_id), 0, "chunks did not cascade");
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM documents WHERE path = ?1",
                [s(&todo.virtual_path)],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0,
            "reconcile must not create a row for the new file"
        );
    }

    /// Second case from the spec: TWO empty candidates. The .brain row must
    /// still be deleted rather than landing in `ambiguous` forever.
    #[test]
    fn excluded_row_deleted_even_with_two_empty_candidates() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let conn = crate::db::connection::open_in_memory().unwrap();

        let brain_path = root.join(".brain").join("errors.log");
        seed_doc(&conn, &s(&brain_path), &hash_of(b""), "user_doc", 1);

        let a = walked(&root, "inbox/a.md", b"");
        let b = walked(&root, "inbox/b.md", b"");

        let out = reconcile_vault(&conn, &[a, b], &root).unwrap();

        assert!(out.deleted.contains(&s(&brain_path)));
        assert!(out.ambiguous.is_empty(), "got {:?}", out.ambiguous);
    }

    /// Spec item 3a scope: the pre-pass is intentionally BROAD -- all of
    /// EXCLUDED_DIRS, not just .brain -- during a non-empty walk.
    #[test]
    fn prepass_deletes_all_excluded_names_on_non_empty_walk() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let conn = crate::db::connection::open_in_memory().unwrap();

        let nm = root.join("node_modules").join("x.md");
        let tgt = root.join("target").join("y.md");
        seed_doc(&conn, &s(&nm), "h1", "user_doc", 1);
        seed_doc(&conn, &s(&tgt), "h2", "user_doc", 1);
        let keep = walked(&root, "notes.md", b"real");
        let keep_id = seed_doc(&conn, &s(&keep.virtual_path), &hash_of(b"real"), "user_doc", 2);

        let out = reconcile_vault(&conn, &[keep.clone()], &root).unwrap();

        assert!(out.deleted.contains(&s(&nm)));
        assert!(out.deleted.contains(&s(&tgt)));
        assert_eq!(path_of(&conn, keep_id), s(&keep.virtual_path));
        assert_eq!(chunk_count(&conn, keep_id), 2);
    }

    /// Spec D1 regression: a vault under an EXCLUDED_DIRS-named ancestor.
    /// A raw absolute-path component check would match `target` on EVERY
    /// row and delete the entire index.
    #[test]
    fn prepass_ignores_excluded_name_in_vault_ancestor() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("target").join("wiki");
        std::fs::create_dir_all(&root).unwrap();
        let conn = crate::db::connection::open_in_memory().unwrap();

        let keep = walked(&root, "notes.md", b"real");
        let keep_id = seed_doc(&conn, &s(&keep.virtual_path), &hash_of(b"real"), "user_doc", 2);
        let phantom = root.join(".brain").join("notes.md");
        seed_doc(&conn, &s(&phantom), "h9", "user_doc", 1);

        let out = reconcile_vault(&conn, &[keep.clone()], &root).unwrap();

        assert_eq!(
            path_of(&conn, keep_id),
            s(&keep.virtual_path),
            "vault under /target/ had its index deleted"
        );
        assert!(!out.deleted.contains(&s(&keep.virtual_path)));
        assert!(out.deleted.contains(&s(&phantom)));
    }

    /// Spec D2b: a row that cannot be relativized against the vault root is
    /// left ALONE, never deleted.
    #[test]
    fn prepass_leaves_unrelativizable_rows_untouched() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("vault");
        std::fs::create_dir_all(&root).unwrap();
        let conn = crate::db::connection::open_in_memory().unwrap();

        let stray = tmp.path().join("elsewhere").join(".brain").join("x.log");
        let stray_id = seed_doc(&conn, &s(&stray), "h1", "user_doc", 1);
        let keep = walked(&root, "notes.md", b"real");
        seed_doc(&conn, &s(&keep.virtual_path), &hash_of(b"real"), "user_doc", 1);

        let out = reconcile_vault(&conn, &[keep], &root).unwrap();

        assert!(
            !out.deleted.contains(&s(&stray)),
            "fail-open violated: deleted a row we could not place in the vault"
        );
        assert_eq!(path_of(&conn, stray_id), s(&stray));
    }

    /// Transaction hoist: when the pre-pass consumes EVERY vanished row the
    /// deletes must still commit -- the old `vanished.is_empty()` early
    /// return sat above the transaction.
    #[test]
    fn prepass_commits_when_it_consumes_every_vanished_row() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let conn = crate::db::connection::open_in_memory().unwrap();

        let brain_path = root.join(".brain").join("errors.log");
        let brain_id = seed_doc(&conn, &s(&brain_path), "h1", "user_doc", 2);
        let keep = walked(&root, "notes.md", b"real");
        let keep_id = seed_doc(&conn, &s(&keep.virtual_path), &hash_of(b"real"), "user_doc", 1);

        let out = reconcile_vault(&conn, &[keep], &root).unwrap();

        assert_eq!(out.deleted, vec![s(&brain_path)]);
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM documents WHERE id = ?1",
                [brain_id],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0,
            "pre-pass delete was not committed"
        );
        assert_eq!(chunk_count(&conn, brain_id), 0);
        assert_eq!(chunk_count(&conn, keep_id), 1);
    }

    /// Spec item 4: the hole punched in the mount-failure safety net is
    /// `.brain`-ONLY. For node_modules/, target/, … an empty walk is not
    /// proof the row should be absent -- it may be a transient unmount,
    /// which is exactly the disaster the guard exists to prevent.
    #[test]
    fn empty_walk_deletes_only_brain_rows() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let conn = crate::db::connection::open_in_memory().unwrap();

        let brain = root.join(".brain").join("errors.log");
        let notes = root.join("notes.md");
        let nm = root.join("node_modules").join("x.md");
        let brain_id = seed_doc(&conn, &s(&brain), "h1", "user_doc", 3);
        let notes_id = seed_doc(&conn, &s(&notes), "h2", "user_doc", 2);
        let nm_id = seed_doc(&conn, &s(&nm), "h3", "user_doc", 1);

        let out = reconcile_vault(&conn, &[], &root).unwrap();

        assert_eq!(out.deleted, vec![s(&brain)]);
        // Spec item 4 asserts the chunk count reaches zero rather than
        // assuming the FK cascade fired.
        assert_eq!(chunk_count(&conn, brain_id), 0);
        assert_eq!(path_of(&conn, notes_id), s(&notes));
        assert_eq!(chunk_count(&conn, notes_id), 2);
        assert_eq!(
            path_of(&conn, nm_id),
            s(&nm),
            "node_modules row must survive an empty walk"
        );
        assert_eq!(chunk_count(&conn, nm_id), 1);
    }

    /// An empty walk on a vault with no .brain rows still changes nothing.
    #[test]
    fn empty_walk_with_no_brain_rows_is_a_no_op() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let conn = crate::db::connection::open_in_memory().unwrap();
        let notes = root.join("notes.md");
        let notes_id = seed_doc(&conn, &s(&notes), "h1", "user_doc", 2);

        let out = reconcile_vault(&conn, &[], &root).unwrap();

        assert_eq!(out, ReconcileOutcome::default());
        assert_eq!(path_of(&conn, notes_id), s(&notes));
    }

    /// Wiki-tier rows never participate, even on the empty-walk path.
    #[test]
    fn empty_walk_leaves_wiki_tier_rows_alone() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let conn = crate::db::connection::open_in_memory().unwrap();
        let wiki = root.join(".brain").join("page.md");
        let wiki_id = seed_doc(&conn, &s(&wiki), "h1", "wiki", 1);

        let out = reconcile_vault(&conn, &[], &root).unwrap();

        assert!(out.deleted.is_empty());
        assert_eq!(path_of(&conn, wiki_id), s(&wiki));
    }
}
