# Tauri Path-Traversal Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate path-traversal vulnerability class on the Tauri command surface by routing all six FS-touching commands through one canonicalize-and-contain helper with regression tests.

**Architecture:** New module `src-tauri/src/vault/safe_path.rs` exposes `safe_vault_path(vault_root, user_path, allowed_subdirs, mode) -> Result<PathBuf, SafePathError>`. Algorithm rejects absolute paths, `..` components, NUL bytes; canonicalizes vault root + (target or parent depending on `PathMode`); requires canonical result starts with one canonicalized allowed subdir. `MustExist` canonicalizes the full target (catches symlink escape). `MayCreate` canonicalizes parent and requires final component be a single plain filename. Six commands in `lib.rs` migrate to this helper. Regression tests at unit (`safe_path` module) and integration (`tests/path_traversal.rs`) levels.

**Tech Stack:** Rust, `std::path`, `thiserror` for typed errors, `tempfile` (already present, dev-dep) for tests, `rusqlite::Connection` test seam reused from existing `tests/helpers/`.

**Spec:** `docs/superpowers/specs/2026-05-07-tauri-path-traversal-hardening-design.md`

---

## File Map

| Action | Path | Responsibility |
|--------|------|----------------|
| Create | `src-tauri/src/vault/safe_path.rs` | Helper, `SafePathError`, `PathMode`, unit tests |
| Modify | `src-tauri/src/vault/mod.rs` | Add `pub mod safe_path;` and re-export `SafePathError`, `PathMode`, `safe_vault_path` |
| Modify | `src-tauri/Cargo.toml` | Add `thiserror = "1"` to `[dependencies]` |
| Modify | `src-tauri/src/lib.rs` | Migrate `read_document` (l.410), `approve_wiki_page` (l.465), `get_proposed_content` (l.565), `save_wiki_page` (l.592), `delete_vault_file` (l.619), `copy_to_vault` (l.635) |
| Create | `src-tauri/tests/path_traversal.rs` | Integration regression tests for all six commands |

Each command is migrated in its own task to keep diffs reviewable and enable bisect.

---

## Task 1: Add `thiserror` dependency

**Files:**
- Modify: `src-tauri/Cargo.toml`

- [ ] **Step 1: Add to `[dependencies]`**

In `src-tauri/Cargo.toml`, under the existing `[dependencies]` table (after `anyhow = "1"`), add:

```toml
thiserror = "1"
```

- [ ] **Step 2: Verify build still works**

Run from repo root:
```bash
cd src-tauri && cargo build -p curated-thoughts 2>&1 | tail -5
```
Expected: `Finished` line, no errors.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/Cargo.toml src-tauri/Cargo.lock
git commit -m "build(tauri): add thiserror dep for safe_path errors"
```

---

## Task 2: `safe_path` module — types and skeleton

**Files:**
- Create: `src-tauri/src/vault/safe_path.rs`
- Modify: `src-tauri/src/vault/mod.rs`

- [ ] **Step 1: Create module file with types only**

Create `src-tauri/src/vault/safe_path.rs`:

```rust
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
```

- [ ] **Step 2: Wire module + re-exports**

Edit `src-tauri/src/vault/mod.rs` to read:

```rust
pub mod config;
pub mod safe_path;
pub use config::VaultConfig;
pub use safe_path::{safe_vault_path, PathMode, SafePathError};
```

- [ ] **Step 3: Verify it compiles**

```bash
cd src-tauri && cargo build -p curated-thoughts 2>&1 | tail -5
```
Expected: `Finished` (the `unimplemented!` body compiles fine).

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/vault/safe_path.rs src-tauri/src/vault/mod.rs
git commit -m "feat(safe_path): module skeleton with SafePathError and PathMode"
```

---

## Task 3: Implement `safe_vault_path` (TDD)

**Files:**
- Modify: `src-tauri/src/vault/safe_path.rs`

Each substep adds one failing test, then makes it pass. Commit after each green test.

### Step 3.1: Test scaffolding

- [ ] **Add `#[cfg(test)] mod tests` block at the bottom of `safe_path.rs`:**

```rust
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
}
```

- [ ] **Run the (empty) test module to verify it builds:**

```bash
cd src-tauri && cargo test -p curated-thoughts vault::safe_path::tests -- --list 2>&1 | tail -5
```
Expected: `0 tests`, no compile errors.

### Step 3.2: Reject absolute paths

- [ ] **Add failing test inside `mod tests`:**

