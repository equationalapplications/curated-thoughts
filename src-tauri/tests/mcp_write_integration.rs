//! Integration tests for MCP write path and OKF frontmatter features
//!
//! Tests cover:
//! - E1: MCP roundtrip (write note, verify frontmatter and SHA-256)
//! - E2: Index workflow (upsert entries, verify no duplicates)
//! - E3: Error propagation (path safety, validation, stale updates)

mod helpers;

use helpers::TestApp;
use tauri_app_lib::okf::{EntityType, OkfFrontmatter};
use serde_json::json;
use std::path::Path;
use std::thread;
use std::time::Duration;

/// Helper: create valid OKF frontmatter for testing
fn create_test_frontmatter(title: &str) -> OkfFrontmatter {
    OkfFrontmatter {
        okf_version: "0.1".to_string(),
        profile: "llm-wiki/1".to_string(),
        title: title.to_string(),
        entity_type: EntityType::Fact,
        tags: Some(vec!["test".to_string(), "integration".to_string()]),
        created_at: "2024-01-01T00:00:00Z".to_string(),
        updated_at: None,
    }
}

/// Helper: compute SHA-256 hash of a string
fn compute_sha256(content: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    hex::encode(hasher.finalize())
}

/// Helper: parse frontmatter from a markdown file
fn parse_frontmatter_from_file(path: &Path) -> OkfFrontmatter {
    let content = std::fs::read_to_string(path).expect("failed to read file");
    let lines: Vec<&str> = content.lines().collect();

    // Find frontmatter boundaries
    let start_idx = lines
        .iter()
        .position(|l| *l == "---")
        .expect("no frontmatter start");
    let end_idx = lines[start_idx + 1..]
        .iter()
        .position(|l| *l == "---")
        .expect("no frontmatter end")
        + start_idx
        + 1;

    let frontmatter_yaml = lines[start_idx + 1..end_idx].join("\n");
    tauri_app_lib::okf::parse_frontmatter(&frontmatter_yaml).expect("failed to parse frontmatter")
}

// ============================================================================
// E1 - MCP Roundtrip Tests
// ============================================================================

#[test]
fn e1_write_new_note_and_verify_frontmatter() {
    let app = TestApp::new();

    let vault_root = app.tmp.path().join("vault");

    // Create vault directory structure first
    let wiki_dir = vault_root.join("wiki");
    std::fs::create_dir_all(&wiki_dir).expect("failed to create wiki directory");

    // Set vault path - REQUIRED before vault_write_note
    app.invoke::<()>("set_vault_path", json!({ "path": vault_root }));

    let note_path = "wiki/test-fact.md";
    let frontmatter = create_test_frontmatter("Test Fact");
    let body = "This is the body of the test fact.";

    // Write the note via Tauri command
    let result: serde_json::Value = app.invoke(
        "vault_write_note",
        json!({
            "path": note_path,
            "frontmatter": frontmatter,
            "body": body,
        }),
    );

    assert_eq!(result["success"], true, "write should succeed");

    let sha256_from_result = result["sha256"]
        .as_str()
        .expect("sha256 should be a string");
    assert!(!sha256_from_result.is_empty(), "sha256 should not be empty");

    // Read the file from disk
    let full_path = vault_root.join(note_path);
    assert!(full_path.exists(), "file should exist at {:?}", full_path);

    let file_content = std::fs::read_to_string(&full_path).expect("failed to read file");

    // Verify SHA-256 matches
    let computed_sha256 = compute_sha256(&file_content);
    assert_eq!(
        computed_sha256, sha256_from_result,
        "SHA-256 from result '{}' should match computed '{}'",
        sha256_from_result, computed_sha256
    );

    // Parse and verify frontmatter
    let parsed_fm = parse_frontmatter_from_file(&full_path);
    assert_eq!(parsed_fm.okf_version, "0.1");
    assert_eq!(parsed_fm.profile, "llm-wiki/1");
    assert_eq!(parsed_fm.title, "Test Fact");
    assert_eq!(parsed_fm.entity_type, EntityType::Fact);
    assert_eq!(
        parsed_fm.tags,
        Some(vec!["test".to_string(), "integration".to_string()])
    );
    assert_eq!(parsed_fm.created_at, "2024-01-01T00:00:00Z");
    assert!(parsed_fm.updated_at.is_none());

    // Verify body is present
    assert!(
        file_content.contains(body),
        "file should contain the body"
    );
}

