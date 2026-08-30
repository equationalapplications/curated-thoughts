//! The ONE vault write-path core (spec v2: `docs/superpowers/specs/2026-08-26-mcp-write-path-okf-frontmatter.md`).
//!
//! Every MCP / Tauri write flows through [`write_note`] or [`upsert_index_entry`];
//! surfaces (`lib.rs` commands, `tool_dispatch.rs` dispatchers) are thin adapters.
//!
//! Contracts implemented here (do not re-implement elsewhere):
//! - Path safety: exclusively `crate::vault::safe_vault_path` — no canonicalize/
//!   `starts_with` hand-rolls in callers (grep gate enforced in CI workflow docs).
//! - Staleness: If-Match style token compare on the EXISTING file's
//!   `updated_at` frontmatter value. File mtimes are NEVER consulted.
//! - Atomic durability: temp-file + rename via `crate::vault::safe_write_bytes`.
//! - Index entry matching: whole-line `## {name}` scan. No regex, no `(?m)`,
//!   no substring `find` — a line equals the header iff `line == "## {name}"`.
//! - Pinned block format (spec v2 §C.4):
//!   `## {name}` / `[[{path}]]` / `- Type: {type}` / `- Key: value`… lines.
//! - Errors use the pinned string shapes: `path_outside_vault`,
//!   `invalid_frontmatter:{detail}`, `stale_update:{current}`,
//!   `index_not_found:{path}`, `invalid_entry_name`, `write_error:{io}`.

use std::path::{Component, Path};

use chrono::SecondsFormat;
use serde_json::Value;

use crate::vault::{
    safe_vault_path, PathMode, SafePathError, AGENTS_DEPOSIT_DIR, NOTE_WRITABLE_SUBDIRS,
    READABLE_SUBDIRS,
};

use super::{
    parse_frontmatter, render_frontmatter, sha256_hash, validate_frontmatter, OkfFrontmatter,
    UpsertError, UpsertResult, WriteNoteError, WriteNoteResult,
};

/// Render a note document: strict YAML frontmatter fence + body.
///
/// `render_frontmatter` already emits the trailing `---\n`; append the body
/// and guarantee exactly one terminating newline.
fn render_document(frontmatter: &OkfFrontmatter, body: &str) -> String {
    let mut doc = render_frontmatter(frontmatter);
    if !body.is_empty() {
        doc.push_str(body);
    }
    if !doc.ends_with('\n') {
        doc.push('\n');
    }
    doc
}

/// Extract the `updated_at` token from an existing document's frontmatter.
///
/// Returns `None` for absent files' content distinctions being made by the
/// caller: malformed / unparsable frontmatter yields `None`, which the caller
/// treats as "no usable token present".
fn extract_updated_at(existing_content: &str) -> Option<String> {
    let mut lines = existing_content.lines();
    if lines.next()? != "---" {
        return None;
    }
    let mut fenced = String::new();
    for line in lines.take(64) {
        if line == "---" {
            return parse_frontmatter(&fenced).ok()?.updated_at;
        }
        fenced.push_str(line);
        fenced.push('\n');
    }
    None
}

/// Enforce If-Match staleness on the existing file's `updated_at` token.
///
/// Rules (spec v2 §B.2, resolved rulings):
/// - File absent → edit proceeds (this is a create).
/// - Token supplied → must EXACTLY match the existing token, else
///   [`WriteNoteError::StaleUpdate`] carries the current token.
/// - No token supplied but the file exists → refused as stale: an edit
///   requires proof the writer saw the current revision.
fn enforce_staleness(
    existing_content: Option<&str>,
    expected_updated_at: Option<&str>,
) -> Result<(), WriteNoteError> {
    let Some(content) = existing_content else {
        return Ok(()); // create path — nothing to be stale against
    };
    let current = extract_updated_at(content);
    let matches_current = match (&current, expected_updated_at) {
        (Some(current), Some(expected)) => current == expected,
        _ => false, // missing token on either side ⇒ cannot prove freshness
    };
    if matches_current {
        Ok(())
    } else {
        Err(WriteNoteError::StaleUpdate {
            updated_at: current.unwrap_or_default(),
        })
    }
}

/// True iff `path` is inside the deposit folder at any depth (incl. subfolders,
/// allowed per Kurt's Aug 29 2026 directive; amended spec
/// `2026-08-27-agent-deposit-write-path.md` §AMENDED 2026-08-29).
fn under_deposit(path: &str) -> bool {
    under_any(path, &[AGENTS_DEPOSIT_DIR])
}