```rust
    #[test]
    fn rejects_absolute_path() {
        let (_g, root) = vault();
        let err = safe_vault_path(&root, "/etc/passwd", allowed(), PathMode::MustExist).unwrap_err();
        assert!(matches!(err, SafePathError::Absolute), "got {err:?}");
    }
```

- [ ] **Run — expect FAIL** (helper still `unimplemented!`):

```bash
cd src-tauri && cargo test -p curated-thoughts safe_path::tests::rejects_absolute_path 2>&1 | tail -10
```
Expected: panic from `unimplemented!`.

- [ ] **Implement the absolute-path branch.** Replace the `safe_vault_path` body with:

```rust
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
    let _ = (vault_root, allowed_subdirs, mode);
    unimplemented!("Task 3.3+")
}
```

- [ ] **Run — expect PASS:**

```bash
cd src-tauri && cargo test -p curated-thoughts safe_path::tests::rejects_absolute_path 2>&1 | tail -5
```
Expected: `test result: ok. 1 passed`.

### Step 3.3: Reject `..` components

- [ ] **Add test:**

```rust
    #[test]
    fn rejects_parent_dir_component() {
        let (_g, root) = vault();
        let err = safe_vault_path(&root, "documents/../../etc/passwd", allowed(), PathMode::MustExist).unwrap_err();
        assert!(matches!(err, SafePathError::Traversal), "got {err:?}");
    }
```

- [ ] **Run — expect FAIL** (still `unimplemented!`).

- [ ] **Add traversal check.** Insert after the `is_absolute()` check, before the `unimplemented!`:

```rust
    use std::path::Component;
    if candidate
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        return Err(SafePathError::Traversal);
    }
```

- [ ] **Run — expect PASS** for both prior tests.

```bash
cd src-tauri && cargo test -p curated-thoughts vault::safe_path::tests 2>&1 | tail -5
```

### Step 3.4: NUL byte rejection

- [ ] **Add test:**

```rust
    #[test]
    fn rejects_nul_byte_in_filename() {
        let (_g, root) = vault();
        let err = safe_vault_path(&root, "documents/foo\0.md", allowed(), PathMode::MustExist).unwrap_err();
        assert!(matches!(err, SafePathError::InvalidName), "got {err:?}");
    }
```

- [ ] **Run — expect PASS** (already covered by NUL check at top of function).

### Step 3.5: `MustExist` happy path

- [ ] **Add test:**

```rust
    #[test]
    fn must_exist_returns_canonical_path() {
        let (_g, root) = vault();
        let target = root.join("documents").join("foo.md");
        fs::write(&target, b"hi").unwrap();
        let out = safe_vault_path(&root, "documents/foo.md", allowed(), PathMode::MustExist).unwrap();
        assert_eq!(out, target.canonicalize().unwrap());
    }
```

- [ ] **Run — expect FAIL** (still `unimplemented!`).

- [ ] **Implement `MustExist` branch.** Replace the trailing `unimplemented!` with:

```rust
    let root_canonical = vault_root
        .canonicalize()
        .map_err(|e| SafePathError::NotFound(format!("{}: {}", vault_root.display(), e)))?;

    let allowed_canonical: Vec<PathBuf> = allowed_subdirs
        .iter()
        .filter_map(|sub| root_canonical.join(sub).canonicalize().ok())
        .collect();
    if allowed_canonical.is_empty() {
        return Err(SafePathError::Outside);
    }

    match mode {
        PathMode::MustExist => {
            let joined = root_canonical.join(candidate);
            let canonical = joined
                .canonicalize()
                .map_err(|e| SafePathError::NotFound(format!("{}: {}", joined.display(), e)))?;
            if allowed_canonical.iter().any(|sub| canonical.starts_with(sub)) {
                Ok(canonical)
            } else {
                Err(SafePathError::Outside)
            }
        }
        PathMode::MayCreate => unimplemented!("Step 3.7"),
    }
```

- [ ] **Run — expect PASS for all `MustExist` tests:**

```bash
cd src-tauri && cargo test -p curated-thoughts vault::safe_path::tests 2>&1 | tail -10
```

### Step 3.6: `MustExist` rejects symlink escape

- [ ] **Add test (Unix-only — symlink permissions vary on Windows):**

```rust
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
```

- [ ] **Run — expect PASS** (canonicalization resolves the symlink and the `starts_with` check fails).

### Step 3.7: `MayCreate` happy path + filename-only enforcement

