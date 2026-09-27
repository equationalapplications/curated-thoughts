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
use std::path::{Component, Path, PathBuf};
use walkdir::WalkDir;

/// CT's own working-directory name. Named separately from `EXCLUDED_DIRS`
/// because the empty-walk branch of `reconcile_vault` is scoped to THIS
/// name only, never to the whole constant (spec item 4).
pub const BRAIN_DIR_NAME: &str = ".brain";

/// Directory names never ingested (build artifacts, deps, VCS internals).
const EXCLUDED_DIRS: &[&str] = &[
    // CT's own working directory. CT writes into it itself
    // (`pipeline/mod.rs:484`) and no code path has ever legitimately
    // ingested from it. See the brain-dir-exclusion spec.
    BRAIN_DIR_NAME,
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

/// Directory names excluded ONLY as the FIRST vault-relative path
/// component. These are CT-managed top-level trees, not generic names:
/// `records` is the working-records tree (sessions, operations, archive),
/// writable via `vault_write_note` but never ingested. A same-named
/// directory NESTED deeper (`documents/x/records/`, `wiki/records/`) is
/// ordinary content and must keep ingesting (M4) — matching the name at
/// any depth made an upgrade silently stop ingesting those trees and let
/// reconcile DELETE their existing rows. Sibling lookalikes
/// (`records-archive/`) were never excluded (spec D4). Spec:
/// docs/superpowers/specs/2026-09-27-vault-ingest-policy-design.md, F1.
const EXCLUDED_ROOT_DIRS: &[&str] = &["records"];

fn is_excluded_dir(dir_name: &str) -> bool {
    EXCLUDED_DIRS.contains(&dir_name)
}

/// True when `rel`'s FIRST component is an excluded root-only directory
/// name. `rel` MUST be vault-root-relative (same contract as
/// [`rel_path_has_excluded_component`]).
fn first_component_is_excluded_root_dir(rel: &Path) -> bool {
    rel.components().next().is_some_and(|c| match c {
        Component::Normal(n) => EXCLUDED_ROOT_DIRS.contains(&n.to_string_lossy().as_ref()),
        _ => false,
    })
}

/// True when any *component* of `rel` is an excluded directory name.
///
/// `rel` MUST be vault-root-relative. Passing an absolute path is a bug:
/// `EXCLUDED_DIRS` names (`target`, `build`, `out`, `venv`, …) occur in
/// ordinary ancestor directories, so an absolute match would reject every
/// path in a vault that happens to live under one (spec D1). There is
/// deliberately no absolute-path convenience overload — use
/// [`abs_path_is_excluded_in_vault`], which relativizes first.
///
/// Any component counts, including the FINAL one: a file literally named
/// `target/out/build` would match. That is acceptable — such a file has no
/// ingestable extension — but callers relying on file-level matching
/// should know the predicate is not directory-only.
pub fn rel_path_has_excluded_component(rel: &Path) -> bool {
    rel.components().any(|c| match c {
        Component::Normal(n) => is_excluded_dir(&n.to_string_lossy()),
        _ => false,
    }) || first_component_is_excluded_root_dir(rel)
}

/// Narrow sibling of [`rel_path_has_excluded_component`] matching ONLY
/// `.brain`.
///
/// Used exclusively by the empty-walk branch of `reconcile_vault`, where
/// punching a hole in the mount-failure safety net for all of
/// `EXCLUDED_DIRS` would be over-scoped: for `node_modules/`, `target/`,
/// … an empty walk is not proof the row should be absent. A distinct
/// function rather than a parameter so the scope difference is visible at
/// the call site (spec item 4).
pub fn rel_path_has_brain_component(rel: &Path) -> bool {
    // `n.to_str() == Some(...)` rather than `n == BRAIN_DIR_NAME`: std has
    // no `&OsStr: PartialEq<&str>` impl, so the direct comparison does not
    // compile.
    rel.components()
        .any(|c| matches!(c, Component::Normal(n) if n.to_str() == Some(BRAIN_DIR_NAME)))
}

/// Relativize `abs` against `vault_root`, trying three prefix pairs:
/// as-configured root, canonical root (macOS `/var` → `/private/var`),
/// and finally a canonicalized `abs` against the canonical root — for a
/// canonical root paired with a NON-canonical event path (symlinked
/// ancestor, or a notify-delivered `/var/...` spelling). Only the third
/// case fails today, and it fails OPEN (stages the `.brain` event).
/// Canonicalizing `abs` cannot mask a genuinely symlinked-out `.brain`:
/// `canonicalize` resolves the symlink, so the strip still misses — same
/// verdict as before, just without the ordinary-spelling false negatives.
///
/// Returns `None` when no prefix pair matches. Callers MUST treat `None`
/// as "not excluded" — see [`abs_path_is_excluded_in_vault`] and spec D2b.
pub fn relativize_to_vault(abs: &Path, vault_root: &Path) -> Option<PathBuf> {
    if let Ok(rel) = abs.strip_prefix(vault_root) {
        return Some(rel.to_path_buf());
    }
    let canonical = std::fs::canonicalize(vault_root).ok()?;
    if let Ok(rel) = abs.strip_prefix(&canonical) {
        return Some(rel.to_path_buf());
    }
    let canonical_abs = std::fs::canonicalize(abs).ok()?;
    canonical_abs
        .strip_prefix(&canonical)
        .ok()
        .map(Path::to_path_buf)
}

/// Fail-open exclusion test for an absolute path against a vault root.
///
/// Returns `false` when the path cannot be relativized against any form
/// of the root (spec D2b): the event stages, the row is left alone.
/// Deleting rows we cannot place inside the vault is the same class of
/// unrecoverable mistake the empty-walk guard exists to prevent.
pub fn abs_path_is_excluded_in_vault(abs: &Path, vault_root: &Path) -> bool {
    relativize_to_vault(abs, vault_root)
        .map(|rel| rel_path_has_excluded_component(&rel))
        .unwrap_or(false)
}

/// `.brain`-only, fail-open counterpart of
/// [`abs_path_is_excluded_in_vault`] (spec item 4).
pub fn abs_path_has_brain_in_vault(abs: &Path, vault_root: &Path) -> bool {
    relativize_to_vault(abs, vault_root)
        .map(|rel| rel_path_has_brain_component(&rel))
        .unwrap_or(false)
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

/// Editor scratch files. These exist for milliseconds; the walker rarely
/// catches one, but the filesystem watcher sees every single one and used to
/// stage a `documents` row for it. The row then outlives the file forever
/// (see spec §3).
const EXCLUDED_FILE_SUFFIXES: &[&str] = &["~", ".tmp", ".swp", ".swx"];

/// Emacs lock (`.#name`) and autosave (`#name#`) prefixes, plus vim's numeric
/// writability probe file, which is literally named `4913`.
const EXCLUDED_FILE_PREFIXES: &[&str] = &[".#", "#"];
const EXCLUDED_FILE_EXACT_TEMP: &[&str] = &["4913"];

pub(crate) fn is_excluded_file(path: &Path) -> bool {
    if let Some(name) = path.file_name() {
        let name = name.to_string_lossy();
        if EXCLUDED_FILE_NAMES.contains(&name.as_ref()) {
            return true;
        }
        if EXCLUDED_FILE_EXACT_TEMP.contains(&name.as_ref()) {
            return true;
        }
        if EXCLUDED_FILE_SUFFIXES.iter().any(|suf| name.ends_with(suf)) {
            return true;
        }
        if EXCLUDED_FILE_PREFIXES
            .iter()
            .any(|pre| name.starts_with(pre))
        {
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
/// `entity_id_for_virtual_path` canonicalizes against (Ruling 2). Symlink
/// following is exclusively [`walk_vault`]'s responsibility now — callers
/// that need ledger-aware descent through `documents/` symlinks should use
/// `walk_vault` directly rather than re-introducing following here.
pub fn collect_files(root: &Path, out: &mut Vec<WalkedFile>, errors: &mut Vec<String>) {
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let walker = WalkDir::new(&canonical_root).follow_links(false);
    let it = walker.into_iter().filter_entry(|e| {
        // The root itself is exempt from the prune. A vault rooted at
        // `<tmp>/.brain/` (or under any EXCLUDED_DIRS name) must still be
        // descended into: `filter_entry` returning false at depth 0
        // short-circuits the ENTIRE walk before any descent, silently
        // returning an empty vault (spec item 2).
        if e.depth() == 0 {
            return true;
        }
        if e.file_type().is_dir() {
            if let Some(name) = e.path().file_name() {
                if is_excluded_dir(&name.to_string_lossy()) {
                    return false;
                }
                // Root-only names (`records`) prune ONLY at depth 1 — the
                // first vault-relative component. Deeper same-named dirs
                // are ordinary content (M4).
                let name_str = name.to_string_lossy();
                if e.depth() == 1 && EXCLUDED_ROOT_DIRS.contains(&name_str.as_ref()) {
                    return false;
                }
                return true;
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
    collect_files(&vault_root, &mut outcome.files, &mut outcome.errors);

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
        let link_rel = format!("documents/{name}");
        if is_excluded_dir(&name) {
            // Report, never skip. `.brain` joined EXCLUDED_DIRS for the
            // walker prune; a silent `continue` here would make an approved
            // `documents/.brain` link vanish from classification entirely
            // and let reconcile delete its rows (spec item 2 carve-out).
            // On a broken symlink, record the link's own path rather than
            // an empty string — matches the broken-symlink error the
            // classification loop pushes further down.
            let target = std::fs::canonicalize(&p)
                .map(|t| t.to_string_lossy().into_owned())
                .unwrap_or_else(|_| p.to_string_lossy().into_owned());
            outcome.denied.push(DeniedLink {
                link: link_rel,
                target,
                reason: crate::trusted_links::DenyReason::ExcludedDirName
                    .message()
                    .to_string(),
            });
            continue;
        }
        // M4: `records` is a ROOT-only exclusion. A link nested under
        // `documents/` (e.g. `documents/records` → a sibling tree) is an
        // ordinary link now — it goes through the normal classification
        // below instead of the blanket excluded-name denial.

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
                collect_files(&target, &mut hits, &mut outcome.errors);
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

#[cfg(test)]
mod tests {
    #[test]
    fn editor_temp_files_are_excluded() {
        use std::path::Path;
        for name in [
            "note.md~",
            "note.md.tmp",
            ".note.md.swp",
            ".note.md.swx",
            ".#note.md",
            "#note.md#",
            "4913",
        ] {
            assert!(
                super::is_excluded_file(Path::new(name)),
                "{name} must be excluded"
            );
        }
    }

    #[test]
    fn ordinary_notes_are_not_excluded() {
        use std::path::Path;
        // The filter must not over-match. These are real vault content.
        for name in [
            "note.md",
            "tmp.md",
            "swap-notes.md",
            "meeting~notes.md",
            "49130.md",
        ] {
            assert!(
                !super::is_excluded_file(Path::new(name)),
                "{name} must NOT be excluded"
            );
        }
    }

    // ---- exclusion predicates (spec D1/D2b/D4) -------------------------

    #[test]
    fn rel_predicate_matches_brain_component_exactly() {
        use std::path::Path;
        assert!(super::rel_path_has_excluded_component(Path::new(
            ".brain/errors.log"
        )));
        assert!(super::rel_path_has_excluded_component(Path::new(
            "immutable-source-files/agents/people/.brain/errors.log"
        )));
        assert!(super::rel_path_has_excluded_component(Path::new(
            "node_modules/pkg/index.js"
        )));
    }

    /// Spec D4: substring lookalikes must still ingest.
    #[test]
    fn rel_predicate_rejects_substring_lookalikes() {
        use std::path::Path;
        assert!(!super::rel_path_has_excluded_component(Path::new(
            "brain/x.md"
        )));
        assert!(!super::rel_path_has_excluded_component(Path::new(
            "my.brain.notes/x.md"
        )));
        assert!(!super::rel_path_has_excluded_component(Path::new(
            ".brainish/x.md"
        )));
        assert!(!super::rel_path_has_excluded_component(Path::new(
            "notes.md"
        )));
    }

    /// Spec item 4: the narrow predicate matches ONLY `.brain`.
    #[test]
    fn brain_predicate_is_narrower_than_excluded_predicate() {
        use std::path::Path;
        assert!(super::rel_path_has_brain_component(Path::new(
            "a/.brain/x.log"
        )));
        assert!(!super::rel_path_has_brain_component(Path::new(
            "node_modules/x.md"
        )));
        assert!(!super::rel_path_has_brain_component(Path::new(
            "target/x.md"
        )));
        // ...but the broad one does match those.
        assert!(super::rel_path_has_excluded_component(Path::new(
            "node_modules/x.md"
        )));
    }

    /// Spec D1: an EXCLUDED_DIRS name in an ANCESTOR of the vault root must
    /// not reject content. Without relativization this deletes/rejects the
    /// whole vault.
    #[test]
    fn abs_predicate_ignores_excluded_name_in_vault_ancestor() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("build").join("wiki");
        std::fs::create_dir_all(&root).unwrap();
        assert!(!super::abs_path_is_excluded_in_vault(
            &root.join("notes.md"),
            &root
        ));
        assert!(super::abs_path_is_excluded_in_vault(
            &root.join(".brain").join("errors.log"),
            &root
        ));

        let root2 = tmp.path().join("target").join("wiki");
        std::fs::create_dir_all(&root2).unwrap();
        assert!(!super::abs_path_is_excluded_in_vault(
            &root2.join("notes.md"),
            &root2
        ));
    }

    /// Spec D2b: a path under neither the as-configured nor the canonical
    /// root is treated as NOT excluded. Deleting what we cannot place is
    /// the unrecoverable mistake.
    #[test]
    fn abs_predicate_fails_open_when_not_relativizable() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("vault");
        std::fs::create_dir_all(&root).unwrap();
        let outside = tmp.path().join("elsewhere").join(".brain").join("x.log");
        assert!(!super::abs_path_is_excluded_in_vault(&outside, &root));
        assert!(!super::abs_path_has_brain_in_vault(&outside, &root));
    }

    /// macOS `/var` → `/private/var`: a non-canonical root must still
    /// relativize via the canonical fallback.
    #[test]
    fn relativize_falls_back_to_canonical_root() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("vault");
        std::fs::create_dir_all(root.join(".brain")).unwrap();
        let canonical_root = std::fs::canonicalize(&root).unwrap();
        let abs = canonical_root.join(".brain").join("errors.log");
        // `root` may be the non-canonical /var form; `abs` is canonical.
        assert_eq!(
            super::relativize_to_vault(&abs, &root),
            Some(std::path::PathBuf::from(".brain/errors.log"))
        );
    }

    /// The mirror case of the test above: CANONICAL root, NON-canonical
    /// event path (symlinked ancestor, or notify delivering `/var/...`
    /// while the config stores `/private/var/...`). Only the
    /// canonicalize-the-input fallback catches this; without it the
    /// failure mode is fail-open — the `.brain` event stages (spec D2b).
    #[cfg(unix)]
    #[test]
    fn relativize_falls_back_to_canonicalized_input() {
        let tmp = tempfile::TempDir::new().unwrap();
        let real_root = tmp.path().join("real-vault");
        std::fs::create_dir_all(real_root.join(".brain")).unwrap();
        // The leaf file must exist for `canonicalize(abs)` (step 3 of
        // `relativize_to_vault`) to resolve the symlink spelling — in
        // production the watcher only fires for paths that exist.
        std::fs::write(real_root.join(".brain").join("errors.log"), b"").unwrap();
        let link_root = tmp.path().join("link-vault");
        std::os::unix::fs::symlink(&real_root, &link_root).unwrap();

        // `root` is canonical (as the desktop pre-pass stores it,
        // lib.rs:1171-1176); `abs` arrives through the symlink spelling.
        let canonical_root = std::fs::canonicalize(&real_root).unwrap();
        let abs = link_root.join(".brain").join("errors.log");
        assert_eq!(
            super::relativize_to_vault(&abs, &canonical_root),
            Some(std::path::PathBuf::from(".brain/errors.log"))
        );

        // A genuinely symlinked-OUT .brain must still NOT relativize —
        // canonicalize resolves the link, the strip misses, fail-open
        // holds (spec D2). `outside-vault` links elsewhere entirely.
        let outside = tmp.path().join("outside-vault");
        std::fs::create_dir_all(outside.join(".brain")).unwrap();
        std::fs::write(outside.join(".brain").join("x.log"), b"").unwrap();
        let outside_link = tmp.path().join("outside-link");
        std::os::unix::fs::symlink(&outside, &outside_link).unwrap();
        assert_eq!(
            super::relativize_to_vault(&outside_link.join(".brain").join("x.log"), &canonical_root),
            None
        );
    }

    /// Spec item 2: a vault rooted AT `.brain` must still be walked. Without
    /// the depth-0 exemption `filter_entry` short-circuits and returns zero
    /// files.
    ///
    /// The nested file is a `.md`: an ingestable extension, so the ONLY
    /// thing that can filter it is the directory prune. A `.log` fixture
    /// would pass even with the prune deleted (the extension gate filters
    /// it anyway) and pin nothing.
    #[test]
    fn collect_files_exempts_the_root_from_the_prune() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join(".brain");
        std::fs::create_dir_all(root.join("nested").join(".brain")).unwrap();
        std::fs::create_dir_all(root.join("my.brain.notes")).unwrap();
        std::fs::write(root.join("notes.md"), b"a").unwrap();
        std::fs::write(root.join("my.brain.notes").join("x.md"), b"b").unwrap();
        std::fs::write(root.join("nested").join(".brain").join("leaked.md"), b"c").unwrap();

        let mut out = Vec::new();
        let mut errs = Vec::new();
        super::collect_files(&root, &mut out, &mut errs);

        let names: Vec<String> = out
            .iter()
            .map(|f| f.virtual_path.to_string_lossy().into_owned())
            .collect();
        assert!(
            names.iter().any(|n| n.ends_with("notes.md")),
            "root exemption failed; walk returned {names:?}"
        );
        assert!(names.iter().any(|n| n.ends_with("my.brain.notes/x.md")));
        assert!(
            !names.iter().any(|n| n.contains("nested/.brain")),
            "descendant .brain was not pruned: {names:?}"
        );
    }

    /// Nested `.brain` is pruned in an ordinary vault. Same `.md`-fixture
    /// rule as above: the extension gate must not be able to pass this test
    /// on the prune's behalf.
    #[test]
    fn collect_files_prunes_nested_brain_dirs() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("vault");
        std::fs::create_dir_all(root.join(".brain")).unwrap();
        std::fs::create_dir_all(root.join("nested").join(".brain")).unwrap();
        std::fs::create_dir_all(root.join("brain")).unwrap();
        std::fs::create_dir_all(root.join(".brainish")).unwrap();
        std::fs::write(root.join("notes.md"), b"a").unwrap();
        std::fs::write(root.join(".brain").join("leaked.md"), b"b").unwrap();
        std::fs::write(root.join("nested").join(".brain").join("leaked.md"), b"c").unwrap();
        std::fs::write(root.join("brain").join("x.md"), b"d").unwrap();
        std::fs::write(root.join(".brainish").join("x.md"), b"e").unwrap();

        let mut out = Vec::new();
        let mut errs = Vec::new();
        super::collect_files(&root, &mut out, &mut errs);
        let names: Vec<String> = out
            .iter()
            .map(|f| f.virtual_path.to_string_lossy().into_owned())
            .collect();

        assert!(names.iter().any(|n| n.ends_with("notes.md")));
        assert!(names.iter().any(|n| n.ends_with("brain/x.md")));
        assert!(names.iter().any(|n| n.ends_with(".brainish/x.md")));
        assert!(
            !names.iter().any(|n| n.contains("/.brain/")),
            "walk leaked .brain content: {names:?}"
        );
    }

    /// Spec item 2 carve-out: a `documents/.brain` symlink must be reported
    /// as Denied, never silently dropped. A silent drop removes it from the
    /// approvals UI and lets reconcile delete its rows as "vanished".
    #[cfg(unix)]
    #[test]
    fn excluded_name_symlink_is_denied_not_silently_skipped() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("vault");
        std::fs::create_dir_all(root.join("documents")).unwrap();
        let target = tmp.path().join("outside-target");
        std::fs::create_dir_all(&target).unwrap();
        std::os::unix::fs::symlink(&target, root.join("documents").join(".brain")).unwrap();

        let outcome = super::walk_vault(&root, &[], None);

        assert_eq!(
            outcome.denied.len(),
            1,
            "expected exactly one denied link, got {:?} (pending: {:?})",
            outcome.denied,
            outcome.pending
        );
        assert_eq!(outcome.denied[0].link, "documents/.brain");
        assert_eq!(
            outcome.denied[0].reason,
            crate::trusted_links::DenyReason::ExcludedDirName.message()
        );
        assert!(
            outcome.pending.is_empty(),
            "excluded-name link must not be offered for approval"
        );
    }

    // ---- records/ exclusion (spec 2026-09-27-vault-ingest-policy, F1) ----

    /// The `records/` tree is never ingested: no file under it may appear in
    /// the walk, while sibling lookalikes still ingest. `records` is
    /// anchored to the FIRST vault-relative component ONLY (M4) — nested
    /// `documents/x/records/` or `wiki/records/` trees are ordinary
    /// content and must still ingest. Fixtures are `.md` so the extension
    /// gate cannot pass this test on the prune's behalf (same rule as the
    /// `.brain` prune tests above).
    #[test]
    fn collect_files_prunes_records_tree() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("vault");
        std::fs::create_dir_all(root.join("records/sessions")).unwrap();
        std::fs::create_dir_all(root.join("records-but-not-really")).unwrap();
        std::fs::create_dir_all(root.join("documents").join("x").join("records")).unwrap();
        std::fs::write(root.join("notes.md"), b"a").unwrap();
        std::fs::write(root.join("records").join("session.md"), b"b").unwrap();
        std::fs::write(root.join("records/sessions").join("deep.md"), b"c").unwrap();
        std::fs::write(root.join("records-but-not-really").join("x.md"), b"d").unwrap();
        std::fs::write(
            root.join("documents")
                .join("x")
                .join("records")
                .join("kept.md"),
            b"e",
        )
        .unwrap();

        let mut out = Vec::new();
        let mut errs = Vec::new();
        super::collect_files(&root, &mut out, &mut errs);
        let names: Vec<String> = out
            .iter()
            .map(|f| f.virtual_path.to_string_lossy().into_owned())
            .collect();

        assert!(names.iter().any(|n| n.ends_with("notes.md")));
        assert!(names
            .iter()
            .any(|n| n.ends_with("records-but-not-really/x.md")));
        // M4: nested records dirs are ordinary content — still ingested.
        assert!(
            names
                .iter()
                .any(|n| n.ends_with("documents/x/records/kept.md")),
            "nested records/ tree must still ingest: {names:?}"
        );
        let root_records_prefix = format!("{}/", root.join("records").to_string_lossy());
        assert!(
            !names
                .iter()
                .any(|n| n.starts_with(root_records_prefix.as_str())),
            "walk leaked vault-root records/ content: {names:?}"
        );
    }

    /// The `records` exclusion is anchored to the FIRST vault-relative
    /// component ONLY (M4): the exact segment `records` as the first
    /// component excludes; the same name NESTED is ordinary content, and
    /// substring/sibling lookalikes still ingest.
    #[test]
    fn records_predicate_matches_first_component_only() {
        use std::path::Path;
        // Vault-root records/ tree: excluded.
        assert!(super::rel_path_has_excluded_component(Path::new(
            "records/session.md"
        )));
        assert!(super::rel_path_has_excluded_component(Path::new(
            "records/sessions/deep/x.md"
        )));
        // Nested records/ dirs: ordinary content, still ingested (M4).
        assert!(!super::rel_path_has_excluded_component(Path::new(
            "documents/x/records/kept.md"
        )));
        assert!(!super::rel_path_has_excluded_component(Path::new(
            "wiki/records/note.md"
        )));
        // Substring lookalikes must still ingest (spec D4 pattern).
        assert!(!super::rel_path_has_excluded_component(Path::new(
            "my.records/x.md"
        )));
        assert!(!super::rel_path_has_excluded_component(Path::new(
            "records-evil/x.md"
        )));
    }

    /// The absolute-path fail-closed predicate covers `records/` too — this
    /// is the hook the watcher pre-pass and reconcile use to keep records
    /// rows out of (and deletable from) the DB.
    #[test]
    fn abs_predicate_excludes_records_under_vault_root() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("vault");
        std::fs::create_dir_all(root.join("records")).unwrap();
        assert!(super::abs_path_is_excluded_in_vault(
            &root.join("records").join("session.md"),
            &root
        ));
        assert!(!super::abs_path_is_excluded_in_vault(
            &root.join("notes.md"),
            &root
        ));
    }

    /// M4: `records` is a ROOT-only exclusion, so a `documents/records`
    /// symlink is an ORDINARY link now. With an in-vault target it is
    /// auto-Trusted (nothing leaves the vault boundary); it is neither
    /// denied nor silently skipped.
    #[cfg(unix)]
    #[test]
    fn records_name_symlink_under_documents_classifies_normally() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("vault");
        std::fs::create_dir_all(root.join("documents").join("real-records")).unwrap();
        let target = root.join("records-target");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("kept.md"), b"k").unwrap();
        std::os::unix::fs::symlink(&target, root.join("documents").join("records")).unwrap();

        let outcome = super::walk_vault(&root, &[], None);

        assert!(
            outcome.denied.is_empty(),
            "records-name link must not be denied anymore: {:?}",
            outcome.denied
        );
        // In-vault target: auto-trusted, its content is ingested.
        assert!(
            outcome
                .files
                .iter()
                .any(|f| f.virtual_path.ends_with("kept.md")),
            "records-link content must ingest: {:?}",
            outcome.files
        );
    }
}
