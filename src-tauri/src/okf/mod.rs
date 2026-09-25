//! OKF bundle serialization per the llm-wiki OKF profile v1
//! (expo-llm-wiki/docs/okf-profile.md, normative).

pub mod bundle_read;
pub mod bundle_write;
pub mod concept;
pub mod entity_index_md;
pub mod event_line;
pub mod fact_file;
pub mod frontmatter;
pub mod ids;
pub mod index_md;
pub mod log_md;
pub mod markdown_links;
pub mod path_allowlist;
pub mod related_section;
pub mod sanitize;
pub mod task_file;
pub mod timefmt;
pub mod types;
pub mod write;
pub mod zip_io;

// Write-path extensions for vault_write_note and vault_upsert_index_entry

use serde::{Deserialize, Serialize};

/// OKF document frontmatter (v0.1)
///
/// Adopted from @equationalapplications/okf, profile: llm-wiki/1
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp-server", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct OkfFrontmatter {
    pub okf_version: String,
    pub profile: String,
    pub title: String,
    pub entity_type: EntityType,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    pub created_at: String,
    pub updated_at: Option<String>,
    /// Vault-relative path of the deposit this one supersedes. Deposit-to-deposit only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
}

/// Entity types for OKF documents
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "mcp-server", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum EntityType {
    Fact,
    Task,
    Event,
    Concept,
    Doc,
}

impl std::fmt::Display for EntityType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            EntityType::Fact => "fact",
            EntityType::Task => "task",
            EntityType::Event => "event",
            EntityType::Concept => "concept",
            EntityType::Doc => "doc",
        };
        write!(f, "{}", s)
    }
}

/// Errors that can occur when writing notes
#[derive(Debug, thiserror::Error)]
pub enum WriteNoteError {
    #[error("Path is outside vault root")]
    PathOutsideVault,
    #[error("Invalid frontmatter: {0}")]
    InvalidFrontmatter(String),
    #[error("Stale update: file was modified since updated_at={updated_at}")]
    StaleUpdate { updated_at: String },
    #[error("Write error: {0}")]
    WriteError(String),
}

/// Result from vault_write_note
#[derive(Debug, Serialize)]
pub struct WriteNoteResult {
    pub success: bool,
    pub path: String,
    pub sha256: String,
    /// The NEW If-Match token written into the file's frontmatter (RFC 3339).
    /// The caller echoes this back verbatim on the next edit.
    pub updated_at: String,
}

/// Errors that can occur when upserting index entries
#[derive(Debug, thiserror::Error)]
pub enum UpsertError {
    #[error("Index file not found: {0}")]
    IndexNotFound(String),
    #[error("Invalid metadata: {0}")]
    InvalidMetadata(String),
    #[error("invalid_entry_name")]
    InvalidEntryName,
    #[error("Path is outside vault root")]
    PathOutsideVault,
    #[error("Write error: {0}")]
    WriteError(String),
}

/// Result from vault_upsert_index_entry
#[derive(Debug, Serialize)]
pub struct UpsertResult {
    pub success: bool,
    pub index_path: String,
    pub entry_id: String,
    pub appended: bool,             // true if entry was new, false if updated
    pub line_number: Option<usize>, // Line number where entry starts (for auditing)
}

/// Validate frontmatter semantics
pub fn validate_frontmatter(fm: &OkfFrontmatter) -> Result<(), String> {
    if fm.okf_version != "0.1" {
        return Err("okf_version must be '0.1'".to_string());
    }
    if fm.profile != "llm-wiki/1" {
        return Err("profile must be 'llm-wiki/1'".to_string());
    }
    if fm.title.trim().is_empty() {
        return Err("title cannot be empty".to_string());
    }
    // Validate ISO 8601 timestamps
    if chrono::DateTime::parse_from_rfc3339(&fm.created_at).is_err() {
        return Err("created_at is not valid ISO 8601".to_string());
    }
    if let Some(ref updated) = fm.updated_at {
        if chrono::DateTime::parse_from_rfc3339(updated).is_err() {
            return Err("updated_at is not valid ISO 8601".to_string());
        }
    }
    // Validate tags length and per-tag length
    if let Some(ref tags) = fm.tags {
        if tags.len() > 20 {
            return Err(format!("too many tags: {} (max 20)", tags.len()));
        }
        for tag in tags {
            if tag.len() > 50 {
                return Err(format!("tag exceeds 50 characters: {}", tag));
            }
        }
    }
    Ok(())
}

/// SHA-256 hash of a string
pub fn sha256_hash(s: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    hex::encode(hasher.finalize())
}