- [ ] **Add tests:**

```rust
    #[test]
    fn may_create_returns_parent_canonical_join_filename() {
        let (_g, root) = vault();
        let out = safe_vault_path(&root, "wiki/new-page.md", allowed(), PathMode::MayCreate).unwrap();
        let expected = root.join("wiki").canonicalize().unwrap().join("new-page.md");
        assert_eq!(out, expected);
    }

    #[test]
    fn may_create_rejects_filename_with_separator() {
        let (_g, root) = vault();
        let err = safe_vault_path(&root, "wiki/sub/x.md", allowed(), PathMode::MayCreate).unwrap_err();
        // sub/ doesn't exist, so parent canonicalize fails → NotFound is also acceptable;
        // the contract is that nested non-existent dirs are not auto-created.
        assert!(
            matches!(err, SafePathError::NotFound(_) | SafePathError::Outside),
            "got {err:?}"
        );
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
```

- [ ] **Run — expect FAIL on the happy path** (still `unimplemented!`).

- [ ] **Implement `MayCreate` branch.** Replace the `MayCreate => unimplemented!(...)` arm with:

```rust
        PathMode::MayCreate => {
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
                .map_err(|e| SafePathError::NotFound(format!("{}: {}", joined_parent.display(), e)))?;
            if !allowed_canonical
                .iter()
                .any(|sub| canonical_parent.starts_with(sub))
            {
                return Err(SafePathError::Outside);
            }
            Ok(canonical_parent.join(filename))
        }
```

- [ ] **Run — expect ALL `safe_path` tests PASS:**

```bash
cd src-tauri && cargo test -p curated-thoughts vault::safe_path::tests 2>&1 | tail -10
```
Expected: `test result: ok. <N> passed`.

### Step 3.8: Vuln 1 regression test

- [ ] **Add test that locks down the `/tmp/anything.md` payload from finding #1:**

```rust
    #[test]
    fn rejects_absolute_md_path_vuln1_regression() {
        let (_g, root) = vault();
        let err = safe_vault_path(&root, "/tmp/pwn.md", &["wiki"], PathMode::MayCreate).unwrap_err();
        assert!(matches!(err, SafePathError::Absolute), "got {err:?}");
    }
```

- [ ] **Run — expect PASS:**

```bash
cd src-tauri && cargo test -p curated-thoughts vault::safe_path::tests 2>&1 | tail -10
```

### Step 3.9: Outside subdir

- [ ] **Add test:**

```rust
    #[test]
    fn rejects_path_outside_allowed_subdirs() {
        let (_g, root) = vault();
        fs::create_dir_all(root.join("other")).unwrap();
        fs::write(root.join("other").join("foo.md"), b"x").unwrap();
        let err = safe_vault_path(&root, "other/foo.md", allowed(), PathMode::MustExist).unwrap_err();
        assert!(matches!(err, SafePathError::Outside), "got {err:?}");
    }
```

- [ ] **Run — expect PASS** (canonical target is under `<root>/other`, no allowed prefix matches).

### Step 3.10: Final unit-test sweep + commit

- [ ] **Run the full module:**

```bash
cd src-tauri && cargo test -p curated-thoughts vault::safe_path 2>&1 | tail -10
```
Expected: all green.

- [ ] **Lint:**

```bash
cd src-tauri && cargo clippy -p curated-thoughts --all-targets -- -D warnings 2>&1 | tail -10
```
Expected: no warnings.

- [ ] **Commit:**

```bash
git add src-tauri/src/vault/safe_path.rs
git commit -m "feat(safe_path): canonicalize-and-contain path helper with unit tests"
```

---

## Task 4: Migrate `read_document`

**Files:**
- Modify: `src-tauri/src/lib.rs` (function at line 410)

- [ ] **Step 1: Replace function body.** Locate `fn read_document` (currently lines 410–428) and replace the whole function with:

```rust
#[tauri::command]
fn read_document(path: String, state: State<VaultConfigState>) -> Result<String, String> {
    let root = match state.0.lock().unwrap().get_vault_path().map_err(|e| e.to_string())? {
        Some(p) => std::path::PathBuf::from(p),
        None => return Err("no vault path set".to_string()),
    };

    let safe = crate::vault::safe_vault_path(
        &root,
        &path,
        &["documents", "wiki"],
        crate::vault::PathMode::MustExist,
    )
    .map_err(|e| e.to_string())?;

    std::fs::read_to_string(&safe).map_err(|e| e.to_string())
}
```

