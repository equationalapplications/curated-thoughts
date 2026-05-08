//! Canonicalize-and-contain helper for path arguments coming from the webview.
//!
//! All `#[tauri::command]` functions that accept a `String` later used as a
//! filesystem path MUST validate it through `safe_vault_path`. The webview is
//! semi-trusted (it loads LLM-generated wiki content that may be prompt-injected),
//! so path arguments are treated as if they came from a remote attacker.

use std::io::Write;
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
    #[error("path exists but is not a regular file")]
    NotARegularFile,
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

fn tmp_sibling_path(target: &Path) -> Result<PathBuf, SafePathError> {
    let parent = target
        .parent()
        .ok_or_else(|| SafePathError::NotFound("parent directory not found".to_string()))?;
    let file_name = target
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or(SafePathError::InvalidName)?;

    let pid = std::process::id();
    for attempt in 0..64u32 {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let candidate = parent.join(format!(".{file_name}.tmp-{pid}-{nanos}-{attempt}"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }

    Err(SafePathError::Io(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "failed to allocate unique temp path",
    )))
}

fn rename_replace(temp: &Path, target: &Path) -> Result<(), SafePathError> {
    match std::fs::rename(temp, target) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            // Windows rename does not replace existing targets. Remove and retry.
            std::fs::remove_file(target)?;
            std::fs::rename(temp, target).map_err(SafePathError::Io)
        }
        Err(e) => Err(SafePathError::Io(e)),
    }
}

pub fn safe_write_bytes(target: &Path, bytes: &[u8]) -> Result<(), SafePathError> {
    let temp = tmp_sibling_path(target)?;

    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
    {
        Ok(f) => f,
        Err(e) => return Err(SafePathError::Io(e)),
    };

    if let Err(e) = file.write_all(bytes) {
        let _ = std::fs::remove_file(&temp);
        return Err(SafePathError::Io(e));
    }

    if let Err(e) = file.sync_all() {
        let _ = std::fs::remove_file(&temp);
        return Err(SafePathError::Io(e));
    }

    drop(file);

    if let Err(e) = rename_replace(&temp, target) {
        let _ = std::fs::remove_file(&temp);
        return Err(e);
    }

    Ok(())
}