#[test]
fn e1_update_existing_note_and_verify_sha256() {
    let app = TestApp::new();

    let vault_root = app.tmp.path().join("vault");

    // Create vault directory structure first
    let wiki_dir = vault_root.join("wiki");
    std::fs::create_dir_all(&wiki_dir).expect("failed to create wiki directory");

    // Set vault path - REQUIRED before vault_write_note
    app.invoke::<()>("set_vault_path", json!({ "path": vault_root }));

    let note_path = "wiki/test-updated.md";
    let body = "Initial body";
    let updated_body = "Updated body";

    // Create initial note
    let initial_fm = create_test_frontmatter("Initial Title");
    app.invoke::<serde_json::Value>("vault_write_note",
        json!({
            "path": note_path,
            "frontmatter": initial_fm,
            "body": body,
        }),
    );

    // Wait a moment to ensure mtime advances
    thread::sleep(Duration::from_millis(10));

    // Update the note
    let updated_at = "2024-01-01T00:00:01Z".to_string();
    let updated_fm = OkfFrontmatter {
        title: "Updated Title".to_string(),
        updated_at: Some(updated_at.clone()),
        ..create_test_frontmatter("Updated Title")
    };

    let result: serde_json::Value = app.invoke(
        "vault_write_note",
        json!({
            "path": note_path,
            "frontmatter": updated_fm,
            "body": updated_body,
        }),
    );

    assert_eq!(result["success"], true, "update should succeed");

    let updated_sha256 = result["sha256"]
        .as_str()
        .expect("sha256 should be a string");
    assert!(!updated_sha256.is_empty(), "sha256 should not be empty");

    let full_path = vault_root.join(note_path);
    let file_content = std::fs::read_to_string(&full_path).expect("failed to read file");

    // Verify SHA-256 matches updated content
    let computed_sha256 = compute_sha256(&file_content);
    assert_eq!(
        computed_sha256, updated_sha256,
        "SHA-256 from result '{}' should match computed '{}'",
        updated_sha256, computed_sha256
    );

    // Verify frontmatter was updated
    let parsed_fm = parse_frontmatter_from_file(&full_path);
    assert_eq!(parsed_fm.title, "Updated Title");
    assert_eq!(parsed_fm.updated_at, Some(updated_at));

    let file_content = std::fs::read_to_string(&full_path).expect("failed to read file");
    assert!(
        file_content.contains(updated_body),
        "file should contain updated body"
    );
    assert!(
        !file_content.contains(body),
        "file should not contain old body"
    );
}

// ============================================================================
// E2 - Index Workflow Tests
// ============================================================================

#[test]
fn e2_upsert_new_entry_appends_with_correct_flags() {
    let app = TestApp::new();

    let vault_root = app.tmp.path().join("vault");

    // Create wiki directory
    let wiki_dir = vault_root.join("wiki");
    std::fs::create_dir_all(&wiki_dir).expect("failed to create wiki directory");

    // Set vault path - REQUIRED before vault_upsert_index_entry
    app.invoke::<()>("set_vault_path", json!({ "path": vault_root }));

    // Create an INDEX.md with existing entry
    let index_path = vault_root.join("wiki/INDEX.md");
    std::fs::write(
        &index_path,
        "# INDEX\nThis file is auto-generated by Curated Thoughts.\n\n## existing-entry\n[[wiki/existing.md]]\n- Type: memory",
    )
    .expect("failed to create INDEX.md");

    // Upsert a new entry
    let result: serde_json::Value = app.invoke(
        "vault_upsert_index_entry",
        json!({
            "indexPath": "wiki/INDEX.md",
            "entryName": "new-entry",
            "entryPath": "wiki/new.md",
            "entryType": "memory",
            "metadata": json!({"date": "2024-01-01", "status": "active"})
        }),
    );

    assert_eq!(result["success"], true, "upsert should succeed");
    assert_eq!(result["appended"], true, "should be appended (new entry)");

    // Read file and verify entry was appended
    let content = std::fs::read_to_string(&index_path).expect("failed to read INDEX.md");
    assert!(
        content.contains("## existing-entry"),
        "existing entry should remain"
    );
    assert!(content.contains("## new-entry"), "new entry should be added");

    // Verify only one instance of the new entry exists
    let new_entry_count = content.matches("## new-entry").count();
    assert_eq!(
        new_entry_count, 1,
        "should have exactly one instance of new-entry"
    );
}