- [ ] **Step 2: Build:**

```bash
cd src-tauri && cargo build -p curated-thoughts 2>&1 | tail -5
```
Expected: `Finished`.

- [ ] **Step 3: Run any existing tests touching `read_document`:**

```bash
cd src-tauri && cargo test -p curated-thoughts read_document 2>&1 | tail -10
```
Expected: pass (or no matches — integration tests added in Task 10).

- [ ] **Step 4: Commit:**

```bash
git add src-tauri/src/lib.rs
git commit -m "fix(tauri): route read_document through safe_vault_path"
```

---

## Task 5: Migrate `save_wiki_page` (Vuln 1 fix)

**Files:**
- Modify: `src-tauri/src/lib.rs` (function at line 592)

- [ ] **Step 1: Replace function body.** Locate `fn save_wiki_page` (currently lines 591–614) and replace with:

```rust
#[tauri::command]
fn save_wiki_page(
    path: String,
    content: String,
    vault_state: State<VaultConfigState>,
) -> Result<(), String> {
    let vault = vault_state
        .0
        .lock()
        .unwrap()
        .get_vault_path()
        .map_err(|e| e.to_string())?
        .ok_or("no vault set".to_string())?;
    let vault_root = std::path::PathBuf::from(&vault);
    // Ensure the allowed subdir exists before resolving the user path.
    std::fs::create_dir_all(vault_root.join("wiki")).map_err(|e| e.to_string())?;

    let safe = crate::vault::safe_vault_path(
        &vault_root,
        &path,
        &["wiki"],
        crate::vault::PathMode::MayCreate,
    )
    .map_err(|e| e.to_string())?;

    std::fs::write(&safe, &content).map_err(|e| e.to_string())?;
    Ok(())
}
```

- [ ] **Step 2: Build + commit:**

```bash
cd src-tauri && cargo build -p curated-thoughts 2>&1 | tail -5
git add src-tauri/src/lib.rs
git commit -m "fix(tauri): close save_wiki_page path-traversal (Vuln 1)

- Replaced && bug + starts_with check with safe_vault_path(MayCreate, [\"wiki\"])
- Absolute paths and .. segments now rejected"
```

---

## Task 6: Migrate `delete_vault_file`

**Files:**
- Modify: `src-tauri/src/lib.rs` (function at line 619)

- [ ] **Step 1: Replace function body.**

```rust
#[tauri::command]
fn delete_vault_file(path: String, state: State<VaultConfigState>) -> Result<(), String> {
    let root = state
        .0
        .lock()
        .unwrap()
        .get_vault_path()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "no vault set".to_string())?;
    let vault_root = std::path::PathBuf::from(&root);

    let safe = crate::vault::safe_vault_path(
        &vault_root,
        &path,
        &["documents"],
        crate::vault::PathMode::MustExist,
    )
    .map_err(|e| e.to_string())?;

    std::fs::remove_file(&safe).map_err(|e| e.to_string())
}
```

Note: callers currently pass an absolute path under the vault. The webview must adapt to passing a vault-relative path (e.g. `documents/foo.md`). Adjust any frontend call sites in the same commit. Search:

```bash
grep -rn "delete_vault_file\|deleteVaultFile" src/ ui/ frontend/ 2>/dev/null | head -20
```

If the frontend passes absolute paths today, change those callers to send a path relative to the vault root.

- [ ] **Step 2: Build + commit:**

```bash
cd src-tauri && cargo build -p curated-thoughts 2>&1 | tail -5
git add src-tauri/src/lib.rs
# Add any frontend files that were updated:
git commit -m "fix(tauri): close delete_vault_file path-traversal (Vuln 3)"
```

---

## Task 7: Migrate `approve_wiki_page`

**Files:**
- Modify: `src-tauri/src/lib.rs` (function at line 465)

- [ ] **Step 1: Replace function body.** The `page_path` value comes from `wiki_pages.path` in SQLite; defense in depth still applies (an LLM-driven write could store a malicious path).

