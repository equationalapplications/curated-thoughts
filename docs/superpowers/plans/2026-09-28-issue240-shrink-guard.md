# vault_write_note size-drop guard (issue #240) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reject `vault_write_note` edits whose rendered body shrinks below half its size (truncated-payload signature) and any payload introducing a context-compaction marker, unless the caller explicitly passes `allow_shrink` — replaying the 2026-09-26 incident (12,860→505 bytes) as a refusal.

**Architecture:** Two guard helpers (`enforce_size_drop`, `enforce_compaction_markers`) in the ONE write-path core (`okf::write::write_note`), running after `render_document` (the measurement basis is the rendered body) and before `safe_write_bytes`. A new `split_frontmatter_fence` helper provides CRLF-safe body offsets. `allow_shrink` plumbs from MCP params (`#[serde(default)]`) and the Tauri command (`Option<bool>` — command params don't honor serde defaults) into the core; both default to `false`.

**Tech Stack:** Rust only (src-tauri). No frontend changes.

**Spec:** `docs/superpowers/specs/2026-09-28-issue240-shrink-guard-design.md` (companion investigation: `2026-09-28-issue240-shrink-guard-investigation.md`)

## Global Constraints

- `MIN_GUARDED_BODY_BYTES: usize = 1024` — the ratio test applies only when the EXISTING body is ≥ this; smaller notes may be fully rewritten without `allow_shrink`.
- Refuse iff `new_bytes * 2 < existing_bytes` (integer math; `new * 2 == existing` is ALLOWED; odd `existing` handled by the integer condition, never "existing/2" phrasing).
- Byte basis (both sides, via `split_frontmatter_fence` offsets): existing body = `content.len() - offset`; new body = `doc.len() - offset_new`. NEVER `collect_frontmatter_fence` (drops `\r` via `lines()` → mis-measures CRLF). Fence-less existing content measures whole-content (offset 0).
- Marker list const: `["[SKILL_PRUNED]", "HERMES-CONTEXT-COMPRESSION"]`. Creates reject ALL; edits reject only markers NOT already present in the existing content — FRONTMATTER INCLUDED (scan the rendered `document` vs the whole existing content; a note whose title quotes a marker stays editable). `allow_shrink` does NOT bypass the marker check.
- Check order pinned: marker check FIRST, then shrink check.
- Error Displays (machine-readable prefix + fixed instruction; never mention `allow_shrink`):
  - `shrink_refused:{existing_bytes}:{new_bytes}: re-read the note and resend the full body`
  - `compaction_marker:{marker}: rephrase and resend without compaction artifacts`
- Tauri command takes `allow_shrink: Option<bool>` + `.unwrap_or(false)` (a plain `bool` becomes a required invoke key and breaks every existing invoke). MCP params field: `#[serde(default)] pub allow_shrink: bool`.
- Run tests: `cargo test -p curated-thoughts okf::write` (pure tempfile tests) and `cargo test -p curated-thoughts --test mcp_write_integration` (plumbing).
- Conventional commits; all work on `feat/issue-240-shrink-guard`, one PR (#248).

---

### Task 1: `split_frontmatter_fence` helper (CRLF-safe offsets)

**Files:**
- Modify: `src-tauri/src/okf/write.rs` (place directly after `collect_frontmatter_fence`, ~:168)
- Test: same file, `mod tests`

**Interfaces:**
- Consumes: nothing.
- Produces: `fn split_frontmatter_fence(content: &str) -> Option<(String, usize)>` — `(frontmatter_inner, body_start_byte_offset)`; `None` = no fence (callers treat whole content as body). Task 2's `body_bytes` and the guards rely on this exact signature.

- [ ] **Step 1: Write the failing tests** (in `mod tests`, near the fence-related tests)

```rust
    #[test]
    fn split_fence_lf_content() {
        let content = "---\ntitle: T\n---\nbody here\n";
        let (inner, offset) = split_frontmatter_fence(content).unwrap();
        assert_eq!(inner, "title: T\n");
        assert_eq!(&content[offset..], "body here\n");
    }

    #[test]
    fn split_fence_crlf_content_byte_exact() {
        let content = "---\r\ntitle: T\r\n---\r\nbody here\r\n";
        let (inner, offset) = split_frontmatter_fence(content).unwrap();
        assert_eq!(inner, "title: T\n");
        assert_eq!(&content[offset..], "body here\r\n");
    }

    #[test]
    fn split_fence_none_without_opener() {
        assert!(split_frontmatter_fence("no fence\n").is_none());
    }

    #[test]
    fn split_fence_none_without_closer_within_cap() {
        let mut content = String::from("---\n");
        for i in 0..70 {
            content.push_str(&format!("k{i}: v\n"));
        }
        assert!(split_frontmatter_fence(&content).is_none());
    }

    #[test]
    fn split_fence_body_bytes_helper_matches() {
        // body_bytes is defined in Task 2; this test pins the contract early.
        let content = "---\r\ntitle: T\r\n---\r\n0123456789\r\n";
        assert_eq!(body_bytes(content), "0123456789\r\n".len());
    }
```

- [ ] **Step 2: Run to verify RED**

Run: `cargo test -p curated-thoughts okf::write`
Expected: compile failure (`split_frontmatter_fence` not found).

- [ ] **Step 3: Implement** (after `collect_frontmatter_fence`):

```rust
/// Split `content` into `(frontmatter_inner, body_start_offset)` where
/// `body_start_offset` is the byte offset at which the note body begins
/// (immediately after the closing `---` line). Same fence rules as
/// [`collect_frontmatter_fence`]: exact `---` opener, closing fence within
/// 64 lines, no partial parse on over-cap fences. Returns `None` when there
/// is no fence; callers treat the WHOLE content as body (offset 0).
///
/// The offset is byte-exact on CRLF files — unlike
/// [`collect_frontmatter_fence`], whose `lines()` view drops `\r`, this
/// helper computes offsets on raw bytes, so `content.len() - offset` is the
/// true body byte length (issue #240: the size-drop guard measures bytes).
fn split_frontmatter_fence(content: &str) -> Option<(String, usize)> {
    let mut offset = 0usize;
    if content.as_bytes().starts_with(b"---\r\n") {
        offset = 5;
    } else if content.as_bytes().starts_with(b"---\n") {
        offset = 4;
    } else {
        return None;
    }
    let mut inner = String::new();
    for _ in 0..64 {
        if offset >= content.len() {
            return None;
        }
        let line_end = content[offset..]
            .find('\n')
            .map_or(content.len(), |i| offset + i);
        let line = &content[offset..line_end];
        let trimmed = line.strip_suffix('\r').unwrap_or(line);
        let next = if line_end < content.len() {
            line_end + 1
        } else {
            line_end
        };
        if trimmed == "---" {
            return Some((inner, next));
        }
        inner.push_str(trimmed);
        inner.push('\n');
        offset = next;
    }
    None
}

/// True body byte length of a rendered note: everything after the
/// frontmatter fence, or the whole content when fence-less.
fn body_bytes(content: &str) -> usize {
    match split_frontmatter_fence(content) {
        Some((_, offset)) => content.len() - offset,
        None => content.len(),
    }
}
```

- [ ] **Step 4: Run to verify GREEN**

Run: `cargo test -p curated-thoughts okf::write`
Expected: ALL PASS (existing 56 tests untouched and green).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/okf/write.rs
git commit -m "feat(okf): CRLF-safe frontmatter split helper for byte-exact body measurement (#240)"
```

### Task 2: Error variants + module-contract update

**Files:**
- Modify: `src-tauri/src/okf/mod.rs` (`WriteNoteError` :75-93)
- Modify: `src-tauri/src/okf/write.rs` (module contract doc :16-18; marker const near `NOTE_WRITABLE_ROOTS` :42)

**Interfaces:**
- Produces: `WriteNoteError::ShrinkRefused { existing_bytes: usize, new_bytes: usize }`, `WriteNoteError::CompactionMarkerRejected { marker: String }`, `COMPACTION_MARKERS: &[&str]`, `MIN_GUARDED_BODY_BYTES: usize` — Task 3's guards and tests use these exact names.

- [ ] **Step 1: Write failing Display tests** (in `write.rs` `mod tests`)

```rust
    #[test]
    fn shrink_refused_display_has_pinned_shape_without_allow_shrink_hint() {
        let e = WriteNoteError::ShrinkRefused { existing_bytes: 12860, new_bytes: 505 };
        let s = e.to_string();
        assert!(s.starts_with("shrink_refused:12860:505"), "{s}");
        assert!(s.contains("re-read the note and resend the full body"), "{s}");
        assert!(!s.contains("allow_shrink"), "must not teach the bypass: {s}");
    }

    #[test]
    fn compaction_marker_display_has_pinned_shape_without_allow_shrink_hint() {
        let e = WriteNoteError::CompactionMarkerRejected { marker: "[SKILL_PRUNED]".into() };
        let s = e.to_string();
        assert!(s.starts_with("compaction_marker:[SKILL_PRUNED]"), "{s}");
        assert!(s.contains("rephrase and resend"), "{s}");
        assert!(!s.contains("allow_shrink"), "must not teach the bypass: {s}");
    }
```

- [ ] **Step 2: Run to verify RED**

Run: `cargo test -p curated-thoughts okf::write`
Expected: compile failure (variants don't exist).

- [ ] **Step 3: Add the variants** to `WriteNoteError` (after `StaleUpdate`):

```rust
    /// Issue #240: an If-Match edit shrank the rendered note body to under
    /// half its size — the signature of a truncated payload — and the caller
    /// did not pass `allow_shrink`. The Display carries a machine-readable
    /// prefix plus the recovery instruction; it deliberately does NOT mention
    /// the override flag (a compacted agent must re-read, not retry blindly).
    #[error("shrink_refused:{existing_bytes}:{new_bytes}: re-read the note and resend the full body")]
    ShrinkRefused { existing_bytes: usize, new_bytes: usize },
    /// Issue #240: the payload introduces a context-compaction marker that
    /// was not already present in the note. Compaction debris must never be
    /// persisted; the escape is to rephrase, not to override.
    #[error("compaction_marker:{marker}: rephrase and resend without compaction artifacts")]
    CompactionMarkerRejected { marker: String },
```

- [ ] **Step 4: Add the consts** to `write.rs` next to `NOTE_WRITABLE_ROOTS` (:42):

```rust
/// Context-compaction markers (issue #240): text a truncated agent payload
/// can carry into a note. A create containing any of these is refused; an
/// edit may only introduce a marker the existing content already contains.
pub const COMPACTION_MARKERS: &[&str] = &["[SKILL_PRUNED]", "HERMES-CONTEXT-COMPRESSION"];
/// Edits of notes whose body is smaller than this are never shrink-guarded:
/// a legitimate full rewrite of a small note must stay possible without
/// `allow_shrink` (the incident this guard replays was 12,860 bytes).
pub const MIN_GUARDED_BODY_BYTES: usize = 1024;
```

- [ ] **Step 5: Update the module-contract doc** (:16-18) — append to the pinned-shapes bullet:

```rust
//!   `index_not_found:{path}`, `invalid_entry_name`, `write_error:{io}`,
//!   `shrink_refused:{existing}:{new}: re-read the note and resend the full
//!   body`, `compaction_marker:{marker}: rephrase and resend without
//!   compaction artifacts`. Display strings ARE the contract; see each
//!   variant's `#[error]` for the authoritative shape.
```

(Replace the existing line list accordingly — keep the `existing_unparsable` sentence intact.)

- [ ] **Step 6: Run to verify GREEN**

Run: `cargo test -p curated-thoughts okf::write`
Expected: ALL PASS.

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src/okf/mod.rs src-tauri/src/okf/write.rs
git commit -m "feat(okf): shrink_refused and compaction_marker refusal variants with pinned displays (#240)"
```

### Task 3: The guards in `write_note` + `allow_shrink` plumbing

**Files:**
- Modify: `src-tauri/src/okf/write.rs` (`write_note` :297-303 signature, guard helpers, write tail :432-436)
- Modify: `src-tauri/src/tool_dispatch.rs` (`dispatch_vault_write_note` :287-305, `VaultWriteNoteParams` :1137-1141, call site :1505-1517)
- Modify: `src-tauri/src/lib.rs` (`vault_write_note` :876-895)
- Test: `src-tauri/src/okf/write.rs` `mod tests`; `src-tauri/tests/mcp_write_integration.rs`

**Interfaces:**
- Consumes: `split_frontmatter_fence`/`body_bytes` (Task 1), error variants + consts (Task 2).
- Produces: `write_note(vault_root, path, frontmatter, body, expected_updated_at, allow_shrink: bool)` — 6th positional param. Both adapters updated in this task; no other callers exist.

- [ ] **Step 1: Write the failing guard tests** (in `write.rs` `mod tests`; helpers `vault()`/`fm()` exist at :983/:990). Helper for a long body:

```rust
    fn long_body(lines: usize) -> String {
        (0..lines).map(|i| format!("line {i} of a substantial note body\n")).collect()
    }

    fn edit(target: &str, body: &str, token: &str, allow_shrink: bool) -> Result<super::WriteNoteResult, crate::okf::WriteNoteError> {
        write_note(&root, target, &fm("T", Some(token)), body, Some(token), allow_shrink)
    }
```

(Adapt the closure shape to the existing test style — tests create the file first via `write_note(..., None, false)`, read back the token with `read_existing_token`, then edit.)

```rust
    #[test]
    fn edit_rejects_truncated_payload_replay_of_incident() {
        let (dir, root) = vault();
        let created = write_note(&root, "wiki/n.md", &fm("T", None), &long_body(400), None, false).unwrap();
        let err = write_note(&root, "wiki/n.md", &fm("T", Some(&created.updated_at)), "stub\n", Some(&created.updated_at), false).unwrap_err();
        let s = err.to_string();
        assert!(s.starts_with("shrink_refused:"), "{s}");
        assert!(s.contains("re-read the note"), "{s}");
        assert!(!s.contains("allow_shrink"), "{s}");
    }

    #[test]
    fn edit_boundary_new_double_is_allowed() {
        // existing body B bytes, new body exactly B/2 → allowed (== is allowed).
        let existing_body = "x".repeat(1024); // ≥ MIN_GUARDED_BODY_BYTES, even
        let new_body = "x".repeat(512);
        let (dir, root) = vault();
        let created = write_note(&root, "wiki/n.md", &fm("T", None), &existing_body, None, false).unwrap();
        write_note(&root, "wiki/n.md", &fm("T", Some(&created.updated_at)), &new_body, Some(&created.updated_at), false).unwrap();
    }

    #[test]
    fn edit_boundary_odd_existing_refused() {
        // existing 1025 bytes, new 512 → 512*2 = 1024 < 1025 → refused.
        let existing_body = "x".repeat(1025);
        let new_body = "x".repeat(512);
        let (dir, root) = vault();
        let created = write_note(&root, "wiki/n.md", &fm("T", None), &existing_body, None, false).unwrap();
        let err = write_note(&root, "wiki/n.md", &fm("T", Some(&created.updated_at)), &new_body, Some(&created.updated_at), false).unwrap_err();
        assert!(err.to_string().starts_with("shrink_refused:1025:512"));
    }

    #[test]
    fn small_notes_may_be_fully_rewritten_without_flag() {
        let (dir, root) = vault();
        let created = write_note(&root, "wiki/n.md", &fm("T", None), &"x".repeat(200), None, false).unwrap();
        write_note(&root, "wiki/n.md", &fm("T", Some(&created.updated_at)), "tiny\n", Some(&created.updated_at), false).unwrap();
    }

    #[test]
    fn allow_shrink_permits_major_shrink() {
        let (dir, root) = vault();
        let created = write_note(&root, "wiki/n.md", &fm("T", None), &long_body(400), None, false).unwrap();
        write_note(&root, "wiki/n.md", &fm("T", Some(&created.updated_at)), "deliberate full rewrite\n", Some(&created.updated_at), true).unwrap();
    }

    #[test]
    fn create_with_marker_is_rejected() {
        let (dir, root) = vault();
        let body = "text [SKILL_PRUNED] more text\n";
        let err = write_note(&root, "wiki/n.md", &fm("T", None), body, None, false).unwrap_err();
        assert!(err.to_string().starts_with("compaction_marker:[SKILL_PRUNED]"), "{err}");
    }

    #[test]
    fn edit_rejects_newly_introduced_marker() {
        let (dir, root) = vault();
        let created = write_note(&root, "wiki/n.md", &fm("T", None), "clean body\n", None, false).unwrap();
        let err = write_note(&root, "wiki/n.md", &fm("T", Some(&created.updated_at)), "clean body\nHERMES-CONTEXT-COMPRESSION\n", Some(&created.updated_at), true).unwrap_err();
        assert!(err.to_string().starts_with("compaction_marker:HERMES-CONTEXT-COMPRESSION"), "{err}");
    }

    #[test]
    fn edit_permits_marker_already_in_existing_frontmatter() {
        // The incident note itself: title quotes the marker. Every legitimate
        // edit re-sends that frontmatter — must NOT be locked (spec D2).
        let (dir, root) = vault();
        let mut note = fm("note about [SKILL_PRUNED]", None);
        note.description = Some("quotes HERMES-CONTEXT-COMPRESSION".into());
        let created = write_note(&root, "wiki/n.md", &note, "body one\n", None, false).unwrap();
        write_note(&root, "wiki/n.md", &note, "body two\n", Some(&created.updated_at), false).unwrap();
    }

    #[test]
    fn marker_check_runs_before_shrink_check() {
        let (dir, root) = vault();
        let created = write_note(&root, "wiki/n.md", &fm("T", None), &long_body(400), None, false).unwrap();
        let err = write_note(&root, "wiki/n.md", &fm("T", Some(&created.updated_at)), "[SKILL_PRUNED]\n", Some(&created.updated_at), false).unwrap_err();
        assert!(err.to_string().starts_with("compaction_marker:"), "marker must win: {err}");
    }

    #[test]
    fn rendered_length_is_the_basis_trailing_newline_added() {
        // Body of exactly 1024 bytes WITHOUT a trailing newline renders at
        // 1025 — the guard measures the RENDERED form; existing rendered is
        // 2048 → 1025*2 = 2050 ≥ 2048 → allowed. (Raw-body math would also
        // allow here; the odd-existing pair in the CRLF test pins the exact
        // disagree-by-one case.)
        let existing_body = "x".repeat(2048);
        let new_body = "x".repeat(1024); // renders +1 newline
        let (dir, root) = vault();
        let created = write_note(&root, "wiki/n.md", &fm("T", None), &existing_body, None, false).unwrap();
        write_note(&root, "wiki/n.md", &fm("T", Some(&created.updated_at)), &new_body, Some(&created.updated_at), false).unwrap();
    }

    #[test]
    fn crlf_note_measures_byte_exact_body() {
        let (dir, root) = vault();
        let existing_body = "x".repeat(1100);
        let created = write_note(&root, "wiki/n.md", &fm("T", None), &existing_body, None, false).unwrap();
        let on_disk = std::fs::read_to_string(root.join("wiki/n.md")).unwrap();
        // Convert the stored file to CRLF line endings to simulate a CRLF note.
        let crlf = on_disk.replace('\n', "\r\n");
        std::fs::write(root.join("wiki/n.md"), &crlf).unwrap();
        let token = read_existing_token(&crlf).unwrap();
        // Body shrinks 1100 → 549 (549*2 = 1098 < 1100 → refused). A
        // fence-length mis-measurement (dropping \r bytes) could flip this.
        let new_body = "x".repeat(549);
        let err = write_note(&root, "wiki/n.md", &fm("T", Some(&token)), &new_body, Some(&token), false).unwrap_err();
        assert!(err.to_string().starts_with("shrink_refused:"), "{err}");
    }
```

- [ ] **Step 2: Run to verify RED**

Run: `cargo test -p curated-thoughts okf::write`
Expected: compile failure (`write_note` takes 5 args).

- [ ] **Step 3: Extend the signature + implement the guards.** `write_note` (:297-303) gains a 6th param:

```rust
pub fn write_note(
    vault_root: &Path,
    path: &str,
    frontmatter: &OkfFrontmatter,
    body: &str,
    expected_updated_at: Option<&str>,
    allow_shrink: bool,
) -> Result<WriteNoteResult, WriteNoteError> {
```

Guard helpers (place after `enforce_staleness`):

```rust
/// Issue #240: refuse a rendered document that introduces a context-
/// compaction marker absent from the existing content (creates: any marker).
/// The scan covers frontmatter AND body on both sides — a note quoting a
/// marker in its title must stay editable. `allow_shrink` does NOT bypass
/// this check.
fn enforce_compaction_markers(
    document: &str,
    existing: Option<&str>,
) -> Result<(), WriteNoteError> {
    let already = existing.unwrap_or("");
    for marker in COMPACTION_MARKERS {
        if document.contains(marker) && !already.contains(marker) {
            return Err(WriteNoteError::CompactionMarkerRejected {
                marker: (*marker).to_string(),
            });
        }
    }
    Ok(())
}

/// Issue #240: refuse an edit whose rendered body shrank below half its
/// size — the signature of a truncated payload — unless the caller
/// explicitly opted in. Applies only to existing bodies ≥
/// [`MIN_GUARDED_BODY_BYTES`]; smaller notes may be fully rewritten.
fn enforce_size_drop(
    existing: Option<&str>,
    document: &str,
    allow_shrink: bool,
) -> Result<(), WriteNoteError> {
    let Some(existing) = existing else {
        return Ok(()); // create path — nothing to shrink against
    };
    let existing_bytes = body_bytes(existing);
    if existing_bytes < MIN_GUARDED_BODY_BYTES {
        return Ok(());
    }
    let new_bytes = body_bytes(document);
    if allow_shrink || new_bytes * 2 >= existing_bytes {
        return Ok(());
    }
    Err(WriteNoteError::ShrinkRefused {
        existing_bytes,
        new_bytes,
    })
}
```

Wire into the tail (between `check_round_trip` and `safe_write_bytes`, :433-435):

```rust
    let document = render_document(&effective_fm, body);
    check_round_trip(&effective_fm, &document)?;

    // Issue #240 guards — AFTER render (the measurement basis is the
    // rendered body) and BEFORE any bytes hit disk. Order pinned: marker
    // check first, then shrink.
    enforce_compaction_markers(&document, existing.as_deref())?;
    enforce_size_drop(existing.as_deref(), &document, allow_shrink)?;

    crate::vault::safe_write_bytes(&target, document.as_bytes())
```

Update the earlier create/edit calls inside `write_note` itself (the bootstrap/parent logic at :321+ may call `write_note` recursively — update any internal call sites to pass `allow_shrink` through).

- [ ] **Step 4: Plumb `allow_shrink` through both adapters.**

`tool_dispatch.rs` `dispatch_vault_write_note` (:287-305):

```rust
pub fn dispatch_vault_write_note(
    vault_dir: &Path,
    path: &str,
    frontmatter: &crate::okf::OkfFrontmatter,
    body: &str,
    allow_shrink: bool,
) -> Result<crate::okf::WriteNoteResult> {
    crate::okf::write::write_note(
        vault_dir,
        path,
        frontmatter,
        body,
        frontmatter.updated_at.as_deref(),
        allow_shrink,
    )
    .map_err(|e| anyhow::anyhow!("{}", e))
}
```

`VaultWriteNoteParams` (:1137-1141):

```rust
pub struct VaultWriteNoteParams {
    pub path: String,
    pub frontmatter: crate::okf::OkfFrontmatter,
    pub body: String,
    /// Issue #240: set true ONLY for a deliberate full rewrite that the
    /// size-drop guard would refuse. Defaults to false; the MCP schema
    /// exposes it via schemars.
    #[serde(default)]
    pub allow_shrink: bool,
}
```

Call site (:1513): `dispatch_vault_write_note(&vault_dir, &p.path, &p.frontmatter, &p.body, p.allow_shrink)`.

`lib.rs` Tauri command (:876-895) — `Option<bool>` + unwrap (command params ignore serde defaults):

```rust
#[tauri::command]
fn vault_write_note(
    _conn: State<DbState>,
    vault_root_state: State<VaultConfigState>,
    path: String,
    frontmatter: okf::OkfFrontmatter,
    body: String,
    allow_shrink: Option<bool>,
) -> Result<okf::WriteNoteResult, String> {
    let vault_root = vault_root_from_state(&vault_root_state)?;
    okf::write::write_note(
        &vault_root,
        &path,
        &frontmatter,
        &body,
        frontmatter.updated_at.as_deref(),
        allow_shrink.unwrap_or(false),
    )
    .map_err(|e| e.to_string())
}
```

- [ ] **Step 5: Run the full write suites; audit RED fixtures**

Run: `cargo test -p curated-thoughts okf::write && cargo test -p curated-thoughts --test mcp_write_integration`
Expected: the new tests PASS. Any EXISTING test that edits a ≥1 KiB body down to <half now fails — fix each by (a) passing `allow_shrink: true` where the shrink is intentional fixture setup, or (b) keeping the edit body ≥ half the original. Do NOT weaken the guard. (Survey confirmed `mcp_write_integration.rs` has zero shrinking-body tests today; the short-bodied #119 bootstrap tests are under the floor and unaffected.)

- [ ] **Step 6: Run to verify GREEN (full crate)**

Run: `cargo test -p curated-thoughts`
Expected: ALL PASS.

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src/okf/write.rs src-tauri/src/okf/mod.rs src-tauri/src/tool_dispatch.rs src-tauri/src/lib.rs
git commit -m "feat(okf): size-drop + compaction-marker guards on vault_write_note (#240)"
```

### Task 4: Spec flip + final verification

**Files:**
- Modify: `docs/superpowers/specs/2026-09-28-issue240-shrink-guard-design.md` (status)

- [ ] **Step 1: Flip the spec status** to `**Status:** Implemented 2026-09-28 (PR #248)`.

- [ ] **Step 2: Full local verification (CI parity)**

Run: `cargo test -p curated-thoughts && cargo clippy -p curated-thoughts --all-targets -- -D warnings && pnpm test`
Expected: all green.

- [ ] **Step 3: Commit + push**

```bash
git add docs/superpowers/specs/2026-09-28-issue240-shrink-guard-design.md
git commit -m "docs(spec): mark #240 design implemented (PR #248)"
git push origin feat/issue-240-shrink-guard
```