pub fn safe_copy_file(src: &Path, target: &Path) -> Result<u64, SafePathError> {
    let temp = tmp_sibling_path(target)?;

    let mut input = std::fs::File::open(src)?;
    let mut output = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
    {
        Ok(f) => f,
        Err(e) => return Err(SafePathError::Io(e)),
    };

    let copied = match std::io::copy(&mut input, &mut output) {
        Ok(n) => n,
        Err(e) => {
            let _ = std::fs::remove_file(&temp);
            return Err(SafePathError::Io(e));
        }
    };

    if let Err(e) = output.sync_all() {
        let _ = std::fs::remove_file(&temp);
        return Err(SafePathError::Io(e));
    }

    drop(output);

    if let Err(e) = rename_replace(&temp, target) {
        let _ = std::fs::remove_file(&temp);
        return Err(e);
    }

    Ok(copied)
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
            if allowed_canonical
                .iter()
                .any(|sub| canonical.starts_with(sub))
            {
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
            let canonical_parent = joined_parent.canonicalize().map_err(|_| {
                SafePathError::NotFound(format!("parent directory not found: {}", user_path))
            })?;
            if !allowed_canonical
                .iter()
                .any(|sub| canonical_parent.starts_with(sub))
            {
                return Err(SafePathError::Outside);
            }
            let target_path = canonical_parent.join(filename);

            // Reject if target already exists as a symlink (or if following it escapes).
            if let Ok(metadata) = target_path.symlink_metadata() {
                if metadata.file_type().is_symlink() {
                    return Err(SafePathError::InvalidName);
                }
                if metadata.is_dir() || !metadata.is_file() {
                    return Err(SafePathError::NotARegularFile);
                }
                // Target exists and is a regular file — verify final canonical containment.
                let target_canonical = target_path.canonicalize().map_err(SafePathError::Io)?;
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
        let err =
            safe_vault_path(&root, "/etc/passwd", allowed(), PathMode::MustExist).unwrap_err();
        assert!(matches!(err, SafePathError::Absolute), "got {err:?}");
    }

    #[test]
    fn rejects_parent_dir_component() {
        let (_g, root) = vault();
        let err = safe_vault_path(
            &root,
            "documents/../../etc/passwd",
            allowed(),
            PathMode::MustExist,
        )
        .unwrap_err();
        assert!(matches!(err, SafePathError::Traversal), "got {err:?}");
    }

    #[test]
    fn rejects_nul_byte_in_filename() {
        let (_g, root) = vault();
        let err = safe_vault_path(&root, "documents/foo\0.md", allowed(), PathMode::MustExist)
            .unwrap_err();
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
        let out =
            safe_vault_path(&root, "documents/foo.md", allowed(), PathMode::MustExist).unwrap();
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
        let out =
            safe_vault_path(&root, "wiki/new-page.md", allowed(), PathMode::MayCreate).unwrap();
        let expected = root
            .join("wiki")
            .canonicalize()
            .unwrap()
            .join("new-page.md");
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
        let err =
            safe_vault_path(&root, "wiki/never/x.md", allowed(), PathMode::MayCreate).unwrap_err();
        assert!(matches!(err, SafePathError::NotFound(_)), "got {err:?}");
    }

    #[test]
    fn may_create_rejects_existing_directory() {
        let (_g, root) = vault();
        fs::create_dir_all(root.join("wiki").join("nested-dir")).unwrap();
        let err =
            safe_vault_path(&root, "wiki/nested-dir", allowed(), PathMode::MayCreate).unwrap_err();
        assert!(matches!(err, SafePathError::NotARegularFile), "got {err:?}");
    }

    #[test]
    fn rejects_absolute_md_path_vuln1_regression() {
        let (_g, root) = vault();
        let err =
            safe_vault_path(&root, "/tmp/pwn.md", &["wiki"], PathMode::MayCreate).unwrap_err();
        assert!(matches!(err, SafePathError::Absolute), "got {err:?}");
    }

    #[test]
    fn rejects_path_outside_allowed_subdirs() {
        let (_g, root) = vault();
        fs::create_dir_all(root.join("other")).unwrap();
        fs::write(root.join("other").join("foo.md"), b"x").unwrap();
        let err =
            safe_vault_path(&root, "other/foo.md", allowed(), PathMode::MustExist).unwrap_err();
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
        let err = safe_vault_path(
            &root,
            "documents/target.md",
            &["documents"],
            PathMode::MustExist,
        )
        .unwrap_err();
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
        symlink(
            outside.path().join("target.md"),
            root.join("wiki").join("evil.md"),
        )
        .unwrap();
        // Attempt to write to evil.md should be rejected (symlink escape)
        let err =
            safe_vault_path(&root, "wiki/evil.md", &["wiki"], PathMode::MayCreate).unwrap_err();
        assert!(matches!(err, SafePathError::InvalidName), "got {err:?}");
    }

    #[test]
    #[cfg(unix)]
    fn safe_write_bytes_replaces_raced_symlink_without_following() {
        use std::os::unix::fs::symlink;

        let (_g, root) = vault();
        let outside = TempDir::new().unwrap();
        let outside_target = outside.path().join("outside.md");
        fs::write(&outside_target, b"outside").unwrap();

        let target =
            safe_vault_path(&root, "wiki/race.md", &["wiki"], PathMode::MayCreate).unwrap();

        // Simulate attacker creating a symlink after path validation but before write.
        symlink(&outside_target, &target).unwrap();

        safe_write_bytes(&target, b"inside").unwrap();

        let written = fs::read_to_string(&target).unwrap();
        let outside_read = fs::read_to_string(&outside_target).unwrap();
        assert_eq!(written, "inside");
        assert_eq!(outside_read, "outside");
        assert!(!target.symlink_metadata().unwrap().is_symlink());
    }

    #[test]
    #[cfg(unix)]
    fn safe_copy_file_replaces_raced_symlink_without_following() {
        use std::os::unix::fs::symlink;

        let (_g, root) = vault();
        let outside = TempDir::new().unwrap();
        let outside_target = outside.path().join("outside.md");
        let src = root.join("documents").join("src.md");

        fs::write(&outside_target, b"outside").unwrap();
        fs::write(&src, b"inside").unwrap();

        let target = safe_vault_path(
            &root,
            "documents/race.md",
            &["documents"],
            PathMode::MayCreate,
        )
        .unwrap();

        // Simulate attacker creating a symlink after path validation but before copy.
        symlink(&outside_target, &target).unwrap();

        safe_copy_file(&src, &target).unwrap();

        let copied = fs::read_to_string(&target).unwrap();
        let outside_read = fs::read_to_string(&outside_target).unwrap();
        assert_eq!(copied, "inside");
        assert_eq!(outside_read, "outside");
        assert!(!target.symlink_metadata().unwrap().is_symlink());
    }
}
