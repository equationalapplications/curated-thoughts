use anyhow::{bail, Context, Result};
use rusqlite::OpenFlags;
use serde::Serialize;
use tauri_app_lib::retrieval::{self, BrainPaths};

pub struct Brain {
    pub paths: BrainPaths,
}

pub fn resolve() -> Result<Brain> {
    let paths = retrieval::resolve_brain_paths();
    if !paths.db_path.exists() {
        bail!(
            "brain.db not found at {} — run ingest first",
            paths.db_path.display()
        );
    }
    Ok(Brain { paths })
}

pub fn open_ro(brain: &Brain) -> Result<rusqlite::Connection> {
    Ok(retrieval::open_brain_readonly(&brain.paths.db_path)?)
}

pub fn open_rw(brain: &Brain) -> Result<rusqlite::Connection> {
    rusqlite::Connection::open_with_flags(
        &brain.paths.db_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
    )
    .context("opening brain.db read-write")
}

/// Exit code contract: 0 ok, 1 error, 2 no results.
pub const EXIT_NO_RESULTS: i32 = 2;

pub fn print_json<T: Serialize>(v: &T) {
    println!("{}", serde_json::to_string_pretty(v).expect("serialize"));
}
