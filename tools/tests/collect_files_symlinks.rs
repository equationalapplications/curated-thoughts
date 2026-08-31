//! Tests for the walker, the trusted-links ledger integration, and the
//! Ruling-2 canonicalization that keeps virtual paths from silently
//! misrouting on non-canonical vault roots.
//!
//! The walker is now split into two layers:
//! - `collect_files` is a plain non-following walker; it canonicalizes its
//!   root (Ruling 2) and emits (virtual_path, read_path) pairs.
//! - `walk_vault` consults the trusted-links ledger and only descends into
//!   documents/-rooted symlinks whose target is Trusted; Pending and Denied
//!   links are reported, never read.

#![cfg(unix)]

use curated_thoughts_tools::cmds::{collect_files, walk_vault, WalkedFile};
use std::fs;
use std::os::unix::fs::symlink;
use tauri_app_lib::entity_id_for_virtual_path;
use tauri_app_lib::trusted_links::TrustedLink;
use tempfile::TempDir;

/// vault/documents/linked -> outside/docs, which holds one markdown file.
fn fixture() -> (TempDir, std::path::PathBuf) {
    let temp = TempDir::new().unwrap();
    let vault = temp.path().join("vault");
    let docs = vault.join("documents");
    fs::create_dir_all(&docs).unwrap();

    let outside = temp.path().join("outside").join("docs");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("design.md"), "# Design\n").unwrap();

    symlink(&outside, docs.join("linked")).unwrap();
    (temp, vault)
}

// ---------------------------------------------------------------------------
// collect_files (plain, non-following) — Ruling 2 regression lives here.
// ---------------------------------------------------------------------------

#[test]
fn plain_files_have_identical_virtual_and_read_paths() {
    let (_temp, vault) = fixture();
    fs::write(vault.join("documents").join("plain.md"), "# Plain\n").unwrap();

    let mut out: Vec<WalkedFile> = Vec::new();
    let mut errors = Vec::new();
    collect_files(&vault, &mut out, &mut errors);

    let hit = out
        .iter()
        .find(|f| f.virtual_path.ends_with("plain.md"))
        .expect("plain file collected");
    assert_eq!(hit.virtual_path, hit.read_path);
}

/// Ruling 2 regression: a vault reached through a non-canonical path (a
/// symlink that resolves to a different absolute path) must still produce
/// virtual paths that `entity_id_for_virtual_path` can route to `tier_fact`.
/// Without the walker's root-canonicalization the prefix strip fails
/// silently and every walked file misroutes to `tier_working::`.
#[test]
fn walker_canonicalizes_root_so_entity_id_routes_under_documents() {
    let temp = TempDir::new().unwrap();
    let real_vault = temp.path().join("real_vault");
    let docs = real_vault.join("documents");
    fs::create_dir_all(&docs).unwrap();
    fs::write(docs.join("design.md"), "# Design\n").unwrap();

    // Reach the vault through a symlink so the input path is non-canonical.
    let linked_vault = temp.path().join("linked_vault");
    symlink(&real_vault, &linked_vault).unwrap();

    let mut out: Vec<WalkedFile> = Vec::new();
    let mut errors = Vec::new();
    collect_files(&linked_vault, &mut out, &mut errors);

    let hit = out
        .iter()
        .find(|f| f.virtual_path.ends_with("design.md"))
        .expect("walked design.md");

    let canonical_root = fs::canonicalize(&real_vault).unwrap();
    let entity_id = entity_id_for_virtual_path(
        hit.virtual_path.to_str().unwrap(),
        Some(canonical_root.to_str().unwrap()),
    );
    assert_eq!(
        entity_id, "tier_fact",
        "walker must build virtual paths from the canonical vault root; \
         otherwise entity_id_for_virtual_path silently misroutes. \
         got virtual_path={:?}",
        hit.virtual_path
    );
}

// ---------------------------------------------------------------------------
// walk_vault — ledger integration.
// ---------------------------------------------------------------------------

#[test]
fn an_unapproved_symlink_is_pending_and_its_content_is_not_collected() {
    let (_temp, vault) = fixture();

    let outcome = walk_vault(&vault, &[], None);

    assert!(
        !outcome
            .files
            .iter()
            .any(|f| f.virtual_path.ends_with("design.md")),
        "unapproved symlink content must not be collected"
    );
    assert_eq!(outcome.pending.len(), 1);
    assert_eq!(outcome.pending[0].link, "documents/linked");
    assert!(outcome.pending[0].target.ends_with("outside/docs"));
}

#[test]
fn an_approved_pair_is_walked() {
    let (temp, vault) = fixture();
    let target = std::fs::canonicalize(temp.path().join("outside").join("docs")).unwrap();

    let ledger = vec![TrustedLink {
        link: "documents/linked".to_string(),
        target: target.to_string_lossy().to_string(),
        approved_at: 1,
    }];

    let outcome = walk_vault(&vault, &ledger, None);

    assert!(
        outcome.pending.is_empty(),
        "approved link must not be pending"
    );
    assert!(
        outcome
            .files
            .iter()
            .any(|f| f.virtual_path.ends_with("design.md")),
        "approved link content must be collected"
    );
}

