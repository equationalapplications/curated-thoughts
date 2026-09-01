//! Vault event queue: stage file events into the documents table.
//!
//! Spec: docs/superpowers/specs/2026-08-25-ct-headless-cli-phase2-watch.md §6
//!
//! 4-stage path hardening:
//!   1. std::path::absolute (resolve symlinks + .. components)
//!   2. std::fs::canonicalize (resolve filesystem-level symlinks; fallback to absolute)
//!   3. Vault-root guard (if CURATED_VAULT_ROOT is set, reject paths outside it)
//!   4. sha256 the bytes; upsert documents row with status='pending'
//!
//! For Delete events: skip step 4 (file is gone); DELETE the documents row.
//! chunks cascade-delete via FK ON DELETE CASCADE.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use notify::EventKind;
use rusqlite::Connection;

/// Stage a vault file event into the documents table.
///
/// `conn` is a mutable SQLite connection (caller manages lifecycle — ephemeral
/// per call from the desktop watcher callback to avoid holding DbState mutex
/// during the sha256 hashing; long-lived from `ct watch`). WAL mode allows
/// concurrent writer + readers; see src-tauri/src/db/connection.rs:130.
pub fn enqueue_vault_event(
    conn: &mut Connection,
    event_kind: notify::EventKind,
    raw_path: &Path,
) -> Result<()> {
    // Canonicalize the vault root the same way as the event path so that
    // symlinked / non-canonical vault roots (e.g. macOS /var → /private/var)
    // can't bypass the containment check. Without this, an event for
    // `/var/vault/note.md` would fail `starts_with("/var/vault")` against a
    // canonicalized `/private/var/vault/note.md` and be silently dropped.
    let vault_root = std::env::var_os("CURATED_VAULT_ROOT")
        .map(|s| {
            let p = PathBuf::from(s);
            std::path::absolute(&p).map(|abs| std::fs::canonicalize(&abs).unwrap_or(abs))
        })
        .transpose()?;

    let abs = std::path::absolute(raw_path)?;
    let canonical = std::fs::canonicalize(&abs).unwrap_or(abs.clone());

    if let Some(vr) = &vault_root {
        if !canonical.starts_with(vr) {
            eprintln!(
                "[watch] skipping out-of-vault path: {}",
                canonical.display()
            );
            return Ok(());
        }
    }

    let path_str = canonical.to_string_lossy().into_owned();

    if matches!(event_kind, EventKind::Remove(_)) {
        conn.execute(
            "DELETE FROM documents WHERE path = ?1",
            rusqlite::params![&path_str],
        )?;
        return Ok(());
    }

    // Add / Modify: hash, upsert.
    let bytes =
        std::fs::read(&canonical).with_context(|| format!("read {}", canonical.display()))?;
    let hash = sha256_hex(&bytes);

    conn.execute(
        "INSERT INTO documents (path, hash, tier, status) \
         VALUES (?1, ?2, 'user_doc', 'pending') \
         ON CONFLICT(path) DO UPDATE SET \
            hash = excluded.hash, \
            status = 'pending' \
         WHERE documents.hash != excluded.hash \
            OR documents.status IN ('pending','error','orphaned')",
        rusqlite::params![&path_str, &hash],
    )?;
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::enqueue_vault_event;
    use crate::db::connection::open_in_memory;
    use crate::db::queries::upsert_document;
    use rusqlite::Connection;
    use std::path::PathBuf;

    // ---- enqueue_vault_event fixtures ----------------------------------

    /// Inline subset of MIGRATION_V1 + V11 sufficient for
    /// `enqueue_vault_event` tests. Keeping the schema local avoids
    /// coupling `queue` to the canonical `src-tauri/src/db/schema.rs`
    /// (which is bigger and changes independently); the columns touched
    /// by `enqueue_vault_event` plus the V11 watermark columns needed by
    /// the dirty-doc selection tests are stable.
    fn enqueue_test_schema_sql() -> &'static str {
        "CREATE TABLE documents (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            path            TEXT    NOT NULL UNIQUE,
            hash            TEXT    NOT NULL,
            tier            TEXT    NOT NULL CHECK(tier IN ('user_doc', 'wiki')),
            folder_rules_id INTEGER,
            last_indexed    INTEGER,
            status          TEXT    NOT NULL DEFAULT 'pending'
                            CHECK(status IN ('pending', 'indexed', 'error', 'orphaned')),
            synth_hash      TEXT,
            synth_model     TEXT,
            synth_at        INTEGER
        );"
    }

    /// Open a fresh in-memory sqlite connection with only the columns
    /// `enqueue_vault_event` touches applied. Using a raw (non-migrated)
    /// connection avoids coupling the test to the canonical migration
    /// stack; if the canonical schema changes the production upsert paths
    /// still validate.
    fn open_seeded_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open raw in-memory brain db");
        conn.execute_batch(enqueue_test_schema_sql())
            .expect("apply minimal documents schema");
        conn
    }

    /// Touch the migrated in-memory schema via `open_in_memory()` (matches
    /// the project style for src-tauri-side tests). Some tests don't need
    /// the full migration stack; those use `open_seeded_conn()` instead.
    #[allow(dead_code)]
    fn open_migrated_conn() -> Connection {
        open_in_memory().expect("open migrated in-memory brain db")
    }

    /// Create a temp vault dir and pre-populate it with a file so notify
    /// events have a real path to canonicalize.
    fn setup_vault_with_file(body: &[u8]) -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("note.md");
        std::fs::write(&path, body).unwrap();
        (tmp, path)
    }

    // ---- enqueue_vault_event tests -------------------------------------

    #[test]
    fn enqueue_add_creates_pending_row() {
        let (vault, file_path) = setup_vault_with_file(b"hello world");
        let mut conn = open_seeded_conn();

        // `enqueue_vault_event` reads CURATED_VAULT_ROOT from the process
        // environment, which is shared by every test in this binary. The
        // `enqueue_out_of_vault_*` tests below set it via `temp_env`, so a
        // test that leaves it unset sees THEIR vault root when it happens to
        // run concurrently — its own TempDir path then fails the containment
        // check, the event is silently skipped, and the assertion below fails
        // intermittently. Pin it to this test's own vault (and take the same
        // `temp_env` lock) so the guard is deterministic.
        temp_env::with_var("CURATED_VAULT_ROOT", Some(vault.path()), || {
            enqueue_vault_event(
                &mut conn,
                notify::EventKind::Create(notify::event::CreateKind::Any),
                &file_path,
            )
            .unwrap();
        });

        let mut stmt = conn
            .prepare("SELECT status, hash, tier FROM documents WHERE path = ?1")
            .unwrap();
        let canonical = std::fs::canonicalize(&file_path).unwrap();
        let mut rows = stmt
            .query(rusqlite::params![canonical.to_string_lossy()])
            .unwrap();
        let row = rows.next().unwrap().unwrap();
        let status: String = row.get(0).unwrap();
        let hash: String = row.get(1).unwrap();
        let tier: String = row.get(2).unwrap();
        assert_eq!(status, "pending");
        assert_eq!(hash, super::sha256_hex(b"hello world"));
        assert_eq!(tier, "user_doc");
        // Touch the vault so the TempDir isn't optimized away (it owns the
        // canonicalized path's parent).
        let _ = vault.path();
    }

    #[test]
    fn enqueue_modify_indexed_with_diff_hash_flips_to_pending() {
        let (vault, file_path) = setup_vault_with_file(b"v1");
        let mut conn = open_seeded_conn();

        // Pre-seed: indexed doc with stale hash + synth watermark.
        let canonical = std::fs::canonicalize(&file_path).unwrap();
        let path_str = canonical.to_string_lossy().into_owned();
        upsert_document(&conn, &path_str, "stale-hash").unwrap();
        conn.execute(
            "UPDATE documents SET status = 'indexed', synth_hash = 'stale-hash', synth_model = 'm' WHERE path = ?1",
            rusqlite::params![&path_str],
        )
        .unwrap();

        // Now mutate the file on disk; Modify event arrives with the new hash.
        std::fs::write(&canonical, b"v2").unwrap();

        // `enqueue_vault_event` reads CURATED_VAULT_ROOT from the process
        // environment, which is shared by every test in this binary. The
        // `enqueue_out_of_vault_*` tests below set it via `temp_env`, so a
        // test that leaves it unset sees THEIR vault root when it happens to
        // run concurrently — its own TempDir path then fails the containment
        // check, the event is silently skipped, and the assertion below fails
        // intermittently. Pin it to this test's own vault (and take the same
        // `temp_env` lock) so the guard is deterministic.
        temp_env::with_var("CURATED_VAULT_ROOT", Some(vault.path()), || {
            enqueue_vault_event(
                &mut conn,
                notify::EventKind::Modify(notify::event::ModifyKind::Any),
                &file_path,
            )
            .unwrap();
        });

        let mut stmt = conn
            .prepare("SELECT status, hash FROM documents WHERE path = ?1")
            .unwrap();
        let mut rows = stmt.query(rusqlite::params![&path_str]).unwrap();
        let row = rows.next().unwrap().unwrap();
        let status: String = row.get(0).unwrap();
        let hash: String = row.get(1).unwrap();
        assert_eq!(status, "pending");
        assert_eq!(hash, super::sha256_hex(b"v2"));
        let _ = vault.path();
    }

    #[test]
    fn enqueue_modify_indexed_with_same_hash_is_noop() {
        let (vault, file_path) = setup_vault_with_file(b"stable");
        let mut conn = open_seeded_conn();

        let canonical = std::fs::canonicalize(&file_path).unwrap();
        let path_str = canonical.to_string_lossy().into_owned();
        let stable_hash = super::sha256_hex(b"stable");
        upsert_document(&conn, &path_str, &stable_hash).unwrap();
        conn.execute(
            "UPDATE documents SET status = 'indexed', synth_hash = ?2, synth_model = 'm' WHERE path = ?1",
            rusqlite::params![&path_str, &stable_hash],
        )
        .unwrap();

        // No filesystem mutation; Modify event fires but bytes are unchanged.
        // `enqueue_vault_event` reads CURATED_VAULT_ROOT from the process
        // environment, which is shared by every test in this binary. The
        // `enqueue_out_of_vault_*` tests below set it via `temp_env`, so a
        // test that leaves it unset sees THEIR vault root when it happens to
        // run concurrently — its own TempDir path then fails the containment
        // check, the event is silently skipped, and the assertion below fails
        // intermittently. Pin it to this test's own vault (and take the same
        // `temp_env` lock) so the guard is deterministic.
        temp_env::with_var("CURATED_VAULT_ROOT", Some(vault.path()), || {
            enqueue_vault_event(
                &mut conn,
                notify::EventKind::Modify(notify::event::ModifyKind::Any),
                &file_path,
            )
            .unwrap();
        });

        let mut stmt = conn
            .prepare("SELECT status FROM documents WHERE path = ?1")
            .unwrap();
        let mut rows = stmt.query(rusqlite::params![&path_str]).unwrap();
        let row = rows.next().unwrap().unwrap();
        let status: String = row.get(0).unwrap();
        assert_eq!(
            status, "indexed",
            "Modify with unchanged bytes must not flip status back to pending"
        );
        let _ = vault.path();
    }

    #[test]
    fn enqueue_delete_removes_row() {
        let (vault, file_path) = setup_vault_with_file(b"to-be-deleted");
        let mut conn = open_seeded_conn();

        let canonical = std::fs::canonicalize(&file_path).unwrap();
        let path_str = canonical.to_string_lossy().into_owned();
        upsert_document(&conn, &path_str, "h").unwrap();
        let count_before: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM documents WHERE path = ?1",
                rusqlite::params![&path_str],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count_before, 1);

        // `enqueue_vault_event` reads CURATED_VAULT_ROOT from the process
        // environment, which is shared by every test in this binary. The
        // `enqueue_out_of_vault_*` tests below set it via `temp_env`, so a
        // test that leaves it unset sees THEIR vault root when it happens to
        // run concurrently — its own TempDir path then fails the containment
        // check, the event is silently skipped, and the assertion below fails
        // intermittently. Pin it to this test's own vault (and take the same
        // `temp_env` lock) so the guard is deterministic.
        temp_env::with_var("CURATED_VAULT_ROOT", Some(vault.path()), || {
            enqueue_vault_event(
                &mut conn,
                notify::EventKind::Remove(notify::event::RemoveKind::Any),
                &file_path,
            )
            .unwrap();
        });

        let count_after: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM documents WHERE path = ?1",
                rusqlite::params![&path_str],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count_after, 0, "Remove event must delete the documents row");
        let _ = vault.path();
    }

    #[test]
    fn enqueue_delete_missing_row_is_noop() {
        let (vault, file_path) = setup_vault_with_file(b"never-indexed");
        let mut conn = open_seeded_conn();

        let canonical = std::fs::canonicalize(&file_path).unwrap();
        let _path_str = canonical.to_string_lossy().into_owned();
        let before: i64 = conn
            .query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))
            .unwrap();
        assert_eq!(before, 0);

        // Remove event for a path we never ingested must not error.
        // `enqueue_vault_event` reads CURATED_VAULT_ROOT from the process
        // environment, which is shared by every test in this binary. The
        // `enqueue_out_of_vault_*` tests below set it via `temp_env`, so a
        // test that leaves it unset sees THEIR vault root when it happens to
        // run concurrently — its own TempDir path then fails the containment
        // check, the event is silently skipped, and the assertion below fails
        // intermittently. Pin it to this test's own vault (and take the same
        // `temp_env` lock) so the guard is deterministic.
        temp_env::with_var("CURATED_VAULT_ROOT", Some(vault.path()), || {
            enqueue_vault_event(
                &mut conn,
                notify::EventKind::Remove(notify::event::RemoveKind::Any),
                &file_path,
            )
            .unwrap();
        });

        let after: i64 = conn
            .query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))
            .unwrap();
        assert_eq!(after, 0);
        let _ = vault.path();
    }

    #[test]
    fn enqueue_out_of_vault_path_is_rejected() {
        // Use temp_env to set CURATED_VAULT_ROOT to a directory that does
        // NOT contain the file we touch — the event must be rejected.
        let inner_vault = tempfile::TempDir::new().unwrap();
        let outer_vault = tempfile::TempDir::new().unwrap();
        let outer_file = outer_vault.path().join("escapee.md");
        std::fs::write(&outer_file, b"outside").unwrap();
        let mut conn = open_seeded_conn();

        temp_env::with_var("CURATED_VAULT_ROOT", Some(inner_vault.path()), || {
            enqueue_vault_event(
                &mut conn,
                notify::EventKind::Create(notify::event::CreateKind::Any),
                &outer_file,
            )
            .unwrap();
        });

        let canonical = std::fs::canonicalize(&outer_file).unwrap();
        let path_str = canonical.to_string_lossy().into_owned();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM documents WHERE path = ?1",
                rusqlite::params![&path_str],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "out-of-vault event must not insert a row");
    }

    /// Positive counterpart to `enqueue_out_of_vault_path_is_rejected`: a
    /// path inside the vault must be ingested when `CURATED_VAULT_ROOT` is
    /// set. Without this, a regression that simply skips every event would
    /// pass the negative test alone.
    #[test]
    fn enqueue_in_vault_path_is_accepted() {
        let vault = tempfile::TempDir::new().unwrap();
        let note = vault.path().join("note.md");
        std::fs::write(&note, b"inside").unwrap();
        let mut conn = open_seeded_conn();

        temp_env::with_var("CURATED_VAULT_ROOT", Some(vault.path()), || {
            enqueue_vault_event(
                &mut conn,
                notify::EventKind::Create(notify::event::CreateKind::Any),
                &note,
            )
            .unwrap();
        });

        let canonical = std::fs::canonicalize(&note).unwrap();
        let path_str = canonical.to_string_lossy().into_owned();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM documents WHERE path = ?1",
                rusqlite::params![&path_str],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "in-vault event must insert a row");
        let _ = vault.path();
    }
}
