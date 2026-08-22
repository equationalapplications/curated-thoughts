//! One-off: ingest every markdown file in the configured vault into the brain DB.
use anyhow::{Context as _, Result};

use tauri_app_lib::db::connection::AppDb;
use tauri_app_lib::indexer::linker::run_linker;
use tauri_app_lib::retrieval;
use tauri_app_lib::vault::VaultConfig;
use tauri_app_lib::{entity_id_for_path, ingest_document_with_vault_root};

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
    for entry in walkdir::WalkDir::new(&vault_root)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let p = entry.path();
        if p.is_file() && p.extension().map(|e| e == "md").unwrap_or(false) {
            files.push(p.to_path_buf());
        }
    }
    files.sort();
    println!(
        "ingesting {} markdown file(s) from {}",
        files.len(),
        vault_root.display()
    );

    let vault_root_str = vault_root.to_str().unwrap();
    let mut entity_ids = std::collections::HashSet::new();
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
