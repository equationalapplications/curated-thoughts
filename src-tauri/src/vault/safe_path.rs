//! Canonicalize-and-contain helper for path arguments coming from the webview.
//!
//! All `#[tauri::command]` functions that accept a `String` later used as a
//! filesystem path MUST validate it through `safe_vault_path`. The webview is
//! semi-trusted (it loads LLM-generated wiki content that may be prompt-injected),
//! so path arguments are treated as if they came from a remote attacker.

use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SafePathError {
    #[error("absolute paths not allowed")]
    Absolute,
    #[error("path contains traversal segment")]
    Traversal,
    #[error("path resolves outside allowed subdirectory")]
    Outside,
    #[error("path component contains invalid characters")]
    InvalidName,
    #[error("path or parent directory not found: {0}")]
    NotFound(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Copy)]
pub enum PathMode {
    /// The target file must already exist; canonicalize the full path.
    MustExist,
    /// The target file may not yet exist; canonicalize the parent directory
    /// and require the final component to be a single plain filename.
    MayCreate,
}

pub fn safe_vault_path(
    vault_root: &Path,
    user_path: &str,
    allowed_subdirs: &[&str],
    mode: PathMode,
) -> Result<PathBuf, SafePathError> {
    if user_path.as_bytes().contains(&0) {
        return Err(SafePathError::InvalidName);
    }
    let candidate = Path::new(user_path);
    if candidate.is_absolute() {
        return Err(SafePathError::Absolute);
    }
    use std::path::Component;
    // Reject paths with .. or drive-prefix components (Windows C:foo attack).
    if candidate
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::Prefix(_)))
    {
        return Err(SafePathError::Traversal);
    }

    let root_canonical = vault_root
        .canonicalize()
        .map_err(|_| SafePathError::NotFound("vault root not found".to_string()))?;

    let allowed_canonical: Vec<PathBuf> = allowed_subdirs
        .iter()
        .filter_map(|sub| root_canonical.join(sub).canonicalize().ok())
        .filter(|canonical_sub| canonical_sub.starts_with(&root_canonical))
        .collect();
    if allowed_canonical.is_empty() {
        return Err(SafePathError::Outside);
    }

    match mode {
        PathMode::MustExist => {
            let joined = root_canonical.join(candidate);
            let canonical = joined
                .canonicalize()
                .map_err(|_| SafePathError::NotFound(format!("file not found: {}", user_path)))?;
            if allowed_canonical.iter().any(|sub| canonical.starts_with(sub)) {
                Ok(canonical)
            } else {
                Err(SafePathError::Outside)
            }
        }
        PathMode::MayCreate => {
            // Reject paths that end with . or .. (directory references, not filenames)
            if user_path.ends_with("/.")
                || user_path.ends_with("/..")
                || user_path == "."
                || user_path == ".."
            {
                return Err(SafePathError::InvalidName);
            }

            let filename = candidate
                .file_name()
                .ok_or(SafePathError::InvalidName)?
                .to_str()
                .ok_or(SafePathError::InvalidName)?;
            if filename.is_empty()
                || filename == "."
                || filename == ".."
                || filename.contains('/')
                || filename.contains('\\')
            {
                return Err(SafePathError::InvalidName);
            }
            let parent = candidate.parent().unwrap_or_else(|| Path::new(""));
            let joined_parent = root_canonical.join(parent);
            let canonical_parent = joined_parent
                .canonicalize()
                .map_err(|_| SafePathError::NotFound(format!("parent directory not found: {}", user_path)))?;
            if !allowed_canonical
                .iter()
                .any(|sub| canonical_parent.starts_with(sub))
            {
                return Err(SafePathError::Outside);
            }
            let target_path = canonical_parent.join(filename);

            // Reject if target already exists as a symlink (or if following it escapes).
            if let Ok(metadata) = target_path.symlink_metadata() {
                if metadata.is_symlink() {
                    return Err(SafePathError::InvalidName);
                }
                // Target exists and is not a symlink — verify final canonical containment.
                let target_canonical = target_path
                    .canonicalize()
                    .map_err(|e| SafePathError::Io(e))?;
                if !allowed_canonical
                    .iter()
                    .any(|sub| target_canonical.starts_with(sub))
                {
                    return Err(SafePathError::Outside);
                }
                Ok(target_canonical)
            } else {
                // Target doesn't exist yet — safe to create.
                Ok(target_path)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Builds a vault layout with `documents/`, `wiki/`, `.brain/proposed/`.
    fn vault() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        fs::create_dir_all(root.join("documents")).unwrap();
        fs::create_dir_all(root.join("wiki")).unwrap();
        fs::create_dir_all(root.join(".brain").join("proposed")).unwrap();
        (dir, root)
    }

    fn allowed() -> &'static [&'static str] {
        &["documents", "wiki", ".brain/proposed"]
    }

    #[test]
    fn rejects_absolute_path() {
        let (_g, root) = vault();
        let err = safe_vault_path(&root, "/etc/passwd", allowed(), PathMode::MustExist).unwrap_err();
        assert!(matches!(err, SafePathError::Absolute), "got {err:?}");
    }

    #[test]
    fn rejects_parent_dir_component() {
        let (_g, root) = vault();
        let err = safe_vault_path(&root, "documents/../../etc/passwd", allowed(), PathMode::MustExist).unwrap_err();
        assert!(matches!(err, SafePathError::Traversal), "got {err:?}");
    }

    #[test]
    fn rejects_nul_byte_in_filename() {
        let (_g, root) = vault();
        let err = safe_vault_path(&root, "documents/foo\0.md", allowed(), PathMode::MustExist).unwrap_err();
        assert!(matches!(err, SafePathError::InvalidName), "got {err:?}");
    }

    #[test]
    #[cfg(windows)]
    fn rejects_windows_drive_prefix() {
        let (_g, root) = vault();
        let err = safe_vault_path(&root, "C:foo.md", allowed(), PathMode::MustExist).unwrap_err();
        assert!(matches!(err, SafePathError::Traversal), "got {err:?}");
    }

    #[test]
    fn must_exist_returns_canonical_path() {
        let (_g, root) = vault();
        let target = root.join("documents").join("foo.md");
        fs::write(&target, b"hi").unwrap();
        let out = safe_vault_path(&root, "documents/foo.md", allowed(), PathMode::MustExist).unwrap();
        assert_eq!(out, target.canonicalize().unwrap());
    }

    #[test]
    #[cfg(unix)]
    fn must_exist_rejects_symlink_escape() {
        use std::os::unix::fs::symlink;
        let (_g, root) = vault();
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("secret"), b"x").unwrap();
        symlink(outside.path(), root.join("documents").join("evil")).unwrap();
        let err = safe_vault_path(
            &root,
            "documents/evil/secret",
            allowed(),
            PathMode::MustExist,
        )
        .unwrap_err();
        assert!(matches!(err, SafePathError::Outside), "got {err:?}");
    }

    #[test]
    fn may_create_returns_parent_canonical_join_filename() {
        let (_g, root) = vault();
        let out = safe_vault_path(&root, "wiki/new-page.md", allowed(), PathMode::MayCreate).unwrap();
        let expected = root.join("wiki").canonicalize().unwrap().join("new-page.md");
        assert_eq!(out, expected);
    }

    #[test]
    fn may_create_rejects_dot_filename() {
        let (_g, root) = vault();
        let err = safe_vault_path(&root, "wiki/.", allowed(), PathMode::MayCreate).unwrap_err();
        assert!(matches!(err, SafePathError::InvalidName), "got {err:?}");
    }

    #[test]
    fn may_create_rejects_missing_parent() {
        let (_g, root) = vault();
        let err = safe_vault_path(&root, "wiki/never/x.md", allowed(), PathMode::MayCreate).unwrap_err();
        assert!(matches!(err, SafePathError::NotFound(_)), "got {err:?}");
    }

    #[test]
    fn rejects_absolute_md_path_vuln1_regression() {
        let (_g, root) = vault();
        let err = safe_vault_path(&root, "/tmp/pwn.md", &["wiki"], PathMode::MayCreate).unwrap_err();
        assert!(matches!(err, SafePathError::Absolute), "got {err:?}");
    }

    #[test]
    fn rejects_path_outside_allowed_subdirs() {
        let (_g, root) = vault();
        fs::create_dir_all(root.join("other")).unwrap();
        fs::write(root.join("other").join("foo.md"), b"x").unwrap();
        let err = safe_vault_path(&root, "other/foo.md", allowed(), PathMode::MustExist).unwrap_err();
        assert!(matches!(err, SafePathError::Outside), "got {err:?}");
    }

    #[test]
    #[cfg(unix)]
    fn rejects_allowed_subdir_that_is_symlink_escape() {
        use std::os::unix::fs::symlink;
        let (_g, root) = vault();
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("target.md"), b"pwned").unwrap();
        // Replace the "documents" subdir with a symlink pointing outside the vault.
        fs::remove_dir(root.join("documents")).unwrap();
        symlink(outside.path(), root.join("documents")).unwrap();
        // Now "documents" canonicalizes to outside the vault, so it should be filtered out.
        let err = safe_vault_path(&root, "documents/target.md", &["documents"], PathMode::MustExist).unwrap_err();
        assert!(matches!(err, SafePathError::Outside), "got {err:?}");
    }

    #[test]
    #[cfg(unix)]
    fn may_create_rejects_symlink_escape() {
        use std::os::unix::fs::symlink;
        let (_g, root) = vault();
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("target.md"), b"pwned").unwrap();
        // Create symlink in wiki/ pointing outside vault
        symlink(outside.path().join("target.md"), root.join("wiki").join("evil.md")).unwrap();
        // Attempt to write to evil.md should be rejected (symlink escape)
        let err = safe_vault_path(&root, "wiki/evil.md", &["wiki"], PathMode::MayCreate).unwrap_err();
        assert!(matches!(err, SafePathError::InvalidName), "got {err:?}");
    }
}
