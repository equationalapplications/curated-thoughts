# Frontmatter Key-Drop Guard (issue #245) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refuse `vault_write_note` If-Match edits that silently drop frontmatter keys the existing note carries, unless the caller passes an explicit `allow_key_drop` confirmation.

**Architecture:** A new guard `enforce_key_preservation` in `src-tauri/src/okf/write.rs::write_note` compares the EXISTING file's frontmatter key set against the key set the INCOMING struct renders. Existing keys come from `existing_frontmatter_keys`, which reads the same fence view the token reader uses. It uses a strict `serde_yaml::Value` mapping parse and falls back to a column-0 line scan for damaged YAML. Dropped keys are split into KNOWN keys (`KeyDropRefused`) and UNKNOWN keys (`KeyDropUnrepresentable`), and the KNOWN partition is reported first. The bypass flag is plumbed exactly like #240's `allow_shrink` (MCP params, `tool_dispatch`, Tauri `Option<bool>`).

**Tech Stack:** Rust (`src-tauri` crate `curated-thoughts`, lib `tauri_app_lib`), `serde_yaml` 0.9, `thiserror`, `schemars` (feature `mcp-server`), `rmcp`.

**Spec:** `docs/superpowers/specs/2026-10-01-issue245-frontmatter-key-drop-guard-design.md` (read it alongside this plan; decisions are cited as D1–D6).

## Global Constraints

- Scope is `src-tauri` only, plus the spec and this plan. No frontend change (spec: "Frontend UX for the refusal — out of scope").
- Display strings are the contract. Pin these verbatim (D5):
  - `key_drop_refused:{keys}: re-send the complete frontmatter or pass an explicit key-drop confirmation`
  - `key_drop_refused:unrepresentable:{keys}: this note carries keys the writer cannot re-emit; migrate the note to the OKF schema outside this tool, or pass an explicit key-drop confirmation`
  - `{keys}` = the sorted key names joined with `,` (no spaces).
- Neither refusal Display nor the MCP tool description may contain the string `allow_key_drop` (D4/D5 — a compacted agent must not learn the bypass from the error).
- Check order in `write_note` (D6): `enforce_staleness` → token rotation → `render_document` → `check_round_trip` → `enforce_compaction_markers` → `enforce_key_preservation` → `enforce_size_drop` → `safe_write_bytes`.
- `updated_at` is exempt from the comparison on both sides (D3).
- `KNOWN_KEYS` exists exactly once, at module scope in `write.rs`, pre-sorted ascending (Problem §).
- Every test call to `write_note` / `dispatch_vault_write_note` passes the two guard flags as NAMED values, never literals (D4). See the plan-vs-spec note in Task 3.
- CI gates (`.github/workflows/ci.yml`), all run from the repo root:
  - `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check`
  - `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --features test-utils -- -D warnings`
  - `cargo test --manifest-path src-tauri/Cargo.toml --features test-utils,mcp-server -- --test-threads=1`
- Commits end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Never squash-merge (repo rule).

## File Structure

| File | Change | Responsibility |
|---|---|---|
| `src-tauri/src/okf/write.rs` | Modify | `KNOWN_KEYS` hoist; `rendered_key_set`; `existing_frontmatter_keys` + line-scan helpers; `enforce_key_preservation`; `write_note` signature + wiring; module-header contract list; all unit + end-to-end tests |
| `src-tauri/src/okf/mod.rs` | Modify | Two `WriteNoteError` variants with pinned Displays |
| `src-tauri/src/tool_dispatch.rs` | Modify | `VaultWriteNoteParams.allow_key_drop`; `dispatch_vault_write_note` param; dispatch arm |
| `src-tauri/src/lib.rs` | Modify | Tauri `vault_write_note` command `allow_key_drop: Option<bool>` |
| `src-tauri/src/mcp_server.rs` | Modify | `vault_write_note` tool description gains both refusals |
| `src-tauri/tests/mcp_write_integration.rs` | Modify | 4 direct `write_note` call sites gain the new argument (named consts) |
| `docs/superpowers/specs/2026-10-01-issue245-frontmatter-key-drop-guard-design.md` | Modify | D4 named-consts note; Status → implemented |

All new tests go at the END of `mod tests` in `write.rs`, under the section comment `// ---- issue #245: frontmatter key-drop guard ----`. That keeps them beside the #240 guard tests they mirror.

---

### Task 1: Key-set helpers (`KNOWN_KEYS` hoist, `rendered_key_set`, `existing_frontmatter_keys`)

**Files:**
- Modify: `src-tauri/src/okf/write.rs` (module top ~L48-55; `collect_frontmatter_fence` ~L160-181; `check_round_trip` ~L606-680; `mod tests` end)

**Interfaces:**
- Consumes: `collect_frontmatter_fence(content: &str) -> Option<String>` (existing).
- Produces (all private to `okf::write`):
  - `const KNOWN_KEYS: [&str; 8]`, sorted ascending.
  - `fn rendered_key_set(fm: &OkfFrontmatter) -> Vec<&'static str>`: the keys the renderer emits for `fm`, in `KNOWN_KEYS` order.
  - `fn existing_frontmatter_keys(content: &str) -> Option<BTreeSet<String>>`: `None` iff there is no fence.
  - `fn line_scan_key(line: &str) -> Option<(String, &str)>`: (key, raw rest-after-colon) for a tier-3 key line.
  - Test fixtures `kd_doc(extra: &[&str], damaged: bool) -> String` and `kd_keys(doc: &str) -> BTreeSet<String>`. Task 2 and Task 3 reuse them.

- [ ] **Step 1: Write the failing tests**

Append to the end of `mod tests` in `src-tauri/src/okf/write.rs`, before the final closing `}`:

