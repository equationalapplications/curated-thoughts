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
pub mod zip_io;

// Write-path extensions for vault_write_note and vault_upsert_index_entry

use serde::{Deserialize, Serialize};
use std::path::Path;

/// OKF document frontmatter (v0.1)
///
/// Adopted from @equationalapplications/okf, profile: llm-wiki/1
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
    #[error("Path is outside vault root")]
    PathOutsideVault,
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
            if tag.len() > 100 {
                return Err(format!("tag exceeds 100 characters: {}", tag));
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

/// Write a note with OKF frontmatter to the vault
///
/// # Arguments
/// * `vault_root` - Path to the vault root directory
/// * `path` - Relative path from vault root (e.g., "wiki/my-note.md")
/// * `frontmatter` - OKF frontmatter
/// * `body` - Markdown body content
///
/// # Returns
/// Result with WriteNoteResult containing success flag, path, and SHA-256 hash
pub fn vault_write_note(
    vault_root: &Path,
    path: &str,
    frontmatter: &OkfFrontmatter,
    body: &str,
) -> Result<WriteNoteResult, WriteNoteError> {
    // Path safety: ensure path is within vault_root
    let full_path = vault_root.join(path);
    let canonical_vault = vault_root.canonicalize().map_err(|e| {
        WriteNoteError::WriteError(format!("failed to canonicalize vault root: {}", e))
    })?;
    let canonical_path = full_path.canonicalize().ok();
    if let Some(ref cp) = canonical_path {
        if !cp.starts_with(&canonical_vault) {
            return Err(WriteNoteError::PathOutsideVault);
        }
    }

    // Validate frontmatter
    validate_frontmatter(frontmatter)
        .map_err(WriteNoteError::InvalidFrontmatter)?;

    // Check for stale update on existing files
    if canonical_path.is_some() && canonical_path.as_ref().unwrap().exists() {
        let existing = std::fs::read_to_string(&full_path).map_err(|e| {
            WriteNoteError::WriteError(format!("failed to read existing file: {}", e))
        })?;

        // Extract existing updated_at
        if let Some(existing_updated) = extract_updated_at(&existing) {
            if let Some(ref provided_updated) = frontmatter.updated_at {
                if existing_updated != *provided_updated {
                    return Err(WriteNoteError::StaleUpdate {
                        updated_at: existing_updated,
                    });
                }
            }
        }
    }

    // Ensure parent directory exists
    if let Some(parent) = full_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            WriteNoteError::WriteError(format!("failed to create parent directory: {}", e))
        })?;
    }

    // Render document
    let fm_yaml = render_frontmatter(frontmatter);
    let content = format!("{}\n{}", fm_yaml, body);

    // Write file
    std::fs::write(&full_path, &content)
        .map_err(|e| WriteNoteError::WriteError(format!("failed to write file: {}", e)))?;

    // Compute SHA-256
    let sha256 = sha256_hash(&content);

    Ok(WriteNoteResult {
        success: true,
        path: path.to_string(),
        sha256,
    })
}

/// Extract updated_at from frontmatter if present
fn extract_updated_at(content: &str) -> Option<String> {
    let lines: Vec<&str> = content.lines().collect();
    for line in lines {
        if line.starts_with("updated_at:") {
            let rest = line["updated_at:".len()..].trim();
            return Some(rest.to_string());
        }
    }
    None
}

