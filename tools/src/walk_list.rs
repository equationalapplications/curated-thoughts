//! tools/src/walk_list.rs
//!
//! Shared vault-walk assembly for every consumer that must see the EXACT
//! file list `ct ingest` feeds to reconcile + ingest (issue #241, spec M4).
//! `cmds::ingest_run` and `drift::drift_cmd` both call
//! [`build_ingest_file_list`], so drift's view can never diverge from what
//! ingest would actually reconcile.

use anyhow::Context;
use std::path::PathBuf;

use crate::paths::BrainPaths;
use tauri_app_lib::walk_vault::{walk_vault, DeniedLink, PendingLink, WalkedFile};

/// The symlink verdicts and walker errors the walk surfaced. They are
/// printed to stderr by [`build_ingest_file_list`] exactly as `ingest_run`
/// always did; this struct hands the same lists back so the ingest caller
/// can keep its failure count and remediation hints without a second walk.
#[derive(Debug, Default)]
pub struct WalkSurfacing {
    pub pending: Vec<PendingLink>,
    pub denied: Vec<DeniedLink>,
    pub errors: Vec<String>,
}

/// The EXACT file list `ct ingest` feeds to reconcile + ingest: resolve the
/// vault root from config and canonicalize it, walk with the symlink-trust
/// re-walk, surface denied/pending/errors, sort+dedup by virtual_path.
/// Returns the CANONICAL root so drift reconciles against the same root
/// ingest used. `ct drift` shares this so its view can never diverge from
/// what ingest would actually reconcile (spec M4).
pub fn build_ingest_file_list(
    paths_b: &BrainPaths,
    trust_new_links: bool,
) -> anyhow::Result<(PathBuf, Vec<WalkedFile>, WalkSurfacing)> {
    let config = tauri_app_lib::vault::VaultConfig::new(paths_b.config_path.clone());
    let vault_root = config
        .vault_root()
        .context("read vault root")?
        .ok_or_else(|| anyhow::anyhow!("vault root missing"))?;
    let vault_root = vault_root.canonicalize().unwrap_or(vault_root);

    // Load the ledger (Task 10) so walk_vault gates every direct-child
    // documents/ symlink through classify_link.
    let mut brain_cfg = tauri_app_lib::config::BrainConfig::load(paths_b)
        .context("read trusted_links ledger from config.json")?;
    let mut outcome = walk_vault(
        &vault_root,
        &brain_cfg.trusted_links,
        dirs::home_dir().as_deref(),
    );

    // Scripted setups: promote every Pending link that survives
    // classify_link (Denied stays Denied — that's the security boundary) and
    // re-walk before ingesting. Persist first so a mid-run crash doesn't
    // leave the walker half-collected with no ledger entry.
    if trust_new_links && !outcome.pending.is_empty() {
        use tauri_app_lib::trusted_links::{classify_link, LinkVerdict, TrustedLink};
        let mut newly_trusted: Vec<TrustedLink> = Vec::new();
        for p in &outcome.pending {
            match classify_link(
                &p.link,
                std::path::Path::new(&p.target),
                &vault_root,
                dirs::home_dir().as_deref(),
                &brain_cfg.trusted_links,
            ) {
                LinkVerdict::Pending => newly_trusted.push(TrustedLink {
                    link: p.link.clone(),
                    target: p.target.clone(),
                    approved_at: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0),
                }),
                LinkVerdict::Denied(_) => {}
                LinkVerdict::Trusted => {}
            }
        }
        if !newly_trusted.is_empty() {
            crate::cmds::replace_trusted_links(&mut brain_cfg.trusted_links, newly_trusted);
            brain_cfg
                .write(paths_b)
                .context("persist newly-trusted links")?;
            outcome = walk_vault(
                &vault_root,
                &brain_cfg.trusted_links,
                dirs::home_dir().as_deref(),
            );
        }
    }

    // Surface pending + denied so a headless run exits with the right
    // remediation hint (spec Risks).
    for d in &outcome.denied {
        eprintln!(
            "refused: {} -> {} ({}). This cannot be approved.",
            d.link, d.target, d.reason
        );
    }
    for p in &outcome.pending {
        eprintln!(
            "pending: {} -> {} is not approved; its content was skipped.\n  approve with: ct trust {}",
            p.link, p.target, p.link
        );
    }
    for e in &outcome.errors {
        eprintln!("warn: {e}");
    }

    let mut files = outcome.files;
    files.sort_by(|a, b| a.virtual_path.cmp(&b.virtual_path));
    files.dedup_by(|a, b| a.virtual_path == b.virtual_path);

    let surfacing = WalkSurfacing {
        pending: outcome.pending,
        denied: outcome.denied,
        errors: outcome.errors,
    };
    Ok((vault_root, files, surfacing))
}
