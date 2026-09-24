//! `export_okf_bundle` — headless OKF bundle export of the whole brain.
//!
//! Nightly-backup companion to the GUI's `okf_export_bundle_cmd`: the same
//! `load_export_entities` + `write_bundle_with_profile` code path (v0.2,
//! profile `llm-wiki/2`), so the bundle content matches what the desktop app
//! writes — except that, being read-only against the live DB, it records no
//! `exported` event rows. All load queries run inside one deferred
//! transaction, so the snapshot is consistent even if the app writes while
//! the export runs (WAL readers never block writers). The zip is written to a
//! temp file and renamed into place only after the write AND a full
//! parse-back self-check succeed, so a failed run never destroys the
//! previous backup. The self-check enforces the same import limits the
//! reader applies (`MAX_ZIP_ENTRIES` / `MAX_TOTAL_BYTES`), so an oversized
//! brain fails loudly here instead of producing a nightly backup that
//! import would refuse to restore.
//!
//! Usage: `export_okf_bundle <dest.zip>` (defaults to `$HOME/brain-okf.zip`).
//! Prints `exported entities=<n> files=<n> sha256=<hex> path=<p>` on success.

use anyhow::{bail, Context, Result};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use tauri_app_lib::okf::bundle_read::parse_bundle;
use tauri_app_lib::okf::bundle_write::write_bundle_with_profile;
use tauri_app_lib::okf::types::{LLM_WIKI_PROFILE_V2, OKF_VERSION_V2};
use tauri_app_lib::okf::zip_io::{read_bundle_source, write_bundle_zip};

fn sha256_hex(path: &Path) -> Result<String> {
    let mut file =
        std::fs::File::open(path).with_context(|| format!("hashing {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file.read(&mut buf).context("reading while hashing")?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn main() -> Result<()> {
    let dest = match std::env::args().nth(1) {
        Some(arg) => {
            if arg == "--help" || arg == "-h" {
                eprintln!("usage: export_okf_bundle [dest.zip]");
                eprintln!();
                eprintln!("Exports the whole brain as an OKF 0.2 bundle.");
                eprintln!("Defaults to $HOME/brain-okf.zip. Honors CURATED_BRAIN_* env vars.");
                std::process::exit(0);
            }
            if arg.starts_with('-') {
                eprintln!("error: unknown flag {arg}");
                eprintln!("usage: export_okf_bundle [dest.zip]");
                std::process::exit(2);
            }
            PathBuf::from(arg)
        }
        None => {
            let home = dirs::home_dir().context("cannot resolve $HOME; pass an explicit dest")?;
            home.join("brain-okf.zip")
        }
    };

    let brain = curated_thoughts_tools::paths::resolve_brain_paths();
    let conn =
        Connection::open_with_flags(&brain.db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_context(|| format!("opening brain db at {}", brain.db_path.display()))?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .context("setting busy timeout")?;

    // One deferred transaction = one consistent snapshot across the entity
    // list and every per-entity fact/task/edge/event query that follows.
    conn.execute_batch("BEGIN DEFERRED")
        .context("beginning export snapshot")?;

    let entities = tauri_app_lib::db::bundle_io::load_export_entities(&conn, None)
        .context("loading entities for export")?;
    let entity_count = entities.len();
    if entity_count == 0 {
        anyhow::bail!("Nothing to export: no entities in the brain.");
    }

    let files = write_bundle_with_profile(&entities, LLM_WIKI_PROFILE_V2, OKF_VERSION_V2)
        .map_err(|e| anyhow::anyhow!(e))?;
    let file_count = files.len();
    conn.execute_batch("COMMIT")
        .context("committing snapshot")?;

    // Self-check BEFORE publishing: same entry-count / decompressed-size
    // caps the import reader enforces, so a backup that would be refused on
    // restore fails here instead.
    let total_bytes: u64 = files.iter().map(|f| f.content.len() as u64).sum();
    if file_count > tauri_app_lib::okf::zip_io::MAX_ZIP_ENTRIES {
        bail!(
            "export would produce {file_count} files; import cap is {} — \
             bundle would not be restorable",
            tauri_app_lib::okf::zip_io::MAX_ZIP_ENTRIES
        );
    }
    if total_bytes > tauri_app_lib::okf::zip_io::MAX_TOTAL_BYTES {
        bail!(
            "export would produce {total_bytes} decompressed bytes; import cap is {} — \
             bundle would not be restorable",
            tauri_app_lib::okf::zip_io::MAX_TOTAL_BYTES
        );
    }

    let dest_dir = if dest
        .parent()
        .map(|p| p.as_os_str().is_empty())
        .unwrap_or(true)
    {
        std::path::PathBuf::from(".")
    } else {
        dest.parent().unwrap().to_path_buf()
    };

    std::fs::create_dir_all(&dest_dir)
        .with_context(|| format!("creating {}", dest_dir.display()))?;

    // Write to a temp sibling, verify, then atomically publish. A failure at
    // any point leaves the previous backup untouched. The temp file is
    // pre-created 0o600 (umask-independent) so the rename can never widen the
    // permissions of an existing 0o600 backup; the bundle is unredacted.
    let tmp = dest.with_extension("zip.partial");
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .with_context(|| format!("creating {}", tmp.display()))?;
    }
    write_bundle_zip(&tmp, &files).with_context(|| format!("writing {}", tmp.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("restricting {}", tmp.display()))?;
    }

    // Round-trip: parse the written bundle back through the real reader.
    let reparsed =
        read_bundle_source(&tmp).with_context(|| format!("re-reading {}", tmp.display()))?;
    parse_bundle(&reparsed).context("round-trip parse of the written bundle failed")?;

    std::fs::rename(&tmp, &dest)
        .with_context(|| format!("publishing {} over {}", tmp.display(), dest.display()))?;

    #[cfg(unix)]
    {
        let dir = std::fs::File::open(&dest_dir)
            .with_context(|| format!("opening {}", dest_dir.display()))?;
        dir.sync_all()
            .with_context(|| format!("syncing {}", dest_dir.display()))?;
    }

    let digest = sha256_hex(&dest)?;
    println!(
        "exported entities={} files={} sha256={} path={}",
        entity_count,
        file_count,
        digest,
        dest.display()
    );
    Ok(())
}
