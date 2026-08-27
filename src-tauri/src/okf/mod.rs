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
    pub appended: bool,  // true if entry was new, false if updated
    pub line_number: Option<usize>,  // Line number where entry starts (for auditing)
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

/// Render frontmatter to YAML string
pub fn render_frontmatter(fm: &OkfFrontmatter) -> String {
    let mut doc = String::from("---\n");
    doc.push_str(&format!("okf_version: {}\n", fm.okf_version));
    doc.push_str(&format!("profile: {}\n", fm.profile));
    doc.push_str(&format!("title: {}\n", fm.title));
    doc.push_str(&format!("entity_type: {}\n", fm.entity_type));
    if let Some(ref tags) = fm.tags {
        if !tags.is_empty() {
            let tags_str = tags
                .iter()
                .map(|t| format!("\"{}\"", t))
                .collect::<Vec<_>>()
                .join(", ");
            doc.push_str(&format!("tags: [{}]\n", tags_str));
        }
    }
    doc.push_str(&format!("created_at: {}\n", fm.created_at));
    if let Some(ref updated) = fm.updated_at {
        doc.push_str(&format!("updated_at: {}\n", updated));
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

#[cfg(test)]
mod tests {
    use super::*;

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
