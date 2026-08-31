//! Vault walker that emits `WalkedFile { virtual_path, read_path }` pairs.
//!
//! Lives in `tauri_app_lib` so the app crate's Tauri commands can call it
//! directly without depending on `curated_thoughts_tools`. The tools crate
//! re-exports these types and uses the same `collect_files` / `walk_vault`
//! entry points — single source of truth for the walker behavior.
//!
//! Symlink following is gated by the trusted-links ledger in
//! [`crate::trusted_links`]. `walk_vault` consults it for every direct-child
//! symlink under `documents/`. `collect_files` is the plain non-following
//! walker used for the in-vault content pass.

use crate::chunker::should_ingest_extension;
use crate::trusted_links::{classify_link, LinkVerdict, TrustedLink};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Directory names never ingested (build artifacts, deps, VCS internals).
const EXCLUDED_DIRS: &[&str] = &[
    "target",
    "node_modules",
    "dist",
    "dist-newstyle",
    ".git",
    ".github",
    ".next",
    ".turbo",
    ".cache",
    "coverage",
    "build",
    "out",
    ".venv",
    "venv",
    "__pycache__",
    ".idea",
    ".vscode",
    ".fastembed_cache",
];

fn is_excluded_dir(dir_name: &str) -> bool {
    EXCLUDED_DIRS.contains(&dir_name)
}

/// File-name patterns never ingested.
const EXCLUDED_FILE_NAMES: &[&str] = &[
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "Cargo.lock",
    "poetry.lock",
    "uv.lock",
    "CHANGELOG.md",
    "CHANGELOG.md.generated",
];

/// Path segments (matched anywhere in the relative path) that mark generated
/// machine output rather than authored knowledge.
const EXCLUDED_PATH_SEGMENTS: &[&str] = &["drizzle/meta/", "gen/schemas/"];

fn is_excluded_file(path: &Path) -> bool {
    if let Some(name) = path.file_name() {
        let name = name.to_string_lossy();
        if EXCLUDED_FILE_NAMES.contains(&name.as_ref()) {
            return true;
        }
    }
    let p = path.to_string_lossy();
    EXCLUDED_PATH_SEGMENTS.iter().any(|seg| p.contains(seg))
}

/// A file the walker found. `virtual_path` is what the DB stores and what
/// tier routing sees; `read_path` is where the bytes actually live. They
/// differ only for content reached through a tracked symlink under
/// `<vault_root>/documents/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkedFile {
    pub virtual_path: PathBuf,
    pub read_path: PathBuf,
}

/// A symlink that needs approval before its content can be ingested.
#[derive(Debug, Clone)]
pub struct PendingLink {
    /// Vault-relative path of the link, e.g. `documents/specs`.
    pub link: String,
    /// Canonicalized current target.
    pub target: String,
}

/// A symlink refused by a non-approvable rule.
#[derive(Debug, Clone)]
pub struct DeniedLink {
    pub link: String,
    pub target: String,
    /// Human-readable rule text from `DenyReason::message`.
    pub reason: String,
}

/// Outcome of [`walk_vault`]: collected files, errors, and the pending/denied
/// symlinks the caller must surface to the user.
#[derive(Debug, Default)]
pub struct WalkOutcome {
    pub files: Vec<WalkedFile>,
    pub errors: Vec<String>,
    pub pending: Vec<PendingLink>,
    pub denied: Vec<DeniedLink>,
}

/// Maximum number of path components in a virtual path once a symlink prefix
/// is applied. Bounds the work a single symlinked repo can add (spec D3).
pub const MAX_VIRTUAL_DEPTH: usize = 16;

/// Plain non-following walker. Canonicalizes its root at entry so every
/// virtual path is joined to the same absolute prefix that
/// `entity_id_for_virtual_path` canonicalizes against (Ruling 2). The
/// `follow_symlinked_doc_dirs` flag is vestigial — symlink following is
/// exclusively [`walk_vault`]'s responsibility now.
pub fn collect_files(
    root: &Path,
    _follow_symlinked_doc_dirs: bool,
    out: &mut Vec<WalkedFile>,
    errors: &mut Vec<String>,
) {
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let walker = WalkDir::new(&canonical_root).follow_links(false);
    let it = walker.into_iter().filter_entry(|e| {
        if e.file_type().is_dir() {
            if let Some(name) = e.path().file_name() {
                return !is_excluded_dir(&name.to_string_lossy());
            }
        }
        true
    });
    for entry in it {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                errors.push(format!("traversal: {e}"));
                continue;
            }
        };
        let p = entry.path();
        if entry.file_type().is_file() && is_excluded_file(p) {
            continue;
        }
        let ft = entry.file_type();
        if ft.is_file()
            && p.extension()
                .map(|e| should_ingest_extension(&e.to_string_lossy()))
                .unwrap_or(false)
        {
            out.push(WalkedFile {
                virtual_path: p.to_path_buf(),
                read_path: p.to_path_buf(),
            });
        }
    }
}