#[test]
fn e2_upsert_existing_entry_replaces_with_correct_flags() {
    let app = TestApp::new();

    let vault_root = app.tmp.path().join("vault");

    // Create wiki directory
    let wiki_dir = vault_root.join("wiki");
    std::fs::create_dir_all(&wiki_dir).expect("failed to create wiki directory");

    // Set vault path - REQUIRED before vault_upsert_index_entry
    app.invoke::<()>("set_vault_path", json!({ "path": vault_root }));

    // Create an INDEX.md with existing entry
    let index_path = vault_root.join("wiki/INDEX.md");
    std::fs::write(
        &index_path,
        "# INDEX\n\nThis file is auto-generated by Curated Thoughts.\n\n## entry-to-update\n[[wiki/old-path.md]]\n- Type: memory\n\n## other-entry\n[[wiki/other.md]]\n- Type: memory\n",
    )
    .expect("failed to create INDEX.md");

    // Upsert the same entry with different path and type
    let result: serde_json::Value = app.invoke(
        "vault_upsert_index_entry",
        json!({
            "indexPath": "wiki/INDEX.md",
            "entryName": "entry-to-update",
            "entryPath": "wiki/new-path.md",
            "entryType": "concept",
            "metadata": json!({"date": "2024-01-02", "status": "inactive"})
        }),
    );

    assert_eq!(result["success"], true, "upsert should succeed");
    assert_eq!(
        result["appended"], false,
        "should be replaced (existing entry)"
    );

    // Read file and verify entry was replaced
    let content = std::fs::read_to_string(&index_path).expect("failed to read INDEX.md");

    // Verify the old path is gone and new path is present
    assert!(
        !content.contains("old-path.md"),
        "old path should be replaced"
    );
    assert!(content.contains("new-path.md"), "new path should be present");

    // Verify metadata was updated
    assert!(content.contains("Type: concept"), "type should be updated");
    assert!(content.contains("2024-01-02"), "date should be updated");

    // Verify only one instance exists
    let entry_count = content.matches("## entry-to-update").count();
    assert_eq!(
        entry_count, 1,
        "should have exactly one instance of entry-to-update"
    );

    // Verify other entry is unchanged
    assert!(content.contains("## other-entry"), "other entry should remain");
    assert!(
        content.contains("wiki/other.md"),
        "other entry path should remain"
    );
}

#[test]
fn e2_multiple_upserts_maintain_single_instance() {
    let app = TestApp::new();

    let vault_root = app.tmp.path().join("vault");

    // Create wiki directory
    let wiki_dir = vault_root.join("wiki");
    std::fs::create_dir_all(&wiki_dir).expect("failed to create wiki directory");

    // Set vault path - REQUIRED before vault_upsert_index_entry
    app.invoke::<()>("set_vault_path", json!({ "path": vault_root }));

    // Create an INDEX.md
    let index_path = vault_root.join("wiki/INDEX.md");
    std::fs::write(
        &index_path,
        "# INDEX\n\nThis file is auto-generated by Curated Thoughts.\n",
    )
    .expect("failed to create INDEX.md");

    // Upsert the same entry multiple times with different paths
    for i in 0..3 {
        app.invoke::<serde_json::Value>("vault_upsert_index_entry",
            json!({
                "indexPath": "wiki/INDEX.md",
                "entryName": "multi-update",
                "entryPath": format!("wiki/note-{}.md", i),
                "entryType": "memory",
                "metadata": json!({"iteration": i})
            }),
        );
    }

    // Read file and verify only one instance exists
    let content = std::fs::read_to_string(&index_path).expect("failed to read INDEX.md");
    let entry_count = content.matches("## multi-update").count();
    assert_eq!(
        entry_count, 1,
        "should have exactly one instance after multiple updates"
    );

    // Verify final state
    assert!(content.contains("wiki/note-2.md"), "should have latest path");
    assert!(!content.contains("wiki/note-0.md"), "should not have old path 0");
    assert!(!content.contains("wiki/note-1.md"), "should not have old path 1");
}