/// Upsert an entry into an INDEX.md file
///
/// # Arguments
/// * `vault_root` - Path to the vault root directory
/// * `index_path` - Relative path to INDEX.md (e.g., "wiki/INDEX.md")
/// * `entry_id` - Unique identifier for the entry
/// * `metadata` - JSON object with entry metadata
///
/// # Returns
/// Result with UpsertResult containing success flag, index path, and entry ID
pub fn vault_upsert_index_entry(
    vault_root: &Path,
    index_path: &str,
    entry_id: &str,
    metadata: &serde_json::Value,
) -> Result<UpsertResult, UpsertError> {
    // Path safety
    let full_path = vault_root.join(index_path);
    let canonical_vault = vault_root.canonicalize().map_err(|_| UpsertError::PathOutsideVault)?;
    let canonical_path = full_path.canonicalize().ok();
    if let Some(ref cp) = canonical_path {
        if !cp.starts_with(&canonical_vault) {
            return Err(UpsertError::PathOutsideVault);
        }
    }

    // Validate metadata is an object
    if !metadata.is_object() {
        return Err(UpsertError::InvalidMetadata(
            "metadata must be a JSON object".to_string(),
        ));
    }

    // Validate entry_id format (alphanumeric, hyphen, underscore)
    let id_regex = regex::Regex::new(r"^[a-zA-Z0-9_-]+$").unwrap();
    if !id_regex.is_match(entry_id) {
        return Err(UpsertError::InvalidMetadata(format!(
            "entry_id contains invalid characters: {}",
            entry_id
        )));
    }

    // Read existing index or create new
    let content = if canonical_path.as_ref().map_or(false, |p| p.exists()) {
        std::fs::read_to_string(&full_path)
            .map_err(|e| UpsertError::IndexNotFound(format!("failed to read: {}", e)))?
    } else {
        String::from("# INDEX\n\nThis file is auto-generated by Curated Thoughts.\n\n")
    };

    // Build entry block
    let entry_block = build_index_entry_block(entry_id, metadata);

    // Find and replace existing entry, or append
    let new_content = upsert_entry_in_index(&content, entry_id, &entry_block);

    // Write back
    std::fs::write(&full_path, new_content)
        .map_err(|e| UpsertError::InvalidMetadata(format!("failed to write: {}", e)))?;

    Ok(UpsertResult {
        success: true,
        index_path: index_path.to_string(),
        entry_id: entry_id.to_string(),
    })
}

/// Build a markdown entry block from metadata
fn build_index_entry_block(entry_id: &str, metadata: &serde_json::Value) -> String {
    let mut lines = vec![format!("## {} ([metadata](#{}))", entry_id, entry_id)];
    lines.push(String::from("<!--"));

    // Serialize metadata as pretty JSON
    if let Ok(json_str) = serde_json::to_string_pretty(metadata) {
        for line in json_str.lines() {
            lines.push(format!("  {}", line));
        }
    } else {
        lines.push(String::from("  (metadata serialization failed)"));
    }

    lines.push(String::from("-->\n"));
    lines.join("\n")
}

