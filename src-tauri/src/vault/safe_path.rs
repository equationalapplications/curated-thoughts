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
    _vault_root: &Path,
    _user_path: &str,
    _allowed_subdirs: &[&str],
    _mode: PathMode,
) -> Result<PathBuf, SafePathError> {
    unimplemented!("Task 3 implements algorithm")
}