/// Characters that YAML double-quoted scalars must escape beyond the
/// ASCII classics: NEL/LS/PS (YAML line breaks) and everything libyaml's
/// reader rejects outright (C0, DEL, other C1, U+FFFE/U+FFFF).
fn has_escape_set_char(value: &str) -> bool {
    value.chars().any(|c| {
        matches!(
            c,
            '\u{85}' | '\u{2028}' | '\u{2029}' | '\u{FFFE}' | '\u{FFFF}'
        ) || c.is_control()
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
    matches!(chars.next(), Some('-' | '?' | ':')) && matches!(chars.next(), None | Some(' '))
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

/// Render frontmatter to YAML string
pub fn render_frontmatter(fm: &OkfFrontmatter) -> String {
    let mut doc = String::from("---\n");
    doc.push_str(&format!("okf_version: {}\n", fm.okf_version));
    doc.push_str(&format!("profile: {}\n", fm.profile));
    doc.push_str(&format!("title: {}\n", render_scalar(&fm.title)));
    doc.push_str(&format!("entity_type: {}\n", fm.entity_type));
    if let Some(ref tags) = fm.tags {
        if !tags.is_empty() {
            let tags_str = tags
                .iter()
                .map(|t| quote_for_note(t))
                .collect::<Vec<_>>()
                .join(", ");
            doc.push_str(&format!("tags: [{}]\n", tags_str));
        }
    }
    doc.push_str(&format!("created_at: {}\n", fm.created_at));
    if let Some(ref updated) = fm.updated_at {
        doc.push_str(&format!("updated_at: {}\n", updated));
    }
    if let Some(ref supersedes) = fm.supersedes {
        doc.push_str(&format!("supersedes: {}\n", render_scalar(supersedes)));
    }
    doc.push_str("---\n");
    doc
}

/// Parse frontmatter from YAML string
pub fn parse_frontmatter(yaml: &str) -> Result<OkfFrontmatter, String> {
    serde_yaml::from_str(yaml).map_err(|e| format!("failed to parse frontmatter: {}", e))
}

// The vault write path (note writes + index upserts) lives in `okf::write`
// (spec v2): ONE core, `safe_vault_path`, If-Match token staleness, atomic
// temp+rename, whole-line entry matching. Thin adapters live in `lib.rs`
// (Tauri commands) and `tool_dispatch.rs` (MCP dispatch).

/// Crate-visible test fixture: a minimal valid frontmatter with the given
/// title. Lives OUTSIDE `mod tests` so sibling modules' test blocks
/// (okf/write.rs) can `use super::super::test_fm_with_title`.
#[cfg(test)]
pub(crate) fn test_fm_with_title(title: &str) -> OkfFrontmatter {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_note_needs_quoting_timestamp_prefix_with_colon() {
        // The shared needs_quoting early-returns false on its loose
        // is_iso8601_timestamp shape check (frontmatter.rs:76) BEFORE the
        // ':' check at :103 — the write-path predicate must not.
        assert!(note_needs_quoting("2026-09-25T14:00:00Z: deploy retro"));
        assert!(note_needs_quoting("Deploy: retro"));
        assert!(note_needs_quoting("C# tips"));
        assert!(note_needs_quoting("Plan\u{2028}B")); // escape-set char only
        assert!(note_needs_quoting("-")); // lone leading indicator
        assert!(note_needs_quoting("?")); // lone leading indicator
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
        assert!(
            doc.contains("tags: [\"say \\\"hi\\\"\", \"ok-tag\"]\n"),
            "got: {doc}"
        );
    }

    #[test]
    fn test_render_frontmatter_new_quote_pins() {
        // Titles containing ':'/'#' anywhere gain quotes on the next write —
        // parse-equivalent, bytes change (accepted, spec §Design.1).
        for t in [
            "C# tips",
            "https://example.com",
            "Ratio 3:1",
            "2024",
            "yes",
            "2026-09-25T14:00:00Z: deploy retro",
        ] {
            let doc = render_frontmatter(&test_fm_with_title(t));
            assert!(
                doc.contains(&format!("title: \"{}\"\n", t)),
                "{t}: got {doc}"
            );
        }
    }

    #[test]
    fn test_entity_type_display() {
        assert_eq!(EntityType::Fact.to_string(), "fact");
        assert_eq!(EntityType::Task.to_string(), "task");
    }

    #[test]
    fn test_validate_frontmatter_valid() {
        let fm = OkfFrontmatter {
            okf_version: "0.1".to_string(),
            profile: "llm-wiki/1".to_string(),
            title: "Test Note".to_string(),
            entity_type: EntityType::Fact,
            tags: Some(vec!["tag1".to_string(), "tag2".to_string()]),
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: None,
            supersedes: None,
        };
        assert!(validate_frontmatter(&fm).is_ok());
    }

    #[test]
    fn test_validate_frontmatter_invalid_version() {
        let fm = OkfFrontmatter {
            okf_version: "0.2".to_string(),
            profile: "llm-wiki/1".to_string(),
            title: "Test Note".to_string(),
            entity_type: EntityType::Fact,
            tags: None,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: None,
            supersedes: None,
        };
        let result = validate_frontmatter(&fm);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("okf_version must be '0.1'"));
    }

    #[test]
    fn test_validate_frontmatter_invalid_profile() {
        let fm = OkfFrontmatter {
            okf_version: "0.1".to_string(),
            profile: "llm-wiki/2".to_string(),
            title: "Test Note".to_string(),
            entity_type: EntityType::Fact,
            tags: None,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: None,
            supersedes: None,
        };
        let result = validate_frontmatter(&fm);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("profile must be 'llm-wiki/1'"));
    }

    #[test]
    fn test_validate_frontmatter_empty_title() {
        let fm = OkfFrontmatter {
            okf_version: "0.1".to_string(),
            profile: "llm-wiki/1".to_string(),
            title: "   ".to_string(),
            entity_type: EntityType::Fact,
            tags: None,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: None,
            supersedes: None,
        };
        let result = validate_frontmatter(&fm);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("title cannot be empty"));
    }

    #[test]
    fn test_validate_frontmatter_invalid_timestamp() {
        let fm = OkfFrontmatter {
            okf_version: "0.1".to_string(),
            profile: "llm-wiki/1".to_string(),
            title: "Test Note".to_string(),
            entity_type: EntityType::Fact,
            tags: None,
            created_at: "not-a-date".to_string(),
            updated_at: None,
            supersedes: None,
        };
        let result = validate_frontmatter(&fm);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("created_at is not valid ISO 8601"));
    }

    #[test]
    fn test_validate_frontmatter_invalid_updated_at() {
        let fm = OkfFrontmatter {
            okf_version: "0.1".to_string(),
            profile: "llm-wiki/1".to_string(),
            title: "Test Note".to_string(),
            entity_type: EntityType::Fact,
            tags: None,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: Some("not-a-date".to_string()),
            supersedes: None,
        };
        let result = validate_frontmatter(&fm);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("updated_at is not valid ISO 8601"));
    }

    #[test]
    fn test_validate_frontmatter_tag_too_long() {
        let fm = OkfFrontmatter {
            okf_version: "0.1".to_string(),
            profile: "llm-wiki/1".to_string(),
            title: "Test Note".to_string(),
            entity_type: EntityType::Fact,
            tags: Some(vec!["a".repeat(101)]),
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: None,
            supersedes: None,
        };
        let result = validate_frontmatter(&fm);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("exceeds 50 characters"));
    }

    #[test]
    fn test_validate_frontmatter_too_many_tags() {
        let tags = (0..25).map(|i| format!("tag{}", i)).collect();
        let fm = OkfFrontmatter {
            okf_version: "0.1".to_string(),
            profile: "llm-wiki/1".to_string(),
            title: "Test Note".to_string(),
            entity_type: EntityType::Fact,
            tags: Some(tags),
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: None,
            supersedes: None,
        };
        let result = validate_frontmatter(&fm);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("too many tags: 25 (max 20)"));
    }

    #[test]
    fn test_sha256_hash() {
        let hash = sha256_hash("hello world");
        assert_eq!(
            hash,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn test_render_frontmatter() {
        let fm = OkfFrontmatter {
            okf_version: "0.1".to_string(),
            profile: "llm-wiki/1".to_string(),
            title: "Test Note".to_string(),
            entity_type: EntityType::Fact,
            tags: Some(vec!["tag1".to_string(), "tag2".to_string()]),
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: Some("2024-01-02T00:00:00Z".to_string()),
            supersedes: None,
        };
        let rendered = render_frontmatter(&fm);
        assert!(rendered.contains("---\n"));
        assert!(rendered.contains("okf_version: 0.1\n"));
        assert!(rendered.contains("profile: llm-wiki/1\n"));
        assert!(rendered.contains("title: Test Note\n"));
        assert!(rendered.contains("entity_type: fact\n"));
        assert!(rendered.contains("tags: [\"tag1\", \"tag2\"]\n"));
        assert!(rendered.contains("created_at: 2024-01-01T00:00:00Z\n"));
        assert!(rendered.contains("updated_at: 2024-01-02T00:00:00Z\n"));
    }

    #[test]
    fn test_parse_frontmatter() {
        let yaml = r#"okf_version: "0.1"
profile: "llm-wiki/1"
title: "Test Note"
entity_type: fact
tags: ["tag1", "tag2"]
created_at: "2024-01-01T00:00:00Z"
updated_at: "2024-01-02T00:00:00Z""#;
        let fm = parse_frontmatter(yaml).unwrap();
        assert_eq!(fm.okf_version, "0.1");
        assert_eq!(fm.profile, "llm-wiki/1");
        assert_eq!(fm.title, "Test Note");
        assert_eq!(fm.entity_type, EntityType::Fact);
        assert_eq!(fm.tags, Some(vec!["tag1".to_string(), "tag2".to_string()]));
        assert_eq!(fm.created_at, "2024-01-01T00:00:00Z");
        assert_eq!(fm.updated_at, Some("2024-01-02T00:00:00Z".to_string()));
    }
}