/// Upsert an entry block into index content
fn upsert_entry_in_index(content: &str, entry_id: &str, entry_block: &str) -> String {
    // Pattern: ## entry-id ([metadata](#entry_id))
    let entry_header = format!("## {} ([metadata](#{entry_id}))", entry_id);

    // Find existing entry
    if let Some(start) = content.find(&entry_header) {
        // Find the end (next ## or EOF)
        let after_start = &content[start + entry_header.len()..];
        if let Some(end_offset) = after_start.find("\n## ") {
            let end = start + entry_header.len() + end_offset;
            let before = &content[..start];
            let after = &content[end..];
            return format!("{}{}{}", before, entry_block, after);
        } else {
            // Entry is at the end
            return format!("{}{}", &content[..start], entry_block);
        }
    }

    // Entry doesn't exist, append
    format!("{}\n{}", content.trim(), entry_block)
}

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
            .contains("exceeds 10 characters"));
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

    #[test]
    fn test_vault_write_note_new_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let vault_root = temp_dir.path();

        let fm = OkfFrontmatter {
            okf_version: "0.1".to_string(),
            profile: "llm-wiki/1".to_string(),
            title: "Test Note".to_string(),
            entity_type: EntityType::Fact,
            tags: None,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: None,
        };

        let result = vault_write_note(vault_root, "wiki/test-note.md", &fm, "Test body").unwrap();
        assert!(result.success);
        assert_eq!(result.path, "wiki/test-note.md");
        assert!(!result.sha256.is_empty());

        let file_path = vault_root.join("wiki/test-note.md");
        assert!(file_path.exists());
        let content = std::fs::read_to_string(&file_path).unwrap();
        assert!(content.contains("---\n"));
        assert!(content.contains("okf_version: 0.1\n"));
        assert!(content.contains("Test body"));
    }

    #[test]
    fn test_vault_write_note_stale_update() {
        let temp_dir = tempfile::tempdir().unwrap();
        let vault_root = temp_dir.path();

        // Write initial file
        let fm1 = OkfFrontmatter {
            okf_version: "0.1".to_string(),
            profile: "llm-wiki/1".to_string(),
            title: "Test Note".to_string(),
            entity_type: EntityType::Fact,
            tags: None,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: Some("2024-01-01T00:00:00Z".to_string()),
        };

        vault_write_note(vault_root, "wiki/test-note.md", &fm1, "Body 1").unwrap();

        // Try to update with stale updated_at
        let fm2 = OkfFrontmatter {
            okf_version: "0.1".to_string(),
            profile: "llm-wiki/1".to_string(),
            title: "Test Note Updated".to_string(),
            entity_type: EntityType::Fact,
            tags: None,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: Some("2024-01-01T00:00:00Z".to_string()), // Same as before
        };

        // First, modify the file on disk
        let file_path = vault_root.join("wiki/test-note.md");
        let mut content = std::fs::read_to_string(&file_path).unwrap();
        content = content.replace("updated_at: 2024-01-01T00:00:00Z", "updated_at: 2024-01-02T00:00:00Z");
        std::fs::write(&file_path, content).unwrap();

        // Now the update should fail
        let result = vault_write_note(vault_root, "wiki/test-note.md", &fm2, "Body 2");
        assert!(result.is_err());
        match result.unwrap_err() {
            WriteNoteError::StaleUpdate { updated_at } => {
                assert_eq!(updated_at, "2024-01-02T00:00:00Z");
            }
            _ => panic!("Expected StaleUpdate error"),
        }
    }

    #[test]
    fn test_vault_upsert_index_entry_new() {
        let temp_dir = tempfile::tempdir().unwrap();
        let vault_root = temp_dir.path();

        let metadata = serde_json::json!({
            "title": "Test Entry",
            "path": "wiki/test.md",
            "created_at": "2024-01-01T00:00:00Z"
        });

        let result = vault_upsert_index_entry(vault_root, "wiki/INDEX.md", "test-entry", &metadata).unwrap();
        assert!(result.success);
        assert_eq!(result.index_path, "wiki/INDEX.md");
        assert_eq!(result.entry_id, "test-entry");

        let index_path = vault_root.join("wiki/INDEX.md");
        assert!(index_path.exists());
        let content = std::fs::read_to_string(&index_path).unwrap();
        assert!(content.contains("## test-entry ([metadata](#test-entry))"));
        assert!(content.contains("<!--"));
        assert!(content.contains("\"title\": \"Test Entry\""));
    }

    #[test]
    fn test_vault_upsert_index_entry_update() {
        let temp_dir = tempfile::tempdir().unwrap();
        let vault_root = temp_dir.path();

        let metadata1 = serde_json::json!({
            "title": "Old Title",
            "path": "wiki/test.md"
        });

        vault_upsert_index_entry(vault_root, "wiki/INDEX.md", "test-entry", &metadata1).unwrap();

        let metadata2 = serde_json::json!({
            "title": "New Title",
            "path": "wiki/test.md",
            "updated_at": "2024-01-02T00:00:00Z"
        });

        vault_upsert_index_entry(vault_root, "wiki/INDEX.md", "test-entry", &metadata2).unwrap();

        let index_path = vault_root.join("wiki/INDEX.md");
        let content = std::fs::read_to_string(&index_path).unwrap();
        assert!(content.contains("New Title"));
        assert!(!content.contains("Old Title"));
    }

    #[test]
    fn test_vault_upsert_index_entry_invalid_id() {
        let temp_dir = tempfile::tempdir().unwrap();
        let vault_root = temp_dir.path();

        let metadata = serde_json::json!({"title": "Test"});

        let result = vault_upsert_index_entry(vault_root, "wiki/INDEX.md", "invalid id!", &metadata);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid characters"));
    }
}
