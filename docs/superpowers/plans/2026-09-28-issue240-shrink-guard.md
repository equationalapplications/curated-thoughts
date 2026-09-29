# vault_write_note size-drop guard (issue #240) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reject `vault_write_note` edits whose rendered body shrinks below half its size (truncated-payload signature) unless the caller explicitly passes `allow_shrink`, and reject ANY payload introducing a context-compaction marker (creates: any marker at all) — `allow_shrink` does NOT bypass the marker check. Replays the 2026-09-26 incident (12,860→505 bytes) as a refusal.

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
        // body_bytes is implemented in THIS task (Task 1 Step 3); this test pins the contract early.
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
    // Review m1: initialize in ONE expression — `let mut offset = 0usize;`
    // followed by unconditional reassignment trips `unused_assignments`,
    // which is fatal under the repo's clippy -D warnings gate.
    let offset = if content.as_bytes().starts_with(b"---\r\n") {
        5
    } else if content.as_bytes().starts_with(b"---\n") {
        4
    } else {
        return None;
    };
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
- Produces: `write_note(vault_root, path, frontmatter, body, expected_updated_at, allow_shrink: bool)` — 6th positional param. **Callers that break:** both adapters, PLUS every existing test call site — the 56 unit tests in `okf/write.rs` and the four `tests/mcp_write_integration.rs` sites (:836, :867, :881, :904), all 5-arg today. "No other callers exist" was wrong; see Task 3 Step 3b.

- [ ] **Step 1: Write the failing guard tests** (in `write.rs` `mod tests`; helpers `vault()`/`fm()` exist at :983/:990). Helper for a long body:

```rust
    fn long_body(lines: usize) -> String {
        (0..lines).map(|i| format!("line {i} of a substantial note body\n")).collect()
    }

```

