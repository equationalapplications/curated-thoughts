//! Path-traversal regression tests for Tauri command surface.
//!
//! Each test exercises the exact `(allowed_subdirs, mode)` tuple that one of
//! the migrated commands in `src-tauri/src/lib.rs` uses, with both a benign
//! payload (must succeed) and a malicious payload (must return Err).

use std::fs;

use tempfile::TempDir;

use tauri_app_lib::vault::{safe_vault_path, PathMode, SafePathError};

fn vault() -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    fs::create_dir_all(root.join("documents")).unwrap();
    fs::create_dir_all(root.join("wiki")).unwrap();
    fs::create_dir_all(root.join(".brain").join("proposed")).unwrap();
    (dir, root)
}

#[test]
fn read_document_benign_documents_path() {
    let (_g, root) = vault();
    let target = root.join("documents").join("note.md");
    fs::write(&target, b"x").unwrap();
    let out = safe_vault_path(
        &root,
        "documents/note.md",
        &["documents", "wiki"],
        PathMode::MustExist,
    )
    .unwrap();
    assert_eq!(out, target.canonicalize().unwrap());
}

#[test]
fn read_document_rejects_traversal_to_etc_passwd() {
    let (_g, root) = vault();
    let err = safe_vault_path(
        &root,
        "documents/../../../etc/passwd",
        &["documents", "wiki"],
        PathMode::MustExist,
    )
    .unwrap_err();
    assert!(matches!(err, SafePathError::Traversal), "got {err:?}");
}

#[test]
fn save_wiki_page_benign_relative_path() {
    let (_g, root) = vault();
    let out = safe_vault_path(&root, "wiki/new.md", &["wiki"], PathMode::MayCreate).unwrap();
    let expected = root.join("wiki").canonicalize().unwrap().join("new.md");
    assert_eq!(out, expected);
}

#[test]
fn save_wiki_page_rejects_absolute_md_payload_vuln1() {
    let (_g, root) = vault();
    let err = safe_vault_path(&root, "/tmp/pwn.md", &["wiki"], PathMode::MayCreate).unwrap_err();
    assert!(matches!(err, SafePathError::Absolute), "got {err:?}");
}

#[test]
fn delete_vault_file_benign_documents_path() {
    let (_g, root) = vault();
    let target = root.join("documents").join("gone.md");
    fs::write(&target, b"x").unwrap();
    let out = safe_vault_path(
        &root,
        "documents/gone.md",
        &["documents"],
        PathMode::MustExist,
    )
    .unwrap();
    assert_eq!(out, target.canonicalize().unwrap());
}

#[test]
fn delete_vault_file_rejects_traversal() {
    let (_g, root) = vault();
    let err = safe_vault_path(
        &root,
        "documents/../../target/file",
        &["documents"],
        PathMode::MustExist,
    )
    .unwrap_err();
    assert!(matches!(err, SafePathError::Traversal), "got {err:?}");
}

#[test]
fn approve_wiki_page_rejects_traversal_in_db_path() {
    let (_g, root) = vault();
    // Simulates a malicious wiki_pages.path row.
    let err =
        safe_vault_path(&root, "../etc/escape.md", &["wiki"], PathMode::MayCreate).unwrap_err();
    assert!(matches!(err, SafePathError::Traversal), "got {err:?}");
}

#[test]
fn copy_to_vault_filename_only_under_documents() {
    let (_g, root) = vault();
    let out = safe_vault_path(
        &root,
        "documents/incoming.txt",
        &["documents"],
        PathMode::MayCreate,
    )
    .unwrap();
    let expected = root
        .join("documents")
        .canonicalize()
        .unwrap()
        .join("incoming.txt");
    assert_eq!(out, expected);
}

#[test]
fn get_proposed_content_benign_proposed_path() {
    let (_g, root) = vault();
    let target = root.join(".brain").join("proposed").join("test.md");
    fs::write(&target, b"proposed content").unwrap();
    let out = safe_vault_path(
        &root,
        ".brain/proposed/test.md",
        &[".brain/proposed"],
        PathMode::MustExist,
    )
    .unwrap();
    assert_eq!(out, target.canonicalize().unwrap());
}

#[test]
fn get_proposed_content_rejects_traversal_in_db_path() {
    let (_g, root) = vault();
    let err = safe_vault_path(
        &root,
        ".brain/proposed/../../etc/passwd",
        &[".brain/proposed"],
        PathMode::MustExist,
    )
    .unwrap_err();
    assert!(matches!(err, SafePathError::Traversal), "got {err:?}");
}

#[test]
fn approve_wiki_page_benign_wiki_path() {
    let (_g, root) = vault();
    let out = safe_vault_path(&root, "wiki/approved.md", &["wiki"], PathMode::MayCreate).unwrap();
    let expected = root
        .join("wiki")
        .canonicalize()
        .unwrap()
        .join("approved.md");
    assert_eq!(out, expected);
}

#[test]
fn approve_wiki_page_rejects_traversal_after_normalization() {
    let (_g, root) = vault();
    let err = safe_vault_path(
        &root,
        "wiki/../etc/escape.md",
        &["wiki"],
        PathMode::MayCreate,
    )
    .unwrap_err();
    assert!(matches!(err, SafePathError::Traversal), "got {err:?}");
}

#[test]
fn copy_to_vault_rejects_traversal_in_filename() {
    let (_g, root) = vault();
    let err = safe_vault_path(
        &root,
        "documents/../../../tmp/evil.txt",
        &["documents"],
        PathMode::MayCreate,
    )
    .unwrap_err();
    assert!(matches!(err, SafePathError::Traversal), "got {err:?}");
}
