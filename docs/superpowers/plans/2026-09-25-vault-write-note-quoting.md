# vault_write_note Frontmatter Quoting Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the false `Stale update` lockout on `vault_write_note` (issue #231) by quoting free-text frontmatter fields at render time, tolerantly reading the If-Match token from pre-existing broken notes, and guarding every write with a strict round-trip check.

**Architecture:** Three layers of defense in the Rust write path (`src-tauri/src/okf/`): (1) write-path-local quoting (`note_needs_quoting`/`quote_for_note`) applied to `title`/`supersedes` conditionally and tags unconditionally, leaving the shared bundle-export helpers untouched; (2) a mandatory strict-first/tolerant-fallback token reader shared by `enforce_staleness` and `prev_token`, which heals every existing broken note on its next edit; (3) a pre-write round-trip guard (struct equality after empty-tags normalization + rendered key-set check) so no future render gap can land on disk. Plus an error-contract split, `updated_at` in `WriteNoteResult`, a report-only repair scan, and a coordinated `core-okf` TS unescape extension.

**Tech Stack:** Rust (Tauri backend), `serde_yaml 0.9.34` (unsafe-libyaml), `chrono` RFC 3339; TypeScript for the `core-okf` package (repo: `expo-llm-wiki`, `packages/okf`).

**Spec:** `docs/superpowers/specs/2026-09-25-vault-write-note-quoting-design.md` (same branch)

## Global Constraints

- Do NOT modify `needs_quoting`, `quote_string`, `serialize_scalar_string`, or `serialize_key` in `src-tauri/src/okf/frontmatter.rs` — bundle-export byte parity depends on them. All new logic lives in `okf/mod.rs`/`okf/write.rs`.
- Byte pins that must keep passing: `mod.rs:388` (`okf_version: 0.1\n`), `mod.rs:392` (clean tags `tags: ["tag1", "tag2"]`), `write.rs:543` (`starts_with("---\nokf_version: 0.1\n")`), `write.rs:910/945` (unquoted normal `supersedes:` deposit path).
- `okf_version`, `profile`, `entity_type`, `created_at`, `updated_at` are NEVER quoted. Only `title`/`supersedes` (conditional) and tags (always) go through the new escaper.
- Error contract: reuse `WriteNoteError::InvalidFrontmatter(String)` with detail prefixes `existing_unparsable:parse`, `existing_unparsable:no_fence`, `existing_unparsable:no_token`. No new error variant. `StaleUpdate{current}` semantics unchanged for genuinely stale tokens.
- The tolerant token scan MUST: try strict parse first; use the same `---` fence collection incl. `lines.take(64)` cap (`write.rs:61`); require `updated_at:` at column 0; exactly ONE occurrence (duplicates ⇒ unparsable); strip surrounding quotes; value must parse as `chrono::DateTime::parse_from_rfc3339`. One shared function serves `enforce_staleness` AND `prev_token` (`write.rs:267`).
- Notes with no fence or no token stay permanently refused over MCP (manual fix required) — no adoption path.
- Tests must pin probe-verified serde_yaml 0.9.34 behaviors (NOT PyYAML assumptions): bare `&x` ⇒ `title=""`; `#foo`/`!foo` ⇒ `""`; `null`/`~`/`true`/`42` ⇒ literal strings; `*foo` ⇒ hard error; quoted `\N`/`\L`/`\P`/`\xNN`/`\uXXXX` round-trip; BOM U+FEFF parses fine (no escape needed).
- Repo convention: REGULAR merge commits (never squash). Conventional commit messages. TDD throughout: every task writes the failing test first, runs it, implements minimally, re-runs, commits.
- Test commands: `cd src-tauri && cargo test <filter>` (workspace compiles the tauri crate; expect existing warnings — only new failures matter).

---

### Task 1: Write-path quoting helpers (`note_needs_quoting`, `quote_for_note`)

**Files:**
- Modify: `src-tauri/src/okf/mod.rs` (add helpers near `render_frontmatter`, ~line 160)
- Test: `src-tauri/src/okf/mod.rs` (existing `#[cfg(test)] mod tests` at ~line 199)

**Interfaces:**
- Consumes: `frontmatter::needs_quoting` (`frontmatter.rs:69`, currently private — this task makes it `pub(crate)`).
- Produces: `pub(crate) fn note_needs_quoting(value: &str) -> bool`, `pub(crate) fn quote_for_note(value: &str) -> String` (ALWAYS returns a double-quoted, fully escaped YAML scalar incl. surrounding `"`), `fn has_escape_set_char(value: &str) -> bool`, `fn lone_indicator_start(value: &str) -> bool`. Task 2 consumes `quote_for_note` + `note_needs_quoting`; Task 3 consumes nothing new.

- [ ] **Step 1: Make `needs_quoting` visible to the crate**

In `src-tauri/src/okf/frontmatter.rs:69`, change `fn needs_quoting(value: &str) -> bool {` to `pub(crate) fn needs_quoting(value: &str) -> bool {`. Change nothing else in the file.

- [ ] **Step 2: Write the failing tests**

Add to the `mod tests` block in `src-tauri/src/okf/mod.rs`:

