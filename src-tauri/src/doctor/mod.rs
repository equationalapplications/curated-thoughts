//! `--doctor` subcommand implementation.
//!
//! Reports the current config path, parse status, and required block presence.
//! Returns distinct exit codes that map to remediation actions:
//!   0: config OK (required blocks present)
//!   1: config missing (run --onboard)
//!   2: malformed JSON (hand-repair or --onboard --force)
//!   3: required blocks absent (run --onboard to fill)
//!
//! Never echoes credential material.

use crate::retrieval::resolve_brain_paths;
use anyhow::Result;

pub fn run_doctor() -> Result<u32> {
    let paths = resolve_brain_paths();
    let report = crate::config::BrainConfig::load_lenient(&paths);

    println!("Config path: {}", paths.config_path.display());
    println!("Brain dir: {}", paths.brain_dir.display());
    println!("DB path: {}", paths.db_path.display());
    println!();

    // Check if file exists
    if !paths.config_path.exists() {
        println!("ERROR: config.json not found");
        println!("Remediation: run `curated-thoughts --onboard`");
        return Ok(1); // Exit code 1
    }

    // Check if malformed
    if report.diagnostics.iter().any(|d| d.contains("malformed")) {
        println!("ERROR: config.json is malformed JSON");
        println!("Remediation: hand-repair the file, or run `curated-thoughts --onboard --force` (backs up to config.json.bak)");
        return Ok(2); // Exit code 2
    }

    // Check for required blocks
    if report.generation_missing || report.embedding_missing {
        println!("ERROR: required config blocks missing");
        if report.generation_missing {
            println!("  - generation block absent");
        }
        if report.embedding_missing {
            println!("  - embedding block absent");
        }
        println!("Remediation: run `curated-thoughts --onboard` to fill missing blocks");
        return Ok(3); // Exit code 3
    }

    // Check vault exists
    if let Some(vault_str) = &report.config.vault_path {
        let vault_path = std::path::PathBuf::from(vault_str);
        if !vault_path.exists() {
            println!("WARNING: vault path does not exist: {}", vault_str);
        }
    }

    // Check DB exists
    if !paths.db_path.exists() {
        println!("WARNING: brain.db not found at {}", paths.db_path.display());
    }

    // All good
    println!("Config OK");
    println!("  Generation: configured");
    println!("  Embedding: configured");

    // Check for legacy plaintext keys (warn without printing)
    if let Ok(gen_api_key) = std::env::var("GENERATION_API_KEY") {
        if !gen_api_key.is_empty() {
            println!("NOTE: generation API key in environment (good practice)");
        }
    }

    Ok(0) // Exit code 0
}
