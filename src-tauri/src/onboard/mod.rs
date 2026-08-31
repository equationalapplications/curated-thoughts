//! `--onboard` subcommand implementation.
//!
//! Interactive headless onboarding: creates vault layout, collects embedding/generation
//! preferences, and writes the unified `BrainConfig`.

use crate::config::BrainConfig;
use crate::embedder::{EmbedProfile, ExternalEmbedProfile};
use crate::inference::config::{GenerationConfig, GenerationProviderKind};
use crate::ontology_config::OntologySelection;
use crate::retrieval::resolve_brain_paths;
use anyhow::{bail, Result};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

pub struct OnboardOptions {
    /// Optional vault path; if absent, reads from stdin.
    pub vault_path: Option<String>,
    /// If true and config.json exists, back it up to config.json.bak before writing.
    pub force: bool,
}

/// Pre-resolved onboarding choices — supplied by tests or by the interactive prompt.
#[derive(Clone)]
pub struct OnboardConfig {
    pub vault_root: PathBuf,
    pub force: bool,
    pub embed_profile: EmbedProfile,
    pub generation: GenerationConfig,
    /// Which ontology the brain is seeded with. CLI default is
    /// `OntologySelection::CLI_DEFAULT` (software-org).
    pub ontology: OntologySelection,
}

impl Default for OnboardConfig {
    fn default() -> Self {
        Self {
            vault_root: PathBuf::new(),
            force: false,
            embed_profile: EmbedProfile::Local {
                model: "nomic-embed-code".to_string(),
            },
            generation: GenerationConfig::default(),
            ontology: OntologySelection::CLI_DEFAULT,
        }
    }
}

/// Expand leading `~` to the user's home directory.
fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        dirs::home_dir()
            .map(|h| h.join(rest).to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string())
    } else {
        path.to_string()
    }
}

/// Read one line from stdin, trimming whitespace. Returns None on EOF.
fn read_line(prompt: &str) -> Result<String> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut buf = String::new();
    let stdin = io::stdin();
    let n = stdin.lock().read_line(&mut buf)?;
    if n == 0 {
        bail!("unexpected EOF on stdin");
    }
    Ok(buf.trim().to_string())
}

/// Interactive prompt flow; returns resolved choices ready for `create_layout_and_onboard`.
fn collect_onboard_config(vault_root: PathBuf, force: bool) -> Result<OnboardConfig> {
    // ── embedding profile ─────────────────────────────────────────────────────
    println!("Embedding profile:");
    println!("  1) Local  (Ollama, default: nomic-embed-code)");
    println!("  2) External (OpenAI-compatible)");

    let embed_profile = match read_line("Choice [1]: ")?.as_str() {
        "2" => {
            let base_url = read_line("  Base URL: ")?;
            let model = read_line("  Model name: ")?;
            EmbedProfile::External {
                profile: ExternalEmbedProfile {
                    base_url,
                    model,
                    api_key: None,
                },
            }
        }
        _ => EmbedProfile::Local {
            model: "nomic-embed-code".to_string(),
        },
    };

    // ── generation provider ────────────────────────────────────────────────────
    println!("Generation provider:");
    println!("  0) Skip / unconfigured");
    println!("  1) Sidecar model");
    println!("  2) External (OpenAI-compatible)");

    let generation = match read_line("Choice [0]: ")?.as_str() {
        "1" => GenerationConfig {
            provider: GenerationProviderKind::Sidecar,
            model_path: None,
            model_name: Some("local".to_string()),
            external_url: None,
            api_key: None,
            timeout_secs: None,
        },
        "2" => {
            let base_url = read_line("  Base URL: ")?;
            let model_name = read_line("  Model name: ")?;
            GenerationConfig {
                provider: GenerationProviderKind::External,
                model_path: None,
                model_name: Some(model_name),
                external_url: Some(base_url),
                api_key: None,
                timeout_secs: None,
            }
        }
        _ => GenerationConfig::default(),
    };

    // ── ontology ──────────────────────────────────────────────────────────────
    println!("Knowledge schema (what kinds of things Tessera tracks):");
    println!("  1) Software team  — specs, handoffs, services, procedures");
    println!("  2) General        — people, places, events, works");
    println!("  3) Let it invent its own");
    println!("  4) None");

    let ontology = match read_line("Choice [1]: ")?.as_str() {
        "2" => OntologySelection::SchemaOrg,
        "3" => OntologySelection::Emergent,
        "4" => OntologySelection::Off,
        _ => OntologySelection::SchemaSoftwareOrg,
    };

    Ok(OnboardConfig {
        vault_root,
        force,
        embed_profile,
        generation,
        ontology,
    })
}

