//! tools/src/write.rs
//!
//! Low-level DB connection helpers for the `ct` headless CLI.
//!
//! `Brain` is the struct that wraps a `BrainPaths` and is passed to
//! `open_ro` / `open_rw`. It used to live in `cli_common::resolve`;
//! phase 2 of the headless CLI split relocates it here so write-side
//! helpers and read-side helpers share a single home.

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};

use crate::paths::BrainPaths;

/// Brain layout: the resolved `BrainPaths` for the current process. Cheap
/// to clone (just three `PathBuf`s); passing by `&Brain` lets `open_ro`
/// and `open_rw` share the same layout contract.
#[derive(Debug, Clone)]
pub struct Brain {
    pub paths: BrainPaths,
}

/// Resolve `BrainPaths` from env. Errors when no `brain.db` exists at the
/// resolved path — `ct` subcommands that follow should fail fast rather
/// than try to open a non-existent database. Extracted from
/// `cli_common::resolve` in phase 2 task 5.
pub fn resolve() -> Result<Brain> {
    use anyhow::{bail, Context};
    let paths = crate::paths::resolve_brain_paths();
    if !paths.db_path.exists() {
        bail!(
            "brain.db not found at {} — run ingest first",
            paths.db_path.display()
        );
    }
    // open_rw historically also exercised the file (it used
    // SQLITE_OPEN_READ_WRITE which is a stat-only call). Mirror that to
    // surface any I/O error early.
    let _ = std::fs::metadata(&paths.db_path)
        .with_context(|| format!("stat {}", paths.db_path.display()))?;
    Ok(Brain { paths })
}

/// Exit code contract for the `ct` CLI: 0 ok, 1 error, 2 no results.
pub const EXIT_NO_RESULTS: i32 = 2;

/// Open a read-only connection to the brain database.
pub fn open_ro(brain: &Brain) -> Result<Connection> {
    tauri_app_lib::retrieval::open_brain_readonly(&brain.paths.db_path)
}

/// Open a read-write connection, creating the database file if it does not
/// exist. Mirrors the pragmas `cli_common::open_rw` historically applied.
///
/// **Busy-timeout pragma (CodeRabbit review on PR #96):** every
/// writer connection that participates in the watcher's per-event
/// reopen must set `PRAGMA busy_timeout = 5000`. Otherwise SQLite's
/// default is 0 — meaning a transient lock from the desktop's WAL
/// checkpoint (or another concurrent ingest) instantly fails with
/// `SQLITE_BUSY`, leaving the event silently dropped. 5s is the
/// value used by `tauri_app_lib::db::AppDb`; matching it keeps the
/// watcher's contention behavior consistent with the desktop.
pub fn open_rw(brain: &Brain) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        &brain.paths.db_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
    )
    .with_context(|| format!("open rw {}", brain.paths.db_path.display()))?;
    // 5s busy timeout — see doc above.
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .context("set busy_timeout on rw connection")?;
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