```rust
    // ---- issue #245: frontmatter key-drop guard ----

    /// Well-formed base frontmatter for key-drop fixtures. Index 2 is the
    /// title line, swapped for an unquoted-colon title to DAMAGE the YAML
    /// (strict parse fails; the token reader's tolerant fallback still
    /// reads `updated_at`).
    const KD_BASE: &[&str] = &[
        "okf_version: \"0.1\"",
        "profile: llm-wiki/1",
        "title: T",
        "entity_type: fact",
        "created_at: \"2026-08-27T00:00:00Z\"",
        "updated_at: \"2026-09-25T01:00:00Z\"",
    ];

    /// Build a note: `KD_BASE` (title damaged when `damaged`) + `extra` lines.
    /// Asserts the fixture's damage flag matches reality, so a "damaged"
    /// test can never silently run on the strict tier (or vice versa).
    fn kd_doc(extra: &[&str], damaged: bool) -> String {
        let mut lines: Vec<&str> = KD_BASE.to_vec();
        if damaged {
            lines[2] = "title: Deploy: retro";
        }
        lines.extend_from_slice(extra);
        let doc = format!("---\n{}\n---\nbody\n", lines.join("\n"));
        let inner = collect_frontmatter_fence(&doc).expect("fixture has a fence");
        assert_eq!(
            serde_yaml::from_str::<serde_yaml::Value>(&inner).is_err(),
            damaged,
            "fixture damage flag must match the strict parse: {doc}"
        );
        doc
    }

    fn kd_keys(doc: &str) -> BTreeSet<String> {
        existing_frontmatter_keys(doc).expect("fixture has a fence")
    }

    #[test]
    fn known_keys_is_sorted_ascending() {
        let mut sorted = KNOWN_KEYS;
        sorted.sort_unstable();
        assert_eq!(sorted, KNOWN_KEYS);
    }

    #[test]
    fn rendered_key_set_omits_none_and_empty_optionals() {
        let mut m = fm("T", None);
        m.tags = Some(vec![]);
        assert_eq!(
            rendered_key_set(&m),
            vec!["created_at", "entity_type", "okf_version", "profile", "title"]
        );
        let mut m = fm("T", Some("2026-09-25T01:00:00Z"));
        m.supersedes = Some("immutable-source-files/agents/v1.md".to_string());
        assert_eq!(rendered_key_set(&m), KNOWN_KEYS.to_vec());
    }

    #[test]
    fn existing_keys_strict_tier_full_set() {
        let keys = kd_keys(&kd_doc(&["tags: [a]"], false));
        let expected: BTreeSet<String> = [
            "created_at",
            "entity_type",
            "okf_version",
            "profile",
            "tags",
            "title",
            "updated_at",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(keys, expected);
    }

    #[test]
    fn existing_keys_strict_tier_known_absent_forms() {
        // D2: the exhaustive ABSENT list for the KNOWN optional fields.
        for (line, key) in [
            ("tags:", "tags"),
            ("tags: null", "tags"),
            ("tags: ~", "tags"),
            ("tags: []", "tags"),
            ("tags: [] # none", "tags"),
            ("supersedes:", "supersedes"),
            ("supersedes: null", "supersedes"),
            ("supersedes: ~", "supersedes"),
            ("supersedes: \"\"", "supersedes"),
        ] {
            let keys = kd_keys(&kd_doc(&[line], false));
            assert!(!keys.contains(key), "{line:?} must count ABSENT: {keys:?}");
        }
    }

    #[test]
    fn existing_keys_strict_tier_present_forms() {
        // D2: any other value form counts PRESENT; unknown keys always do.
        for (line, key) in [
            ("tags: \"\"", "tags"),
            ("tags: foo", "tags"),
            ("tags: {}", "tags"),
            ("supersedes: immutable-source-files/agents/v1.md", "supersedes"),
            ("aliases: []", "aliases"),
            ("aliases: null", "aliases"),
        ] {
            let keys = kd_keys(&kd_doc(&[line], false));
            assert!(keys.contains(key), "{line:?} must count PRESENT: {keys:?}");
        }
    }

    #[test]
    fn existing_keys_strict_tier_non_string_key_is_collected_not_rejected() {
        // D1 (review 2026-10-02): unlike check_round_trip's reject loop, the
        // EXISTING side collects a non-string key (Debug form) and never errors.
        let keys = kd_keys(&kd_doc(&["1: x"], false));
        let debug = format!("{:?}", serde_yaml::Value::Number(1.into()));
        assert!(keys.contains(&debug), "{debug} missing from {keys:?}");
    }

    #[test]
    fn existing_keys_line_scan_tier() {
        // Damaged YAML → tier 3. Title still extracted (key before FIRST ':').
        let keys = kd_keys(&kd_doc(&[], true));
        assert!(keys.contains("title") && keys.contains("updated_at"), "{keys:?}");

        // Inline absent forms for known optionals count ABSENT…
        for line in ["tags: []", "tags:", "tags: null", "tags: ~", "supersedes: \"\""] {
            let keys = kd_keys(&kd_doc(&[line], true));
            let key = line.split(':').next().unwrap();
            assert!(!keys.contains(key), "{line:?} must count ABSENT: {keys:?}");
        }
        // …unless continued on the next line (block sequence stays PRESENT).
        let keys = kd_keys(&kd_doc(&["tags:", "  - a"], true));
        assert!(keys.contains("tags"), "indented continuation: {keys:?}");
        let keys = kd_keys(&kd_doc(&["tags:", "- a"], true));
        assert!(keys.contains("tags"), "`- ` continuation: {keys:?}");
        // Non-empty inline values, and the accepted trailing-comment divergence.
        for line in ["tags: foo", "tags: \"\"", "tags: [] # none", "tags: [a]"] {
            let keys = kd_keys(&kd_doc(&[line], true));
            assert!(keys.contains("tags"), "{line:?} must count PRESENT: {keys:?}");
        }
        // Unknown keys are PRESENT whatever their value.
        let keys = kd_keys(&kd_doc(&["aliases: []"], true));
        assert!(keys.contains("aliases"), "{keys:?}");
        // Nested block mapping: parent key PRESENT, indented child is not a key.
        let keys = kd_keys(&kd_doc(&["source:", "  url: https://x"], true));
        assert!(keys.contains("source") && !keys.contains("url"), "{keys:?}");
        // D1 (d): no character-class restriction.
        let keys = kd_keys(&kd_doc(&["1: x", "my-key: y"], true));
        assert!(keys.contains("1") && keys.contains("my-key"), "{keys:?}");
    }

    #[test]
    fn line_scan_key_extraction_rule() {
        // D1 (d): column-0, not whitespace/#/-, contains ':'; key = text
        // before the FIRST ':', trimmed, one matching quote pair stripped.
        assert_eq!(line_scan_key("\"quoted\": v").map(|(k, _)| k), Some("quoted".to_string()));
        assert_eq!(line_scan_key("'single': v").map(|(k, _)| k), Some("single".to_string()));
        assert_eq!(line_scan_key("a: b: c").map(|(k, r)| (k, r.trim())), Some(("a".to_string(), "b: c")));
        assert_eq!(line_scan_key("some key: v").map(|(k, _)| k), Some("some key".to_string()));
        for not_a_key in ["# comment: x", "- item: x", "  indented: x", "\tindented: x", "", "no colon here"] {
            assert!(line_scan_key(not_a_key).is_none(), "{not_a_key:?}");
        }
    }

    #[test]
    fn existing_keys_fence_less_is_none() {
        // Unreachable through write_note (enforce_staleness refuses no_fence
        // first); pinned as defense-in-depth.
        assert!(existing_frontmatter_keys("no fence\n").is_none());
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --features test-utils,mcp-server --lib okf::write::tests -- --test-threads=1`
Expected: compile FAIL. `KNOWN_KEYS`, `rendered_key_set`, `existing_frontmatter_keys`, `line_scan_key` and `BTreeSet` are not found.

- [ ] **Step 3: Implement**

3a. Add the import below `use std::path::{Component, Path};` near the top of `write.rs`:

```rust
use std::collections::BTreeSet;
```

3b. Add the module-scope `KNOWN_KEYS` right after `MIN_GUARDED_BODY_BYTES` (~L55):

```rust
/// Every frontmatter key the typed `OkfFrontmatter` can render. ONE list,
/// shared by `check_round_trip`, [`rendered_key_set`] and the issue #245
/// key-drop guard — a second copy is exactly the drift the shared helper
/// exists to prevent. Pre-sorted ascending (issue #231 review bonus): the
/// exact-set zip in `check_round_trip` compares against this order directly,
/// so the literal must never be reshuffled without keeping it sorted.
const KNOWN_KEYS: [&str; 8] = [
    "created_at",
    "entity_type",
    "okf_version",
    "profile",
    "supersedes",
    "tags",
    "title",
    "updated_at",
];
```

3c. Add the existing-side helpers directly after `collect_frontmatter_fence` (~L181). The `cfg_attr` allow is temporary: Task 3 wires the helper into `write_note` and deletes it. The same pattern was used for `split_frontmatter_fence` in #240.

