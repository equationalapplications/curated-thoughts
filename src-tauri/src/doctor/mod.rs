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
use std::io::Write;

/// Run the doctor report against stdout.
pub fn run_doctor() -> Result<u32> {
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    run_doctor_to(&mut handle)
}

/// Run the doctor report, writing to `out`.
///
/// Split out from [`run_doctor`] so tests can capture the full report and
/// assert that no credential material is ever echoed.
pub fn run_doctor_to(out: &mut dyn Write) -> Result<u32> {
    let paths = resolve_brain_paths();

    writeln!(out, "Config path: {}", paths.config_path.display())?;
    writeln!(out, "Brain dir: {}", paths.brain_dir.display())?;
    writeln!(out, "DB path: {}", paths.db_path.display())?;
    writeln!(out)?;

    // Check if file exists. File-missing is reported by the path-exists check
    // below (exit 1); a missing file causes `load_lenient` to return Ok with
    // all *_missing flags set, which is the correct "needs onboarding" signal.
    if !paths.config_path.exists() {
        writeln!(out, "ERROR: config.json not found")?;
        writeln!(out, "Remediation: run `curated-thoughts --onboard`")?;
        return Ok(1); // Exit code 1
    }

    // Fatal load errors (malformed JSON, non-object root, non-string
    // vault_path) are reported as exit code 2 with a typed error message.
    // load_lenient's typed Result removed the previous string-match on
    // "malformed" inside diagnostics.
    let report = match crate::config::BrainConfig::load_lenient(&paths) {
        Ok(r) => r,
        Err(e) => {
            writeln!(out, "ERROR: config.json could not be loaded: {}", e)?;
            writeln!(
                out,
                "Remediation: hand-repair the file, or run `curated-thoughts --onboard --force` (backs up to config.json.bak)"
            )?;
            return Ok(2); // Exit code 2
        }
    };

    // Check for required blocks
    if report.generation_missing || report.embedding_missing {
        writeln!(out, "ERROR: required config blocks missing")?;
        if report.generation_missing {
            writeln!(out, "  - generation block absent")?;
        }
        if report.embedding_missing {
            writeln!(out, "  - embedding block absent")?;
        }
        writeln!(
            out,
            "Remediation: run `curated-thoughts --onboard` to fill missing blocks"
        )?;
        return Ok(3); // Exit code 3
    }

    // Check vault exists
    if let Some(vault_str) = &report.config.vault_path {
        // Expand a leading `~` so `~/vault` is resolved against the user's home
        // directory instead of being treated as a literal directory name.  This
        // mirrors onboarding's `expand_tilde` policy and prevents a spurious
        // "vault path does not exist" warning on a valid home-relative path.
        let expanded = if let Some(rest) = vault_str.strip_prefix("~/") {
            dirs::home_dir()
                .map(|h| h.join(rest).to_string_lossy().into_owned())
                .unwrap_or_else(|| vault_str.clone())
        } else {
            vault_str.clone()
        };
        let vault_path = std::path::PathBuf::from(&expanded);
        if !vault_path.exists() {
            writeln!(out, "WARNING: vault path does not exist: {}", vault_str)?;
        }
    }

    // Check DB exists
    if !paths.db_path.exists() {
        writeln!(
            out,
            "WARNING: brain.db not found at {}",
            paths.db_path.display()
        )?;
    }

    // All good
    writeln!(out, "Config OK")?;
    writeln!(out, "  Generation: configured")?;
    writeln!(out, "  Embedding: configured")?;

    // Check for legacy plaintext keys (warn without printing)
    if let Ok(gen_api_key) = std::env::var("GENERATION_API_KEY") {
        if !gen_api_key.is_empty() {
            writeln!(
                out,
                "NOTE: generation API key in environment (good practice)"
            )?;
        }
    }

    // Surface any legacy plaintext keys still living in config.json so the
    // operator can migrate them to env vars and reduce disk exposure.
    if report
        .config
        .generation
        .api_key
        .as_deref()
        .map_or(false, |s| !s.is_empty())
    {
        writeln!(
            out,
            "WARNING: generation.api_key found in config.json — migrate to GENERATION_API_KEY env var"
        )?;
    }
    if let Some(profile) = &report.config.embed_profile {
        let key_present = match profile {
            crate::embedder::EmbedProfile::Cloud { api_key, .. } => !api_key.is_empty(),
            crate::embedder::EmbedProfile::External { profile } => {
                profile.api_key.as_deref().map_or(false, |s| !s.is_empty())
            }
            crate::embedder::EmbedProfile::Local { .. } => false,
        };
        if key_present {
            writeln!(
                out,
                "WARNING: embed_profile.api_key found in config.json — migrate to EMBED_API_KEY env var"
            )?;
        }
    }

    Ok(0) // Exit code 0
}