// ============================================================================
// E3 - Error Propagation Tests
// ============================================================================

#[test]
fn e3_write_path_outside_vault_fails() {
    let app = TestApp::new();

    let vault_root = app.tmp.path().join("vault");
    std::fs::create_dir_all(&vault_root).expect("failed to create vault directory");

    // Set vault path - so the path traversal check runs
    app.invoke::<()>("set_vault_path", json!({ "path": vault_root }));

    let frontmatter = create_test_frontmatter("Escaping Path");

    let result: Result<serde_json::Value, _> = app.invoke_result(
        "vault_write_note",
        json!({
            "path": "../outside-vault.md",
            "frontmatter": frontmatter,
            "body": "This should fail",
        }),
    );

    assert!(result.is_err(), "writing outside vault should fail");
    let err = result.unwrap_err();
    let err_str = err.to_string();
    assert!(
        err_str.contains("outside") || err_str.contains("Path"),
        "error should mention path: {}",
        err_str
    );
}

#[test]
fn e3_write_with_invalid_entity_type_fails() {
    let app = TestApp::new();

    let vault_root = app.tmp.path().join("vault");

    // Create vault directory structure first
    let wiki_dir = vault_root.join("wiki");
    std::fs::create_dir_all(&wiki_dir).expect("failed to create wiki directory");

    // Set vault path - REQUIRED before vault_write_note
    app.invoke::<()>("set_vault_path", json!({ "path": vault_root }));

    // Create invalid frontmatter with wrong entity_type as JSON
    let invalid_frontmatter = json!({
        "okf_version": "0.1",
        "profile": "llm-wiki/1",
        "title": "Invalid Type",
        "entity_type": "invalid_type",
        "created_at": "2024-01-01T00:00:00Z"
    });

    let result: Result<serde_json::Value, _> = app.invoke_result(
        "vault_write_note",
        json!({
            "path": "wiki/invalid.md",
            "frontmatter": invalid_frontmatter,
            "body": "This should fail",
        }),
    );

    assert!(result.is_err(), "invalid entity_type should fail");
    let err = result.unwrap_err();
    let err_str = err.to_string();
    assert!(
        err_str.contains("frontmatter") || err_str.contains("entity_type"),
        "error should mention frontmatter: {}",
        err_str
    );
}

#[test]
fn e3_write_with_malformed_timestamp_fails() {
    let app = TestApp::new();

    let vault_root = app.tmp.path().join("vault");

    // Create vault directory structure first
    let wiki_dir = vault_root.join("wiki");
    std::fs::create_dir_all(&wiki_dir).expect("failed to create wiki directory");

    // Set vault path - REQUIRED before vault_write_note
    app.invoke::<()>("set_vault_path", json!({ "path": vault_root }));

    // Create invalid frontmatter with malformed timestamp
    let invalid_frontmatter = json!({
        "okf_version": "0.1",
        "profile": "llm-wiki/1",
        "title": "Bad Timestamp",
        "entity_type": "fact",
        "created_at": "not-a-valid-timestamp"
    });

    let result: Result<serde_json::Value, _> = app.invoke_result(
        "vault_write_note",
        json!({
            "path": "wiki/bad-timestamp.md",
            "frontmatter": invalid_frontmatter,
            "body": "This should fail",
        }),
    );

    assert!(result.is_err(), "malformed timestamp should fail");
    let err = result.unwrap_err();
    let err_str = err.to_string();
    assert!(
        err_str.contains("frontmatter") || err_str.contains("ISO") || err_str.contains("timestamp"),
        "error should mention timestamp or frontmatter: {}",
        err_str
    );
}