```rust
/// Issue #245 D1: the EXISTING note's frontmatter key set, or `None` when
/// there is no fence (unreachable through `write_note`: `enforce_staleness`
/// refuses `existing_unparsable:no_fence` first). Reads the SAME fence view
/// as the token reader. Three tiers:
/// - strict `serde_yaml::Value` mapping → its keys. NOT `parse_frontmatter`
///   (that silently drops unknown keys). Non-string keys (`1: x`) are
///   collected in Debug form — never rejected: on the existing side they are
///   data to protect, unlike `check_round_trip`'s rendered-output reject.
/// - parse fails or is not a mapping → column-0 line scan
///   ([`line_scan_keys`]), so damaged notes stay guarded, never skipped.
///
/// Both tiers apply the D2 normalization to the KNOWN optional fields only:
/// an empty `tags`/`supersedes` counts ABSENT (see [`strict_value_absent`],
/// [`line_scan_value_absent`]); every other key counts PRESENT.
#[cfg_attr(not(test), allow(dead_code))] // consumed by enforce_key_preservation (Task 3)
fn existing_frontmatter_keys(content: &str) -> Option<BTreeSet<String>> {
    let inner = collect_frontmatter_fence(content)?;
    if let Ok(serde_yaml::Value::Mapping(map)) = serde_yaml::from_str::<serde_yaml::Value>(&inner)
    {
        let mut keys = BTreeSet::new();
        for (key, value) in &map {
            match key.as_str() {
                Some(name) if strict_value_absent(name, value) => {}
                Some(name) => {
                    keys.insert(name.to_string());
                }
                None => {
                    keys.insert(format!("{key:?}"));
                }
            }
        }
        return Some(keys);
    }
    Some(line_scan_keys(&inner))
}

/// D2 strict-tier ABSENT forms — KNOWN optional fields only. `tags`: null /
/// `~` / empty / `[]` (the renderer omits both `None` and `Some(vec![])`).
/// `supersedes`: null / `~` / `""` (an empty pointer can never be re-sent —
/// `under_deposit("")` refuses it). Everything else is PRESENT.
#[cfg_attr(not(test), allow(dead_code))] // consumed via existing_frontmatter_keys (Task 3)
fn strict_value_absent(key: &str, value: &serde_yaml::Value) -> bool {
    use serde_yaml::Value;
    match key {
        "tags" => matches!(value, Value::Null) || matches!(value, Value::Sequence(s) if s.is_empty()),
        "supersedes" => {
            matches!(value, Value::Null) || matches!(value, Value::String(s) if s.is_empty())
        }
        _ => false,
    }
}

/// D1 tier 3: column-0 line scan over a damaged fence. A key line that is
/// followed by a continuation (any indented line, or a `-` list item) is
/// always PRESENT — that keeps block-valued keys from reading as empty.
#[cfg_attr(not(test), allow(dead_code))] // consumed via existing_frontmatter_keys (Task 3)
fn line_scan_keys(inner: &str) -> BTreeSet<String> {
    let lines: Vec<&str> = inner.lines().collect();
    let mut keys = BTreeSet::new();
    for (i, line) in lines.iter().enumerate() {
        let Some((key, rest)) = line_scan_key(line) else {
            continue;
        };
        let continued = lines.get(i + 1).is_some_and(|next| {
            next.starts_with(char::is_whitespace) || *next == "-" || next.starts_with("- ")
        });
        if !continued && line_scan_value_absent(&key, rest.trim()) {
            continue;
        }
        keys.insert(key);
    }
    keys
}

/// D1 (d) key-extraction rule: a line is a key line iff it is non-empty,
/// starts at column 0 with a character other than whitespace, `#` or `-`,
/// and contains `:`. Key = text before the FIRST `:`, trimmed, one matching
/// pair of surrounding `"`/`'` stripped. NO character-class restriction
/// (`1`, `my-key`, `some key` are keys). Returns the raw rest after the colon.
#[cfg_attr(not(test), allow(dead_code))] // consumed via existing_frontmatter_keys (Task 3)
fn line_scan_key(line: &str) -> Option<(String, &str)> {
    let first = line.chars().next()?;
    if first.is_whitespace() || first == '#' || first == '-' {
        return None;
    }
    let (raw, rest) = line.split_once(':')?;
    let raw = raw.trim();
    let key = ['"', '\'']
        .iter()
        .find_map(|q| {
            raw.strip_prefix(*q)
                .and_then(|r| r.strip_suffix(*q))
                .filter(|_| raw.len() >= 2)
        })
        .unwrap_or(raw);
    Some((key.to_string(), rest))
}

/// D1 (b) tier-3 ABSENT forms — same known-field restriction as the strict
/// tier. The inline value is compared verbatim after trimming; trailing
/// comments are NOT stripped (`tags: [] # none` counts PRESENT here — the
/// accepted stricter-direction divergence, D1 (c)).
#[cfg_attr(not(test), allow(dead_code))] // consumed via existing_frontmatter_keys (Task 3)
fn line_scan_value_absent(key: &str, value: &str) -> bool {
    match key {
        "tags" => matches!(value, "" | "null" | "~" | "[]"),
        "supersedes" => matches!(value, "" | "null" | "~" | "\"\""),
        _ => false,
    }
}
```

3d. Add `rendered_key_set` directly ABOVE `fn check_round_trip`. Its doc comment must not break the existing `check_round_trip` doc block, so place it above the `///` lines that start that block:

```rust
/// The key set the renderer emits for `fm`, in [`KNOWN_KEYS`] order. serde
/// skips `tags` (None or empty), `updated_at` (None) and `supersedes` (None).
/// Shared by `check_round_trip` and the issue #245 key-drop guard (D2) so
/// the two computations cannot drift.
fn rendered_key_set(fm: &OkfFrontmatter) -> Vec<&'static str> {
    KNOWN_KEYS
        .iter()
        .copied()
        .filter(|k| {
            !((*k == "tags" && fm.tags.as_ref().is_none_or(|t| t.is_empty()))
                || (*k == "updated_at" && fm.updated_at.is_none())
                || (*k == "supersedes" && fm.supersedes.is_none()))
        })
        .collect()
}
```

3e. In `check_round_trip`, delete the function-local block that starts at the comment `// Pre-sorted ascending (issue #231 review bonus)` and runs through the `expected_keys` `.collect();`. That covers the local `const KNOWN_KEYS`, `let known_sorted`, the `// Both directions…` comment and the filter. Replace it with:

```rust
    // Both directions: no unknown key, no missing key (exact set equality
    // against the normalized set the renderer emits — shared helper).
    let expected_keys = rendered_key_set(effective_fm);
```