/// Walk a vault, consulting the trusted-links ledger for every direct-child
/// symlink under `documents/`. Unapproved links are reported, never read.
pub fn walk_vault(vault_root: &Path, ledger: &[TrustedLink], home: Option<&Path>) -> WalkOutcome {
    let mut outcome = WalkOutcome::default();

    // Canonicalize so classify_link's path comparisons see matching
    // prefixes; on macOS TempDir resolves through /var → /private/var and
    // a non-canonical vault_root would silently match as Trusted against a
    // canonicalized target.
    let vault_root = std::fs::canonicalize(vault_root).unwrap_or_else(|_| vault_root.to_path_buf());

    // In-vault content first; this pass never follows symlinks.
    collect_files(&vault_root, false, &mut outcome.files, &mut outcome.errors);

    let documents = vault_root.join("documents");
    let entries = match std::fs::read_dir(&documents) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return outcome,
        Err(e) => {
            outcome.errors.push(format!("read documents/: {e}"));
            return outcome;
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                outcome.errors.push(format!("read documents/: {e}"));
                continue;
            }
        };
        let p = entry.path();
        let is_symlink = std::fs::symlink_metadata(&p)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);
        if !is_symlink {
            continue;
        }

        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if is_excluded_dir(&name) {
            continue;
        }
        let link_rel = format!("documents/{name}");

        let target = match std::fs::canonicalize(&p) {
            Ok(t) => t,
            Err(e) => {
                outcome
                    .errors
                    .push(format!("broken symlink {}: {e}", p.display()));
                continue;
            }
        };
        if !target.is_dir() {
            outcome.errors.push(format!(
                "symlink {} does not point at a directory",
                p.display()
            ));
            continue;
        }

        match classify_link(&link_rel, &target, &vault_root, home, ledger) {
            LinkVerdict::Denied(reason) => outcome.denied.push(DeniedLink {
                link: link_rel,
                target: target.to_string_lossy().to_string(),
                reason: reason.message().to_string(),
            }),
            LinkVerdict::Pending => outcome.pending.push(PendingLink {
                link: link_rel,
                target: target.to_string_lossy().to_string(),
            }),
            LinkVerdict::Trusted => {
                let mut hits: Vec<WalkedFile> = Vec::new();
                collect_files(&target, false, &mut hits, &mut outcome.errors);
                for hit in hits {
                    let rel = match hit.read_path.strip_prefix(&target) {
                        Ok(r) => r,
                        Err(_) => continue,
                    };
                    let virtual_path = p.join(rel);
                    // Measure depth relative to the vault root so the budget
                    // reflects symlinked content only — vault-root components
                    // would otherwise eat most of the budget at deep paths.
                    let relative_depth = virtual_path
                        .strip_prefix(&vault_root)
                        .map(|r| r.components().count())
                        .unwrap_or_else(|_| virtual_path.components().count());
                    if relative_depth > MAX_VIRTUAL_DEPTH {
                        outcome.errors.push(format!(
                            "depth: {} exceeds the {MAX_VIRTUAL_DEPTH}-segment budget, skipping",
                            virtual_path.display()
                        ));
                        continue;
                    }
                    outcome.files.push(WalkedFile {
                        virtual_path,
                        read_path: hit.read_path,
                    });
                }
            }
        }
    }

    outcome
}

/// Convenience: collect every entity id that the walked files route to, in
/// one pass. Used by the CLI ingest runner to populate the linker set.
pub fn entity_ids_for(files: &[WalkedFile], vault_root: &Path) -> HashSet<String> {
    use crate::pipeline::entity_id_for_virtual_path;
    let root = vault_root.to_str().unwrap_or("");
    files
        .iter()
        .map(|f| entity_id_for_virtual_path(f.virtual_path.to_str().unwrap_or(""), Some(root)))
        .collect()
}
