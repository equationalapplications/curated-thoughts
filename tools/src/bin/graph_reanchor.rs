//! Heals a brain.db whose wiki graph was severed: purges edges with no live
//! endpoint, then backfills entry embeddings. Both steps are idempotent and
//! resumable. See `docs/superpowers/specs/2026-08-31-wiki-graph-reanchor-entry-embeddings-design.md`.
//!
//! Dry-run by default — pass `--yes` to write.
//!
//! ```text
//! cargo run --manifest-path tools/Cargo.toml --bin graph_reanchor
//! cargo run --manifest-path tools/Cargo.toml --bin graph_reanchor -- --yes
//! ```

use anyhow::{Context as _, Result};
use rusqlite::Connection;
use std::path::{Path, PathBuf};

use curated_thoughts_tools::graph_reanchor::{count_orphan_edges, purge_orphan_edges};
use tauri_app_lib::embed_sweep::sweep_null_embeddings;
use tauri_app_lib::retrieval;

fn print_help() {
    eprintln!(
        "graph_reanchor — heal the wiki knowledge graph (edge purge + embedding backfill)\n\n\
         USAGE:\n    graph_reanchor [--yes]\n\n\
         FLAGS:\n\
         \x20   --yes     Apply changes. Without it, runs read-only and prints the plan.\n\
         \x20   -h,--help Print this help.\n\n\
         Backs up brain.db (+ -wal, -shm) before the first write."
    );
}

fn backup_db(db_path: &Path) -> Result<PathBuf> {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let backup = db_path.with_file_name(format!(
        "{}.bak-pre-graphreanchor-{ts}",
        db_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("brain.db")
    ));
    // Crash-consistent snapshot for a live WAL database: SQLite's own
    // `VACUUM INTO` rewrites the db into a single self-contained file with
    // no -wal / -shm handling required. The earlier `std::fs::copy` over
    // brain.db / -wal / -shm could land torn if writes interleaved with
    // the copies — and this binary's own printed instructions tell the
    // operator that restoring this backup is the rollback path for the
    // destructive purge.
    let src = Connection::open(db_path)
        .with_context(|| format!("open {} for backup", db_path.display()))?;
    src.execute(
        "VACUUM INTO ?1",
        rusqlite::params![backup.to_string_lossy()],
    )
    .with_context(|| {
        format!(
            "VACUUM INTO {} failed — refusing to proceed without a consistent backup",
            backup.display()
        )
    })?;
    Ok(backup)
}

fn main() -> Result<()> {
    let mut apply = false;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--yes" => apply = true,
            "--help" | "-h" => {
                print_help();
                return Ok(());
            }
            other => {
                eprintln!("error: unknown flag {other}");
                std::process::exit(2);
            }
        }
    }

    let paths = retrieval::resolve_brain_paths();

    if !apply {
        let conn = retrieval::open_brain_readonly(&paths.db_path)?;
        let orphans = count_orphan_edges(&conn)?;
        let null_entries: i64 = conn.query_row(
            "SELECT COUNT(*) FROM llm_wiki_entries
              WHERE deleted_at IS NULL AND embedding_blob IS NULL",
            [],
            |r| r.get(0),
        )?;
        println!("DRY RUN — no changes written. Re-run with --yes to apply.");
        println!("  db:                    {}", paths.db_path.display());
        println!("  orphaned edges to purge: {orphans}");
        println!("  entries to embed:        {null_entries}");
        return Ok(());
    }

    let profile = retrieval::load_embed_profile(&paths.config_path)
        .context("load the active embed profile")?;

    let backup = backup_db(&paths.db_path)?;
    println!("backup written: {}", backup.display());

    let conn = Connection::open(&paths.db_path)
        .with_context(|| format!("open {}", paths.db_path.display()))?;
    // Every writer connection in this workspace sets `busy_timeout` (see
    // `tools/src/write.rs::open_rw`). SQLite's default is 0, so a transient
    // lock from the desktop's WAL checkpoint would fail this migration
    // instantly with SQLITE_BUSY — and because step 1 and step 2 commit
    // separately, an instant failure can leave the DB half-migrated with the
    // operator holding only a backup and no record of which step landed.
    // 5s matches the rest of the workspace.
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .context("set busy_timeout on the migration writer connection")?;

    // Step 1 — edge purge. No outbox rows: edges are not replicated (spec §2).
    let removed = purge_orphan_edges(&conn)?;
    let still_orphaned = count_orphan_edges(&conn)?;
    anyhow::ensure!(
        still_orphaned == 0,
        "post-condition failed: {still_orphaned} orphaned edges remain"
    );
    println!("step 1: purged {removed} orphaned edges (post-check: 0 remaining)");

    // Step 2 — embedding backfill, via the same sweep the runtime uses.
    // usize::MAX batches: the migration is explicitly allowed to run to
    // completion, unlike the bounded runtime trigger.
    let report = sweep_null_embeddings(&conn, &profile, usize::MAX)?;
    println!(
        "step 2: embedded {} entries ({} failed, {} still null)",
        report.filled, report.failed, report.remaining_null
    );

    if report.remaining_null > 0 {
        let mut stmt = conn.prepare(
            "SELECT id FROM llm_wiki_entries
              WHERE deleted_at IS NULL AND embedding_blob IS NULL ORDER BY id",
        )?;
        let ids = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        println!("  still null: {}", ids.join(", "));
        println!("  re-run this tool to retry, or let the runtime sweep pick them up.");
    }

    println!(
        "done. rollback = stop the app and restore {}",
        backup.display()
    );
    Ok(())
}