The comparison and error message below it stay unchanged. `expected_keys` is still a `Vec<&str>`, so `{expected_keys:?}` prints the same way.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --features test-utils,mcp-server --lib okf::write::tests -- --test-threads=1`
Expected: PASS, all new tests plus every existing `okf::write` test (the round-trip refactor is behavior-preserving).

If `existing_keys_strict_tier_non_string_key_is_collected_not_rejected` fails on the Debug string, print `keys` and fix the assertion's expected Debug form. Do not change the helper: Debug form is the spec'd representation.

- [ ] **Step 5: Lint**

Run: `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --features test-utils -- -D warnings`
Expected: clean. If clippy flags the `line_scan_key` quote-strip closure, rewrite it as an explicit `for q in ['"', '\'']` loop with the same semantics.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/okf/write.rs
git commit -m "feat(okf): existing-side frontmatter key-set extraction (issue #245)

Hoist KNOWN_KEYS to module scope, share the renderer key-set
normalization (rendered_key_set) with check_round_trip, and add
existing_frontmatter_keys: strict serde_yaml::Value tier + column-0
line-scan tier for damaged YAML, known-optional null/empty normalization.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Error variants + `enforce_key_preservation`

**Files:**
- Modify: `src-tauri/src/okf/mod.rs` (`enum WriteNoteError`, after `CompactionMarkerRejected` ~L105)
- Modify: `src-tauri/src/okf/write.rs` (new fn after `enforce_size_drop` ~L311; `mod tests` end)

**Interfaces:**
- Consumes: `existing_frontmatter_keys`, `rendered_key_set`, `KNOWN_KEYS` (Task 1); test fixtures `kd_doc`, `fm`.
- Produces:
  - `WriteNoteError::KeyDropRefused { keys: Vec<String> }` and `WriteNoteError::KeyDropUnrepresentable { keys: Vec<String> }` (public).
  - `fn enforce_key_preservation(existing: Option<&str>, incoming: &OkfFrontmatter, allow_key_drop: bool) -> Result<(), WriteNoteError>` (private to `okf::write`).

- [ ] **Step 1: Write the failing tests**

Append to the issue #245 section at the end of `mod tests` in `write.rs`:

```rust
    fn fm_without_tags(token: Option<&str>) -> OkfFrontmatter {
        let mut m = fm("T", token);
        m.tags = None;
        m
    }

    #[test]
    fn key_drop_refused_display_has_pinned_shape_without_flag_name() {
        let e = WriteNoteError::KeyDropRefused {
            keys: vec!["supersedes".into(), "tags".into()],
        };
        assert_eq!(
            e.to_string(),
            "key_drop_refused:supersedes,tags: re-send the complete frontmatter or pass an explicit key-drop confirmation"
        );
        assert!(!e.to_string().contains("allow_key_drop"));
    }

    #[test]
    fn key_drop_unrepresentable_display_has_pinned_shape_without_flag_name() {
        let e = WriteNoteError::KeyDropUnrepresentable {
            keys: vec!["aliases".into(), "type".into()],
        };
        assert_eq!(
            e.to_string(),
            "key_drop_refused:unrepresentable:aliases,type: this note carries keys the writer cannot re-emit; migrate the note to the OKF schema outside this tool, or pass an explicit key-drop confirmation"
        );
        assert!(!e.to_string().contains("allow_key_drop"));
    }

    #[test]
    fn key_preservation_create_path_and_fence_less_pass() {
        assert!(enforce_key_preservation(None, &fm_without_tags(None), false).is_ok());
        // Defense-in-depth: unreachable post-staleness, must not panic.
        assert!(enforce_key_preservation(Some("no fence\n"), &fm_without_tags(None), false).is_ok());
    }

    #[test]
    fn key_preservation_refuses_known_drop_names_exactly_that_key() {
        let doc = kd_doc(&["tags: [a]"], false);
        let err = enforce_key_preservation(Some(&doc), &fm_without_tags(None), false).unwrap_err();
        assert!(
            matches!(&err, WriteNoteError::KeyDropRefused { keys } if keys == &["tags"]),
            "{err}"
        );
    }

    #[test]
    fn key_preservation_known_partition_reported_first() {
        // D5 precedence pin (Opus design-c2 MAJOR 1): legacy `type:` + `tags`,
        // payload drops `tags` → KeyDropRefused naming ONLY `tags`.
        let doc = kd_doc(&["type: fact", "tags: [a]"], false);
        let err = enforce_key_preservation(Some(&doc), &fm_without_tags(None), false).unwrap_err();
        assert!(
            matches!(&err, WriteNoteError::KeyDropRefused { keys } if keys == &["tags"]),
            "{err}"
        );
        assert!(!err.to_string().contains("type"), "{err}");
    }

    #[test]
    fn key_preservation_unknown_only_is_unrepresentable() {
        let doc = kd_doc(&["aliases: []"], false);
        let err = enforce_key_preservation(Some(&doc), &fm("T", None), false).unwrap_err();
        assert!(
            matches!(&err, WriteNoteError::KeyDropUnrepresentable { keys } if keys == &["aliases"]),
            "{err}"
        );
    }

    #[test]
    fn key_preservation_updated_at_exempt_and_adding_keys_never_refuses() {
        // Existing has no tags; incoming adds tags and carries no updated_at.
        let doc = kd_doc(&[], false);
        assert!(enforce_key_preservation(Some(&doc), &fm("T", None), false).is_ok());
    }

    #[test]
    fn key_preservation_flag_bypasses_both_partitions() {
        let allow_key_drop = true;
        let doc = kd_doc(&["type: fact", "aliases: []", "tags: [a]"], false);
        assert!(enforce_key_preservation(Some(&doc), &fm_without_tags(None), allow_key_drop).is_ok());
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --features test-utils,mcp-server --lib okf::write::tests::key_ -- --test-threads=1`
Expected: compile FAIL. The `KeyDropRefused`/`KeyDropUnrepresentable` variants and `enforce_key_preservation` are not found.

- [ ] **Step 3: Implement**

3a. In `src-tauri/src/okf/mod.rs`, insert after the `CompactionMarkerRejected { marker: String },` variant:

```rust
    /// Issue #245: an If-Match edit omitted frontmatter keys (in `KNOWN_KEYS`)
    /// the existing note carries, and the caller did not confirm the drop.
    /// Keys sorted, joined with `,`. Like `ShrinkRefused`, the Display never
    /// names the override flag.
    #[error(
        "key_drop_refused:{}: re-send the complete frontmatter or pass an explicit key-drop confirmation",
        .keys.join(",")
    )]
    KeyDropRefused { keys: Vec<String> },
    /// Issue #245: the edit would drop keys OUTSIDE `KNOWN_KEYS` — legacy or
    /// hand-added keys the typed struct cannot re-emit, so "re-send the
    /// frontmatter" can never fix it. Reported only after the known
    /// partition is clean (D5 precedence pin).
    #[error(
        "key_drop_refused:unrepresentable:{}: this note carries keys the writer cannot re-emit; migrate the note to the OKF schema outside this tool, or pass an explicit key-drop confirmation",
        .keys.join(",")
    )]
    KeyDropUnrepresentable { keys: Vec<String> },
