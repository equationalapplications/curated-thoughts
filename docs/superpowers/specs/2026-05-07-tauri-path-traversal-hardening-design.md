# Tauri Path-Traversal Hardening

Date: 2026-05-07
Status: Implemented

## Background

Security review of the Tauri command surface in `src-tauri/src/lib.rs` flagged three high-severity path-traversal vulnerabilities. All three Tauri commands accept a `path: String` from the webview and validate it with `Path::starts_with`, which is component-based and does not resolve `..` segments. One command also has a logic bug (`&&` where `||` was intended) that lets any absolute path ending in `.md` bypass the vault-directory check.

The webview is treated as semi-trusted: it loads locally generated wiki content, including LLM output that may be prompt-injected. Any `#[tauri::command]` reachable from the webview must treat path arguments as untrusted.

### Findings recap

| # | Command | Location | Bug |
|---|---------|----------|-----|
| 1 | `save_wiki_page` | `lib.rs:608` | `&&` instead of `||`; absolute path written verbatim |
| 2 | `read_document` | `lib.rs:418` | `starts_with` doesn't resolve `..`; arbitrary file read |
| 3 | `delete_vault_file` | `lib.rs:628` | `starts_with` doesn't resolve `..`; arbitrary file delete |

## Goals

- Eliminate the path-traversal vulnerability class on the Tauri command surface.
- Provide a single, well-tested helper that all FS-touching commands use.
- Add regression tests that fail against the current code and pass after the fix.

## Non-goals

- Hardening the `wiki_exec` / `wiki_run` SQL surface (separate spec).
- Webview CSP and Tauri IPC allowlist tightening.
- Symlink policy beyond a canonical-prefix containment check.

## Design

### Shared helper

New module: `src-tauri/src/vault/safe_path.rs`.

```rust
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
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

pub enum PathMode {
    /// The target file must already exist; canonicalize the full path.
    MustExist,
    /// The target file may not yet exist; canonicalize the parent directory
    /// and require the final component to be a single plain filename.
    MayCreate,
}

/// Resolve `user_path` against `vault_root`, restricting the result to one of
/// the `allowed_subdirs` (relative to `vault_root`).
///
/// Always returns a canonical, absolute `PathBuf` that is provably contained
/// within one of the allowed subdirectories.
pub fn safe_vault_path(
    vault_root: &Path,
    user_path: &str,
    allowed_subdirs: &[&str],
    mode: PathMode,
) -> Result<PathBuf, SafePathError>;
```

Algorithm:

1. Reject `user_path` if it contains a NUL byte.
2. Treat `user_path` as a `Path`. Reject if `is_absolute()`.
3. Reject if any component is `Component::ParentDir` (`..`).
4. Canonicalize `vault_root` once; if it fails, return `NotFound`.
5. Compute `joined = vault_root_canonical.join(user_path)`.
6. Branch on `mode`:
   - `MustExist`: `canonicalize(joined)`. Require the canonical result to start with `vault_root_canonical.join(subdir)` (also canonicalized) for at least one `subdir` in `allowed_subdirs`. This rejects symlinks that escape the allowed subdirectory.
   - `MayCreate`:
     - The final component must be a single `Component::Normal` containing no path separator and not equal to `.` or `..`.
     - Canonicalize the parent directory; require it to start with one of the canonicalized `allowed_subdirs` paths.
     - Return `parent_canonical.join(filename)` (uncanonicalized, since the file does not yet exist).
7. If no allowed subdirectory matches, return `Outside`.

Notes:

- All canonical-prefix comparisons use the canonical form of the allowed subdirectory, computed at call time (cheap; small set).
- The helper does not create directories. Callers that need `mkdir -p` (e.g. `save_wiki_page` ensuring `wiki/` exists) call `fs::create_dir_all` for the *allowed subdirectory* before invoking the helper, not for arbitrary user-supplied paths.

### Migration