(Review M1: the old draft's `fn edit(...)` helper referenced `root` out of scope — a compile error — and was never called; it is deleted. Tests create the file first via `write_note(..., None, false)` (or `fs::write` for marker seeds), read back the token with `read_existing_token`, then edit inline.)

```rust
    #[test]
    fn edit_rejects_truncated_payload_replay_of_incident() {
        // Spec L114-116 exact byte counts (review M3): 12,860 → 505 RENDERED.
        // Bodies are built WITH their trailing newline (render_document adds
        // one only if missing — review R1), so 12859+1 = 12860 on disk,
        // 504+1 = 505 rendered.
        let (_g, root) = vault(); // review M1: dir is unused; `_g` matches existing test style
        let existing_body = format!("{}\n", "x".repeat(12859)); // 12,860 bytes rendered
        let new_body = format!("{}\n", "x".repeat(504));         // 505 bytes rendered
        let created = write_note(&root, "wiki/n.md", &fm("T", None), &existing_body, None, false).unwrap();
        let err = write_note(&root, "wiki/n.md", &fm("T", Some(&created.updated_at)), &new_body, Some(&created.updated_at), false).unwrap_err();
        let s = err.to_string();
        assert!(s.starts_with("shrink_refused:12860:505"), "{s}");
        assert!(s.contains("re-read the note"), "{s}");
        assert!(!s.contains("allow_shrink"), "{s}");
    }

    #[test]
    fn edit_boundary_new_double_is_allowed() {
        // B1 fix: bodies carry their own trailing newline. Rendered sizes:
        // existing 1023+1 = 1024, new 511+1 = 512 → 512*2 == 1024 → ALLOWED
        // (== must pass). (Old draft used 1024/512 raw; render made them
        // 1025/513, so the equality case never actually tested equality.)
        let existing_body = format!("{}\n", "x".repeat(1023)); // 1024 rendered
        let new_body = format!("{}\n", "x".repeat(511));       // 512 rendered
        let (_g, root) = vault(); // review M1: dir is unused; `_g` matches existing test style
        let created = write_note(&root, "wiki/n.md", &fm("T", None), &existing_body, None, false).unwrap();
        write_note(&root, "wiki/n.md", &fm("T", Some(&created.updated_at)), &new_body, Some(&created.updated_at), false).unwrap();
    }

    #[test]
    fn edit_boundary_odd_existing_refused() {
        // B1 fix: rendered sizes existing 1024+1 = 1025, new 511+1 = 512 →
        // 512*2 = 1024 < 1025 → refused with the exact prefix. (Old draft's
        // 1025/512 raw bodies rendered 1026/513 → 513*2 = 1026, NOT < 1026,
        // so unwrap_err() panicked — the exact spec L35-37 trap.)
        let existing_body = format!("{}\n", "x".repeat(1024)); // 1025 rendered
        let new_body = format!("{}\n", "x".repeat(511));       // 512 rendered
        let (_g, root) = vault(); // review M1: dir is unused; `_g` matches existing test style
        let created = write_note(&root, "wiki/n.md", &fm("T", None), &existing_body, None, false).unwrap();
        let err = write_note(&root, "wiki/n.md", &fm("T", Some(&created.updated_at)), &new_body, Some(&created.updated_at), false).unwrap_err();
        assert!(err.to_string().starts_with("shrink_refused:1025:512"));
    }

    #[test]
    fn edit_boundary_plus_one_newline_alone_can_refuse() {
        // Spec L128-130 (review B1): a new body WITHOUT a trailing \n that
        // is refused only because render adds the +1. Existing renders 1025
        // (1024+\n). Raw-new 511 renders 512 → 512*2 = 1024 < 1025 →
        // refused — but raw math on 511 gives the same verdict. To pin the
        // +1 ITSELF, use raw-new 512: raw math says 512*2 = 1024 < 1025
        // (refuse), rendered math says 513*2 = 1026 ≥ 1025 (allow). Rendered
        // wins → the write SUCCEEDS. This test fails if anyone switches the
        // measurement basis to raw bodies.
        let existing_body = format!("{}\n", "x".repeat(1024)); // 1025 rendered
        let new_body = "x".repeat(512);                        // NO newline → renders 513
        let (_g, root) = vault(); // review M1: dir is unused; `_g` matches existing test style
        let created = write_note(&root, "wiki/n.md", &fm("T", None), &existing_body, None, false).unwrap();
        write_note(&root, "wiki/n.md", &fm("T", Some(&created.updated_at)), &new_body, Some(&created.updated_at), false).unwrap();
    }

    #[test]
    fn small_notes_may_be_fully_rewritten_without_flag() {
        let (_g, root) = vault(); // review M1: dir is unused; `_g` matches existing test style
        let created = write_note(&root, "wiki/n.md", &fm("T", None), &"x".repeat(200), None, false).unwrap();
        write_note(&root, "wiki/n.md", &fm("T", Some(&created.updated_at)), "tiny\n", Some(&created.updated_at), false).unwrap();
    }

    #[test]
    fn allow_shrink_permits_major_shrink() {
        let (_g, root) = vault(); // review M1: dir is unused; `_g` matches existing test style
        let created = write_note(&root, "wiki/n.md", &fm("T", None), &long_body(400), None, false).unwrap();
        write_note(&root, "wiki/n.md", &fm("T", Some(&created.updated_at)), "deliberate full rewrite\n", Some(&created.updated_at), true).unwrap();
    }

    #[test]
    fn create_with_marker_is_rejected() {
        let (_g, root) = vault(); // review M1: dir is unused; `_g` matches existing test style
        let body = "text [SKILL_PRUNED] more text\n";
        let err = write_note(&root, "wiki/n.md", &fm("T", None), body, None, false).unwrap_err();
        assert!(err.to_string().starts_with("compaction_marker:[SKILL_PRUNED]"), "{err}");
    }

    #[test]
    fn edit_rejects_newly_introduced_marker() {
        let (_g, root) = vault(); // review M1: dir is unused; `_g` matches existing test style
        let created = write_note(&root, "wiki/n.md", &fm("T", None), "clean body\n", None, false).unwrap();
        let err = write_note(&root, "wiki/n.md", &fm("T", Some(&created.updated_at)), "clean body\nHERMES-CONTEXT-COMPRESSION\n", Some(&created.updated_at), true).unwrap_err();
        assert!(err.to_string().starts_with("compaction_marker:HERMES-CONTEXT-COMPRESSION"), "{err}");
    }

    #[test]
    fn edit_permits_marker_already_in_existing_frontmatter() {
        // B2 fix (review): write_note CREATE refuses every marker — including
        // in the title — so the seed note must go straight to disk via
        // fs::write with a hand-built valid fence. OkfFrontmatter has NO
        // `description` field (fields: okf_version, profile, title,
        // entity_type, tags, created_at, updated_at, supersedes) — the old
        // draft's `note.description = …` was a compile error; the second
        // marker lives in `tags`.
        let (_g, root) = vault(); // review M1: dir is unused; `_g` matches existing test style
        let existing = [
            "---",
            "okf_version: 1",
            "title: \"note about [SKILL_PRUNED]\"",
            "tags: [\"quotes HERMES-CONTEXT-COMPRESSION\"]",
            "created_at: \"2026-09-01T00:00:00Z\"",
            "updated_at: \"2026-09-01T00:00:00Z\"",
            "---",
            "body one",
            "",
        ]
        .join("\n");
        std::fs::create_dir_all(root.join("wiki")).unwrap();
        std::fs::write(root.join("wiki/n.md"), &existing).unwrap();
        let token = read_existing_token(&existing).unwrap();
        // Every legitimate edit re-sends that frontmatter — must NOT be
        // locked (spec D2). Use fm("note about [SKILL_PRUNED]", …) so the new
        // document carries the same markers the existing one has.
        let note = fm("note about [SKILL_PRUNED]", Some(&token));
        write_note(&root, "wiki/n.md", &note, "body two\n", Some(&token), false).unwrap();
    }

    #[test]
    fn edit_rejects_marker_in_existing_body_when_not_resent() {
        // Review B2 addition (spec L119): marker already in the existing
        // BODY, and the edit drops it — allowed, because the guard only
        // refuses NEWLY INTRODUCED markers (document contains ∧ ¬existing
        // contains). Seeded via fs::write for the same create-refusal reason.
        let (_g, root) = vault(); // review M1: dir is unused; `_g` matches existing test style
        let existing = [
            "---",
            "okf_version: 1",
            "title: \"t\"",
            "created_at: \"2026-09-01T00:00:00Z\"",
            "updated_at: \"2026-09-01T00:00:00Z\"",
            "---",
            "text [SKILL_PRUNED] from an old compaction",
            "",
        ]
        .join("\n");
        std::fs::create_dir_all(root.join("wiki")).unwrap();
        std::fs::write(root.join("wiki/n.md"), &existing).unwrap();
        let token = read_existing_token(&existing).unwrap();
        write_note(&root, "wiki/n.md", &fm("t", Some(&token)), "clean replacement\n", Some(&token), false).unwrap();
    }

    #[test]
    fn marker_check_runs_before_shrink_check() {
        let (_g, root) = vault(); // review M1: dir is unused; `_g` matches existing test style
        let created = write_note(&root, "wiki/n.md", &fm("T", None), &long_body(400), None, false).unwrap();
        let err = write_note(&root, "wiki/n.md", &fm("T", Some(&created.updated_at)), "[SKILL_PRUNED]\n", Some(&created.updated_at), false).unwrap_err();
        assert!(err.to_string().starts_with("compaction_marker:"), "marker must win: {err}");
    }

    #[test]
    fn body_bytes_counts_fence_less_content_whole() {
        // Review m6 (spec L131-135): the fence-less path is UNREACHABLE
        // through write_note — enforce_staleness refuses no_fence first — so
        // this pins the helper directly instead of an end-to-end refusal.
        assert_eq!(body_bytes("no fence\n"), 9);
        assert_eq!(body_bytes(""), 0);
    }

    #[test]
    fn params_allow_shrink_omitted_defaults_false_and_true_parses() {
        // Review m7 (spec L121-122): plumbing tests. Key omitted → false.
        let v: serde_json::Value = serde_json::json!({ "path": "wiki/n.md", "frontmatter": {}, "body": "b" });
        let p: crate::tool_dispatch::VaultWriteNoteParams = serde_json::from_value(v).unwrap();
        assert!(!p.allow_shrink);
        let v: serde_json::Value = serde_json::json!({ "path": "wiki/n.md", "frontmatter": {}, "body": "b", "allow_shrink": true });
        let p: crate::tool_dispatch::VaultWriteNoteParams = serde_json::from_value(v).unwrap();
        assert!(p.allow_shrink);
    }

    #[test]
    fn shrink_refusal_reaches_mcp_surface_via_anyhow() {
        // Review m7: a shrink refusal must surface through dispatch's
        // anyhow!("{}") mapping — assert the message survives the mapping
        // and still carries the exact prefix (never "allow_shrink").
        // (Arrange: seed a guarded note; Act: dispatch_vault_write_note with
        // a halved body; Assert: err.to_string() starts_with
        // "shrink_refused:" and does not contain "allow_shrink".)
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
        let (_g, root) = vault(); // review M1: dir is unused; `_g` matches existing test style
        let created = write_note(&root, "wiki/n.md", &fm("T", None), &existing_body, None, false).unwrap();
        write_note(&root, "wiki/n.md", &fm("T", Some(&created.updated_at)), &new_body, Some(&created.updated_at), false).unwrap();
    }

    #[test]
    fn crlf_note_measures_byte_exact_body() {
        let (_g, root) = vault(); // review M1: dir is unused; `_g` matches existing test style
        let existing_body = "x".repeat(1100);
        let created = write_note(&root, "wiki/n.md", &fm("T", None), &existing_body, None, false).unwrap();
        let on_disk = std::fs::read_to_string(root.join("wiki/n.md")).unwrap();
        // Convert the stored file to CRLF line endings to simulate a CRLF note.
        let crlf = on_disk.replace('\n', "\r\n");
        std::fs::write(root.join("wiki/n.md"), &crlf).unwrap();
        let token = read_existing_token(&crlf).unwrap();
        // M4 fix: pin the EXACT error, not just the prefix — a prefix-only
        // assert passes even if fence measurement silently drops \r bytes.
        // Existing: raw 1100-body + \n, whole file converted to CRLF → the
        // renderer-normalized existing body measures 1101 (its trailing
        // newline); rendered new body 549+1 = 550 → 550*2 = 1100 < 1101 →
        // refused. (If the measured pair differs by the CRLF frontmatter
        // bytes, adjust these two numbers — but the assert MUST stay exact,
        // per review R1's render semantics.)
        let new_body = "x".repeat(549);
        let err = write_note(&root, "wiki/n.md", &fm("T", Some(&token)), &new_body, Some(&token), false).unwrap_err();
        let s = err.to_string();
        assert!(s.starts_with("shrink_refused:"), "{s}");
        // Exact-pair assert (guard against silent \r-dropping fence bugs):
        assert_eq!(s.split(':').nth(1), Some("1101"), "{s}");
        assert_eq!(s.split(':').nth(2), Some("550"), "{s}");
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

(Review m2: the bootstrap/parent logic does NOT call `write_note` recursively — it only re-enters `safe_vault_path`. There is no internal recursive call site to update; don't go looking for one.)

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

- [ ] **Step 3b: Update EVERY existing caller (review M2 — the compile-break step).** The signature change breaks the build before any test runs: all 56 existing `write_note` unit tests in `okf/write.rs` and the four `tests/mcp_write_integration.rs` call sites (:836, :867, :881, :904) are 5-arg today. Append `, false` (the old behavior) to every call:

```bash
# mechanical pass — then compile to catch any dispatch_vault_write_note
# direct calls the same way:
rg -n 'write_note\(' src-tauri --type rust
```

Any direct `dispatch_vault_write_note` callers in the integration tests get the new trailing `allow_shrink: false` argument the same way. The crate must COMPILE before Step 5's test run.

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


---

## Review resolutions (pre-implementation, verified against source 2026-09-28)

- **R1 — `render_document` (okf/write.rs:53-62):** appends the body, then ensures exactly one trailing `\n` (only appends if missing). No blank-line insertion between fence and body. Byte math: a body without trailing `\n` gains exactly +1 rendered byte; one ending in `\n` gains 0. All boundary tests above are built with the newline already on.
- **R2 — `WriteNoteResult` (okf/mod.rs:97-104):** fields are `success`, `path`, `sha256`, `updated_at: String` — `created.updated_at` IS valid; no change needed.
- **R3 — `collect_frontmatter_fence` (okf/write.rs:146-169):** opener must be the line `---` (`lines()` strips a trailing `\r`, so CRLF openers pass); the opener does NOT count toward the 64-line `take(64)` cap; no closing fence within the cap → `None`.
- **R4 — `tests/mcp_write_integration.rs`:** all four sites (:836, :867, :881, :904) call the library `write_note(...)` helper directly with 5 args — they break exactly as M2 predicts; Step 3b covers them.
- **m8 (spec text):** the spec's "no normalization step" wording (L52-53, L75-77) is wrong — the body IS normalized to end with exactly one `\n` by `render_document`. Fix the spec text in Task 4's spec flip: "the measurement basis is the RENDERED document, whose body ends with exactly one trailing `\n` (added only if missing)".
