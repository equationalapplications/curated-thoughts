//! The walker must report symlinked content under its vault-relative virtual
//! path, never the canonical target path. And the vault root must be
//! canonicalized before virtual paths are built so entity_id_for_virtual_path's
//! prefix-strip succeeds on non-canonical vault roots (Ruling 2).

#![cfg(unix)]

use curated_thoughts_tools::cmds::{collect_files, WalkedFile};
use std::fs;
use std::os::unix::fs::symlink;
use tauri_app_lib::entity_id_for_virtual_path;
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

#[test]
fn symlinked_file_is_reported_under_the_vault_relative_virtual_path() {
    let (_temp, vault) = fixture();
    let mut out: Vec<WalkedFile> = Vec::new();
    let mut errors = Vec::new();

    collect_files(&vault, true, &mut out, &mut errors);

    let hit = out
        .iter()
        .find(|f| f.virtual_path.to_string_lossy().ends_with("design.md"))
        .expect("symlinked file must be collected");

    let virtual_str = hit.virtual_path.to_string_lossy().to_string();
    assert!(
        virtual_str.contains("/documents/linked/"),
        "virtual path lost the symlink prefix: {virtual_str}"
    );
    assert!(
        !virtual_str.contains("/outside/"),
        "virtual path leaked the canonical target: {virtual_str}"
    );
    assert!(
        hit.read_path.to_string_lossy().contains("/outside/"),
        "read path must point at the real file: {:?}",
        hit.read_path
    );
    assert!(hit.read_path.exists(), "read path must be openable");
}

#[test]
fn plain_files_have_identical_virtual_and_read_paths() {
    let (_temp, vault) = fixture();
    fs::write(vault.join("documents").join("plain.md"), "# Plain\n").unwrap();

    let mut out: Vec<WalkedFile> = Vec::new();
    let mut errors = Vec::new();
    collect_files(&vault, true, &mut out, &mut errors);

    let hit = out
        .iter()
        .find(|f| f.virtual_path.ends_with("plain.md"))
        .expect("plain file collected");
    assert_eq!(hit.virtual_path, hit.read_path);
}

#[test]
fn a_symlink_nested_inside_a_resolved_target_is_not_followed() {
    let (temp, vault) = fixture();
    let deeper = temp.path().join("deeper");
    fs::create_dir_all(&deeper).unwrap();
    fs::write(deeper.join("secret.md"), "# Secret\n").unwrap();
    symlink(&deeper, temp.path().join("outside").join("docs").join("nested")).unwrap();

    let mut out: Vec<WalkedFile> = Vec::new();
    let mut errors = Vec::new();
    collect_files(&vault, true, &mut out, &mut errors);

    assert!(
        !out.iter().any(|f| f.virtual_path.ends_with("secret.md")),
        "nested symlinks must never be descended into"
    );
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
    collect_files(&linked_vault, true, &mut out, &mut errors);

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