```

3b. In `write.rs`, add after `enforce_size_drop`:

```rust
/// Issue #245: refuse an edit whose payload drops frontmatter keys the
/// existing note carries, unless the caller explicitly confirmed. Compares
/// the existing key set ([`existing_frontmatter_keys`]) against the keys the
/// INCOMING struct renders ([`rendered_key_set`]); `updated_at` is exempt
/// (rotated on every write). KNOWN dropped keys are reported before UNKNOWN
/// ones — `allow_key_drop` bypasses BOTH, so this precedence is what keeps
/// known keys from being lost behind an unrepresentable-key message.
#[cfg_attr(not(test), allow(dead_code))] // wired into write_note in Task 3
fn enforce_key_preservation(
    existing: Option<&str>,
    incoming: &OkfFrontmatter,
    allow_key_drop: bool,
) -> Result<(), WriteNoteError> {
    let Some(existing_keys) = existing.and_then(existing_frontmatter_keys) else {
        return Ok(()); // create path, or fence-less (refused upstream)
    };
    if allow_key_drop {
        return Ok(());
    }
    let incoming_keys = rendered_key_set(incoming);
    let (mut known, mut unknown) = (Vec::new(), Vec::new());
    // BTreeSet iteration is sorted, so both partitions come out sorted.
    for key in existing_keys {
        if key == "updated_at" || incoming_keys.contains(&key.as_str()) {
            continue;
        }
        if KNOWN_KEYS.contains(&key.as_str()) {
            known.push(key);
        } else {
            unknown.push(key);
        }
    }
    if !known.is_empty() {
        return Err(WriteNoteError::KeyDropRefused { keys: known });
    }
    if !unknown.is_empty() {
        return Err(WriteNoteError::KeyDropUnrepresentable { keys: unknown });
    }
    Ok(())
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --features test-utils,mcp-server --lib okf::write::tests -- --test-threads=1`
Expected: PASS.

- [ ] **Step 5: Lint + commit**

Run: `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --features test-utils -- -D warnings` (expect clean). Then:

```bash
git add src-tauri/src/okf/mod.rs src-tauri/src/okf/write.rs
git commit -m "feat(okf): KeyDropRefused/KeyDropUnrepresentable + enforce_key_preservation (issue #245)

Two pinned-Display refusal variants (never naming the bypass flag) and
the key-preservation guard: existing-vs-rendered key sets, updated_at
exempt, KNOWN partition reported before UNKNOWN.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Wire the guard into `write_note` + `allow_key_drop` plumbing

**Files:**
- Modify: `src-tauri/src/okf/write.rs` (module header L16-22; `write_note` doc + signature ~L399-423; guard block ~L555-559; remove the Task 1/2 `cfg_attr` allows; every test call site; new tests)
- Modify: `src-tauri/src/tool_dispatch.rs` (`dispatch_vault_write_note` ~L287-307; `VaultWriteNoteParams` ~L1138-1148; dispatch arm ~L1520-1526)
- Modify: `src-tauri/src/lib.rs` (`vault_write_note` command ~L877-898)
- Modify: `src-tauri/tests/mcp_write_integration.rs` (4 call sites ~L836/868/883/907)
- Modify: spec D4 (named-consts note)

**Interfaces:**
- Consumes: `enforce_key_preservation` (Task 2); fixtures `kd_doc`, `fm_without_tags` (Tasks 1–2).
- Produces:
  - `pub fn write_note(vault_root: &Path, path: &str, frontmatter: &OkfFrontmatter, body: &str, expected_updated_at: Option<&str>, allow_shrink: bool, allow_key_drop: bool) -> Result<WriteNoteResult, WriteNoteError>`
  - `pub fn dispatch_vault_write_note(vault_dir: &Path, path: &str, frontmatter: &OkfFrontmatter, body: &str, allow_shrink: bool, allow_key_drop: bool) -> Result<WriteNoteResult>`
  - `VaultWriteNoteParams { …, pub allow_key_drop: bool }` (`#[serde(default)]`)
  - Test consts `NO_SHRINK`/`NO_KEY_DROP` (both `false`) in `write.rs` `mod tests` and in `tests/mcp_write_integration.rs`.

**Plan-vs-spec note (named flags):** D4 says test call sites "bind named locals first". The 77 existing `write_note` calls in `write.rs` tests plus 4 integration calls would each gain two `let` lines. This plan uses module-level named consts (`NO_SHRINK`, `NO_KEY_DROP`) at sites that pass the default, and named `let` locals at sites that pass `true`. That serves the same intent: a swap reads `NO_KEY_DROP, NO_SHRINK` and is visibly wrong in review. Step 9 records this in spec D4.

- [ ] **Step 1: Write the failing end-to-end tests**

Append to the issue #245 section of `mod tests` in `write.rs`:

```rust
    /// Seed `rel` with raw `doc` bytes (bypasses write_note — needed for
    /// legacy/damaged/hand-edited fixtures) and return its If-Match token.
    fn kd_seed(root: &Path, rel: &str, doc: &str) -> String {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, doc).unwrap();
        read_existing_token(doc).expect("seed fixture carries a readable token")
    }

    /// Edit with the seed's own body ("body\n") so the shrink guard is inert.
    fn kd_edit(
        root: &Path,
        rel: &str,
        m: &OkfFrontmatter,
        token: &str,
        allow_key_drop: bool,
    ) -> Result<WriteNoteResult, WriteNoteError> {
        let allow_shrink = false;
        write_note(root, rel, m, "body\n", Some(token), allow_shrink, allow_key_drop)
    }

    #[test]
    fn key_drop_incident_replay_tags_and_supersedes() {
        let (_g, root) = deposit_vault();
        let v1 = "immutable-source-files/agents/v1.md";
        let v2 = "immutable-source-files/agents/v2.md";
        write_note(&root, v1, &fm("V1", None), "old\n", None, NO_SHRINK, NO_KEY_DROP).unwrap();
        let mut m = fm("V2", None);
        m.supersedes = Some(v1.to_string());
        let created = write_note(&root, v2, &m, "body\n", None, NO_SHRINK, NO_KEY_DROP).unwrap();
        // Mangled payload: both optional keys gone.
        let mut mangled = fm_without_tags(Some(&created.updated_at));
        mangled.supersedes = None;
        let err = kd_edit(&root, v2, &mangled, &created.updated_at, NO_KEY_DROP).unwrap_err();
        let s = err.to_string();
        assert!(s.starts_with("key_drop_refused:supersedes,tags:"), "{s}");
        assert!(!s.contains("allow_key_drop"), "{s}");
        // Refused write left the file untouched.
        let on_disk = fs::read_to_string(root.join(v2)).unwrap();
        assert_eq!(read_existing_token(&on_disk).unwrap(), created.updated_at);
    }

    #[test]
    fn key_drop_required_field_omission_fails_at_parse_not_guard() {
        // Opus c1 M1: required fields can't reach the guard.
        let v = serde_json::json!({
            "path": "wiki/n.md",
            "frontmatter": {
                "okf_version": "0.1", "profile": "llm-wiki/1", "title": "T",
                "created_at": "2026-09-01T00:00:00Z"
            },
            "body": "b"
        });
        let err = serde_json::from_value::<crate::tool_dispatch::VaultWriteNoteParams>(v)
            .unwrap_err()
            .to_string();
        assert!(err.contains("entity_type"), "{err}");
    }

    #[test]
    fn key_drop_unknown_key_strict_and_line_scan_tiers() {
        for damaged in [false, true] {
            let (_g, root) = vault();
            let token = kd_seed(&root, "wiki/n.md", &kd_doc(&["aliases: []"], damaged));
            let err = kd_edit(&root, "wiki/n.md", &fm("T", Some(&token)), &token, NO_KEY_DROP)
                .unwrap_err();
            assert!(
                matches!(&err, WriteNoteError::KeyDropUnrepresentable { keys } if keys == &["aliases"]),
                "damaged={damaged}: {err}"
            );
            assert!(err.to_string().contains("unrepresentable"), "{err}");
        }
    }

    #[test]
    fn key_drop_mixed_reports_known_first_end_to_end() {
        let (_g, root) = vault();
        let token = kd_seed(&root, "wiki/n.md", &kd_doc(&["type: fact", "tags: [a]"], false));
        let err = kd_edit(&root, "wiki/n.md", &fm_without_tags(Some(&token)), &token, NO_KEY_DROP)
            .unwrap_err();
        assert!(
            matches!(&err, WriteNoteError::KeyDropRefused { keys } if keys == &["tags"]),
            "{err}"
        );
    }

    #[test]
    fn key_drop_damaged_yaml_inline_empty_vs_block_tags() {
        // Inline `tags: []` counts absent → dropping it succeeds.
        let (_g, root) = vault();
        let token = kd_seed(&root, "wiki/n.md", &kd_doc(&["tags: []"], true));
        kd_edit(&root, "wiki/n.md", &fm_without_tags(Some(&token)), &token, NO_KEY_DROP)
            .expect("inline-empty tags must count absent");
        // Block-sequence tags is PRESENT → dropping it refuses (guard NOT skipped).
        let (_g, root) = vault();
        let token = kd_seed(&root, "wiki/n.md", &kd_doc(&["tags:", "  - a"], true));
        let err = kd_edit(&root, "wiki/n.md", &fm_without_tags(Some(&token)), &token, NO_KEY_DROP)
            .unwrap_err();
        assert!(err.to_string().starts_with("key_drop_refused:tags:"), "{err}");
    }

    #[test]
    fn key_drop_damaged_nested_block_mapping_unknown_key() {
        let (_g, root) = vault();
        let token = kd_seed(&root, "wiki/n.md", &kd_doc(&["source:", "  url: https://x"], true));
        let err = kd_edit(&root, "wiki/n.md", &fm("T", Some(&token)), &token, NO_KEY_DROP)
            .unwrap_err();
        assert!(
            matches!(&err, WriteNoteError::KeyDropUnrepresentable { keys } if keys == &["source"]),
            "{err}"
        );
    }

    #[test]
    fn key_drop_known_absent_forms_never_false_drop() {
        for line in [
            "tags: null",
            "tags: ~",
            "tags: []",
            "supersedes: ~",
            "supersedes: null",
            "supersedes: \"\"",
        ] {
            let (_g, root) = vault();
            let token = kd_seed(&root, "wiki/n.md", &kd_doc(&[line], false));
            kd_edit(&root, "wiki/n.md", &fm_without_tags(Some(&token)), &token, NO_KEY_DROP)
                .unwrap_or_else(|e| panic!("{line:?} must count absent: {e}"));
        }
    }

    #[test]
    fn key_drop_damaged_present_scalar_forms_refuse() {
        // `tags: foo`, `tags: ""` (non-list), `tags: [] # none` (trailing
        // comment — accepted stricter-direction divergence) all PRESENT.
        for line in ["tags: foo", "tags: \"\"", "tags: [] # none"] {
            let (_g, root) = vault();
            let token = kd_seed(&root, "wiki/n.md", &kd_doc(&[line], true));
            let err = kd_edit(&root, "wiki/n.md", &fm_without_tags(Some(&token)), &token, NO_KEY_DROP)
                .unwrap_err();
            assert!(
                matches!(&err, WriteNoteError::KeyDropRefused { keys } if keys == &["tags"]),
                "{line:?}: {err}"
            );
        }
        // Non-list tags does not wedge: re-sending non-empty tags succeeds.
        let (_g, root) = vault();
        let token = kd_seed(&root, "wiki/n.md", &kd_doc(&["tags: \"\""], true));
        kd_edit(&root, "wiki/n.md", &fm("T", Some(&token)), &token, NO_KEY_DROP)
            .expect("sending non-empty tags keeps the key");
    }

    #[test]
    fn key_drop_non_string_keys_both_tiers() {
        let (_g, root) = vault();
        let token = kd_seed(&root, "wiki/n.md", &kd_doc(&["1: x"], false));
        let err = kd_edit(&root, "wiki/n.md", &fm("T", Some(&token)), &token, NO_KEY_DROP)
            .unwrap_err();
        let debug = format!("{:?}", serde_yaml::Value::Number(1.into()));
        assert!(
            matches!(&err, WriteNoteError::KeyDropUnrepresentable { keys } if keys == &[debug.clone()]),
            "{err}"
        );
        let (_g, root) = vault();
        let token = kd_seed(&root, "wiki/n.md", &kd_doc(&["1: x", "my-key: y"], true));
        let err = kd_edit(&root, "wiki/n.md", &fm("T", Some(&token)), &token, NO_KEY_DROP)
            .unwrap_err();
        assert!(
            matches!(&err, WriteNoteError::KeyDropUnrepresentable { keys } if keys == &["1", "my-key"]),
            "{err}"
        );
    }

    #[test]
    fn key_drop_non_deposit_supersedes_drop_needs_flag() {
        // Spec D4 sibling case: a wiki note with a hand-added `supersedes`
        // cannot re-send it (deposit-only), so only the flag gets through.
        // Also pins "drop exactly one key names exactly that key".
        let (_g, root) = vault();
        let token = kd_seed(
            &root,
            "wiki/n.md",
            &kd_doc(&["tags: [a]", "supersedes: immutable-source-files/agents/v1.md"], false),
        );
        let err = kd_edit(&root, "wiki/n.md", &fm("T", Some(&token)), &token, NO_KEY_DROP)
            .unwrap_err();
        assert!(
            matches!(&err, WriteNoteError::KeyDropRefused { keys } if keys == &["supersedes"]),
            "{err}"
        );
        let allow_key_drop = true;
        kd_edit(&root, "wiki/n.md", &fm("T", Some(&token)), &token, allow_key_drop)
            .expect("explicit confirmation drops the stale pointer");
    }

    #[test]
    fn key_drop_flag_permits_known_and_unrepresentable() {
        let allow_key_drop = true;
        let (_g, root) = vault();
        let token = kd_seed(&root, "wiki/n.md", &kd_doc(&["type: fact", "tags: [a]"], false));
        kd_edit(&root, "wiki/n.md", &fm_without_tags(Some(&token)), &token, allow_key_drop)
            .expect("flag bypasses both partitions");
        let on_disk = fs::read_to_string(root.join("wiki/n.md")).unwrap();
        // Line-prefix check: a bare `contains("type:")` would match `entity_type:`.
        assert!(
            !on_disk.lines().any(|l| l.starts_with("type:") || l.starts_with("tags:")),
            "{on_disk}"
        );
    }

    #[test]
    fn key_drop_adding_keys_never_refuses() {
        let (_g, root) = vault();
        let token = kd_seed(&root, "wiki/n.md", &kd_doc(&[], false));
        kd_edit(&root, "wiki/n.md", &fm("T", Some(&token)), &token, NO_KEY_DROP)
            .expect("adding tags is not a drop");
    }

    #[test]
    fn key_drop_crlf_note_refused() {
        let (_g, root) = vault();
        write_note(&root, "wiki/n.md", &fm("T", None), "body\n", None, NO_SHRINK, NO_KEY_DROP)
            .unwrap();
        let lf = fs::read_to_string(root.join("wiki/n.md")).unwrap();
        let crlf = lf.replace('\n', "\r\n");
        fs::write(root.join("wiki/n.md"), &crlf).unwrap();
        let token = read_existing_token(&crlf).unwrap();
        let err = kd_edit(&root, "wiki/n.md", &fm_without_tags(Some(&token)), &token, NO_KEY_DROP)
            .unwrap_err();
        assert!(err.to_string().starts_with("key_drop_refused:tags:"), "{err}");
    }

    #[test]
    fn marker_check_runs_before_key_drop_check() {
        // D6: marker > key-drop.
        let (_g, root) = vault();
        let token = kd_seed(&root, "wiki/n.md", &kd_doc(&["tags: [a]"], false));
        let err = write_note(
            &root,
            "wiki/n.md",
            &fm_without_tags(Some(&token)),
            "[SKILL_PRUNED]\n",
            Some(&token),
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap_err();
        assert!(err.to_string().starts_with("compaction_marker:"), "{err}");
    }

    #[test]
    fn key_drop_check_runs_before_shrink_check() {
        // D6: key-drop > shrink.
        let (_g, root) = vault();
        let created = write_note(
            &root,
            "wiki/n.md",
            &fm("T", None),
            &long_body(400),
            None,
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap();
        let err = write_note(
            &root,
            "wiki/n.md",
            &fm_without_tags(Some(&created.updated_at)),
            "tiny\n",
            Some(&created.updated_at),
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap_err();
        assert!(err.to_string().starts_with("key_drop_refused:"), "key-drop must win: {err}");
    }

    #[test]
    fn params_allow_key_drop_omitted_defaults_false_and_true_parses() {
        let fm_json = serde_json::json!({
            "okf_version": "0.1",
            "profile": "llm-wiki/1",
            "title": "T",
            "entity_type": "fact",
            "created_at": "2026-09-01T00:00:00Z"
        });
        let v = serde_json::json!({ "path": "wiki/n.md", "frontmatter": fm_json, "body": "b" });
        let p: crate::tool_dispatch::VaultWriteNoteParams = serde_json::from_value(v).unwrap();
        assert!(!p.allow_key_drop);
        let v = serde_json::json!({ "path": "wiki/n.md", "frontmatter": fm_json, "body": "b", "allow_key_drop": true });
        let p: crate::tool_dispatch::VaultWriteNoteParams = serde_json::from_value(v).unwrap();
        assert!(p.allow_key_drop);
    }

    #[test]
    fn key_drop_refusal_reaches_mcp_surface_via_anyhow() {
        let (_g, root) = vault();
        let created = write_note(&root, "wiki/n.md", &fm("T", None), "body\n", None, NO_SHRINK, NO_KEY_DROP)
            .unwrap();
        let err = crate::tool_dispatch::dispatch_vault_write_note(
            &root,
            "wiki/n.md",
            &fm_without_tags(Some(&created.updated_at)),
            "body\n",
            NO_SHRINK,
            NO_KEY_DROP,
        )
        .unwrap_err();
        let s = err.to_string();
        assert!(s.starts_with("key_drop_refused:tags:"), "{s}");
        assert!(!s.contains("allow_key_drop"), "must not teach the bypass: {s}");
    }
```

Add the named consts at the TOP of `mod tests`, right after `use tempfile::TempDir;`:

```rust
    /// Issue #245 D4: `write_note` takes two adjacent guard bools — pass them
    /// by NAME, never as literals, so a swap reads wrong in review.
    const NO_SHRINK: bool = false;
    const NO_KEY_DROP: bool = false;
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --features test-utils,mcp-server --lib okf::write::tests -- --test-threads=1`
Expected: compile FAIL. The new tests call `write_note` with 7 args (E0061), and `allow_key_drop` is not a field of `VaultWriteNoteParams`.

- [ ] **Step 3: Change `write_note`'s signature and wire the guard**

In `write.rs`, extend the `write_note` doc list after the `expected_updated_at` bullet:

```rust
/// * `allow_shrink` — issue #240 confirmation for a >50% body shrink.
/// * `allow_key_drop` — issue #245 confirmation for dropping frontmatter keys
///   the existing note carries (see [`enforce_key_preservation`]).
```

Change the signature to:

```rust
pub fn write_note(
    vault_root: &Path,
    path: &str,
    frontmatter: &OkfFrontmatter,
    body: &str,
    expected_updated_at: Option<&str>,
    allow_shrink: bool,
    allow_key_drop: bool,
) -> Result<WriteNoteResult, WriteNoteError> {
```

Replace the guard block (`// Issue #240 guards — …` through `enforce_size_drop(…)?;`) with:

```rust
    // Issue #240/#245 guards — AFTER render (the shrink measurement basis is
    // the rendered body) and BEFORE any bytes hit disk. Order pinned (#245
    // D6): marker (root cause, no bypass) → key-drop (more specific than a
    // shrink) → shrink.
    enforce_compaction_markers(&document, existing.as_deref())?;
    enforce_key_preservation(existing.as_deref(), frontmatter, allow_key_drop)?;
    enforce_size_drop(existing.as_deref(), &document, allow_shrink)?;
```

Delete every `#[cfg_attr(not(test), allow(dead_code))]` line added in Tasks 1–2. There are six: `existing_frontmatter_keys`, `strict_value_absent`, `line_scan_keys`, `line_scan_key`, `line_scan_value_absent`, `enforce_key_preservation`.

Extend the module-header refusal list (L19-21): replace

```rust
//!   `shrink_refused:{existing}:{new}: re-read the note and resend the full
//!   body`, `compaction_marker:{marker}: rephrase and resend without
//!   compaction artifacts`. Display strings ARE the contract; see each
```

with

```rust
//!   `shrink_refused:{existing}:{new}: re-read the note and resend the full
//!   body`, `compaction_marker:{marker}: rephrase and resend without
//!   compaction artifacts`, `key_drop_refused:{keys}: …` and
//!   `key_drop_refused:unrepresentable:{keys}: …` (issue #245 — an edit
//!   dropping existing frontmatter keys). Display strings ARE the contract; see each
```

- [ ] **Step 4: Plumb `allow_key_drop` through the surfaces**

`src-tauri/src/tool_dispatch.rs`, `dispatch_vault_write_note`: add the `allow_key_drop: bool,` param after `allow_shrink: bool,`, and pass `allow_key_drop,` after `allow_shrink,` in the `write_note` call.

`VaultWriteNoteParams`, after the `allow_shrink` field:

```rust
    /// Set true only when the user explicitly asked to remove these
    /// frontmatter keys from the note. Never set it to retry after a refused
    /// write; re-read the note and resend the complete frontmatter instead.
    #[serde(default)]
    pub allow_key_drop: bool,
```

Dispatch arm (~L1520): after `p.allow_shrink,` add `p.allow_key_drop,`.

`src-tauri/src/lib.rs`, `vault_write_note` command: add the param `allow_key_drop: Option<bool>,` after `allow_shrink: Option<bool>,`. Add a comment line beneath the existing `allow_shrink` comment:

```rust
    // allow_key_drop (issue #245): omitted ⇒ false — Tauri command params do
    // not honor #[serde(default)], hence Option + unwrap_or.
```

Then pass `allow_key_drop.unwrap_or(false),` after `allow_shrink.unwrap_or(false),`.

- [ ] **Step 5: Sweep every existing test call site (compiler-driven)**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --features test-utils,mcp-server --no-run 2>&1 | grep -E '^error|-->' | head -200`

Every E0061 site in `write.rs` `mod tests` (~77 `write_note(` calls plus 1 `dispatch_vault_write_note(` at ~L2723) gets one of two treatments:
- Allow-shrink argument is the literal `false`: replace it with `NO_SHRINK` and append `NO_KEY_DROP` as the next argument. Example: `write_note(&root, "wiki/n.md", &note, "body two\n", Some(&token), false)` → `write_note(&root, "wiki/n.md", &note, "body two\n", Some(&token), NO_SHRINK, NO_KEY_DROP)`.
- Allow-shrink argument is the literal `true` (`allow_shrink_permits_major_shrink` ~L2535, `edit_rejects_newly_introduced_marker` ~L2570): bind locals first, `let allow_shrink = true;` and `let allow_key_drop = false;`, then pass `allow_shrink, allow_key_drop`.

`src-tauri/tests/mcp_write_integration.rs`: directly after `use tauri_app_lib::okf::write::write_note;` add

```rust
/// Issue #245 D4: name the two adjacent guard bools — never pass literals.
const NO_SHRINK: bool = false;
const NO_KEY_DROP: bool = false;
```

and change the 4 call sites' trailing `false,` to `NO_SHRINK,` + `NO_KEY_DROP,`.

Re-run the `--no-run` command until it compiles with zero errors.

- [ ] **Step 6: Run the full suite (also the audit step)**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --features test-utils,mcp-server -- --test-threads=1`
Expected: PASS, all lib tests and integration binaries.

Audit (spec Testing, last bullets): an EXISTING test that now fails with `key_drop_refused` is an edit that legally drops keys. Read it. If the drop is intentional, route it through `let allow_key_drop = true;`. If it's a fixture accident, make the edit re-send the key. Never weaken the guard to make a test pass. Inspection while planning found no such test: `fm()` always carries `tags`, and the `supersedes` tests are creates. So zero failures are expected, and any failure deserves a real look.

- [ ] **Step 7: Lint**

Run: `cargo fmt --manifest-path src-tauri/Cargo.toml` then `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --features test-utils -- -D warnings` and `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --features test-utils,mcp-server -- -D warnings`
Expected: clean. If clippy raises `too_many_arguments` on `write_note` (7 args is the default threshold), add `#[allow(clippy::too_many_arguments)]` on `write_note` with the comment `// issue #245 D4: bare bools kept deliberately; a params struct would rewrite every #240-era call site`.

- [ ] **Step 8: Record the named-consts deviation in spec D4**

In `docs/superpowers/specs/2026-10-01-issue245-frontmatter-key-drop-guard-design.md`, D4 signature-note bullet, replace

```
     locals, never literals — a swap then reads wrong in review. This covers BOTH the unit
```

with

```
     locals, never literals — a swap then reads wrong in review. (Implementation: default-
     valued sites pass module-level named consts `NO_SHRINK`/`NO_KEY_DROP` instead of two
     `let` lines at each of ~80 sites — same swap-visibility; `true` sites bind locals.)
     This covers BOTH the unit
```

- [ ] **Step 9: Commit**

```bash
git add src-tauri/src/okf/write.rs src-tauri/src/tool_dispatch.rs src-tauri/src/lib.rs \
  src-tauri/tests/mcp_write_integration.rs \
  docs/superpowers/specs/2026-10-01-issue245-frontmatter-key-drop-guard-design.md
git commit -m "feat(okf): enforce frontmatter key preservation on If-Match edits (issue #245)

write_note refuses edits that drop existing frontmatter keys unless
allow_key_drop is set; order marker > key-drop > shrink. Plumbed through
VaultWriteNoteParams (serde default false), dispatch_vault_write_note and
the Tauri command (Option<bool>). Test call sites pass named guard flags.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: MCP tool description, spec status, final gates, PR

**Files:**
- Modify: `src-tauri/src/mcp_server.rs:127` (`vault_write_note` description)
- Modify: `src-tauri/src/okf/write.rs` (`mod tests` end)
- Modify: spec Status line

**Interfaces:**
- Consumes: everything above.
- Produces: the agent-facing contract text.

- [ ] **Step 1: Write the failing test**

Append to the issue #245 section of `mod tests` in `write.rs`. `mcp_server.rs` has no test module and its tool router needs a live server, so this is the spec's sanctioned source-text assertion:

```rust
    #[test]
    fn mcp_vault_write_note_description_teaches_refusals_not_flag() {
        let src = include_str!("../mcp_server.rs");
        let tool = src
            .find("name = \"vault_write_note\"")
            .expect("vault_write_note tool attribute present");
        let after = &src[tool..];
        let open = after.find("description = \"").expect("description present")
            + "description = \"".len();
        let len = after[open..].find('"').expect("description closes");
        let desc = &after[open..open + len];
        assert!(desc.contains("key_drop_refused:{keys}"), "{desc}");
        assert!(desc.contains("key_drop_refused:unrepresentable:{keys}"), "{desc}");
        assert!(!desc.contains("allow_key_drop"), "must not teach the bypass: {desc}");
        assert!(!desc.contains("allow_shrink"), "must not teach the bypass: {desc}");
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --features test-utils,mcp-server --lib okf::write::tests::mcp_vault_write_note_description -- --test-threads=1`
Expected: FAIL. The assertion `desc.contains("key_drop_refused:{keys}")` fails.

- [ ] **Step 3: Extend the description**

In `src-tauri/src/mcp_server.rs`, `vault_write_note` `description = "…"`, replace

```
compaction_marker:{marker} means the payload introduces a context-compaction artifact — rephrase and resend without it.
```

with

```
compaction_marker:{marker} means the payload introduces a context-compaction artifact — rephrase and resend without it; key_drop_refused:{keys} means the payload omitted frontmatter keys the note already has — re-read the note and resend the complete frontmatter; key_drop_refused:unrepresentable:{keys} means the note carries keys this tool cannot write — the note must be migrated outside this tool.
```

(Keep it on the one existing line. The description must contain no `"` characters.)

- [ ] **Step 4: Run to verify pass**

Run the Step 2 command. Expected: PASS.

- [ ] **Step 5: Update the spec status**

In the spec, replace the `**Status:**` line with:

```
**Status:** Implemented on `feat/issue-245-frontmatter-key-drop-guard` — plan `docs/superpowers/plans/2026-10-02-issue245-frontmatter-key-drop-guard.md`. Review converged before implementation (Opus design-c3 APPROVE WITH NITS; CodeRabbit, Claude review and `/code-review high` 2026-10-02 applied). See [Revision history](#revision-history).
```

- [ ] **Step 6: Full CI-equivalent gate**

Run, in order:
- `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check`
- `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --features test-utils -- -D warnings`
- `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --features test-utils,mcp-server -- -D warnings`
- `cargo test --manifest-path src-tauri/Cargo.toml --features test-utils,mcp-server -- --test-threads=1`

Expected: all clean/PASS. Report actual counts and output. Do not paraphrase a failure as a pass.

- [ ] **Step 7: Commit + push**

```bash
git add src-tauri/src/mcp_server.rs src-tauri/src/okf/write.rs \
  docs/superpowers/specs/2026-10-01-issue245-frontmatter-key-drop-guard-design.md
git commit -m "feat(mcp): vault_write_note description documents key_drop_refused (issue #245)

Agents learn both new refusals and their remedies from the tool
description; the bypass flag stays out of it (schema-only). Spec status
flipped to implemented.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git push
```

- [ ] **Step 8: Retitle + rewrite the PR body (#255)**

PR #255 currently says "spec-stage PR … implementation is a separate PR". That contradicts the one-branch rule. Update it:

```bash
gh pr edit 255 --title "feat(okf): frontmatter key-drop guard on If-Match edits (issue #245)" --body-file <scratchpad>/pr255-body.md
```

The body must cover: Summary (spec + plan + implementation, Fixes #245), root cause (unchanged section), design D1–D6 (unchanged), what changed (files table from this plan), test evidence (actual gate output from Step 6), review state (existing section plus `/code-review high` 2026-10-02), and the migration-backlog note (17 legacy-key notes now refuse until migrated). End with `🤖 Generated with [Claude Code](https://claude.com/claude-code)`.

- [ ] **Step 9: Verify CI on the pushed tip**

Run: `gh pr view 255 --json mergeable,mergeStateStatus,headRefOid` and `gh pr checks 255`
A CONFLICTING PR runs zero CI silently, so confirm `mergeable == MERGEABLE` and that checks actually ran on `headRefOid` before calling the PR ready.
