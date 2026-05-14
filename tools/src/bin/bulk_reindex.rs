//! Re-chunks and re-embeds every indexed `user_doc` in the vault brain DB — for upgrades to
//! `ast_*` chunk strategies or embedding models without mutating files on disk.
//!
//! Uses the same `CURATED_BRAIN_*` env vars as MCP / desktop (`~/.brain` by default).
//!
//! Examples (repo root):
//! ```text
//! cargo run --manifest-path tools/Cargo.toml --bin bulk_reindex -- --dry-run
//! cargo run --manifest-path tools/Cargo.toml --bin bulk_reindex -- --limit 500
//! ```

use anyhow::{anyhow, Context as _, Result};
use std::{collections::HashSet, path::Path};

use tauri_app_lib::db::{list_indexed_user_doc_paths, AppDb};
use tauri_app_lib::indexer::linker::run_linker;
use tauri_app_lib::{entity_id_for_path, ingest_document_with_vault_root};
use tauri_app_lib::retrieval;
use tauri_app_lib::vault::VaultConfig;

struct Args {
    dry_run: bool,
    limit: Option<usize>,
    path_contains: Option<String>,
}

fn parse_args() -> Args {
    let mut dry_run = false;
    let mut limit = None;
    let mut path_contains = None;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--dry-run" => dry_run = true,
            "--limit" => {
                let n: usize = it.next().and_then(|s| s.parse().ok()).unwrap_or_else(|| {
                    eprintln!("error: --limit requires a positive integer");
                    std::process::exit(2);
                });
                limit = Some(n);
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            s if s.starts_with('-') => {
                eprintln!("error: unknown flag {s}");
                std::process::exit(2);
            }
            other => {
                if path_contains.is_some() {
                    eprintln!("error: only one PATH_FILTER substring allowed");
                    std::process::exit(2);
                }
                path_contains = Some(other.to_owned());
            }
        }
    }
    Args {
        dry_run,
        limit,
        path_contains,
    }
}

fn print_help() {
    eprintln!(
        "\
bulk_reindex — rebuild chunks + embeddings for indexed vault docs

USAGE:
    bulk_reindex [OPTIONS] [PATH_FILTER]

OPTIONS:
    --dry-run       List matching paths only
    --limit N       Process at most N documents
    -h, --help      This help

Environment:
    CURATED_BRAIN_DIR, CURATED_BRAIN_DB, CURATED_BRAIN_CONFIG (same as MCP)
"
    );
}

fn main() -> Result<()> {
    let args = parse_args();
    let paths_b = retrieval::resolve_brain_paths();
    let profile =
        retrieval::load_embed_profile(&paths_b.config_path).context("read embed profile")?;

    let db = AppDb::open(&paths_b.db_path).context("open brain database")?;
    let conn = &db.0;
    let config = VaultConfig::new(paths_b.config_path.clone());
    let vault_root = config
        .vault_root()
        .context("read vault root")?
        .ok_or_else(|| anyhow!("vault root missing"))?;
    // Canonicalize so entity_id_for_path can strip the vault prefix from the
    // canonical document paths stored by the watcher (matches pipeline startup logic).
    let vault_root = vault_root.canonicalize().unwrap_or(vault_root);
    let vault_root_str = vault_root
        .to_str()
        .ok_or_else(|| anyhow!("invalid vault root path"))?;

    let mut paths = list_indexed_user_doc_paths(conn).context("list indexed paths")?;
    if let Some(ref sub) = args.path_contains {
        paths.retain(|p| p.contains(sub));
    }
    if let Some(n) = args.limit {
        paths.truncate(n);
    }

    if args.dry_run {
        println!("dry-run: {} documents", paths.len());
        for p in &paths {
            println!("  {p}");
        }
        return Ok(());
    }

    let total = paths.len();
    let mut entity_ids = HashSet::new();
    for (i, path) in paths.iter().enumerate() {
        if !Path::new(path).exists() {
            eprintln!("[{}/{}] skip missing: {}", i + 1, total, path);
            continue;
        }
        ingest_document_with_vault_root(conn, &profile, path, true, Some(vault_root_str))
            .with_context(|| format!("reindex {}", path))?;
        entity_ids.insert(entity_id_for_path(path, Some(vault_root_str)));
        if (i + 1) % 25 == 0 || i + 1 == total {
            eprintln!("[{}/{}] done …", i + 1, total);
        }
    }

    for entity_id in entity_ids {
        if let Err(e) = run_linker(conn, &entity_id, 0) {
            eprintln!("[linker] run_linker error ({}): {}", entity_id, e);
        }
    }
    println!("Reindexed {} document(s).", total);
    Ok(())
}
