# MCP Write Tools & OKF Frontmatter

Curated Thoughts v0.1 adds two new MCP tools for writing vault content with standardized OKF frontmatter: `vault_write_note` and `vault_upsert_index_entry`.

## Overview

These tools enable AI agents (via MCP) to write markdown files to your vault with structured metadata. They:

- **Validate frontmatter** before writing (fail fast on invalid inputs)
- **Prevent lost updates** with If-Match content tokens (`updated_at` must exactly match the file's current token)
- **Ensure path safety** by rejecting paths outside the vault
- **Provide atomic writes** to avoid partial/corrupted files

## Tool 1: `vault_write_note`

Write a markdown note with OKF v0.1 frontmatter to your vault.

### Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `path` | `string` | Yes | Vault-relative path (e.g., `"memories/my-fact.md"`) |
| `frontmatter` | `object` | Yes | OKF v0.1 frontmatter object (see schema below) |
| `body` | `string` | Yes | Markdown body content |

### Frontmatter Schema

```json
{
  "okf_version": "0.1",
  "profile": "llm-wiki/1",
  "title": "string",
  "entity_type": "fact|task|event|concept|doc",
  "tags": ["string"],           // Optional, max 20 tags, max 100 chars each
  "created_at": "ISO 8601",     // Required for new files
  "updated_at": "ISO 8601"      // Required for edits (stale update detection)
}
```

### Validation Rules

- `okf_version` must be exactly `"0.1"`
- `profile` must be exactly `"llm-wiki/1"`
- `title` must be non-empty (after trimming)
- `entity_type` must be one of: `fact`, `task`, `event`, `concept`, `doc`
- `created_at` and `updated_at` must parse as ISO 8601 (e.g., `"2026-08-26T12:34:56Z"`)
- `tags` array: max 20 tags, max 50 characters per tag
- Unknown fields are rejected (strict schema)

### Return Value

```json
{
  "success": true,
  "path": "string",      // Vault-relative path (as provided)
  "sha256": "string"     // SHA-256 hash of written content
}
```

### Error Cases

| Error | Description |
|-------|-------------|
| `path_outside_vault` | Path contains `..` or is outside vault root |
| `invalid_frontmatter` | Frontmatter validation failed (details in message) |
| `stale_update` | Provided `updated_at` does not exactly match the file's current token (shape: `stale_update:{current}`) |
| `write_error` | File system write failed |

### Example: Write a new fact

```json
{
  "path": "memories/rust-async-patterns.md",
  "frontmatter": {
    "okf_version": "0.1",
    "profile": "llm-wiki/1",
    "title": "Rust async patterns",
    "entity_type": "fact",
    "tags": ["rust", "async", "patterns"],
    "created_at": "2026-08-26T12:34:56Z",
    "updated_at": "2026-08-26T12:34:56Z"
  },
  "body": "## Key patterns\n\nUse `async fn` for readability, `Future` trait for control."
}
```

### Example: Edit existing note (stale update detection)

```json
{
  "path": "memories/rust-async-patterns.md",
  "frontmatter": {
    "okf_version": "0.1",
    "profile": "llm-wiki/1",
    "title": "Rust async patterns (updated)",
    "entity_type": "fact",
    "tags": ["rust", "async", "patterns"],
    "created_at": "2026-08-26T12:34:56Z",
    "updated_at": "2026-08-26T12:45:00Z"  // Must EXACTLY MATCH the file's current updated_at (read it first)
  },
  "body": "## Key patterns\n\nUse `async fn` for readability, `Future` trait for control.\n\n## New section\n\nUse `tokio::spawn` for concurrent tasks."
}
```

## Tool 2: `vault_upsert_index_entry`

Add or update an entry in an INDEX.md file (e.g., for cataloging memories, tasks, or procedures).

### Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `index_path` | `string` | Yes | Vault-relative path to an EXISTING INDEX.md (never auto-created), e.g. `"people/tessera/INDEX.md"` |
| `entry_name` | `string` | Yes | Entry header name (whole-line matched); letters, digits, spaces, `_`, `-`, `.` |
| `entry_path` | `string` | Yes | Vault-relative path of the note this entry links to (must exist) |
| `entry_type` | `string` | Yes | Displayed on the `- Type:` line, e.g. `"memory"`, `"handoff"` |
| `metadata` | `object` | No | Extra key/values rendered as `- Key: value` lines |

Wire note: MCP clients may use either `camelCase` (`indexPath`, `entryName`, `entryPath`, `entryType`) or `snake_case`.

### Entry Name Rules (spec v2)

- Matched by WHOLE-LINE equality against `## {entry_name}` headers — no regex, no substring matching
- Allowed characters: letters, digits, spaces, `_`, `-`, `.`
- Must be non-empty after trimming
- Prefix collisions are safe: updating `"alpha"` never touches a neighboring `"alphabet"` block
- Examples: `"my-entry"`, `"task-123"`, `"procedure_name"`, `"Database schema migration"`

### Metadata Schema

Any valid JSON object. Common fields:

```json
{
  "title": "string",
  "path": "string",        // Target markdown path
  "created_at": "ISO 8601",
  "updated_at": "ISO 8601",
  "status": "string",      // e.g., "active", "completed"
  "type": "string"         // e.g., "memory", "handoff", "procedure"
}
```

### Return Value

```json
{
  "success": true,
  "index_path": "string",   // Index file path
  "entry_id": "string",     // Echoes the entry_name you provided
  "appended": boolean,      // true if new entry, false if updated
  "line_number": number     // Line number where entry starts
}
```

### Error Cases

| Error | Description |
|-------|-------------|
| `path_outside_vault` | Index path is outside vault root |
| `index_not_found` | Index file doesn't exist (NEVER auto-created — create it first) |
| `invalid_metadata` | Metadata is not a valid JSON object |
| `invalid_entry_name` | Entry name empty or contains forbidden characters |
| `path_outside_vault` | Entry path resolves outside the vault root |

### Example: Add new entry

```json
{
  "index_path": "people/tessera/INDEX.md",
  "entry_name": "handoff-2026-08-26",
  "entry_path": "people/tessera/migration-2026-08-26.md",
  "entry_type": "handoff",
  "metadata": {
    "title": "Database schema migration",
    "created_at": "2026-08-26T12:34:56Z",
    "status": "pending"
  }
}
```

### Example: Update existing entry

```json
{
  "index_path": "people/tessera/INDEX.md",
  "entry_name": "handoff-2026-08-26",
  "entry_path": "people/tessera/migration-2026-08-26.md",
  "entry_type": "handoff",
  "metadata": {
    "title": "Database schema migration",
    "created_at": "2026-08-26T12:34:56Z",
    "updated_at": "2026-08-26T14:30:00Z",
    "status": "completed"
  }
}

The upsert writes a pinned block:

```markdown
## handoff-2026-08-26
[[people/tessera/migration-2026-08-26.md]]
- Type: handoff
- Title: Database schema migration
- Created_at: 2026-08-26T12:34:56Z
- Updated_at: 2026-08-26T14:30:00Z
- Status: completed
```

Repeated calls with the same `entry_name` replace the existing block in place (idempotent — no duplicates).
```

## OKF Frontmatter Schema Reference

Adopted profile: `llm-wiki/1` from [@equationalapplications/okf](https://github.com/equationalapplications/okf).

### Required Fields

| Field | Type | Description | Example |
|-------|------|-------------|---------|
| `okf_version` | `string` | OKF version (fixed) | `"0.1"` |
| `profile` | `string` | Profile identifier (fixed) | `"llm-wiki/1"` |
| `title` | `string` | Human-readable title | `"Rust async patterns"` |
| `entity_type` | `enum` | Entity category | `"fact"`, `"task"`, `"event"`, `"concept"`, `"doc"` |
| `created_at` | `string` | ISO 8601 timestamp | `"2026-08-26T12:34:56Z"` |
| `updated_at` | `string` | ISO 8601 timestamp (required on edits) | `"2026-08-26T12:45:00Z"` |

### Optional Fields

| Field | Type | Description | Constraints |
|-------|------|-------------|-------------|
| `tags` | `array[string]` | Topic labels | Max 20 tags, max 100 chars each |

### Entity Types

| Type | Use Case |
|------|----------|
| `fact` | Factual information, discoveries, learned truths |
| `task` | Actionable items, to-do items, procedures |
| `event` | Temporal occurrences, meetings, milestones |
| `concept` | Abstract ideas, models, frameworks |
| `doc` | Documentation, reference material, guides |

## Valid Frontmatter Examples

### Fact

```yaml
---
okf_version: "0.1"
profile: "llm-wiki/1"
title: Rust async patterns
entity_type: fact
tags: ["rust", "async", "patterns"]
created_at: "2026-08-26T12:34:56Z"
updated_at: "2026-08-26T12:45:00Z"
---

## Key patterns

Use `async fn` for readability, `Future` trait for control.
```

### Task

```yaml
---
okf_version: "0.1"
profile: "llm-wiki/1"
title: Implement write path
entity_type: task
tags: ["mcp", "okf", "priority-high"]
created_at: "2026-08-26T10:00:00Z"
updated_at: "2026-08-26T10:00:00Z"
---

- [ ] Add `OkfFrontmatter` struct
- [ ] Implement `vault_write_note` command
- [ ] Add unit tests
```

### Event

```yaml
---
okf_version: "0.1"
profile: "llm-wiki/1"
title: MCP design review
entity_type: event
tags: ["mcp", "design-review"]
created_at: "2026-08-26T14:00:00Z"
updated_at: "2026-08-26T14:30:00Z"
---

Attendees: Tessera, Hermes

Discussed:
- Tool naming conventions
- Error handling patterns
- Migration strategy
```

### Concept

```yaml
---
okf_version: "0.1"
profile: "llm-wiki/1"
title: Stale update detection
entity_type: concept
tags: ["optimistic-locking", "concurrency"]
created_at: "2026-08-26T11:00:00Z"
updated_at: "2026-08-26T11:00:00Z"
---

Optimistic locking pattern that prevents overwriting changes made by another process.

Uses `updated_at` timestamp comparison to detect conflicts.
```

### Doc

```yaml
---
okf_version: "0.1"
profile: "llm-wiki/1"
title: MCP write path usage
entity_type: doc
tags: ["mcp", "documentation"]
created_at: "2026-08-26T09:00:00Z"
updated_at: "2026-08-26T09:00:00Z"
---

Guide for using the new MCP write tools.
```

## Common Use Cases & Workflows

### Workflow 1: Capture a learned fact

1. Agent discovers new information
2. Call `vault_write_note` with `entity_type: "fact"`
3. Include relevant tags for discoverability
4. Optionally call `vault_upsert_index_entry` to catalog in a topic index

### Workflow 2: Create a task from conversation

1. Agent identifies actionable item
2. Call `vault_write_note` with `entity_type: "task"`
3. Use markdown checkboxes for subtasks
4. Track status in frontmatter or body

### Workflow 3: Log an event

1. Agent records a meeting or milestone
2. Call `vault_write_note` with `entity_type: "event"`
3. Include attendees, outcomes, decisions
4. Add to a chronological index

### Workflow 4: Maintain a handoff log

1. Agent completes work handoff
2. Call `vault_write_note` with handoff details
3. Call `vault_upsert_index_entry` to add to `people/{name}/INDEX.md`
4. On edits, read the file first and echo back its exact `updated_at` token (If-Match)

### Workflow 5: Multi-agent collaboration

1. Agent A writes a note; the file now carries an `updated_at` token (A rotates it on every write)
2. Agent B attempts an edit with an outdated token (A wrote since B last read)
3. Tool returns `stale_update:{current}` — ordering does NOT matter; only exact match passes
4. Agent B reads the current file, merges changes, and echoes back the CURRENT token verbatim
5. Retry succeeds

Wall-clock comparisons are never used — a older-looking timestamp that matches the stored token is accepted, and a newer one that doesn't is refused.

## Error Handling Guidance

### Handling `path_outside_vault`

**Cause**: Path contains `..` traversal or is outside vault root.

**Solution**: Use vault-relative paths only.

```json
// ❌ Invalid
{ "path": "../outside-vault.md" }

// ❌ Invalid
{ "path": "/absolute/path.md" }

// ✅ Valid
{ "path": "memories/note.md" }
```

### Handling `invalid_frontmatter`

**Cause**: Frontmatter validation failed.

**Common issues**:
- Missing required fields
- Wrong `okf_version` or `profile`
- Invalid `entity_type`
- Malformed ISO 8601 timestamp
- Too many or too long tags

**Solution**: Check error message, fix validation issue, retry.

### Handling `stale_update`

**Cause**: The provided `updated_at` does not exactly equal the file's current token — someone wrote after you last read.

**Solution**:
1. Read the current file and copy its exact `updated_at` token
2. Merge your changes with the current content
3. Send that token back unchanged in `frontmatter.updated_at`
4. Retry the write

**Workflow**:
```text
1. vault_write_note → stale_update error
2. Read file (or use existing read tools)
3. Merge changes
4. vault_write_note with the CURRENT updated_at token → success
```

### Handling `index_not_found`

**Cause**: The index file doesn't exist.

**Solution**: Index files are never auto-created (spec v2). Create `INDEX.md` yourself — even empty — then retry the upsert.

## Migration Notes

### New files only

This implementation **does not back-migrate existing vault files**. Existing files without OKF frontmatter remain functional.

### Backwards compatibility

- Read tools (`vault_semantic_search`, `wiki_search`, etc.) are unchanged
- Old vault files remain discoverable
- No breaking changes for existing workflows

### Optional future migration

**Option A**: Manual migration
- Use agents to rewrite legacy files with OKF frontmatter
- Requires manual triggering

**Option B**: Librarian synthesis (planned)
- Librarian infers metadata from content
- Rewrites files with OKF frontmatter automatically

**Option C**: CLI migration tool (not implemented)
- `migrate_to_okf` command could add frontmatter to legacy files
- Not prioritized; agents naturally transition to new format

## Testing & Verification

### Verify MCP tool registration

```bash
# List available MCP tools
./target/debug/curated_thoughts_mcp --help | grep vault

# Should show:
# vault_write_note
# vault_upsert_index_entry
```

### Run unit tests

```bash
# OKF validation tests
cargo test -p tauri-app-lib --lib --features test-utils validate_frontmatter

# Write note tests
cargo test -p tauri-app-lib --lib --features test-utils vault_write_note

# Index entry tests
cargo test -p tauri-app-lib --lib --features test-utils vault_upsert_index_entry
```

### Test write path via MCP

1. Start MCP server
2. Call `vault_write_note` with valid frontmatter
3. Verify file appears in vault
4. Check SHA-256 matches returned value
5. Try stale update (modify file, retry with old timestamp)

## References

- **Spec**: `docs/superpowers/specs/2026-08-26-mcp-write-path-okf-frontmatter.md`
- **Implementation**: `src-tauri/src/okf/mod.rs`
- **PR Checklist**: `docs/superpowers/pr-101-checklist.md`
- **OKF Profile**: [@equationalapplications/okf](https://github.com/equationalapplications/okf)
- **MCP Server**: `tools/src/bin/curated_thoughts_mcp.rs`

## Changelog

### v0.1 (2026-08-26)

- ✅ Added `vault_write_note` MCP tool
- ✅ Added `vault_upsert_index_entry` MCP tool
- ✅ Implemented OKF v0.1 frontmatter validation
- ✅ Added stale update detection
- ✅ Added path safety checks
- ✅ Comprehensive unit tests