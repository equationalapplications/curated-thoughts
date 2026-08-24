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
    "target",
    "node_modules",
    "dist",
    "dist-newstyle",
    ".git",
    ".github",
    ".next",
    ".turbo",
    ".cache",
    "coverage",
    "build",
    "out",
    ".venv",
    "venv",
    "__pycache__",
    ".idea",
    ".vscode",
    ".fastembed_cache",
];

fn is_excluded_dir(dir_name: &str) -> bool {
    EXCLUDED_DIRS.contains(&dir_name)
}

/// Collect files from a directory tree. `follow_symlinked_doc_dirs` enables
/// following symlinked directories whose parent is exactly
/// `<vault_root>/documents` (the staging contract); nested symlinks and
/// symlinks to files are never followed. Traversal errors are returned so an
/// unreadable path can't silently shrink the corpus.
fn collect_files(
    root: &Path,
    follow_symlinked_doc_dirs: bool,
    out: &mut Vec<PathBuf>,
    errors: &mut Vec<String>,
) {
    let walker = walkdir::WalkDir::new(root).follow_links(false);
    let mut it = walker.into_iter().filter_entry(|e| {
        // Skip excluded dirs by name at any depth.
        if e.file_type().is_dir() {
            if let Some(name) = e.path().file_name() {
                return !is_excluded_dir(&name.to_string_lossy());
            }
        }
        true
    });
    while let Some(entry) = it.next() {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                errors.push(format!("traversal: {e}"));
                continue;
            }
        };
        let p = entry.path();
        let ft = entry.file_type();
        if ft.is_file()
            && p.extension()
                .map(|e| should_ingest_extension(&e.to_string_lossy()))
                .unwrap_or(false)
        {
            out.push(p.to_path_buf());
        } else if follow_symlinked_doc_dirs && ft.is_symlink() {
            // Only follow symlinks that are DIRECT children of
            // <root>/documents, whose names aren't excluded, and whose target
            // is a directory. Never follow file symlinks or nested ones.
            let parent_is_documents = p
                .parent()
                .map(|par| par.file_name().map(|n| n == "documents").unwrap_or(false))
                .unwrap_or(false)
                && entry.depth() == 1;
            let name_excluded = p
                .file_name()
                .map(|n| is_excluded_dir(&n.to_string_lossy()))
                .unwrap_or(false);
            if !parent_is_documents || name_excluded {
                continue;
            }
            match std::fs::canonicalize(p) {
                Ok(target) if target.is_dir() => {
                    // Recurse into the resolved target with symlink-following
                    // OFF, so nested symlinks inside are never descended into.
                    collect_files(&target, false, out, errors)
                }
                Ok(_) => eprintln!(
                    "warn: symlink {} does not point at a directory, skipping",
                    p.display()
                ),
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
    let mut walk_errors = Vec::new();
    collect_files(&vault_root, true, &mut files, &mut walk_errors);
    files.sort();
    files.dedup();

    // Traversal errors count as failures so an unreadable path can't make a
    // partial run look complete.
    let mut failed = walk_errors.len();
    for e in &walk_errors {
        eprintln!("warn: {e}");
    }
    println!(
        "ingesting {} file(s) from {}",
        files.len(),
        vault_root.display()
    );

    let vault_root_str = vault_root.to_str().unwrap();
    let mut entity_ids = HashSet::new();
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
