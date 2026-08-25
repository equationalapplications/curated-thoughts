//! tools/src/write.rs
//!
//! Low-level DB connection helpers for the `ct` headless CLI.
//!
//! `Brain` is the existing struct returned by `cli_common::resolve()`; the
//! wrappers here take `&Brain` so callers and integration tests can switch
//! `cli_common::open_ro` → `crate::write::open_ro` without churning call
//! sites. The actual open logic stays in `tauri_app_lib::retrieval` (for
//! read-only) and the read-write flag set already used by `cli_common`
//! (for read-write).

use crate::cli_common::Brain;
use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};

/// Open a read-only connection to the brain database.
pub fn open_ro(brain: &Brain) -> Result<Connection> {
    tauri_app_lib::retrieval::open_brain_readonly(&brain.paths.db_path)
}

/// Open a read-write connection, creating the database file if it does not
/// exist. Mirrors the pragmas `cli_common::open_rw` has historically applied.
pub fn open_rw(brain: &Brain) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        &brain.paths.db_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
    )
    .with_context(|| format!("open rw {}", brain.paths.db_path.display()))?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::{resolve_brain_paths, BrainPaths};
    use tempfile::TempDir;

    /// `open_rw` should create the DB file when it doesn't exist and let us
    /// execute a trivial statement.
    #[test]
    fn open_rw_creates_db_file() {
        let tmp = TempDir::new().unwrap();
        let paths = BrainPaths {
            brain_dir: tmp.path().to_path_buf(),
            config_path: tmp.path().join("config.json"),
            db_path: tmp.path().join("brain.db"),
        };
        let brain = Brain { paths };
        let conn = open_rw(&brain).expect("open_rw should succeed on missing file");
        conn.execute_batch("CREATE TABLE t(x INTEGER);")
            .expect("trivial DDL should succeed");
    }

    /// `open_ro` must fail cleanly when the DB file is missing.
    #[test]
    fn open_ro_errors_when_db_missing() {
        let tmp = TempDir::new().unwrap();
        let paths = BrainPaths {
            brain_dir: tmp.path().to_path_buf(),
            config_path: tmp.path().join("config.json"),
            db_path: tmp.path().join("brain.db"),
        };
        let brain = Brain { paths };
        let result = open_ro(&brain);
        assert!(
            result.is_err(),
            "open_ro on missing brain.db must error, got {:?}",
            result
        );
        let msg = format!("{}", result.err().unwrap());
        assert!(
            msg.contains("brain.db not found"),
            "error message should mention missing brain.db, got: {msg}"
        );
    }

    /// `open_ro` should succeed on a freshly-created DB.
    #[test]
    fn open_ro_succeeds_on_existing_db() {
        let tmp = TempDir::new().unwrap();
        let paths = BrainPaths {
            brain_dir: tmp.path().to_path_buf(),
            config_path: tmp.path().join("config.json"),
            db_path: tmp.path().join("brain.db"),
        };
        let brain = Brain { paths };
        // Create the file via open_rw first.
        {
            let conn = open_rw(&brain).expect("open_rw should create file");
            conn.execute_batch("CREATE TABLE t(x INTEGER);").unwrap();
        }
        let conn = open_ro(&brain).expect("open_ro on existing DB must succeed");
        // Read-only: SELECT works, CREATE should fail.
        conn.execute_batch("SELECT 1").expect("SELECT must work");
    }

    /// `resolve_brain_paths` is re-exported and still callable.
    #[test]
    fn resolve_brain_paths_is_callable() {
        let _ = resolve_brain_paths();
    }
}