/// True iff `path` lies at any depth under one of `allowed_subdirs`.
/// Component-based, so a sibling prefix (`immutable-source-files/agents-evil`)
/// never matches an allowed root (`immutable-source-files/agents`).
///
/// `Component::CurDir` is dropped first: `Path::components` normalizes interior
/// `.` away but KEEPS a leading one, so `./wiki/x.md` would otherwise compare as
/// `[".", "wiki", ...]` and fail to match `wiki`. `safe_vault_path` accepts a
/// leading `./` (it only rejects `ParentDir`/`Prefix`), so this check must too.
fn under_any(path: &str, allowed_subdirs: &[&str]) -> bool {
    let comps: Vec<&str> = Path::new(path)
        .components()
        .filter(|c| !matches!(c, Component::CurDir))
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    allowed_subdirs.iter().any(|sub| {
        let prefix: Vec<&str> = sub.split('/').collect();
        comps.len() > prefix.len() && comps[..prefix.len()] == prefix[..]
    })
}

/// Create `rel_parent` under `vault_root` one component at a time, refusing to
/// traverse a symlinked component.
///
/// `std::fs::create_dir_all` follows symlinks on components that already exist,
/// so a symlink planted inside the vault (by a sync conflict, a restored backup,
/// or the user) would let directories be created *outside* the vault root. The
/// round-two `safe_vault_path` call still rejects the write, so no file is ever
/// written there — but the out-of-vault directories would persist. Creating the
/// chain stepwise keeps every side effect of a rejected write inside the vault.
///
/// NOT atomic: each component is stat'd then created, so a writer racing this
/// loop could swap a just-created directory for a symlink before the next
/// `mkdir` follows it. Closing that window needs `mkdirat(_, O_NOFOLLOW)` per
/// component; `create_dir_all` had the identical exposure, so this is a
/// narrowing, not a guarantee. Local vault write access is required to exploit.
fn create_parents_no_symlink(vault_root: &Path, rel_parent: &Path) -> std::io::Result<()> {
    let mut cur = vault_root.to_path_buf();
    // `rel_parent` is already vetted by safe_vault_path: relative, no `..`, no
    // prefix components — so every component here is a plain name.
    for comp in rel_parent.components() {
        cur.push(comp);
        match std::fs::symlink_metadata(&cur) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("symlinked path component: {}", cur.display()),
                ));
            }
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("path component is not a directory: {}", cur.display()),
                ));
            }
            // Only a genuine absence means "create it"; an EACCES/ELOOP from
            // stat must surface as itself, not as a confusing create_dir error.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => std::fs::create_dir(&cur)?,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Write a note with OKF frontmatter to the vault (single core, spec v2).