/// Execute the onboarding with pre-resolved choices (used by tests and CLI entrypoint).
pub fn create_layout_and_onboard(config: OnboardConfig) -> Result<()> {
    // ── create vault layout ────────────────────────────────────────────────────
    crate::vault::layout::create_vault_layout(&config.vault_root)
        .map_err(|e| anyhow::anyhow!("failed to create vault layout: {e}"))?;

    // ── resolve brain paths ────────────────────────────────────────────────────
    let paths = resolve_brain_paths();

    // ── load / create BrainConfig ─────────────────────────────────────────────
    // Determine whether we need to replace or merge.
    // A malformed config.json can't be parsed, so we back it up and start fresh.
    let needs_force = config.force
        || (paths.config_path.exists()
            && std::fs::read_to_string(&paths.config_path)
                .ok()
                .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                .is_none());

    let mut cfg = if needs_force {
        if paths.config_path.exists() {
            let backup = paths.config_path.with_extension("json.bak");
            std::fs::copy(&paths.config_path, &backup).map_err(|e| {
                anyhow::anyhow!(
                    "failed to back up existing config to {}: {}",
                    backup.display(),
                    e
                )
            })?;

            // `load_lenient` returns Err on malformed JSON / non-object root /
            // non-string vault_path. We already backed the file up above, so
            // when it can't be loaded we blank it (single read+parse path —
            // no need to re-read and re-parse after the needs_force check).
            match BrainConfig::load_lenient(&paths) {
                Ok(report) => report.config,
                Err(_) => {
                    std::fs::write(&paths.config_path, "{}")?;
                    BrainConfig::default()
                }
            }
        } else {
            BrainConfig::default()
        }
    } else if paths.config_path.exists() {
        let report = BrainConfig::load_lenient(&paths).map_err(|e| {
            anyhow::anyhow!("config.json failed to load during onboarding merge: {e}")
        })?;
        report.config
    } else {
        BrainConfig::default()
    };

    cfg.vault_path = Some(config.vault_root.to_string_lossy().into_owned());
    cfg.embed_profile = Some(config.embed_profile);
    cfg.generation = config.generation;
    cfg.ontology.schema = Some(config.ontology);

    cfg.write(&paths)
        .map_err(|e| anyhow::anyhow!("failed to write config: {e}"))?;

    // ── print agent-client snippet ─────────────────────────────────────────────
    println!("\nSetup complete.");
    println!("\nConfig written to {}", paths.config_path.display());
    println!("\nTo run your sidecar:");
    println!(
        "  export CURATED_BRAIN_CONFIG={}",
        paths.config_path.display()
    );
    println!("  curated-thoughts --mcp");

    Ok(())
}

pub fn run_onboard(opts: OnboardOptions) -> Result<()> {
    // ── vault path ──────────────────────────────────────────────────────────────
    let vault_input = match opts.vault_path {
        Some(v) => v,
        None => read_line("Vault path [default: ~/.brain]: ")?,
    };

    let vault_input = vault_input.trim();
    let vault_root = if vault_input.is_empty() {
        dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
    } else {
        PathBuf::from(expand_tilde(vault_input))
    };

    let config = collect_onboard_config(vault_root, opts.force)?;
    create_layout_and_onboard(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn expand_tilde_replaces_home() {
        let home = dirs::home_dir().unwrap();
        let expanded = expand_tilde(&format!(
            "~/test/{}",
            if cfg!(windows) { "foo" } else { "bar" }
        ));
        assert!(expanded.starts_with(home.to_string_lossy().as_ref()));
        assert!(expanded.ends_with(if cfg!(windows) {
            "test\\foo"
        } else {
            "test/bar"
        }));
    }

    #[test]
    fn expand_tilde_passes_through_when_no_tilde() {
        let expanded = expand_tilde("/absolute/path");
        assert_eq!(expanded, "/absolute/path");
    }

    #[test]
    fn expand_tilde_handles_empty_when_no_home() {
        let expanded = expand_tilde("~/foo");
        assert!(expanded.ends_with("foo"));
    }

    #[test]
    fn create_layout_and_onboard_creates_directories() {
        let temp = TempDir::new().unwrap();
        let vault = temp.path().join("vault");

        let cfg = OnboardConfig {
            vault_root: vault.clone(),
            force: false,
            embed_profile: EmbedProfile::Local {
                model: "nomic-embed-code".to_string(),
            },
            generation: GenerationConfig::default(),
            ontology: OntologySelection::CLI_DEFAULT,
        };

        create_layout_and_onboard(cfg).expect("onboard should succeed");

        assert!(vault.join("immutable-source-files").is_dir());
        assert!(vault.join("wiki").is_dir());
        assert!(vault.join("immutable-source-files/agents").is_dir());
        assert!(vault.join(".brain/converted").is_dir());
    }

    #[test]
    fn create_layout_and_onboard_is_idempotent() {
        let temp = TempDir::new().unwrap();
        let vault = temp.path().join("vault");

        let cfg = OnboardConfig {
            vault_root: vault.clone(),
            force: false,
            ..Default::default()
        };

        create_layout_and_onboard(cfg.clone()).expect("first");
        create_layout_and_onboard(cfg).expect("second (idempotent)");
    }
}
