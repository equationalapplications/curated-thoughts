//! One-shot tier backfill for llm_wiki_entries.
//!
//! Dry-run by default — pass `--yes` to write. See
//! `docs/superpowers/specs/2026-09-01-memory-architecture-intent-implementation-design.md` §3.3.
//!
//! ```text
//! cargo run --manifest-path tools/Cargo.toml --bin tier_backfill
//! cargo run --manifest-path tools/Cargo.toml --bin tier_backfill -- --yes
//! ```

use anyhow::Result;
use curated_thoughts_tools::tier_backfill::{apply_backfill, plan_backfill, read_marker};
use tauri_app_lib::retrieval;

fn print_help() {
    eprintln!(
        "tier_backfill — classify deposit-origin wiki entries with a stored tier\n\n\
         USAGE:\n    tier_backfill [--yes]\n\n\
         FLAGS:\n\
         \x20   --yes     Apply changes. Without it, runs read-only and prints the plan.\n\
         \x20   -h,--help Print this help.\n\n\
         Only entries with certain deposit provenance and a NULL tier are touched.\n\
         Reruns are safe: the marker pins the cohort's tier, so a config change\n\
         cannot retier existing rows or split the cohort."
    );
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
    let config = tauri_app_lib::config::BrainConfig::load_lenient(&paths)
        .map_err(|e| anyhow::anyhow!("load brain config: {e}"))?;
    let config_default = config.config.deposit_default_tier();

    if !apply {
        let conn = retrieval::open_brain_readonly(&paths.db_path)?;
        let marker = read_marker(&conn)?;
        let tier = marker
            .as_ref()
            .map(|m| m.deposit_default_used.clone())
            .unwrap_or_else(|| config_default.to_string());
        let plan = plan_backfill(&conn, &tier)?;

        match &marker {
            Some(m) => println!(
                "marker present: runs={} rows_classified={} deposit_default_used={} (pins this run)",
                m.runs, m.rows_classified, m.deposit_default_used
            ),
            None => println!("no marker: this would be run 1, using config default {tier:?}"),
        }
        println!("\n{:<40} {}", "ENTRY ID", "TIER");
        for (id, t) in &plan {
            println!("{id:<40} {t}");
        }
        println!(
            "\n{} entr{} would be classified. Re-run with --yes to apply.",
            plan.len(),
            if plan.len() == 1 { "y" } else { "ies" }
        );
        return Ok(());
    }

    let mut conn = rusqlite::Connection::open(&paths.db_path)?;
    let marker = apply_backfill(&mut conn, config_default)?;
    println!(
        "applied: runs={} rows_classified={} deposit_default_used={}",
        marker.runs, marker.rows_classified, marker.deposit_default_used
    );
    Ok(())
}
