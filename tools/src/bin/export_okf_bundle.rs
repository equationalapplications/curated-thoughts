//! `export_okf_bundle` — headless OKF bundle export of the whole brain.
//!
//! Nightly-backup companion to the GUI's `okf_export_bundle_cmd`: same
//! `load_export_entities` + `write_bundle_with_profile` code path (v0.2,
//! profile `llm-wiki/2`), so the output is byte-compatible with what the
//! desktop app writes. Read-only against the live DB — safe to run while
//! the app is open (WAL readers don't block writers).
//!
//! Usage: `export_okf_bundle <dest.zip>` (defaults to `$HOME/brain-okf.zip`).
//! Prints `exported entities=<n> files=<n> sha256=<hex> path=<p>` on success.

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::PathBuf;
use tauri_app_lib::okf::bundle_write::write_bundle_with_profile;
use tauri_app_lib::okf::types::{LLM_WIKI_PROFILE_V2, OKF_VERSION_V2};
use tauri_app_lib::okf::zip_io::write_bundle_zip;

fn main() -> Result<()> {
    let dest = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join("brain-okf.zip"));

    let brain = curated_thoughts_tools::paths::resolve_brain_paths();
    let conn =
        Connection::open_with_flags(&brain.db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_context(|| format!("opening brain db at {}", brain.db_path.display()))?;

    let entities = tauri_app_lib::db::bundle_io::load_export_entities(&conn, None)
        .context("loading entities for export")?;
    let entity_count = entities.len();
    if entity_count == 0 {
        anyhow::bail!("Nothing to export: no entities in the brain.");
    }

    let files = write_bundle_with_profile(&entities, LLM_WIKI_PROFILE_V2, OKF_VERSION_V2)
        .map_err(|e| anyhow::anyhow!(e))?;
    let file_count = files.len();

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    write_bundle_zip(&dest, &files).with_context(|| format!("writing {}", dest.display()))?;

    // Digest for the watchdog log so each night's artifact is verifiable.
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(&dest)?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let digest: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();

    println!(
        "exported entities={} files={} sha256={} path={}",
        entity_count,
        file_count,
        digest,
        dest.display()
    );
    Ok(())
}