#[test]
fn a_repointed_link_becomes_pending_again() {
    let (temp, vault) = fixture();
    let ledger = vec![TrustedLink {
        link: "documents/linked".to_string(),
        target: "/some/other/place".to_string(),
        approved_at: 1,
    }];
    let _ = temp;

    let outcome = walk_vault(&vault, &ledger, None);

    assert_eq!(
        outcome.pending.len(),
        1,
        "stale target must not grant trust"
    );
    assert!(outcome.files.is_empty());
}

#[test]
fn a_denied_target_reports_its_rule_and_is_never_walked() {
    let temp = TempDir::new().unwrap();
    let vault = temp.path().join("vault");
    fs::create_dir_all(vault.join("documents")).unwrap();
    // Link straight at the directory that contains the vault.
    symlink(temp.path(), vault.join("documents").join("everything")).unwrap();

    let target = fs::canonicalize(temp.path()).unwrap();
    let ledger = vec![TrustedLink {
        link: "documents/everything".to_string(),
        target: target.to_string_lossy().to_string(),
        approved_at: 1,
    }];

    let outcome = walk_vault(&vault, &ledger, None);

    assert!(outcome.files.is_empty(), "denied target must not be walked");
    assert_eq!(outcome.denied.len(), 1);
    assert!(
        outcome.denied[0].reason.contains("vault"),
        "reason must name the rule, got: {}",
        outcome.denied[0].reason
    );
}

#[test]
fn a_broken_symlink_is_an_error_not_a_silent_skip() {
    let temp = TempDir::new().unwrap();
    let vault = temp.path().join("vault");
    fs::create_dir_all(vault.join("documents")).unwrap();
    symlink(
        temp.path().join("does-not-exist"),
        vault.join("documents").join("dangling"),
    )
    .unwrap();

    let outcome = walk_vault(&vault, &[], None);

    assert!(
        outcome.errors.iter().any(|e| e.contains("dangling")),
        "broken symlink must be reported as an error: {:?}",
        outcome.errors
    );
}

/// MAX_VIRTUAL_DEPTH must count vault-relative segments only — vault-root
/// components should not eat into the budget. Without the strip_prefix
/// guard, a vault reached through a deep absolute path would skip legitimate
/// content.
#[test]
fn depth_budget_counts_only_vault_relative_segments() {
    use curated_thoughts_tools::cmds::MAX_VIRTUAL_DEPTH;

    // Build a deeply nested outside target — the deepest file lands well
    // past MAX_VIRTUAL_DEPTH, but vault-relative only by a small margin.
    let temp = TempDir::new().unwrap();
    let vault = temp.path().join("vault");
    fs::create_dir_all(vault.join("documents")).unwrap();

    // Vault itself lives many components deep so an absolute-path component
    // count would overshoot the budget even for shallow inside-vault files.
    let deep_outside = temp
        .path()
        .join("a")
        .join("b")
        .join("c")
        .join("d")
        .join("outside");
    fs::create_dir_all(&deep_outside).unwrap();
    let leaf = deep_outside.join("leaf.md");
    fs::write(&leaf, "# Leaf\n").unwrap();

    symlink(&deep_outside, vault.join("documents").join("linked")).unwrap();

    let target = std::fs::canonicalize(&deep_outside).unwrap();
    let ledger = vec![TrustedLink {
        link: "documents/linked".to_string(),
        target: target.to_string_lossy().to_string(),
        approved_at: 1,
    }];

    let outcome = walk_vault(&vault, &ledger, None);

    // The leaf is 1 segment past `documents/linked/` — well inside the budget
    // when measured relative to the vault root.
    assert!(
        outcome
            .files
            .iter()
            .any(|f| f.virtual_path.ends_with("leaf.md")),
        "vault-relative depth must not overshoot MAX_VIRTUAL_DEPTH; errors={:?}",
        outcome.errors
    );
    assert!(
        !outcome
            .errors
            .iter()
            .any(|e| e.contains("exceeds the") && e.contains("leaf.md")),
        "leaf.md must not be reported as over-depth; errors={:?}",
        outcome.errors
    );

    // Sanity check: an actual over-depth file is still rejected.
    let very_deep_outside = temp.path().join("x");
    fs::create_dir_all(&very_deep_outside).unwrap();
    let mut cur = very_deep_outside.clone();
    for _ in 0..(MAX_VIRTUAL_DEPTH + 4) {
        cur = cur.join("seg");
        fs::create_dir_all(&cur).unwrap();
    }
    let too_deep = cur.join("deep.md");
    fs::write(&too_deep, "# Deep\n").unwrap();
    symlink(&very_deep_outside, vault.join("documents").join("deep")).unwrap();

    let target2 = std::fs::canonicalize(&very_deep_outside).unwrap();
    let ledger2 = vec![TrustedLink {
        link: "documents/deep".to_string(),
        target: target2.to_string_lossy().to_string(),
        approved_at: 1,
    }];
    let outcome2 = walk_vault(&vault, &ledger2, None);
    assert!(
        !outcome2
            .files
            .iter()
            .any(|f| f.virtual_path.ends_with("deep.md")),
        "truly over-depth files must still be skipped"
    );
    assert!(
        outcome2
            .errors
            .iter()
            .any(|e| e.contains("deep.md") && e.contains("exceeds the")),
        "over-depth file must surface as an error: {:?}",
        outcome2.errors
    );
}
