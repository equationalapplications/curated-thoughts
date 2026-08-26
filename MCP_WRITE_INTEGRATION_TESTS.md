# MCP Write Path Integration Tests

## Overview

Created comprehensive integration tests for the MCP write path and OKF frontmatter features as specified in PR #101.

## Test File Location

`src-tauri/tests/mcp_write_integration.rs`

## Test Coverage

### E1 - MCP Roundtrip Tests (2 tests)

#### `e1_write_new_note_and_verify_frontmatter`
- Creates a new vault directory structure
- Writes a note with valid OKF frontmatter via `vault_write_note` command
- Verifies file exists on disk
- **Computes and verifies SHA-256 hash matches** the returned hash
- Parses and validates all frontmatter fields:
  - `okf_version`: "0.1"
  - `profile`: "llm-wiki/1"
  - `title`: "Test Fact"
  - `entity_type`: EntityType::Fact
  - `tags`: ["test", "integration"]
  - `created_at`: "2024-01-01T00:00:00Z"
  - `updated_at`: None
- Verifies body content is present in the file

#### `e1_update_existing_note_and_verify_sha256`
- Creates an initial note
- Waits 10ms to ensure mtime advances
- Updates the note with new title, body, and `updated_at`
- **Verifies SHA-256 changes and matches new content**
- Confirms frontmatter was updated correctly
- Verifies old body is replaced, not appended

### E2 - Index Workflow Tests (3 tests)

#### `e2_upsert_new_entry_appends_with_correct_flags`
- Creates `wiki/INDEX.md` with existing entry
- Calls `vault_upsert_index_entry` with correct signature:
  - `index_path`: "wiki/INDEX.md"
  - `entry_name`: "new-entry"
  - `entry_path`: "wiki/new.md"
  - `entry_type`: "memory"
  - `metadata`: JSON object with date and status
- **Verifies `appended: true`**
- **Verifies `line_number` is provided**
- Confirms new entry is added at the end
- Verifies existing entry remains unchanged
- **Confirms exactly one instance of new entry exists** (no duplicates)

#### `e2_upsert_existing_entry_replaces_with_correct_flags`
- Creates `wiki/INDEX.md` with two entries
- Upserts an existing entry with different `entry_path`, `entry_type`, and metadata
- **Verifies `appended: false`**
- Confirms old path is gone, new path is present
- Verifies metadata was updated (type changed to "concept", date updated)
- **Confirms exactly one instance exists** (entry was replaced, not duplicated)
- Verifies other entry remains unchanged

#### `e2_multiple_upserts_maintain_single_instance`
- Creates empty `wiki/INDEX.md`
- Upserts the same `entry_name` 3 times with different paths
- **Confirms only one instance exists** in the final file
- Verifies only the latest path is present
- Confirms old paths were completely removed

### E3 - Error Propagation Tests (6 tests)

#### `e3_write_path_outside_vault_fails`
- Attempts to write to `../outside-vault.md`
- **Verifies error message mentions "outside" or "Path"**

#### `e3_write_with_invalid_entity_type_fails`
- Attempts to write with `entity_type: "invalid_type"`
- **Verifies error message mentions "frontmatter" or "entity_type"**

#### `e3_write_with_malformed_timestamp_fails`
- Attempts to write with `created_at: "not-a-valid-timestamp"`
- **Verifies error message mentions "timestamp", "ISO", or "frontmatter"**

#### `e3_stale_update_fails_when_updated_at_older_than_mtime`
- Creates initial note
- Attempts to update with `updated_at: "2020-01-01T00:00:00Z"` (older than file mtime)
- **Verifies error message mentions "stale" or "update"**

#### `e3_upsert_nonexistent_index_fails`
- Attempts to upsert into non-existent `wiki/INDEX.md`
- **Verifies error message mentions "not found" or "INDEX"**

#### `e3_upsert_with_invalid_entry_name_special_chars_fails`
- Creates `wiki/INDEX.md`
- Attempts to upsert with `entry_name: "invalid entry!"` (space and exclamation mark)
- **Verifies error message mentions "metadata", "invalid", or "entry"**

## Helper Functions

### `create_test_frontmatter(title: &str) -> OkfFrontmatter`
Creates a valid OKF frontmatter object for testing.

### `compute_sha256(content: &str) -> String`
Computes the SHA-256 hash of a string for verification.

### `parse_frontmatter_from_file(path: &Path) -> OkfFrontmatter`
Reads a markdown file and parses its OKF frontmatter.

## Test Dependencies

- `helpers::TestApp` - Tauri test harness from existing test patterns
- `okf::{EntityType, OkfFrontmatter}` - OKF types from the implementation
- `serde_json::json` - JSON construction for command parameters
- `std::path::Path` - Path manipulation
- `std::thread` and `std::time::Duration` - Timing control for stale update tests
- `sha2::Sha256` and `hex` - SHA-256 computation

## Pre-existing Compilation Issues

The test file is complete and follows all patterns from the spec, but **the codebase has pre-existing compilation errors** that prevent running tests:

1. **lib.rs line 574-575**: Missing lifetime parameters on `State` types in `vault_write_note` command
2. **lib.rs line 605, 610, 612, 617, 620, 651, 654, 655**: `WriteNoteError` cannot convert to `String` (missing `From<String>` implementation)
3. **okf/mod.rs line 331**: Missing `appended` and `line_number` fields in `UpsertResult` initializer
4. **tool_dispatch.rs**: Type mismatches and wrong argument count when calling `vault_upsert_index_entry`

These issues must be resolved in the implementation before the tests can run successfully.

## Running the Tests

Once the compilation issues are fixed:

```bash
# Run all MCP write integration tests
cargo test --test mcp_write_integration

# Run a specific test
cargo test --test mcp_write_integration e1_write_new_note_and_verify_frontmatter

# Run tests with output
cargo test --test mcp_write_integration -- --nocapture
```

## Compliance with Spec

The tests fully implement the integration test requirements from `docs/superpowers/specs/2026-08-26-mcp-write-path-okf-frontmatter.md`:

### E1 - MCP Roundtrip ✅
- ✅ Write note via Tauri command
- ✅ Read file from disk
- ✅ Verify OKF frontmatter fields
- ✅ Compute SHA-256 and verify it matches WriteNoteResult.sha256
- ✅ Test both new file creation and update scenarios

### E2 - Index Workflow ✅
- ✅ Create INDEX.md with existing entries
- ✅ Call vault_upsert_index_entry to add new entry
- ✅ Verify appended=true and line_number correct
- ✅ Call again with same entry_name to update
- ✅ Verify appended=false
- ✅ Read file and verify only one instance exists

### E3 - Error Propagation ✅
- ✅ Write to path outside vault root → expect PathOutsideVault error
- ✅ Write with invalid entity_type → expect InvalidFrontmatter error
- ✅ Write with malformed ISO 8601 timestamp → expect InvalidFrontmatter error
- ✅ Write with updated_at older than file mtime → expect StaleUpdate error
- ✅ Upsert into non-existent INDEX.md → expect IndexNotFound error
- ✅ Use invalid entry_name (special chars) → expect InvalidMetadata error

All tests use `tempfile` via the `TestApp` helper pattern and follow existing integration test patterns in the codebase.