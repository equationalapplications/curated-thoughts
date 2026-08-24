//! One-off: ingest every ingestible file in the configured vault into the brain DB.
//! Honors the pipeline's extension filter (code, docs, configs) and skips build
//! artifacts / VCS dirs. Symlinked directories directly under the vault root are
//! followed one level (spec 2026-05-05-second-brain-app-design.md L228).
use anyhow::{Context as _, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use tauri_app_lib::chunker::should_ingest_extension;
use tauri_app_lib::db::connection::AppDb;
use tauri_app_lib::indexer::linker::run_linker;
use tauri_app_lib::retrieval;
use tauri_app_lib::vault::VaultConfig;
use tauri_app_lib::{entity_id_for_path, ingest_document_with_vault_root};

/// Directory names never ingested (build artifacts, deps, VCS internals).
const EXCLUDED_DIRS: &[&str] = &[
    "target", "node_modules", "dist", "dist-newstyle", ".git", ".github",
    ".next", ".turbo", ".cache", "coverage", "build", "out", ".venv",
    "venv", "__pycache__", ".idea", ".vscode", ".fastembed_cache",
];

fn is_excluded_dir(dir_name: &str) -> bool {
    EXCLUDED_DIRS.contains(&dir_name)
}

/// Collect files from a directory tree. `follow` marks a symlinked root whose
/// children we take one level deep only (per spec: follow one level, no
/// recursion through further symlinks).
fn collect_files(root: &Path, follow_symlinks_one_level: bool, out: &mut Vec<PathBuf>) {
    let it = walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            // Skip excluded dirs by name at any depth.
            if e.file_type().is_dir() {
                if let Some(name) = e.path().file_name() {
                    return !is_excluded_dir(&name.to_string_lossy());
                }
            }
            true
        });
    for entry in it.flatten() {
        let p = entry.path();
        let ft = entry.file_type();
        if ft.is_file()
            && p.extension()
                .map(|e| should_ingest_extension(&e.to_string_lossy()))
                .unwrap_or(false)
        {
            out.push(p.to_path_buf());
        } else if follow_symlinks_one_level && ft.is_symlink() {
            // One-level symlink follow: descend into the target but never
            // recurse through nested symlinks. (Depth is relative to the walk
            // root, so any top-level symlinked dir qualifies regardless of
            // which subdirectory of the vault holds it.)
            match std::fs::canonicalize(p) {
                Ok(target) => collect_files(&target, false, out),
                Err(e) => eprintln!("warn: broken symlink {}, skipping: {e}", p.display()),
            }
        }
    }
}

fn main() -> Result<()> {
    let paths_b = retrieval::resolve_brain_paths();
    let profile =
        retrieval::load_embed_profile(&paths_b.config_path).context("read embed profile")?;
    let db = AppDb::open(&paths_b.db_path).context("open brain database")?;
    let conn = &db.0;

    let config = VaultConfig::new(paths_b.config_path.clone());
    let vault_root = config
        .vault_root()
        .context("read vault root")?
        .ok_or_else(|| anyhow::anyhow!("vault root missing"))?;
    let vault_root = vault_root.canonicalize().unwrap_or(vault_root);

    let mut files = Vec::new();
    collect_files(&vault_root, true, &mut files);
    files.sort();
    files.dedup();

    println!(
        "ingesting {} file(s) from {}",
        files.len(),
        vault_root.display()
    );

    let vault_root_str = vault_root.to_str().unwrap();
    let mut entity_ids = HashSet::new();
    let mut failed = 0usize;
    for (i, f) in files.iter().enumerate() {
        match ingest_document_with_vault_root(
            conn,
            &profile,
            f.to_str().unwrap(),
            true,
            Some(vault_root_str),
        ) {
            Ok(_) => {
                entity_ids.insert(entity_id_for_path(
                    f.to_str().unwrap(),
                    Some(vault_root_str),
                ));
                println!("[{}/{}] ok: {}", i + 1, files.len(), f.display());
            }
            Err(e) => {
                failed += 1;
                eprintln!("[{}/{}] FAILED {}: {}", i + 1, files.len(), f.display(), e);
                let mut src = e.source();
                while let Some(s) = src {
                    eprintln!("    caused by: {s}");
                    src = s.source();
                }
            }
        }
    }

    for entity_id in &entity_ids {
        if let Err(e) = run_linker(conn, entity_id, 0) {
            eprintln!("[linker] {}: {}", entity_id, e);
        }
    }
    println!(
        "done: {} docs, {} entities, {} failed",
        files.len(),
        entity_ids.len(),
        failed
    );
    Ok(())
}