Replace ad-hoc validation in the following commands. Each row lists the allowed subdirectories (relative to vault root) and the mode.

| Command | File | Allowed subdirs | Mode |
|---------|------|-----------------|------|
| `read_document` | `lib.rs:410` | `documents`, `wiki` | `MustExist` |
| `save_wiki_page` | `lib.rs:592` | `wiki` | `MayCreate` |
| `delete_vault_file` | `lib.rs:619` | `documents` | `MustExist` |
| `approve_wiki_page` | `lib.rs:465` | `wiki` | `MayCreate` |
| `get_proposed_content` | `lib.rs:565` | `.brain/proposed` | `MustExist` |
| `copy_to_vault` | `lib.rs:635` | `documents` | `MayCreate` (dest filename only) |

For `approve_wiki_page` and `get_proposed_content`, the path comes from the `wiki_pages.path` column. Even though the row was inserted by trusted backend code, the helper is still applied: defense in depth, and it removes the implicit trust assumption from the read site.

For `copy_to_vault`, the destination is computed from `src.file_name()`. Validate the resulting filename through the helper in `MayCreate` mode under `documents/`.

### Audit pass

Grep for every `#[tauri::command]` whose signature includes a `String` that is later used as a path. For each one, route through `safe_vault_path` or document in code why the value is trusted. Acceptable trusted sources:

- `vault_path` returned from the Tauri file picker dialog (still canonicalize before use).
- Paths derived purely from constants and the canonicalized `vault_root`.

Anything else routes through the helper.

### Error mapping

The helper returns `SafePathError`. Tauri commands map it to the existing `Result<_, String>` shape via `err.to_string()`. The variants intentionally avoid leaking the absolute path in the message; the user-facing string is the variant's `Display`.

## Tests

### Unit tests in `src-tauri/src/vault/safe_path.rs`

Each test runs against a temp directory created by `tempfile::TempDir`, with `documents/`, `wiki/`, and `.brain/proposed/` pre-created.

- `documents/foo.md` exists → returns canonical path under `documents/`.
- `documents/../../etc/passwd` → `Traversal`.
- `/etc/passwd` → `Absolute`.
- `/tmp/anything.md` (regression for Vuln 1; absolute path ending in `.md`) → `Absolute`.
- `other/foo.md` → `Outside`.
- Symlink `documents/evil → /tmp/escape`, request `documents/evil/x` in `MustExist` → `Outside`.
- `foo\0.md` → `InvalidName`.
- `MayCreate` with parent dir that does not exist → `NotFound`.
- `MayCreate` with filename containing `/` → `InvalidName`.
- `MayCreate` with filename `..` or `.` → `InvalidName`.

### Integration tests in `src-tauri/tests/path_traversal.rs`

Each migrated command is exercised twice: once with a benign path (must succeed) and once with the corresponding malicious payload (must return `Err`). The malicious payloads include:

- `read_document`: `<vault>/documents/../../../etc/passwd`
- `save_wiki_page`: `/tmp/pwn.md` (Vuln 1 regression payload)
- `delete_vault_file`: `<vault>/documents/../../target/file`

Tests call the command function directly with constructed `State<_>` values; they do not require booting the full Tauri runtime.

## Rollout

Single PR:

1. Add `safe_path` module with helper, errors, and unit tests.
2. Migrate the six commands listed above.
3. Add the integration tests.
4. Run `cargo test --all` and `cargo clippy --all-targets -- -D warnings`.

No data migration, no config changes, no API surface change visible to the frontend (the `Result<_, String>` shapes are unchanged; only error messages differ).

## Threat model note

The webview is treated as semi-trusted. It loads local content including wiki pages generated from LLM output, which can be prompt-injected by ingested documents. Any `#[tauri::command]` reachable from the webview must validate path arguments as if they came from a remote attacker. This spec establishes the helper that enforces that invariant; future commands that touch the filesystem MUST go through it.