```rust
#[tauri::command]
fn approve_wiki_page(
    id: i64,
    content: String,
    vault_path: String,
    db_state: State<DbState>,
) -> Result<(), String> {
    let guard = db_state.0.lock().unwrap();
    let conn = &guard.0;
    let page_path: String = conn
        .query_row("SELECT path FROM wiki_pages WHERE id = ?1", [id], |r| r.get(0))
        .map_err(|e| e.to_string())?;

    let vault_root = std::path::PathBuf::from(&vault_path);
    std::fs::create_dir_all(vault_root.join("wiki")).map_err(|e| e.to_string())?;

    let safe = crate::vault::safe_vault_path(
        &vault_root,
        &format!("wiki/{}", page_path),
        &["wiki"],
        crate::vault::PathMode::MayCreate,
    )
    .map_err(|e| e.to_string())?;

    std::fs::write(&safe, &content).map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE wiki_pages SET status = 'approved', last_synced = unixepoch() WHERE id = ?1",
        [id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
```

- [ ] **Step 2: Build + commit:**

```bash
cd src-tauri && cargo build -p curated-thoughts 2>&1 | tail -5
git add src-tauri/src/lib.rs
git commit -m "fix(tauri): validate approve_wiki_page page_path through safe helper"
```

---

## Task 8: Migrate `get_proposed_content`

**Files:**
- Modify: `src-tauri/src/lib.rs` (function at line 565)

- [ ] **Step 1: Replace function body:**

```rust
#[tauri::command]
fn get_proposed_content(
    page_id: i64,
    db_state: State<DbState>,
    vault_state: State<VaultConfigState>,
) -> Result<String, String> {
    let page_path: String = {
        let guard = db_state.0.lock().unwrap();
        guard.0
            .query_row("SELECT path FROM wiki_pages WHERE id = ?1", [page_id], |r| r.get(0))
            .map_err(|e| e.to_string())?
    };
    let vault = vault_state
        .0
        .lock()
        .unwrap()
        .get_vault_path()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "no vault set".to_string())?;
    let vault_root = std::path::PathBuf::from(&vault);

    let safe = crate::vault::safe_vault_path(
        &vault_root,
        &format!(".brain/proposed/{}", page_path),
        &[".brain/proposed"],
        crate::vault::PathMode::MustExist,
    );

    Ok(match safe {
        Ok(p) => std::fs::read_to_string(&p)
            .unwrap_or_else(|_| format!("# {}\n\n*Proposed wiki page — content not available.*", page_path)),
        Err(_) => format!("# {}\n\n*Proposed wiki page — content not available.*", page_path),
    })
}
```

Note: the existing function silently falls back to a placeholder if the file is missing; this preserves that UX while still rejecting traversal payloads stored in `wiki_pages.path`.

- [ ] **Step 2: Build + commit:**

```bash
cd src-tauri && cargo build -p curated-thoughts 2>&1 | tail -5
git add src-tauri/src/lib.rs
git commit -m "fix(tauri): validate get_proposed_content path through safe helper"
```

---

## Task 9: Migrate `copy_to_vault`

**Files:**
- Modify: `src-tauri/src/lib.rs` (function at line 635)

The destination is built from `src.file_name()`, which already strips directory components — but route through the helper anyway for the `InvalidName` checks (e.g. NUL bytes in source basename).

- [ ] **Step 1: Replace function body:**

```rust
#[tauri::command]
fn copy_to_vault(src_path: String, vault_path: String) -> Result<String, String> {
    let src = std::path::Path::new(&src_path);
    let file_name = src
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| "invalid filename".to_string())?;
    let vault_root = std::path::PathBuf::from(&vault_path);
    std::fs::create_dir_all(vault_root.join("documents")).map_err(|e| e.to_string())?;

    let dest = crate::vault::safe_vault_path(
        &vault_root,
        &format!("documents/{}", file_name),
        &["documents"],
        crate::vault::PathMode::MayCreate,
    )
    .map_err(|e| e.to_string())?;

    std::fs::copy(src, &dest).map_err(|e| e.to_string())?;
    Ok(dest.to_string_lossy().into_owned())
}
```

- [ ] **Step 2: Build + commit:**

```bash
cd src-tauri && cargo build -p curated-thoughts 2>&1 | tail -5
git add src-tauri/src/lib.rs
git commit -m "fix(tauri): validate copy_to_vault destination through safe helper"
```

---

## Task 10: Integration regression tests

**Files:**
- Create: `src-tauri/tests/path_traversal.rs`

These tests call the migrated commands' underlying logic without booting the full Tauri runtime by exercising `safe_vault_path` against the same allowed-subdir lists that each command uses. This locks down the vulnerability payloads from the spec and catches future drift if a maintainer changes a command's allowed subdirs.

- [ ] **Step 1: Create test file:**