```rust
    #[test]
    fn test_note_needs_quoting_timestamp_prefix_with_colon() {
        // The shared needs_quoting early-returns false on its loose
        // is_iso8601_timestamp shape check (frontmatter.rs:76) BEFORE the
        // ':' check at :103 — the write-path predicate must not.
        assert!(note_needs_quoting("2026-09-25T14:00:00Z: deploy retro"));
        assert!(note_needs_quoting("Deploy: retro"));
        assert!(note_needs_quoting("C# tips"));
        assert!(note_needs_quoting("Plan\u{2028}B")); // escape-set char only
        assert!(note_needs_quoting("-"));  // lone leading indicator
        assert!(note_needs_quoting("?"));  // lone leading indicator
        assert!(note_needs_quoting("*foo")); // shared needs_quoting covers
        assert!(note_needs_quoting("[WIP] retry"));
        assert!(note_needs_quoting("2024")); // reserved/number-like via shared
        assert!(note_needs_quoting("yes"));
    }

    #[test]
    fn test_note_needs_quoting_negative_cases() {
        assert!(!note_needs_quoting("Plain title"));
        assert!(!note_needs_quoting("Tessera"));
        assert!(!note_needs_quoting("immutable-source-files/agents/x.md"));
    }

    #[test]
    fn test_quote_for_note_escapes_full_set() {
        assert_eq!(quote_for_note("say \"hi\""), "\"say \\\"hi\\\"\"");
        assert_eq!(quote_for_note("a\\b"), "\"a\\\\b\"");
        assert_eq!(quote_for_note("a\nb"), "\"a\\nb\"");
        assert_eq!(quote_for_note("a\u{85}b"), "\"a\\Nb\"");
        assert_eq!(quote_for_note("a\u{2028}b"), "\"a\\Lb\"");
        assert_eq!(quote_for_note("a\u{2029}b"), "\"a\\Pb\"");
        assert_eq!(quote_for_note("a\u{7f}b"), "\"a\\x7fb\"");
        assert_eq!(quote_for_note("a\u{1}b"), "\"a\\x01b\"");
        assert_eq!(quote_for_note("a\u{fffe}b"), "\"a\\ufffeb\"");
        assert_eq!(quote_for_note("Deploy: retro"), "\"Deploy: retro\"");
    }
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cd src-tauri && cargo test 'quoting::' 2>&1 | tail -20` (one filter per invocation — cargo accepts a single TESTNAME; or run the two filters separately)
Expected: COMPILE ERROR (`note_needs_quoting` / `quote_for_note` not found).

- [ ] **Step 4: Implement the helpers**

In `src-tauri/src/okf/mod.rs`, directly above `render_frontmatter` (~line 160), add:

```rust
/// Characters that YAML double-quoted scalars must escape beyond the
/// ASCII classics: NEL/LS/PS (YAML line breaks) and everything libyaml's
/// reader rejects outright (C0, DEL, other C1, U+FFFE/U+FFFF).
fn has_escape_set_char(value: &str) -> bool {
    value.chars().any(|c| {
        matches!(c, '\u{85}' | '\u{2028}' | '\u{2029}' | '\u{FFFE}' | '\u{FFFF}')
            || c.is_control()
            || c == '\u{7F}'
            || ('\u{80}'..='\u{9F}').contains(&c)
    })
}

/// A leading `-`, `?` or `:` that is alone or followed by a space would be
/// read as a block indicator. The shared needs_quoting only catches these
/// when followed by a space; the lone form is a lockout too (serde_yaml
/// reads `title: -` as a block sequence).
fn lone_indicator_start(value: &str) -> bool {
    let mut chars = value.chars();
    matches!(chars.next(), Some('-' | '?' | ':'))
        && matches!(chars.next(), None | Some(' '))
}

/// Write-path quoting predicate for `title` and `supersedes`.
///
/// NOT the shared `needs_quoting`: that function early-returns false on its
/// loose `is_iso8601_timestamp` shape check (frontmatter.rs:76) BEFORE the
/// `:`/`#` check at :103, so `2026-09-25T14:00:00Z: deploy retro` would slip
/// through unquoted. Here we OR the shared predicate with explicit checks —
/// the shared function stays byte-frozen for bundle export parity.
pub(crate) fn note_needs_quoting(value: &str) -> bool {
    crate::okf::frontmatter::needs_quoting(value)
        || value.contains(':')
        || value.contains('#')
        || has_escape_set_char(value)
        || lone_indicator_start(value)
}

