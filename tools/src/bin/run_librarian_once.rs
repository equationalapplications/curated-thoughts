//! One-off: run the Active Librarian over already-ingested vault documents.
//! Walks every indexed document in the brain DB and calls librarian::generate_summary,
//! which respects folder_rules (index = skip, summarize/synthesize = propose).
use anyhow::{Context as _, Result};

use tauri_app_lib::db::connection::AppDb;
use tauri_app_lib::librarian;
use tauri_app_lib::retrieval;

fn main() -> Result<()> {
    let paths = retrieval::resolve_brain_paths();
    if !paths.db_path.is_file() {
        anyhow::bail!(
            "brain database not found at {} — run the app (or ingest_vault_once) first",
            paths.db_path.display()
        );
    }
    // Honor split CURATED_BRAIN_DB / CURATED_BRAIN_CONFIG: resolve the vault
    // root from the resolved config path, not from db_path's parent.
    let mut db = AppDb::open_with_config(&paths.db_path, &paths.config_path)
        .with_context(|| format!("open brain database {}", paths.db_path.display()))?;
    let conn = &db.0;

    let docs: Vec<(i64, String)> = {
        let mut stmt = conn.prepare("SELECT id, path FROM documents ORDER BY path")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };

    // Fallback model only matters for sidecar mode; config overrides it (see synthesis.rs).
    let model = "llama3.2:3b";
    println!(
        "running librarian over {} document(s) with fallback model {model}",
        docs.len()
    );

    let mut ok = 0usize;
    let mut failed = 0usize;
    for (i, (_id, path)) in docs.iter().enumerate() {
        match librarian::generate_summary(&mut db.0, path, model) {
            Ok(()) => {
                ok += 1;
                println!("[{}/{}] ok: {}", i + 1, docs.len(), path);
            }
            Err(e) => {
                failed += 1;
                eprintln!("[{}/{}] FAILED: {}: {e}", i + 1, docs.len(), path);
            }
        }
    }
    println!("done: {ok} ok, {failed} failed");
    Ok(())
}