///
/// * `vault_root` — absolute path to the vault root.
/// * `path` — vault-relative path (e.g. `wiki/my-note.md` or
///   `immutable-source-files/agents/my-note.md`). Agent deposits may nest at
///   any depth under `agents/` (per-agent subfolders allowed; amended spec
///   `2026-08-27-agent-deposit-write-path.md` §AMENDED 2026-08-29). Validated
///   with `safe_vault_path(_, _, NOTE_WRITABLE_SUBDIRS, PathMode::MayCreate)`.
///   Missing parent directories are created component-by-component without
///   traversing symlinks (see [`create_parents_no_symlink`]), then the
///   resolution is repeated so every containment/symlink decision stays inside
///   `safe_vault_path`.
/// * `frontmatter` — OKF frontmatter; validated; `updated_at` defaults to
///   now (RFC 3339, UTC) when omitted.
/// * `body` — markdown body; normalized to end with exactly one `\n`.
/// * `expected_updated_at` — If-Match token: required to equal the existing
///   file's token when the file already exists (see [`enforce_staleness`]).
pub fn write_note(
    vault_root: &Path,
    path: &str,
    frontmatter: &OkfFrontmatter,
    body: &str,
    expected_updated_at: Option<&str>,
) -> Result<WriteNoteResult, WriteNoteError> {
    validate_frontmatter(frontmatter).map_err(WriteNoteError::InvalidFrontmatter)?;

    // Validate supersession: deposit-to-deposit only, target must exist.
    if let Some(ref supersedes_path) = frontmatter.supersedes {
        // Both ends must be deposits (component-based check; string
        // `starts_with` would accept sibling prefixes like `agents-evil/`).
        if !under_deposit(supersedes_path) {
            return Err(WriteNoteError::InvalidFrontmatter(format!(
                "supersedes must reference a deposit under {}: got {}",
                AGENTS_DEPOSIT_DIR, supersedes_path
            )));
        }
        if !under_deposit(path) {
            return Err(WriteNoteError::InvalidFrontmatter(format!(
                "supersedes is deposit-only: note path must be a deposit under {}: got {}",
                AGENTS_DEPOSIT_DIR, path
            )));
        }

        // Resolve the target with the deposit-only allowlist. MustExist
        // already guarantees is_file() (safe_vault_path rejects dirs and
        // non-regular files), so the resolution error IS the not-found case.
        safe_vault_path(
            vault_root,
            supersedes_path,
            &[AGENTS_DEPOSIT_DIR],
            PathMode::MustExist,
        )
        .map_err(|_| {
            WriteNoteError::InvalidFrontmatter(format!("supersedes_not_found:{}", supersedes_path))
        })?;
    }

    let target = match safe_vault_path(vault_root, path, NOTE_WRITABLE_SUBDIRS, PathMode::MayCreate)
    {
        Ok(target) => target,
        Err(SafePathError::NotFound(ref msg)) if msg.contains("parent directory not found") => {
            // Parent dirs don't exist yet. Path shape was already vetted
            // (absolute/`..`/NUL/dot-enders reject before any FS access), so
            // create the parents and re-resolve; round two re-canonicalizes
            // and enforces containment + symlink rules inside safe_vault_path.
            //
            // Check containment LEXICALLY first: round two would reject an
            // out-of-tree path anyway, but only after the bootstrap had already
            // created the directories, leaving them behind on a rejected write.
            if !under_any(path, NOTE_WRITABLE_SUBDIRS) {
                return Err(WriteNoteError::PathOutsideVault);
            }
            let rel_parent = Path::new(path)
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .ok_or_else(|| {
                    WriteNoteError::WriteError(format!(
                        "write_error:missing parent component: {}",
                        path
                    ))
                })?;
            create_parents_no_symlink(vault_root, rel_parent).map_err(|e| {
                WriteNoteError::WriteError(format!("write_error:create_parents_no_symlink: {}", e))
            })?;
            safe_vault_path(vault_root, path, NOTE_WRITABLE_SUBDIRS, PathMode::MayCreate)
                .map_err(map_safe_err_note)?
        }
        Err(e) => return Err(map_safe_err_note(e)),
    };

    let existing = std::fs::read_to_string(&target).ok();
    enforce_staleness(existing.as_deref(), expected_updated_at)?;

    // Rotate the token on EVERY successful write. Floor: the previous file
    // token, so the successor is always strictly newer even when both calls
    // land inside the same millisecond — a reused token can never verify.
    let prev_token = existing.as_deref().and_then(extract_updated_at);
    let now = chrono::Utc::now();
    let mut fresh = now.to_rfc3339_opts(SecondsFormat::Millis, true);
    if let Some(prev) = prev_token
        .as_deref()
        .and_then(|p| chrono::DateTime::parse_from_rfc3339(p).ok())
    {
        if let Ok(cur) = chrono::DateTime::parse_from_rfc3339(&fresh) {
            if cur <= prev {
                fresh = (prev + chrono::Duration::milliseconds(1))
                    .to_rfc3339_opts(SecondsFormat::Millis, true);
            }
        }
    }
    let mut effective_fm = frontmatter.clone();
    effective_fm.updated_at = Some(fresh);
    if effective_fm.created_at.trim().is_empty() && existing.is_none() {
        return Err(WriteNoteError::InvalidFrontmatter(
            "created_at is required on create".to_string(),
        ));
    }

    let document = render_document(&effective_fm, body);

    crate::vault::safe_write_bytes(&target, document.as_bytes())
        .map_err(|e| WriteNoteError::WriteError(format!("write_error:{}", e)))?;

    Ok(WriteNoteResult {
        success: true,
        path: path.to_string(),
        sha256: sha256_hash(&document),
    })
}

fn map_safe_err_note(e: SafePathError) -> WriteNoteError {
    match e {
        SafePathError::Absolute
        | SafePathError::Traversal
        | SafePathError::Outside
        | SafePathError::InvalidName
        | SafePathError::NotARegularFile => WriteNoteError::PathOutsideVault,
        SafePathError::NotFound(msg) => {
            WriteNoteError::WriteError(format!("write_error:not found: {}", msg))
        }
        SafePathError::Io(e) => WriteNoteError::WriteError(format!("write_error:{}", e)),
    }
}

/// Validate an index entry name (pinned error: `invalid_entry_name`).
///
/// Rejects empty/whitespace names, surrounding whitespace, newline injection,
/// and `#` (would forge headers or comments).
fn validate_entry_name(entry_name: &str) -> Result<(), UpsertError> {
    // Spec v2: letters, digits, spaces, underscore, hyphen, dot. Everything
    // else (including '#', newlines, leading/trailing whitespace) is refused.
    let valid = !entry_name.is_empty()
        && entry_name.trim() == entry_name
        && entry_name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '_' | '-' | '.'));
    if valid {
        Ok(())
    } else {
        Err(UpsertError::InvalidEntryName)
    }
}