/// Escape `value` for a YAML double-quoted scalar and wrap it in `"`.
/// Covers: `\\ \" \n \r \t`, NEL (`\N`), LS (`\L`), PS (`\P`), remaining
/// C0/DEL as `\xNN`, other C1 and U+FFFE/U+FFFF as `\uXXXX`.
/// supersedes the private frontmatter::quote_string for the NOTE write path
/// only — that one stays frozen (bundle-export parity).
pub(crate) fn quote_for_note(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{85}' => out.push_str("\\N"),
            '\u{2028}' => out.push_str("\\L"),
            '\u{2029}' => out.push_str("\\P"),
            '\u{FFFE}' => out.push_str("\\ufffe"),
            '\u{FFFF}' => out.push_str("\\uffff"),
            c if ('\u{80}'..='\u{9F}').contains(&c) => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c if c.is_control() || c == '\u{7F}' => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cd src-tauri && cargo test 'quoting::' 2>&1 | tail -5`
Expected: 3 tests PASS (also run the full `cargo test` once — the shared `needs_quoting` visibility change must break nothing: `cargo test 2>&1 | tail -3`).

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/okf/frontmatter.rs src-tauri/src/okf/mod.rs
git commit -m "feat(okf): write-path quoting predicates note_needs_quoting/quote_for_note (issue #231)"
```

---

### Task 2: Apply quoting in `render_frontmatter` + byte pins

**Files:**
- Modify: `src-tauri/src/okf/mod.rs:162-187` (`render_frontmatter`)
- Test: `src-tauri/src/okf/mod.rs` (`mod tests`)

**Interfaces:**
- Consumes: `note_needs_quoting`, `quote_for_note` (Task 1).
- Produces: `render_frontmatter` with the new quoting rules — every later task and all byte pins depend on this exact output shape. New helper `fn render_scalar(value: &str) -> String` (conditional quoting for title/supersedes).

- [ ] **Step 1: Write the failing byte-pin tests**

Add to `mod tests` in `mod.rs`:

```rust
    #[test]
    fn test_render_frontmatter_title_quoted_when_needed() {
        let fm = test_fm_with_title("Deploy: retro");
        let doc = render_frontmatter(&fm);
        assert!(doc.contains("title: \"Deploy: retro\"\n"), "got: {doc}");
        // Clean title stays unquoted (byte-identical with old behavior).
        let clean = render_frontmatter(&test_fm_with_title("Test Note"));
        assert!(clean.contains("title: Test Note\n"));
    }

    #[test]
    fn test_render_frontmatter_tags_always_quoted_and_escaped() {
        let mut fm = test_fm_with_title("T");
        fm.tags = Some(vec!["say \"hi\"".to_string(), "ok-tag".to_string()]);
        let doc = render_frontmatter(&fm);
        // ALWAYS quoted (escaper runs even on clean tags — preserves the
        // existing quoted shape) — byte-compatible with mod.rs:392 pin:
        assert!(doc.contains("tags: [\"say \\\"hi\\\"\", \"ok-tag\"]\n"), "got: {doc}");
    }

    #[test]
    fn test_render_frontmatter_new_quote_pins() {
        // Titles containing ':'/'#' anywhere gain quotes on the next write —
        // parse-equivalent, bytes change (accepted, spec §Design.1).
        for t in ["C# tips", "https://example.com", "Ratio 3:1", "2024", "yes"] {
            let doc = render_frontmatter(&test_fm_with_title(t));
            assert!(doc.contains(&format!("title: \"{}\"\n", t)), "{t}: got {doc}");
        }
    }

    fn test_fm_with_title(title: &str) -> OkfFrontmatter {
        OkfFrontmatter {
            okf_version: "0.1".to_string(),
            profile: "llm-wiki/1".to_string(),
            title: title.to_string(),
            entity_type: EntityType::Fact,
            tags: None,
            created_at: "2026-09-25T00:00:00Z".to_string(),
            updated_at: None,
            supersedes: None,
        }
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cd src-tauri && cargo test render_frontmatter 2>&1 | tail -10` (single filter covers all three pins)
Expected: the first two FAIL (assertions on quoting behavior), `test_fm_with_title` undefined compile error may surface first — that is the expected red.

- [ ] **Step 3: Implement in `render_frontmatter`**

In `mod.rs`, above `render_frontmatter` add the conditional wrapper, and change the three render lines:

```rust
/// Conditional quoting for `title` / `supersedes`: quote only when the value
/// needs it, so a normal deposit path or plain title keeps today's bytes
/// (pins write.rs:910/945).
fn render_scalar(value: &str) -> String {
    if note_needs_quoting(value) {
        quote_for_note(value)
    } else {
        value.to_string()
    }
}
```

Then in `render_frontmatter`:
- Line ~166 `doc.push_str(&format!("title: {}\n", fm.title));` becomes
  `doc.push_str(&format!("title: {}\n", render_scalar(&fm.title)));`
- Line ~172 inside the tags loop, `format!("\"{}\"", t)` becomes `quote_for_note(t)` (unconditional — flow context: `,`/`]` are mid-value indicators).
- Line ~183 `doc.push_str(&format!("supersedes: {}\n", fm.supersedes));` becomes
  `doc.push_str(&format!("supersedes: {}\n", render_scalar(fm.supersedes)));`
Leave `okf_version`, `profile`, `entity_type`, `created_at`, `updated_at` lines untouched.

- [ ] **Step 4: Run the FULL test suite — byte pins must hold**

Run: `cd src-tauri && cargo test 2>&1 | tail -8`
Expected: new tests PASS; pre-existing pins (`test_render_frontmatter`, `d1_create_note_writes_frontmatter_and_hash`, `write.rs:543` pin, `910/945` supersedes pins) still PASS. If a pre-existing pin fails, the quoting rule is wrong — fix the rule, never the pin.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/okf/mod.rs
git commit -m "feat(okf): quote title/supersedes conditionally, tags always, in render_frontmatter (issue #231)"
```

---

### Task 3: Pre-write round-trip guard (struct + key-set)

**Files:**
- Modify: `src-tauri/src/okf/write.rs` (`write_note`, guard inserted immediately before the `safe_write_bytes` call — locate it below `fn write_note` at `write.rs:188`; the byte-pin test at `write.rs:543` shows the doc string shape)
- Test: `src-tauri/src/okf/write.rs` (`mod tests`)

**Interfaces:**
- Consumes: `render_document` (write.rs:39), `parse_frontmatter` (`mod.rs:190`), `render_frontmatter` (Task 2 output).
- Produces: `fn verify_round_trip(doc: &str, fm: &OkfFrontmatter) -> Result<(), WriteNoteError>` (private). `write_note` calls it before `safe_write_bytes` and returns its error unmapped. Task 7 relies on its injection rejection.

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `write.rs`:

```rust
    #[test]
    fn round_trip_guard_accepts_valid_render() {
        let fm = crate::okf::test_fm_with_title("Deploy: retro");
        let doc = super::render_document(&fm, "body\n");
        assert!(super::verify_round_trip(&doc, &fm).is_ok());
    }

    #[test]
    fn round_trip_guard_rejects_unknown_key_injection() {
        // A future renderer gap that lets a title split lines must be caught
        // by the key-set check even when all KNOWN fields round-trip fine
        // (OkfFrontmatter has no deny_unknown_fields).
        let fm = crate::okf::test_fm_with_title("T");
        let doc = "---\nokf_version: 0.1\nprofile: llm-wiki/1\ntitle: T\nentity_type: fact\ncreated_at: 2026-09-25T00:00:00Z\nstatus: approved\n---\nbody\n";
        let err = super::verify_round_trip(doc, &fm).unwrap_err();
        assert!(err.to_string().contains("round_trip"), "got: {err}");
    }

    #[test]
    fn round_trip_guard_accepts_empty_tags_list() {
        // render_frontmatter DROPS Some(vec![]) (mod.rs:168-169); the struct
        // compare must normalize before comparing (round-3 MAJOR-1).
        let mut fm = crate::okf::test_fm_with_title("T");
        fm.tags = Some(vec![]);
        let doc = super::render_document(&fm, "");
        assert!(super::verify_round_trip(&doc, &fm).is_ok());
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cd src-tauri && cargo test round_trip_guard 2>&1 | tail -10`
Expected: COMPILE ERROR (`verify_round_trip` not found; `test_fm_with_title` needs `pub(crate)` — if Task 2 left it private in `mod tests`, move it to `#[cfg(test)] pub(crate) fn test_fm_with_title` in `okf/mod.rs` outside the tests module so `write.rs` tests can import it).

- [ ] **Step 3: Implement `verify_round_trip` and wire into `write_note`**

In `write.rs` (near `render_document`):

```rust
/// Pre-write guard: parse the rendered document back and require (a) the
/// resulting frontmatter equals the effective one after normalizing
/// `Some(vec![])` tags to `None` (render drops empty lists, mod.rs:168-169),
/// and (b) the fence contains EXACTLY the rendered key set — the struct
/// compare alone cannot see unknown injected keys (no deny_unknown_fields).
fn verify_round_trip(doc: &str, fm: &OkfFrontmatter) -> Result<(), WriteNoteError> {
    let inner = doc
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---"))
        .map(|(fence, _)| fence)
        .ok_or_else(|| WriteNoteError::InvalidFrontmatter("round_trip: no fence".into()))?;
    let mut parsed: OkfFrontmatter = crate::okf::parse_frontmatter(inner)
        .map_err(|e| WriteNoteError::InvalidFrontmatter(format!("round_trip: {e}")))?;
    // Normalize: empty tag list renders as absent.
    if parsed.tags.as_ref().is_some_and(|t| t.is_empty()) {
        parsed.tags = None;
    }
    let mut expected = fm.clone();
    if expected.tags.as_ref().is_some_and(|t| t.is_empty()) {
        expected.tags = None;
    }
    if parsed != expected {
        return Err(WriteNoteError::InvalidFrontmatter(
            "round_trip: frontmatter mismatch".into(),
        ));
    }
    let yaml: serde_yaml::Mapping = serde_yaml::from_str(inner)
        .map_err(|e| WriteNoteError::InvalidFrontmatter(format!("round_trip: {e}")))?;
    let mut rendered_keys: Vec<String> = vec![
        "okf_version".into(),
        "profile".into(),
        "title".into(),
        "entity_type".into(),
        "created_at".into(),
    ];
    if fm.tags.as_ref().is_some_and(|t| !t.is_empty()) {
        rendered_keys.push("tags".into());
    }
    if fm.updated_at.is_some() {
        rendered_keys.push("updated_at".into());
    }
    if fm.supersedes.is_some() {
        rendered_keys.push("supersedes".into());
    }
    for key in &rendered_keys {
        if !yaml.contains_key(serde_yaml::Value::String(key.clone())) {
            return Err(WriteNoteError::InvalidFrontmatter(format!(
                "round_trip: rendered key missing: {key}"
            )));
        }
    }
    if yaml.len() != rendered_keys.len() {
        return Err(WriteNoteError::InvalidFrontmatter(
            "round_trip: unexpected extra keys".into(),
        ));
    }
    Ok(())
}
```

In `write_note`, immediately before the `safe_write_bytes(...)` call for the NOTE path, insert:

```rust
    verify_round_trip(&doc, &effective_fm)?;
```

(`doc` is the rendered document string; use the exact local names at that call site — read the surrounding code first.)

- [ ] **Step 4: Run full suite**

Run: `cd src-tauri && cargo test 2>&1 | tail -5`
Expected: all PASS, including every pre-existing `write_note` test (the guard must accept every valid legacy render the suite writes — if a legacy test writes an unquoted colon title and now gets `InvalidFrontmatter`, that is the guard working; that specific test gets a follow-up assertion, do not delete it — see Task 7's legacy-fixture test).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/okf/write.rs src-tauri/src/okf/mod.rs
git commit -m "feat(okf): pre-write round-trip guard with key-set check (issue #231)"
```

---

### Task 4: Tolerant If-Match token read (strict-first, shared by staleness + prev_token)

**Files:**
- Modify: `src-tauri/src/okf/write.rs:55-69` (`extract_updated_at`), `write.rs:79-98` (`enforce_staleness`), `write.rs:267` (`prev_token` call site)
- Test: `src-tauri/src/okf/write.rs` (`mod tests`)

**Interfaces:**
- Consumes: `parse_frontmatter` (strict), `chrono::DateTime::parse_from_rfc3339`.
- Produces: `enum TokenReadError { Unparsable, NoFence, NoToken }` (implements Display), `fn read_existing_token(content: &str) -> Result<String, TokenReadError>` (private). `enforce_staleness` maps `Err(e)` → `WriteNoteError::InvalidFrontmatter(format!("existing_unparsable:{}", e))`; `prev_token` maps `Err(_)` → `None` (healing writes skip the strictly-newer floor). Task 5 owns the error TEXTS; Task 7 the differential test.

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `write.rs`:

```rust
    #[test]
    fn token_read_strict_parse_wins_on_clean_fence() {
        let doc = "---\nokf_version: 0.1\nprofile: llm-wiki/1\ntitle: T\nentity_type: fact\ncreated_at: 2026-09-25T00:00:00Z\nupdated_at: 2026-09-25T01:00:00Z # trailing comment\n---\n";
        // Hand-edited trailing comment: strict parser yields the bare value.
        assert_eq!(
            super::read_existing_token(doc).unwrap(),
            "2026-09-25T01:00:00Z"
        );
    }

    #[test]
    fn token_read_tolerant_recovers_from_broken_colon_title() {
        // The issue-#231 shape: unquoted colon title breaks the strict parse.
        let doc = "---\nokf_version: 0.1\nprofile: llm-wiki/1\ntitle: Deploy: retro\nentity_type: fact\ncreated_at: 2026-09-25T00:00:00Z\nupdated_at: 2026-09-25T01:00:00Z\n---\n";
        assert_eq!(
            super::read_existing_token(doc).unwrap(),
            "2026-09-25T01:00:00Z"
        );
    }

    #[test]
    fn token_read_tolerant_rejects_duplicate_or_noncol0_or_bad_rfc3339() {
        let base = "---\nokf_version: 0.1\nprofile: llm-wiki/1\ntitle: a: b\nentity_type: fact\ncreated_at: 2026-09-25T00:00:00Z\n";
        // duplicate updated_at lines
        assert!(matches!(
            super::read_existing_token(&format!("{base}updated_at: 2026-09-25T01:00:00Z\nupdated_at: 2026-09-25T02:00:00Z\n---\n")),
            Err(super::TokenReadError::Unparsable)
        ));
        // key not at column 0
        assert!(matches!(
            super::read_existing_token(&format!("{base}  updated_at: 2026-09-25T01:00:00Z\n---\n")),
            Err(super::TokenReadError::NoToken)
        ));
        // value not RFC 3339
        assert!(matches!(
            super::read_existing_token(&format!("{base}updated_at: not-a-date\n---\n")),
            Err(super::TokenReadError::Unparsable)
        ));
        // no fence at all
        assert!(matches!(
            super::read_existing_token("just some text\n"),
            Err(super::TokenReadError::NoFence)
        ));
    }

    #[test]
    fn token_read_tolerant_obeys_64_line_cap() {
        // Same take(64) cap as the fence collector (write.rs:61): a closing
        // fence beyond 64 lines is treated exactly like the strict path —
        // no fence ⇒ NoFence.
        let mut doc = String::from("---\ntitle: a: b\n");
        for i in 0..70 {
            doc.push_str(&format!("k{i}: v{i}\n"));
        }
        doc.push_str("---\n");
        assert!(matches!(
            super::read_existing_token(&doc),
            Err(super::TokenReadError::NoFence)
        ));
    }

    #[test]
    fn enforce_staleness_reports_unparsable_not_stale() {
        let doc = "---\nokf_version: 0.1\nprofile: llm-wiki/1\ntitle: a: b\nentity_type: fact\ncreated_at: 2026-09-25T00:00:00Z\n---\n";
        let err = super::enforce_staleness(Some(doc), Some("2026-09-25T01:00:00Z")).unwrap_err();
        assert!(
            err.to_string().contains("existing_unparsable"),
            "got: {err}"
        );
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cd src-tauri && cargo test token_read 2>&1 | tail -10` && `cd src-tauri && cargo test enforce_staleness_reports 2>&1 | tail -5`
Expected: COMPILE ERROR (`read_existing_token`, `TokenReadError` not found).

- [ ] **Step 3: Implement `read_existing_token`; rewire `enforce_staleness` + `prev_token`**

Replace `extract_updated_at` (write.rs:55-69) with:

```rust
/// Why an existing note's If-Match token could not be read.
#[derive(Debug)]
enum TokenReadError {
    /// No `---` fence found within the 64-line collection cap.
    NoFence,
    /// Fence found but no usable `updated_at:` token line.
    NoToken,
    /// Duplicate `updated_at:` lines or a non-RFC-3339 value.
    Unparsable,
}

impl std::fmt::Display for TokenReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            TokenReadError::NoFence => "no_fence",
            TokenReadError::NoToken => "no_token",
            TokenReadError::Unparsable => "parse",
        };
        write!(f, "{s}")
    }
}

/// Read the If-Match token from an existing document.
///
/// Strict parse FIRST (so hand-edited values like `updated_at: X # note`
/// behave exactly as today); only on failure, a tolerant line-scan with all
/// of: same fence collection incl. the `lines.take(64)` cap; `updated_at:`
/// at column 0; exactly one occurrence; quotes stripped; RFC 3339 required.
/// ONE function serves enforce_staleness AND prev_token so the two can
/// never disagree (differential test in Task 7).
fn read_existing_token(content: &str) -> Result<String, TokenReadError> {
    let Some((inner, _)) = content
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---"))
    else {
        return Err(TokenReadError::NoFence);
    };
    if let Ok(fm) = crate::okf::parse_frontmatter(inner) {
        if let Some(token) = fm.updated_at {
            return Ok(token);
        }
        return Err(TokenReadError::NoToken);
    }
    // Tolerant fallback (issue #231 healing path).
    let mut hits: Vec<&str> = Vec::new();
    let mut closed = false;
    let mut lines = content.lines();
    if lines.next() != Some("---") {
        return Err(TokenReadError::NoFence);
    }
    for line in lines.take(64) {
        if line == "---" {
            closed = true;
            break;
        }
        if let Some(rest) = line.strip_prefix("updated_at:") {
            hits.push(rest.trim());
        }
    }
    if !closed {
        // Same take(64) cap as the fence collector: no closing fence within
        // the cap is "no fence" (matches the strict path's view).
        return Err(TokenReadError::NoFence);
    }
    if hits.len() != 1 {
        return Err(if hits.is_empty() {
            TokenReadError::NoToken
        } else {
            TokenReadError::Unparsable // duplicates: refuse to pick
        });
    }
    let raw = hits[0].trim();
    let unquoted = raw
        .strip_prefix('"')
        .and_then(|r| r.strip_suffix('"'))
        .or_else(|| raw.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')))
        .unwrap_or(raw);
    chrono::DateTime::parse_from_rfc3339(unquoted)
        .map(|_| unquoted.to_string())
        .map_err(|_| TokenReadError::Unparsable)
}
```

`enforce_staleness` (write.rs:79-98) becomes:

```rust
fn enforce_staleness(
    existing_content: Option<&str>,
    expected_updated_at: Option<&str>,
) -> Result<(), WriteNoteError> {
    let Some(content) = existing_content else {
        return Ok(()); // create path — nothing to be stale against
    };
    let current = read_existing_token(content).map_err(|e| {
        WriteNoteError::InvalidFrontmatter(format!("existing_unparsable:{}", e))
    })?;
    match expected_updated_at {
        Some(expected) if expected == current => Ok(()),
        _ => Err(WriteNoteError::StaleUpdate { updated_at: current }),
    }
}
```

At the `prev_token` call site (`write.rs:267`), replace the `extract_updated_at(...)` call with `read_existing_token(...).ok()` and delete the now-dead `extract_updated_at`. (On a broken fence the healing write proceeds with no floor — that is the intended healing behavior.)

- [ ] **Step 4: Run full suite**

Run: `cd src-tauri && cargo test 2>&1 | tail -5`
Expected: all PASS. The stale-update tests at `write.rs:560/573` must still pass (clean fences → strict path, unchanged behavior).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/okf/write.rs
git commit -m "feat(okf): strict-first tolerant If-Match token read, shared by staleness and prev_token (issue #231)"
```

---

### Task 5: Error contract + `updated_at` in `WriteNoteResult` + docs/description updates

**Files:**
- Modify: `src-tauri/src/okf/mod.rs:87-92` (`WriteNoteResult`), `src-tauri/src/okf/write.rs:16-18` (pinned error-shape doc comment), `src-tauri/src/mcp_server.rs:128` (tool description), `src-tauri/src/tool_dispatch.rs:287-304` (`dispatch_vault_write_note` result), `docs/spec/` spec-v2 file (locate via `grep -rn "stale_update" docs/`)
- Test: `src-tauri/src/okf/write.rs`, `src-tauri/src/tool_dispatch.rs`

**Interfaces:**
- Consumes: `read_existing_token` (Task 4), `write_note` return path.
- Produces: `WriteNoteResult { success: bool, path: String, sha256: String, updated_at: String }` — additive field; MCP JSON gains `updated_at`. No other task consumes this.

- [ ] **Step 1: Write the failing test**

In `write.rs` `mod tests` (the existing `d1_create_note_writes_frontmatter_and_hash` at ~line 530 asserts the result shape — extend it):

```rust
    #[test]
    fn write_note_result_carries_fresh_token() {
        let (result, _) = write_note_into_tempdir("Deploy: retro", None); // same helper shape as d1 test
        assert_eq!(result.success, true);
        assert!(!result.updated_at.is_empty());
        chrono::DateTime::parse_from_rfc3339(&result.updated_at).unwrap();
    }
```

(Adapt to the actual d1 test's setup helper — read `write.rs:530-560` first and mirror its tempdir/vault pattern exactly.)

- [ ] **Step 2: Run to verify failure**

Run: `cd src-tauri && cargo test write_note_result_carries 2>&1 | tail -6`
Expected: COMPILE ERROR (no field `updated_at`).

- [ ] **Step 3: Implement**

1. `mod.rs:87-92`: add `pub updated_at: String,` to `WriteNoteResult` (after `sha256`).
2. In `write_note`'s success return: set `updated_at` to the token written into the frontmatter (the value assigned to `fm.updated_at` on the create/edit paths — read the code; it is the `now`-derived RFC 3339 string).
3. `write.rs:16-18` doc comment: document the new contract — `stale_update:{current}` unchanged; `invalid_frontmatter` with `existing_unparsable:parse|no_fence|no_token` details; note that no-fence/no-token notes stay permanently refused over MCP.
4. `mcp_server.rs:128` tool description: correct the error text promise to match `mod.rs:80` actual message and mention `existing_unparsable:*` + the new `updated_at` result field.
5. spec-v2 doc (`grep -rn "stale_update" docs/` to find it): add the error-contract amendment section mirroring step 3.

- [ ] **Step 4: Run full suite + tool_dispatch tests**

Run: `cd src-tauri && cargo test 2>&1 | tail -5`
Expected: all PASS. Any JSON-shape test in `tool_dispatch.rs` tests asserting the old result shape gets the added field asserted, not removed.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/okf/mod.rs src-tauri/src/okf/write.rs src-tauri/src/mcp_server.rs src-tauri/src/tool_dispatch.rs docs/
git commit -m "feat(okf): existing_unparsable error contract + updated_at in WriteNoteResult (issue #231)"
```

---

### Task 6: Report-only repair scan

**Files:**
- Create: `src-tauri/src/okf/repair_scan.rs`
- Modify: `src-tauri/src/okf/mod.rs` (add `pub mod repair_scan;`), `src-tauri/src/lib.rs` (one `#[tauri::command]` wrapper near `vault_write_note` at ~line 877; register it in the invoke handler list)
- Test: `src-tauri/src/okf/repair_scan.rs` (`mod tests`)

**Interfaces:**
- Consumes: `read_existing_token` (Task 4) for classification.
- Produces: `pub struct UnparsableNote { pub path: String, pub reason: String }`, `pub fn scan_unparsable_notes(vault_root: &Path) -> Vec<UnparsableNote>` — REPORT-ONLY; writes nothing. Tauri command `scan_unparsable_notes` returns the vec as JSON.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn scan_reports_broken_and_clean_notes_correctly() {
        let tmp = tempfile::tempdir().unwrap();
        let wiki = tmp.path().join("wiki");
        std::fs::create_dir_all(&wiki).unwrap();
        // broken (colon title, old format)
        std::fs::write(
            wiki.join("broken.md"),
            "---\nokf_version: 0.1\nprofile: llm-wiki/1\ntitle: a: b\nentity_type: fact\ncreated_at: 2026-09-25T00:00:00Z\nupdated_at: 2026-09-25T01:00:00Z\n---\nbody\n",
        ).unwrap();
        // clean
        std::fs::write(
            wiki.join("clean.md"),
            "---\nokf_version: 0.1\nprofile: llm-wiki/1\ntitle: Fine\nentity_type: fact\ncreated_at: 2026-09-25T00:00:00Z\nupdated_at: 2026-09-25T02:00:00Z\n---\nbody\n",  # clean fixture MUST carry a token (no-token => reported as existing_unparsable:no_token — CodeRabbit catch)
        # 
        ).unwrap();
        let hits = scan_unparsable_notes(tmp.path());
        assert_eq!(hits.len(), 1);
        assert!(hits[0].path.ends_with("broken.md"));
    }
}
```

(`tempfile` is already a dev-dependency — see existing `write.rs` tests.)

- [ ] **Step 2: Run to verify failure**

Run: `cd src-tauri && cargo test scan_reports 2>&1 | tail -6`
Expected: COMPILE ERROR (module missing).

- [ ] **Step 3: Implement**

`repair_scan.rs`: walk `wiki/` and `immutable-source-files/agents/` (re-use `walkdir` as `src-tauri/src/walk_vault.rs:17` does; only `.md` files; skip nothing else), read each file, call `read_existing_token` (make it `pub(crate)` in Task 4), and on `Err(e)` push `UnparsableNote { path, reason: format!("existing_unparsable:{}", e) }`. Sort by path. The function NEVER writes. Register the module + Tauri command; log a `tracing::info!` line with the hit count at the end of the scan when invoked.

- [ ] **Step 4: Run to verify pass + full suite**

Run: `cd src-tauri && cargo test scan_reports 2>&1 | tail -3 && cargo test 2>&1 | tail -3`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/okf/repair_scan.rs src-tauri/src/okf/mod.rs src-tauri/src/lib.rs
git commit -m "feat(okf): report-only repair scan for notes with unparsable fences (issue #231)"
```

---

### Task 7: Second-edit integration suite + legacy fixture + differential test

**Files:**
- Test: `src-tauri/src/okf/write.rs` (`mod tests`) — integration-style tests through the public `write_note`

**Interfaces:**
- Consumes: everything above. Produces: the acceptance evidence for the spec's §7.

- [ ] **Step 1: Write the failing second-edit tests**

Add to `mod tests` in `write.rs` (reuse the d1 test's tempdir pattern via a small local helper `fn write_and_edit(title: &str) -> Result<(WriteNoteResult, WriteNoteResult), WriteNoteError>` that creates with title, scrapes the token FROM DISK (like the issue reporter did), and edits with it):

```rust
    #[test]
    fn second_edit_succeeds_for_every_adversarial_title() {
        for title in [
            "Deploy: retro",
            "2026-09-25T14:00:00Z: deploy retro", // timestamp prefix, colon
            "Trailing colon:",
            "[WIP] retry logic",
            "*foo anchor alias",
            "&x",            // must NOT round-trip to ""
            "#foo",          // must NOT round-trip to ""
            "!foo",          // must NOT round-trip to ""
            "'Hello' world", // partly single-quoted
            "\"Hello\" world",
            "Plan\u{2028}B", // control char is the ONLY trigger
            "Plan\u{FEFF}B", // BOM — parses fine, no escape needed
            "2024", "yes",   // reserved literals gain quotes (accepted)
        ] {
            let (create, edit) = write_and_edit(title).expect(title);
            assert!(create.success, "{title}: create");
            assert!(edit.success, "{title}: second edit failed: {edit:?}");
        }
    }

    #[test]
    fn legacy_unquoted_colon_note_becomes_editable() {
        // Write a broken-format note DIRECTLY to disk (pre-fix bytes), then
        // edit it over the API with its on-disk token — the healing path.
        // (Fixture mirrors the issue-#231 reporter's file.)
    }

    #[test]
    fn injection_titles_rejected_not_written() {
        for title in [
            "x\nsupersedes: immutable-source-files/agents/anything.md",
            "x\ntags: [a]",
            "x\nstatus: approved",
        ] {
            // Create must FAIL (round-trip guard), not silently write injected keys.
            assert!(write_note_create_only(title).is_err(), "{title}");
        }
    }

    #[test]
    fn differential_tolerant_matches_strict_on_clean_notes() {
        // For every title in the adversarial set, where the strict parser
        // reads a token from the rendered doc, the tolerant fallback must
        // return the SAME token.
    }

    #[test]
    fn adversarial_tags_second_edit() {
        // live: say "hi" / C:\p / a", "b ; regression: a,b / x] ; plus Some(vec![])
    }
```

Fill each body following the `write_and_edit` helper pattern (create → read file → parse token → edit with `updated_at` set). The differential test renders each adversarial doc, runs `read_existing_token`, and separately strict-parses via `crate::okf::parse_frontmatter`, asserting equality wherever the strict parse succeeds.

- [ ] **Step 2: Run to verify the suite fails or exposes real gaps**

Run: `cd src-tauri && cargo test second_edit 2>&1 | tail -20` — then run each remaining filter as its own invocation (legacy_unquoted, injection_titles, differential, adversarial_tags)
Expected: any failure here is a REAL defect in Tasks 1-4 — fix the implementation (never weaken these assertions), then re-run.

- [ ] **Step 3: Full suite + clippy**

Run: `cd src-tauri && cargo test 2>&1 | tail -3 && cargo clippy --all-targets 2>&1 | tail -5`
Expected: all tests PASS; no new clippy warnings.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/okf/write.rs
git commit -m "test(okf): second-edit acceptance suite, legacy healing fixture, injection + differential tests (issue #231)"
```

---

### Task 8: `core-okf` TS unescape extension + version bump (separate repo, coordinated)

**Files:**
- Modify: `~/code/github/equationalapplications/expo-llm-wiki/packages/okf/src/` (the TS source implementing `unescapeFrontmatterString` — currently handles only `\\ \" \n \r \t`; find the source file, NOT `dist/`)
- Modify: `~/code/github/equationalapplications/curated-thoughts/package.json` (bump `@equationalapplications/core-okf` once the TS PR publishes)

**Interfaces:**
- Consumes: the exact escape forms emitted by Task 1's `quote_for_note`: `\\ \" \n \r \t \N \L \P \xNN \uXXXX`.
- Produces: `unescapeFrontmatterString` decoding all of them; TS display tests proving a quoted colon title and each escape round-trip.

- [ ] **Step 1: Write failing TS tests** in `packages/okf` mirroring Task 1's escape table (Vitest/Jest — match the package's existing runner).

- [ ] **Step 2: Extend the unescape switch**: `\\n \\r \\t` exist; add `\\N` → U+0085, `\\L` → U+2028, `\\P` → U+2029, `\\xNN` (2 hex digits) → code point, `\\uXXXX` (4 hex digits) → code point; unknown escape → keep both characters literally (matches the current lenient behavior for unknown pairs).

- [ ] **Step 3: Run TS tests; version bump the package; open the expo-llm-wiki PR** (REGULAR merge policy; link both PRs to each other).

- [ ] **Step 4: After the expo-llm-wiki PR merges and publishes, bump CT**: `pnpm up @equationalapplications/core-okf@<new>` in CT; commit `chore(deps): core-okf <version> for issue #231 escape parity` on this branch. (If the release train is not ready, leave this step as the documented final commit — do NOT fake a bump against an unpublished version.)

- [ ] **Step 5: Commit/push**

```bash
git add package.json pnpm-lock.yaml
git commit -m "chore(deps): bump core-okf for issue #231 frontend escape parity"
```

---

## Final verification (after all tasks)

- [ ] `cd src-tauri && cargo test 2>&1 | tail -3` — full suite green
- [ ] `cargo clippy --all-targets 2>&1 | tail -3` — no new warnings
- [ ] Live end-to-end: build the sidecar binary, reproduce the original issue-#231 scenario over raw stdio JSON-RPC (create colon-titled note → scrape token → edit with token) and confirm SUCCESS; then run the repair scan command against a vault containing the legacy broken fixture and confirm it REPORTS (not heals) it.
- [ ] Push branch; confirm PR #232 checks green (`gh pr checks 232`); request review.
