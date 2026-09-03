//! Trust-on-first-use ledger for symlinks the ingest walker may follow.
//!
//! The boundary this enforces is exfiltration, not tidiness: ingested content
//! is sent to embedding and generation providers that may be external, so an
//! unapproved symlink target is never read. The noise-exclusion rules in
//! `cmds.rs` are NOT a security boundary. See spec D3a.

use serde::{Deserialize, Serialize};
use std::path::{Component, Path};

/// One approved `(link, target)` pair. Matching the pair — not the target
/// alone — is what makes a repointed symlink a fresh approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedLink {
    /// Vault-relative path of the symlink itself, e.g. `documents/specs`.
    pub link: String,
    /// Canonicalized target at the time of approval.
    pub target: String,
    /// Unix seconds.
    pub approved_at: i64,
}

/// Why a link can never be approved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DenyReason {
    FilesystemRoot,
    HomeDirectory,
    /// The target is an ancestor of the vault root.
    VaultAncestor,
    /// The target contains the vault root.
    ContainsVault,
    /// The target is an ancestor of a target already in the ledger.
    AncestorOfTrusted,
}

impl DenyReason {
    pub fn message(self) -> &'static str {
        match self {
            DenyReason::FilesystemRoot => "target is the filesystem root",
            DenyReason::HomeDirectory => "target is the home directory itself",
            DenyReason::VaultAncestor => "target is an ancestor of the vault root",
            DenyReason::ContainsVault => "target contains the vault root",
            DenyReason::AncestorOfTrusted => "target is an ancestor of an already-trusted target",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkVerdict {
    /// Walk it.
    Trusted,
    /// Not walked; needs approval.
    Pending,
    /// Not walked, and approval is refused.
    Denied(DenyReason),
}

/// Component-wise containment. `is_within(a, b)` is true when `a` is `b` or
/// lives beneath it. Never use string prefixes here: `/x/proj` must not
/// authorize `/x/proj-secrets`.
pub fn is_within(candidate: &Path, ancestor: &Path) -> bool {
    let mut c = candidate.components();
    for comp in ancestor.components() {
        match c.next() {
            Some(actual) if actual == comp => {}
            _ => return false,
        }
    }
    true
}

/// True when `link` can safely be joined onto `vault_root` as a
/// vault-relative path. `Path::join` **replaces** the base when its argument
/// is absolute — or, on Windows, merely carries a drive prefix (`C:foo`) or a
/// root (`\foo`) — so an unvalidated `link` would make both this function's
/// own `vault_root.join(link)` and the classification below operate on a path
/// outside the vault entirely (issue #142). ParentDir (`..`) components are
/// also refused (CodeRabbit, PR #144): `join` preserves them lexically, so
/// `../outside-link` canonicalizes OUTSIDE the vault and can persist a
/// traversal string as `TrustedLink::link`. Both entry points into
/// `approve_into` (the Tauri `approve_link` command and the CLI `ct trust`
/// subcommand) feed it raw user input, so the guard lives here, at the join,
/// not at one caller.
pub fn is_vault_relative_link(link: &str) -> bool {
    !Path::new(link).components().any(|c| {
        matches!(
            c,
            Component::Prefix(_) | Component::RootDir | Component::ParentDir
        )
    })
}

/// Normalize away `.` and `..` lexically so `/vault/..` compares correctly.
/// Callers pass canonicalized paths where possible; this covers the rest.
fn lexical_normalize(p: &Path) -> std::path::PathBuf {
    let mut out = std::path::PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Approve a symlink: canonicalize its target, classify it, and on a
/// `Pending` verdict (the only case a ledger entry is needed — spec D3a)
/// remove any stale entry for the same `link` and append the new pair.
/// Centralizes the canonicalize-classify-retain-push pattern so the Tauri
/// `approve_link` command and the CLI `ct trust` subcommand stay in sync.
///
/// On `Trusted` (in-vault) the ledger is left untouched, matching the CLI's
/// existing "is already trusted" branch and spec D3a's "in-vault symlinks are
/// auto-trusted because nothing leaves the vault boundary" rule.
///
/// `vault_root` should be canonicalized by the caller.
///
/// Errors when `link` is not vault-relative. This must fire BEFORE the
/// canonicalize/join so an absolute `link` never touches the filesystem as a
/// joined path and never reaches the ledger (`TrustedLink::link` is
/// vault-relative by contract; see issue #140 for the load-boundary half of
/// that contract). Echos of `link` in the error string are the caller's to
/// redact.
pub fn approve_into(
    ledger: &mut Vec<TrustedLink>,
    link: &str,
    vault_root: &Path,
    home: Option<&Path>,
) -> Result<LinkVerdict, String> {
    if !is_vault_relative_link(link) {
        return Err(format!("{link} is not vault-relative"));
    }
    let target = std::fs::canonicalize(vault_root.join(link))
        .map_err(|e| format!("{link} could not be resolved: {e}"))?;
    let verdict = classify_link(link, &target, vault_root, home, ledger);
    if matches!(verdict, LinkVerdict::Pending) {
        ledger.retain(|e| e.link != link);
        ledger.push(TrustedLink {
            link: link.to_string(),
            target: target.to_string_lossy().to_string(),
            approved_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
        });
    }
    Ok(verdict)
}

/// Classify one symlink. `target` should already be canonicalized by the
/// caller; `link_rel` is the vault-relative path of the link itself.
pub fn classify_link(
    link_rel: &str,
    target: &Path,
    vault_root: &Path,
    home: Option<&Path>,
    ledger: &[TrustedLink],
) -> LinkVerdict {
    let target = lexical_normalize(target);
    let vault_root = lexical_normalize(vault_root);

    // Non-approvable denials come first: they outrank any ledger entry.
    if target.parent().is_none() {
        return LinkVerdict::Denied(DenyReason::FilesystemRoot);
    }
    if let Some(h) = home {
        // `target` is canonicalized by the caller (symlinks resolved); `home`
        // must be compared on the same footing or a symlinked $HOME (e.g.
        // `/home/user` -> `/mnt/data/user`) fails to match here and falls
        // through to whatever the resolved path's other rules produce,
        // missing the "this literally is home" denial. Canonicalize `home`
        // too, falling back to lexical normalization only when resolution
        // fails (e.g. a dangling/unreadable path) — that fallback is no
        // worse than the pre-fix behavior, not a regression.
        let home_normalized = std::fs::canonicalize(h).unwrap_or_else(|_| lexical_normalize(h));
        if target == home_normalized {
            return LinkVerdict::Denied(DenyReason::HomeDirectory);
        }
    }
    if target != vault_root && is_within(&vault_root, &target) {
        // The target sits above the vault — that's the only condition the
        // pre-check leaves when we get here (target != vault_root already
        // excluded equality), and `is_within`/`starts_with` agree
        // component-wise on normalized paths. There is no separate
        // `VaultAncestor` framing reachable from this code; the variant is
        // retained for taxonomy completeness and surfaced by callers that
        // distinguish on it (see the regression test in
        // `src-tauri/tests/trusted_links.rs`).
        return LinkVerdict::Denied(DenyReason::ContainsVault);
    }
    for entry in ledger {
        let trusted = lexical_normalize(Path::new(&entry.target));
        if trusted != target && is_within(&trusted, &target) {
            return LinkVerdict::Denied(DenyReason::AncestorOfTrusted);
        }
    }

    // Inside the vault needs no approval — nothing leaves the vault boundary.
    if is_within(&target, &vault_root) {
        return LinkVerdict::Trusted;
    }

    // Outside the vault: exact (link, target) pair or nothing.
    let matched = ledger
        .iter()
        .any(|e| e.link == link_rel && lexical_normalize(Path::new(&e.target)) == target);

    if matched {
        LinkVerdict::Trusted
    } else {
        LinkVerdict::Pending
    }
}