/// Render the pinned index entry block (spec v2 §C.4). Always `\n`-terminated.
fn render_index_entry_block(
    entry_name: &str,
    entry_path: &str,
    entry_type: &str,
    metadata: Option<&Value>,
) -> Result<String, UpsertError> {
    let mut block = format!(
        "## {}\n[[{}]]\n- Type: {}\n",
        entry_name, entry_path, entry_type
    );
    if let Some(metadata) = metadata {
        if !metadata.is_null() {
            let map = metadata.as_object().ok_or_else(|| {
                UpsertError::InvalidMetadata("metadata must be a JSON object".to_string())
            })?;
            for (key, value) in map {
                let rendered = match value {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                let key = key.trim();
                if key.is_empty() || key.contains('\n') {
                    return Err(UpsertError::InvalidMetadata(format!(
                        "invalid metadata key: {:?}",
                        key
                    )));
                }
                // Legacy-visible form: keys render capitalized ("- Status:").
                let mut display = key.to_string();
                if let Some(first) = display.get_mut(0..1) {
                    first.make_ascii_uppercase();
                }
                block.push_str(&format!("- {}: {}\n", display, rendered.replace('\n', " ")));
            }
        }
    }
    Ok(block)
}

/// Whole-line scan for the entry header. Returns the 0-based line index of
/// `## {entry_name}` or `None`. Exact-match only: no regex, no substrings.
fn find_entry_header_line(content: &str, entry_name: &str) -> Option<usize> {
    let header = format!("## {}", entry_name);
    content.lines().position(|line| line == header)
}

/// Replace-or-append the pinned block using whole-line matching.
///
/// Update: replaces from the matched header line through the line before the
/// next `## ` header (or EOF). Append: one blank line, then the block at EOF.
/// Returns `(new_content, appended, header_line_number_1based)`.
fn upsert_entry_in_content(content: &str, entry_name: &str, block: &str) -> (String, bool, usize) {
    let Some(header_idx) = find_entry_header_line(content, entry_name) else {
        let mut new_content = String::from(content);
        if !new_content.ends_with('\n') && !new_content.is_empty() {
            new_content.push('\n');
        }
        if !new_content.ends_with("\n\n") {
            new_content.push('\n');
        }
        new_content.push_str(block);
        let line_number = content.lines().count() + 2;
        return (new_content, true, line_number);
    };

    let lines: Vec<&str> = content.lines().collect();
    let next_header_idx = lines[header_idx + 1..]
        .iter()
        .position(|line| line.starts_with("## "))
        .map(|offset| header_idx + 1 + offset)
        .unwrap_or(lines.len());

    let mut out = String::new();
    for line in &lines[..header_idx] {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(block);
    for line in &lines[next_header_idx..] {
        out.push_str(line);
        out.push('\n');
    }
    (out, false, header_idx + 1)
}

/// Upsert one entry into an existing vault INDEX.md (single core, spec v2).
///
/// * `index_path` — vault-relative; the file MUST exist (never auto-created;
///   pinned error `index_not_found:{path}`).
/// * `entry_path` — vault-relative note path the entry links to; must resolve
///   to a regular file inside the vault.
/// * Matching/replacement semantics: see [`find_entry_header_line`] /
///   [`upsert_entry_in_content`]. Atomic via `safe_write_bytes`.
#[allow(clippy::too_many_arguments)]
pub fn upsert_index_entry(
    vault_root: &Path,
    index_path: &str,
    entry_name: &str,
    entry_path: &str,
    entry_type: &str,
    metadata: Option<&Value>,
) -> Result<UpsertResult, UpsertError> {
    validate_entry_name(entry_name)?;
    if entry_type.trim().is_empty() || entry_type.contains(['\n', '\r']) {
        return Err(UpsertError::InvalidMetadata(format!(
            "invalid entry type: {:?}",
            entry_type
        )));
    }

    let canonical_index = match safe_vault_path(
        vault_root,
        index_path,
        READABLE_SUBDIRS,
        PathMode::MustExist,
    ) {
        Ok(p) => p,
        Err(SafePathError::NotFound(_)) => {
            return Err(UpsertError::IndexNotFound(index_path.to_string()))
        }
        Err(e) => return Err(map_safe_err_upsert(e)),
    };
    let canonical_entry_target = safe_vault_path(
        vault_root,
        entry_path,
        READABLE_SUBDIRS,
        PathMode::MustExist,
    )
    .map_err(map_safe_err_upsert)?;

    let content = std::fs::read_to_string(&canonical_index)
        .map_err(|e| UpsertError::WriteError(format!("write_error:read: {}", e)))?;

    let block = render_index_entry_block(entry_name, entry_path, entry_type, metadata)?;
    let (new_content, appended, line_number) =
        upsert_entry_in_content(&content, entry_name, &block);

    // Sanity: referenced note must exist before the index points at it.
    debug_assert!(canonical_entry_target.exists());

    crate::vault::safe_write_bytes(&canonical_index, new_content.as_bytes())
        .map_err(|e| UpsertError::WriteError(format!("write_error:{}", e)))?;

    Ok(UpsertResult {
        success: true,
        index_path: index_path.to_string(),
        entry_id: entry_name.to_string(),
        appended,
        line_number: Some(line_number),
    })
}

fn map_safe_err_upsert(e: SafePathError) -> UpsertError {
    match e {
        SafePathError::Absolute
        | SafePathError::Traversal
        | SafePathError::Outside
        | SafePathError::InvalidName
        | SafePathError::NotARegularFile => UpsertError::PathOutsideVault,
        SafePathError::NotFound(msg) => {
            UpsertError::WriteError(format!("write_error:not found: {}", msg))
        }
        SafePathError::Io(e) => UpsertError::WriteError(format!("write_error:{}", e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use tempfile::TempDir;

    fn vault() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        fs::create_dir_all(root.join("wiki")).unwrap();
        (dir, root)
    }

    fn fm(title: &str, updated_at: Option<&str>) -> OkfFrontmatter {
        OkfFrontmatter {
            okf_version: "0.1".to_string(),
            profile: "llm-wiki/1".to_string(),
            title: title.to_string(),
            entity_type: super::super::EntityType::Fact,
            tags: Some(vec!["test".to_string()]),
            created_at: "2026-08-27T00:00:00Z".to_string(),
            updated_at: updated_at.map(str::to_string),
            supersedes: None,
        }
    }

    /// D1 — create: writes frontmatter + body, fills updated_at, vault-relative path.
    #[test]
    fn d1_create_note_writes_frontmatter_and_hash() {
        let (_g, root) = vault();
        let result = write_note(
            &root,
            "wiki/test-note.md",
            &fm("T", None),
            "Body line.\nSecond.\n",
            None,
        )
        .unwrap();
        assert_eq!(result.path, "wiki/test-note.md");
        assert!(result.success);
        let content = fs::read_to_string(root.join("wiki/test-note.md")).unwrap();
        assert!(content.starts_with("---\nokf_version: 0.1\n"));
        assert!(content.contains("updated_at: 20"));
        assert!(content.ends_with("Second.\n"));
        assert_eq!(result.sha256, sha256_hash(&content));
    }

    /// D2 — stale edit without token is refused; token must exact-match.
    #[test]
    fn d2_edit_requires_exact_token() {
        let (_g, root) = vault();
        write_note(&root, "wiki/n.md", &fm("T", None), "v1\n", None).unwrap();
        let current =
            extract_updated_at(&fs::read_to_string(root.join("wiki/n.md")).unwrap()).unwrap();

        // No token → refused (cannot prove freshness).
        let err = write_note(&root, "wiki/n.md", &fm("T", None), "v2\n", None).unwrap_err();
        assert!(
            matches!(err, WriteNoteError::StaleUpdate { ref updated_at } if updated_at == &current)
        );

        // Wrong token → refused.
        let err = write_note(
            &root,
            "wiki/n.md",
            &fm("T", None),
            "v2\n",
            Some("1999-01-01T00:00:00Z"),
        )
        .unwrap_err();
        assert!(
            matches!(err, WriteNoteError::StaleUpdate { ref updated_at } if updated_at == &current)
        );

        // Correct token → succeeds, token rotates.
        write_note(&root, "wiki/n.md", &fm("T", None), "v2\n", Some(&current)).unwrap();
        let bumped =
            extract_updated_at(&fs::read_to_string(root.join("wiki/n.md")).unwrap()).unwrap();
        assert_ne!(current, bumped);
    }

    /// D3 — path traversal is refused by safe_vault_path (no escapes ever).
    #[test]
    fn d3_traversal_rejected() {
        let (_g, root) = vault();
        let err = write_note(&root, "../outside.md", &fm("T", None), "x\n", None).unwrap_err();
        assert!(matches!(err, WriteNoteError::PathOutsideVault));
        let err = write_note(&root, "/etc/passwd", &fm("T", None), "x\n", None).unwrap_err();
        assert!(matches!(err, WriteNoteError::PathOutsideVault));
    }

    /// D4 — nested parents are created safely, then re-validated.
    #[test]
    fn d4_creates_missing_parents_safely() {
        let (_g, root) = vault();
        write_note(
            &root,
            "wiki/deep/er/note.md",
            &fm("Deep", None),
            "x\n",
            None,
        )
        .unwrap();
        assert!(root.join("wiki/deep/er/note.md").is_file());
    }

    /// D5 — index upsert: create + idempotent update, no duplicates.
    #[test]
    fn d5_upsert_no_duplicates() {
        let (_g, root) = vault();
        write_note(&root, "wiki/a.md", &fm("A", None), "x\n", None).unwrap();
        fs::write(
            root.join("wiki/INDEX.md"),
            "# Index\n\n## other\n[[b.md]]\n- Type: doc\n",
        )
        .unwrap();

        let r1 =
            upsert_index_entry(&root, "wiki/INDEX.md", "alpha", "wiki/a.md", "fact", None).unwrap();
        assert!(r1.appended);
        let c1 = fs::read_to_string(root.join("wiki/INDEX.md")).unwrap();
        assert_eq!(c1.matches("## alpha\n").count(), 1);

        let r2 = upsert_index_entry(
            &root,
            "wiki/INDEX.md",
            "alpha",
            "wiki/a.md",
            "fact",
            Some(&json!({"status":"live"})),
        )
        .unwrap();
        assert!(!r2.appended);
        let c2 = fs::read_to_string(root.join("wiki/INDEX.md")).unwrap();
        assert_eq!(c2.matches("## alpha\n").count(), 1);
        assert!(c2.contains("- Status: live"));
        assert!(c2.starts_with("# Index\n\n## other\n"));
        assert_eq!(
            r2.line_number,
            Some(7),
            "line numbers are 1-based against the file"
        );
    }

    /// D6 — prefix collisions never match: `## alph` != `## alpha`.
    #[test]
    fn d6_prefix_collision_isolated() {
        let (_g, root) = vault();
        fs::write(root.join("wiki/a.md"), "---\nokf_version: 0.1\n---\n").unwrap();
        fs::write(root.join("wiki/z.md"), "---\nokf_version: 0.1\n---\n").unwrap();
        fs::write(
            root.join("wiki/INDEX.md"),
            "## alpha\n[[a.md]]\n- Type: fact\n\n## alphabet\n[[z.md]]\n- Type: doc\n",
        )
        .unwrap();
        upsert_index_entry(&root, "wiki/INDEX.md", "alpha", "wiki/a.md", "fact", None).unwrap();
        let c = fs::read_to_string(root.join("wiki/INDEX.md")).unwrap();
        assert!(c.contains("## alphabet\n[[z.md]]\n- Type: doc\n"));
        assert_eq!(c.matches("## alpha\n").count(), 1);
        // Substring machines (contains/find) would have corrupted `alphabet`.
    }

    /// D7 — index must exist (no auto-create) and entry refs must be safe.
    #[test]
    fn d7_index_not_auto_created_and_names_validated() {
        let (_g, root) = vault();
        let err = upsert_index_entry(&root, "wiki/MISSING.md", "x", "wiki/a.md", "fact", None)
            .unwrap_err();
        assert!(matches!(err, UpsertError::IndexNotFound(ref p) if p == "wiki/MISSING.md"));

        fs::write(root.join("wiki/a.md"), "---\nokf_version: 0.1\n---\n").unwrap();
        fs::write(root.join("wiki/INDEX.md"), "").unwrap();
        let err =
            upsert_index_entry(&root, "wiki/INDEX.md", "", "wiki/a.md", "fact", None).unwrap_err();
        assert!(matches!(err, UpsertError::InvalidEntryName));
        let err = upsert_index_entry(
            &root,
            "wiki/INDEX.md",
            "bad name!",
            "wiki/a.md",
            "fact",
            None,
        );
        assert!(matches!(err, Err(UpsertError::InvalidEntryName))); // '!' is outside the pinned charset
        upsert_index_entry(
            &root,
            "wiki/INDEX.md",
            "good name",
            "wiki/a.md",
            "fact",
            None,
        )
        .unwrap(); // spaces ARE legal; headers pin exact lines
    }

    /// Block format pin: header/link/type/metadata lines, one per line.
    #[test]
    fn block_format_pinned() {
        let b =
            render_index_entry_block("n", "p.md", "fact", Some(&json!({"status":"live","n":2})))
                .unwrap();
        assert_eq!(b, "## n\n[[p.md]]\n- Type: fact\n- N: 2\n- Status: live\n");
    }

    /// Whole-line matcher ignores indented or commented look-alikes.
    #[test]
    fn matcher_requires_whole_line() {
        let content = "- ## fake\n\ntext ## fake\n## fake extra\n";
        assert_eq!(find_entry_header_line(content, "fake"), None);
        assert_eq!(find_entry_header_line("## fake\n", "fake"), Some(0));
    }

    // ---- Agent deposit write path (spec: 2026-08-27-agent-deposit-write-path.md) ----

    /// Vault fixture with the deposit dir present (post-Phase-2 state).
    fn deposit_vault() -> (TempDir, std::path::PathBuf) {
        let (dir, root) = vault();
        fs::create_dir_all(root.join("immutable-source-files/agents")).unwrap();
        (dir, root)
    }

    /// AD1 — flat deposit write succeeds; file lands under agents/.
    #[test]
    fn ad1_flat_deposit_write_succeeds() {
        let (_g, root) = deposit_vault();
        let result = write_note(
            &root,
            "immutable-source-files/agents/mem.md",
            &fm("Agent memory", None),
            "deposited\n",
            None,
        )
        .unwrap();
        assert!(result.success);
        assert!(root.join("immutable-source-files/agents/mem.md").is_file());
    }

    /// AD2 — nested deposits succeed (amended spec §AMENDED 2026-08-29:
    /// subfolders under `agents/` are allowed, any depth).
    #[test]
    fn ad2_nested_deposit_write_succeeds() {
        let (_g, root) = deposit_vault();
        let result = write_note(
            &root,
            "immutable-source-files/agents/people/tessera/x.md",
            &fm("Nested", None),
            "deposited\n",
            None,
        )
        .unwrap();
        assert!(result.success);
        assert!(root
            .join("immutable-source-files/agents/people/tessera/x.md")
            .is_file());
    }

    /// E2 — deep-path deposit (4 levels under `agents/`) succeeds; missing
    /// intermediate dirs are bootstrapped by the parent-create retry.
    #[test]
    fn e2_deep_nested_deposit_write_succeeds() {
        let (_g, root) = deposit_vault();
        let result = write_note(
            &root,
            "immutable-source-files/agents/products/curated-thoughts/specs/y.md",
            &fm("Deep", None),
            "deposited\n",
            None,
        )
        .unwrap();
        assert!(result.success);
        assert!(root
            .join("immutable-source-files/agents/products/curated-thoughts/specs/y.md")
            .is_file());
    }

    /// AD3 — write outside the deposit prefix is rejected (path safety).
    #[test]
    fn ad3_user_source_write_rejected() {
        let (_g, root) = deposit_vault();
        let err = write_note(
            &root,
            "immutable-source-files/secrets.md",
            &fm("T", None),
            "x\n",
            None,
        )
        .unwrap_err();
        assert!(matches!(err, WriteNoteError::PathOutsideVault));
    }

    /// AD3b — a rejected write leaves no directories behind. Restores the
    /// no-side-effect assertion that was dropped with the old flat-layout AD2
    /// test; nothing else in this file covered it.
    #[test]
    fn ad3b_rejected_write_creates_no_dirs() {
        let (_g, root) = deposit_vault();
        let err = write_note(
            &root,
            "immutable-source-files/agents-evil/nested/mem.md",
            &fm("T", None),
            "x\n",
            None,
        )
        .unwrap_err();
        assert!(matches!(err, WriteNoteError::PathOutsideVault));
        assert!(!root.join("immutable-source-files/agents-evil").exists());
    }

    /// A leading `./` still resolves. `Path::components` keeps a leading
    /// `CurDir`, so the lexical `under_any` gate on the parent-bootstrap branch
    /// must drop it — `safe_vault_path` accepts `./` (it rejects only `..` and
    /// prefix components), and rejecting it here would be a regression.
    #[test]
    fn dot_prefixed_path_with_missing_parents_still_writes() {
        let (_g, root) = deposit_vault();
        write_note(
            &root,
            "./wiki/deep/er/dot.md",
            &fm("Dot", None),
            "x\n",
            None,
        )
        .unwrap();
        assert!(root.join("wiki/deep/er/dot.md").is_file());

        write_note(
            &root,
            "./immutable-source-files/agents/nested/dot.md",
            &fm("Dot", None),
            "x\n",
            None,
        )
        .unwrap();
        assert!(root
            .join("immutable-source-files/agents/nested/dot.md")
            .is_file());
    }

    /// AD3c — a symlinked component under `agents/` is never traversed when
    /// bootstrapping parents. `create_dir_all` would follow it and create dirs
    /// outside the vault root (the write itself is still rejected by round-two
    /// containment, but the directories would persist).
    #[cfg(unix)]
    #[test]
    fn ad3c_symlinked_parent_component_creates_nothing_outside() {
        let (_g, root) = deposit_vault();
        let outside = root.parent().unwrap().join("outside-target");
        fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("immutable-source-files/agents/sub"))
            .unwrap();

        let err = write_note(
            &root,
            "immutable-source-files/agents/sub/deep/mem.md",
            &fm("T", None),
            "x\n",
            None,
        )
        .unwrap_err();

        assert!(matches!(err, WriteNoteError::WriteError(_)));
        assert!(
            !outside.join("deep").exists(),
            "create_dir_all followed the symlink and escaped the vault"
        );
    }

    /// AD4 — first deposit into a missing agents/ bootstraps parents (lazy path).
    #[test]
    fn ad4_lazy_bootstrap_missing_agents_dir() {
        let (_g, root) = vault(); // no immutable-source-files/agents
        write_note(
            &root,
            "immutable-source-files/agents/first.md",
            &fm("First", None),
            "x\n",
            None,
        )
        .unwrap();
        assert!(root
            .join("immutable-source-files/agents/first.md")
            .is_file());
    }

    /// AD5 — supersedes happy path: target exists → write succeeds and the
    /// supersedes value round-trips through render → parse.
    #[test]
    fn ad5_supersedes_valid_roundtrip() {
        let (_g, root) = deposit_vault();
        write_note(
            &root,
            "immutable-source-files/agents/v1.md",
            &fm("V1", None),
            "old\n",
            None,
        )
        .unwrap();
        let mut m = fm("V2", None);
        m.supersedes = Some("immutable-source-files/agents/v1.md".to_string());
        write_note(
            &root,
            "immutable-source-files/agents/v2.md",
            &m,
            "new\n",
            None,
        )
        .unwrap();
        let raw = fs::read_to_string(root.join("immutable-source-files/agents/v2.md")).unwrap();
        assert!(raw.contains("supersedes: immutable-source-files/agents/v1.md"));
        let parsed = extract_fm(&raw);
        assert_eq!(
            parsed.supersedes.as_deref(),
            Some("immutable-source-files/agents/v1.md")
        );
    }

    /// AD5b — supersedes roundtrip with a NESTED target: both ends inside
    /// `agents/` at depth, containment check passes (amended spec
    /// §AMENDED 2026-08-29).
    #[test]
    fn ad5b_supersedes_nested_target_roundtrip() {
        let (_g, root) = deposit_vault();
        write_note(
            &root,
            "immutable-source-files/agents/people/tessera/v1.md",
            &fm("V1", None),
            "old\n",
            None,
        )
        .unwrap();
        let mut m = fm("V2", None);
        m.supersedes = Some("immutable-source-files/agents/people/tessera/v1.md".to_string());
        write_note(
            &root,
            "immutable-source-files/agents/people/tessera/v2.md",
            &m,
            "new\n",
            None,
        )
        .unwrap();
        let raw =
            fs::read_to_string(root.join("immutable-source-files/agents/people/tessera/v2.md"))
                .unwrap();
        assert!(raw.contains("supersedes: immutable-source-files/agents/people/tessera/v1.md"));
        let parsed = extract_fm(&raw);
        assert_eq!(
            parsed.supersedes.as_deref(),
            Some("immutable-source-files/agents/people/tessera/v1.md")
        );
    }

    /// AD6 — supersedes pointing outside agents/ → InvalidFrontmatter.
    #[test]
    fn ad6_supersedes_outside_deposit_rejected() {
        let (_g, root) = deposit_vault();
        write_note(&root, "wiki/target.md", &fm("T", None), "x\n", None).unwrap();
        let mut m = fm("Evil", None);
        m.supersedes = Some("wiki/target.md".to_string());
        let err =
            write_note(&root, "immutable-source-files/agents/e.md", &m, "x\n", None).unwrap_err();
        assert!(matches!(err, WriteNoteError::InvalidFrontmatter(_)));
    }

    /// AD7 — supersedes to a non-existent deposit → InvalidFrontmatter.
    #[test]
    fn ad7_supersedes_missing_target_rejected() {
        let (_g, root) = deposit_vault();
        let mut m = fm("T", None);
        m.supersedes = Some("immutable-source-files/agents/ghost.md".to_string());
        let err =
            write_note(&root, "immutable-source-files/agents/n.md", &m, "x\n", None).unwrap_err();
        match err {
            WriteNoteError::InvalidFrontmatter(ref detail) => {
                assert!(detail.contains("supersedes_not_found"));
            }
            other => panic!("expected InvalidFrontmatter, got {other:?}"),
        }
    }

    /// AD8 — sibling-prefix attack: `agents-evil/` must NOT pass as a deposit
    /// (string starts_with would accept it; component check must not).
    #[test]
    fn ad8_sibling_prefix_rejected() {
        let (_g, root) = deposit_vault();
        fs::create_dir_all(root.join("immutable-source-files/agents-evil")).unwrap();
        fs::write(root.join("immutable-source-files/agents-evil/x.md"), "x\n").unwrap();
        let mut m = fm("Evil", None);
        m.supersedes = Some("immutable-source-files/agents-evil/x.md".to_string());
        let err =
            write_note(&root, "immutable-source-files/agents/e.md", &m, "x\n", None).unwrap_err();
        assert!(matches!(err, WriteNoteError::InvalidFrontmatter(_)));
    }

    /// AD9 — supersedes is deposit-only: wiki notes cannot carry it.
    #[test]
    fn ad9_wiki_note_cannot_supersede() {
        let (_g, root) = deposit_vault();
        write_note(
            &root,
            "immutable-source-files/agents/v1.md",
            &fm("V1", None),
            "x\n",
            None,
        )
        .unwrap();
        let mut m = fm("W", None);
        m.supersedes = Some("immutable-source-files/agents/v1.md".to_string());
        let err = write_note(&root, "wiki/w.md", &m, "x\n", None).unwrap_err();
        assert!(matches!(err, WriteNoteError::InvalidFrontmatter(_)));
    }

    /// Parse a rendered document's frontmatter block.
    fn extract_fm(raw: &str) -> OkfFrontmatter {
        let fenced: String = raw
            .lines()
            .skip(1) // opening ---
            .take_while(|l| l != &"---")
            .fold(String::new(), |mut acc, l| {
                acc.push_str(l);
                acc.push('\n');
                acc
            });
        parse_frontmatter(&fenced).unwrap()
    }
}