```rust
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
    let out = safe_vault_path(&root, "documents/note.md", &["documents", "wiki"], PathMode::MustExist).unwrap();
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
    let out = safe_vault_path(&root, "documents/gone.md", &["documents"], PathMode::MustExist).unwrap();
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
    let err = safe_vault_path(&root, "../etc/escape.md", &["wiki"], PathMode::MayCreate).unwrap_err();
    assert!(matches!(err, SafePathError::Traversal), "got {err:?}");
}

#[test]
fn copy_to_vault_filename_only_under_documents() {
    let (_g, root) = vault();
    let out = safe_vault_path(&root, "documents/incoming.txt", &["documents"], PathMode::MayCreate).unwrap();
    let expected = root.join("documents").canonicalize().unwrap().join("incoming.txt");
    assert_eq!(out, expected);
}
```

- [ ] **Step 2: Run integration tests:**

```bash
cd src-tauri && cargo test -p curated-thoughts --test path_traversal 2>&1 | tail -15
```
Expected: all tests pass.

- [ ] **Step 3: Full test suite + clippy sweep:**

```bash
cd src-tauri && cargo test -p curated-thoughts 2>&1 | tail -20
cd src-tauri && cargo clippy -p curated-thoughts --all-targets -- -D warnings 2>&1 | tail -10
```
Expected: no failures, no clippy warnings.

- [ ] **Step 4: Commit:**

```bash
git add src-tauri/tests/path_traversal.rs
git commit -m "test(tauri): regression tests for path-traversal vulns 1-3"
```

---

## Task 11: Audit pass

**Files:**
- Modify: `src-tauri/src/lib.rs` (only if audit finds an unmigrated command)

- [ ] **Step 1: Find every Tauri command whose signature includes a `String` later used as a path:**

```bash
cd src-tauri && grep -nE "^fn .*: String" src/lib.rs | grep -B0 "path\|file" | head -30
```

Cross-reference each match with the migration table in the spec. For any command that touches the filesystem with a user-supplied `String` and is **not** routed through `safe_vault_path`:

- If the value is a trusted source (`vault_path` from the Tauri file picker dialog, constants), add a one-line comment justifying it: `// trusted: from Tauri file picker dialog (canonicalize before use)`.
- Otherwise, migrate it through the helper using the same template as Tasks 4–9.

- [ ] **Step 2: Document audit result.** If no further migrations are needed, note it in the commit message. If new migrations were added, append them as commits in this task.

- [ ] **Step 3: Final clippy + test:**

```bash
cd src-tauri && cargo test -p curated-thoughts 2>&1 | tail -10
cd src-tauri && cargo clippy -p curated-thoughts --all-targets -- -D warnings 2>&1 | tail -5
```

- [ ] **Step 4: Commit (only if changes):**

```bash
git add src-tauri/src/lib.rs
git commit -m "chore(tauri): audit pass — all path-accepting commands routed through safe helper"
```

---

## Self-review checklist (run before opening PR)

- [ ] All six commands from spec migration table use `safe_vault_path`.
- [ ] Vuln 1 regression test (`/tmp/pwn.md` → `Absolute`) is present and green.
- [ ] Vuln 2 regression test (`documents/../../../etc/passwd` → `Traversal`) is present and green.
- [ ] Vuln 3 regression test (`documents/../../target/file` → `Traversal`) is present and green.
- [ ] Symlink-escape test passes on Unix.
- [ ] `cargo clippy --all-targets -- -D warnings` is clean.
- [ ] No frontend caller still passes absolute paths to the migrated commands (search the frontend and adjust call sites in the same PR).

---

## Plan self-review vs spec

| Spec § | Satisfied by |
|--------|--------------|
| Shared helper module | Task 2 + Task 3 |
| `SafePathError` variants | Task 2 (types), Task 3 (each variant exercised by a test) |
| `PathMode::MustExist` algorithm | Step 3.5 + Step 3.6 |
| `PathMode::MayCreate` algorithm | Step 3.7 |
| Migration table (6 commands) | Tasks 4–9 |
| Unit tests in `safe_path.rs` | Task 3 (all spec test cases covered) |
| Integration tests in `tests/path_traversal.rs` | Task 10 |
| Audit pass | Task 11 |
| Threat-model invariant documented | Task 2 module docstring |

**Execution options:**

1. **Subagent-driven (recommended)** — fresh subagent per task, human review checkpoints (`superpowers:subagent-driven-development`).
2. **Inline execution** — run tasks in series here (`superpowers:executing-plans`).

Which approach?