#[test]
fn e3_stale_update_fails_when_updated_at_older_than_mtime() {
    let app = TestApp::new();

    let vault_root = app.tmp.path().join("vault");

    // Create vault directory structure first
    let wiki_dir = vault_root.join("wiki");
    std::fs::create_dir_all(&wiki_dir).expect("failed to create wiki directory");

    // Set vault path - REQUIRED before vault_write_note
    app.invoke::<()>("set_vault_path", json!({ "path": vault_root }));

    let note_path = "wiki/stale-update.md";

    // Create initial note
    let initial_fm = create_test_frontmatter("Stale Test");
    app.invoke::<serde_json::Value>("vault_write_note",
        json!({
            "path": note_path,
            "frontmatter": initial_fm,
            "body": "Initial",
        }),
    );

    // Try to update with an older updated_at (in the past)
    let old_updated_at = "2020-01-01T00:00:00Z".to_string();
    let stale_fm = OkfFrontmatter {
        title: "Stale Update".to_string(),
        updated_at: Some(old_updated_at.clone()),
        ..create_test_frontmatter("Stale Update")
    };

    let result: Result<serde_json::Value, _> = app.invoke_result(
        "vault_write_note",
        json!({
            "path": note_path,
            "frontmatter": stale_fm,
            "body": "Stale body",
        }),
    );

    assert!(result.is_err(), "stale update should fail");
    let err = result.unwrap_err();
    let err_str = err.to_string();
    assert!(
        err_str.contains("stale") || err_str.contains("update"),
        "error should mention stale update: {}",
        err_str
    );
}

#[test]
fn e3_upsert_nonexistent_index_fails() {
    let app = TestApp::new();

    let vault_root = app.tmp.path().join("vault");

    // Create wiki directory but no INDEX.md
    let wiki_dir = vault_root.join("wiki");
    std::fs::create_dir_all(&wiki_dir).expect("failed to create wiki directory");

    // Set vault path - REQUIRED before vault_upsert_index_entry
    app.invoke::<()>("set_vault_path", json!({ "path": vault_root }));

    let result: Result<serde_json::Value, _> = app.invoke_result(
        "vault_upsert_index_entry",
        json!({
            "indexPath": "wiki/INDEX.md",
            "entryName": "new-entry",
            "entryPath": "wiki/new.md",
            "entryType": "memory",
            "metadata": json!({"date": "2024-01-01"})
        }),
    );

    assert!(result.is_err(), "upsert into non-existent index should fail");
    let err = result.unwrap_err();
    let err_str = err.to_string();
    assert!(
        err_str.contains("not found") || err_str.contains("INDEX"),
        "error should mention index not found: {}",
        err_str
    );
}

#[test]
fn e3_upsert_with_invalid_entry_name_special_chars_fails() {
    let app = TestApp::new();

    let vault_root = app.tmp.path().join("vault");

    // Create wiki directory and INDEX.md
    let wiki_dir = vault_root.join("wiki");
    std::fs::create_dir_all(&wiki_dir).expect("failed to create wiki directory");

    // Set vault path - REQUIRED before vault_upsert_index_entry
    app.invoke::<()>("set_vault_path", json!({ "path": vault_root }));

    let index_path = vault_root.join("wiki/INDEX.md");
    std::fs::write(
        &index_path,
        "# INDEX\n\nThis file is auto-generated by Curated Thoughts.\n",
    )
    .expect("failed to create INDEX.md");

    // Try to upsert with special characters in entry_name
    let result: Result<serde_json::Value, _> = app.invoke_result(
        "vault_upsert_index_entry",
        json!({
            "indexPath": "wiki/INDEX.md",
            "entryName": "invalid entry!", // contains space and exclamation
            "entryPath": "wiki/new.md",
            "entryType": "memory",
            "metadata": json!({"date": "2024-01-01"})
        }),
    );

    assert!(result.is_err(), "invalid entry_name should fail");
    let err = result.unwrap_err();
    let err_str = err.to_string();
    assert!(
        err_str.contains("metadata") || err_str.contains("invalid") || err_str.contains("entry"),
        "error should mention invalid metadata or entry name: {}",
        err_str
    );
}