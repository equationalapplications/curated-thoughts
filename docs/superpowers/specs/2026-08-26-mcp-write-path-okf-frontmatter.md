# Spec: MCP Write Path + OKF Frontmatter

**Date:** 2026-08-26
**Author:** Hermes Agent (Tessera)
**Status:** Draft
**Related:** `procedures/curated-thoughts-improvement-backlog.md` (MCP write path + OKF frontmatter items)

---

## Problem Statement

### 1. No MCP write capabilities
The Curated Thoughts MCP server (`curated-thoughts-mcp`) exposes **read-only** tools:
- `vault_semantic_search`
- `vault_related_chunks`
- `wiki_search`
- `wiki_traverse_graph`
- `wiki_get_ontology`
- `curated_entities` (read-only)
- `create_entities`/`create_relations` (ct-memory-eval third-party server, slated for removal)

Today, "add to long-term memory" means:
- Dropping a markdown file into `~/Documents/equational-wiki/` via filesystem
- This breaks the MCP contract for agents that only have tool access (no shell)
- Tessera can do it, but subagents cannot

### 2. No standardized frontmatter
Vault files use inconsistent or missing frontmatter. The chunker/embedder cannot:
- Extract metadata uniformly (entity_type, tags, created_at)
- Populate the wiki layer from agent-written notes
- Support temporal queries or entity-scoped retrieval

### 3. Wiki layer remains empty
The `llm_wiki_entries` / `llm_wiki_edges` tables are permanently populated only by the desktop app's commit UI. Agent-written notes never contribute to the wiki graph because:
- No MCP write surface
- No frontmatter contract for the librarian to consume
- This blocks the "software factory in a box" vision

---

## Vision Alignment

From `curated-thoughts-vision.md`:
> North star: CT as a software factory in a box with expert layers, built-in agency, and chat.

This PR serves that vision by:
1. Giving agents first-class write access to the vault via MCP
2. Establishing the OKF document contract as the lingua franca for all vault content
3. Enabling the wiki layer to ingest and reason over agent-generated knowledge

---

## Goals

### Primary
1. Add `vault_write_note` MCP tool that writes markdown files with OKF frontmatter
2. Add `vault_upsert_index_entry` MCP tool for auditable INDEX updates
3. Adopt OKF frontmatter on all new vault files going forward

### Secondary
1. Validate frontmatter schemas before write (fail fast on invalid inputs)
2. Propagate OKF metadata to chunk-level symbols/tags (chunker enhancement)
3. Document migration path for existing vault files (optional, not blocking)

---

## Non-Goals

1. **Back-migrating existing vault files** — too risky at scale; new files only
2. **Wiki layer librarian synthesis** — that's a separate item in the backlog ("make the librarian synthesize into wiki entries")
3. **OKF profile v0.1 → v1 migration** — we adopt the current normative profile (`llm-wiki/1`) from day one
4. **Removing ct-memory-eval** — that's a cleanup after this PR validates the write path

---

## Proposed Solution

### 1. `vault_write_note` MCP tool

**Tool Name:** `vault_write_note`

**Parameters:**
```json
{
  "path": "string",           // Vault-relative path, e.g., "memories/my-fact.md"
  "frontmatter": {            // OKF v0.1 frontmatter object
    "title": "string",
    "entity_type": "fact|task|event|concept|doc",
    "tags": ["string"],
    "created_at": "ISO 8601 string",
    "updated_at": "ISO 8601 string"  // Required on edits
  },
  "body": "string"             // Markdown body, may contain [[WikiLink]] edges
}
```

**Behavior:**
1. Validate `path` is under the configured vault root (reject `../`, absolute paths)
2. Parse `frontmatter`:
   - `title`: required, non-empty
   - `entity_type`: required, enum of `fact|task|event|concept|doc`
   - `tags`: optional, array of strings
   - `created_at`: required on new files, ISO 8601
   - `updated_at`: required on edits, ISO 8601
3. If file exists and `frontmatter.updated_at` ≤ file's mtime → reject (stale update)
4. Construct the full document:
   ```yaml
   ---
   okf_version: 0.1
   profile: llm-wiki/1
   title: <title>
   entity_type: <entity_type>
   tags: [<tags>]
   created_at: <created_at>
   updated_at: <updated_at>
   ---
   <body>
   ```
5. Write to disk atomically (write to temp, then `rename()`)
6. Return:
   - `success: true`
   - `path: string` (absolute path on disk)
   - `sha256: string` (file hash after write)

**Error Cases:**
- Path outside vault → `path_outside_vault`
- Invalid frontmatter → `invalid_frontmatter` (details)
- Stale update (`updated_at` ≤ mtime) → `stale_update`
- Write failure → `write_error` (reason)

**Rust Integration:**
```rust
#[tauri::command]
async fn vault_write_note(
    conn: Connection,
    vault_root: PathBuf,
    path: String,
    frontmatter: OkfFrontmatter,
    body: String,
) -> Result<WriteNoteResult, String>
```

---

### 2. `vault_upsert_index_entry` MCP tool

**Tool Name:** `vault_upsert_index_entry`

**Parameters:**
```json
{
  "index_path": "string",      // e.g., "people/tessera/INDEX.md"
  "entry_name": "string",      // The anchor name
  "entry_path": "string",      // Target markdown path, e.g., "people/tessera/my-fact.md"
  "entry_type": "string",      // e.g., "memory", "handoff", "procedure"
  "metadata": {                // Optional structured metadata
    "date": "ISO 8601",
    "status": "string",
    ...
  }
}
```

**Behavior:**
1. Validate `index_path` and `entry_path` are under vault root
2. Read `index_path` file
3. Find existing entry by `entry_name` (regex: `^## \{entry_name\}`)
4. If exists:
   - Update the link target to `[[<entry_path>]]`
   - Update metadata block (if provided)
5. If doesn't exist:
   - Append new entry at end:
     ```markdown
     ## {entry_name}
     [[{entry_path}]]
     - Type: {entry_type}
     - Date: {date}
     - Status: {status}
     ```
6. Write atomically (temp + rename)
7. Return:
   - `success: true`
   - `appended: boolean` (true if new entry, false if updated)
   - `line_number: number` (where entry was placed)

**Error Cases:**
- Path outside vault → `path_outside_vault`
- Index file missing → `index_not_found` (create with user approval?)
- Invalid metadata → `invalid_metadata`

**Rust Integration:**
```rust
#[tauri::command]
async fn vault_upsert_index_entry(
    conn: Connection,
    vault_root: PathBuf,
    index_path: String,
    entry_name: String,
    entry_path: String,
    entry_type: String,
    metadata: Option<serde_json::Value>,
) -> Result<UpsertResult, String>
```

---

### 3. OKF Frontmatter Schema

**Adopted Profile:** `llm-wiki/1` (current normative from `@equationalapplications/okf`)

**Required Fields:**
```yaml
okf_version: 0.1
profile: llm-wiki/1
title: <string>
entity_type: fact|task|event|concept|doc
created_at: <ISO 8601>
updated_at: <ISO 8601>  # Required on edits
```

**Optional Fields:**
```yaml
tags: [<string>]
```

**Validation Rules:**
1. `title` must be non-empty
2. `entity_type` must be one of the 5 enums
3. `created_at` / `updated_at` must parse as ISO 8601
4. `tags` must be an array of strings (max length: 10 per tag, 20 tags total)
5. Unknown fields are rejected (strict schema)

**Rust Struct:**
```rust
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct OkfFrontmatter {
    pub okf_version: String,
    pub profile: String,
    pub title: String,
    pub entity_type: EntityType,
    pub tags: Option<Vec<String>>,
    pub created_at: String,
    pub updated_at: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EntityType {
    Fact,
    Task,
    Event,
    Concept,
    Doc,
}
```

---

### 4. Chunker Enhancement (Optional, Low-Priority)

The chunker should extract OKF metadata into chunk-level symbols/tags:

**Before:**
```rust
// chunks.symbol_name is only set for AST symbols
// chunks.tags is only set for folder_rules
```

**After:**
```rust
// When ingesting an OKF file:
// - chunk.symbol_name ← frontmatter.title (first chunk only)
// - chunk.tags ← frontmatter.tags (all chunks from that file)
// - chunk.strategy ← "okf_document" (new strategy)
```

**Rationale:**
- Makes vault documents discoverable to semantic search by title/tags
- Enables temporal queries on `created_at` (add `indexed_at` column to `chunks`)
- Bridges the wiki layer gap (librarian can read OKF metadata)

**Blocking:** This is a **separate PR**. This spec only writes the frontmatter; chunker adoption comes later.

---

## Implementation Plan

### Phase 1: Core Write Path (Blocking)
1. Add `OkfFrontmatter` struct + validation in `src-tauri/src/okf/mod.rs` (new module)
2. Add `vault_write_note` command in `src-tauri/src/lib.rs`
3. Add `vault_upsert_index_entry` command in `src-tauri/src/lib.rs`
4. Register both tools in MCP server (`tools/src/bin/curated_thoughts_mcp.rs`)
5. Write unit tests:
   - `test_okf_frontmatter_validation`
   - `test_vault_write_note_new_file`
   - `test_vault_write_note_stale_update`
   - `test_vault_upsert_index_entry_new`
   - `test_vault_upsert_index_entry_update`

### Phase 2: MCP Tool Registration (Blocking)
1. Add tool schemas to MCP tools/list response
2. Implement tool handlers in `tools/src/bin/curated_thoughts_mcp.rs`
3. Add integration tests that drive the MCP handlers via JSON-RPC

### Phase 3: Documentation (Blocking)
1. Update `docs/mcp-tools.md` with new tools
2. Update `procedures/curated-thoughts-improvement-backlog.md` (mark P1 items done)
3. Add migration note for existing vault files (new files only, no back-migration)

### Phase 4: Chunker Adoption (Non-Blocking, Separate PR)
1. Modify `src-tauri/src/chunker/mod.rs` to extract OKF metadata
2. Add `indexed_at` column to `chunks` (MIGRATION_V13)
3. Update ingest pipeline to populate metadata

---

## Testing Strategy

### Unit Tests (D1–D5)
**D1 — OKF validation:**
- Valid frontmatter → passes
- Missing `title` → fails
- Invalid `entity_type` → fails
- Invalid ISO 8601 → fails
- Unknown field → fails

**D2 — `vault_write_note` new file:**
- File doesn't exist → creates with correct frontmatter
- File path outside vault → rejected
- Stale update check not triggered (no file)

**D3 — `vault_write_note` stale update:**
- File exists with mtime = T
- Frontmatter.updated_at = T - 1s → rejected
- Frontmatter.updated_at = T + 1s → accepted

**D4 — `vault_upsert_index_entry` new:**
- Index file exists → appends entry at end
- Entry link is `[[target_path]]`
- Returns `appended: true`

**D5 — `vault_upsert_index_entry` update:**
- Entry exists → updates link + metadata
- Returns `appended: false`

### Integration Tests (E1–E3)
**E1 — MCP roundtrip:**
- Call `vault_write_note` via JSON-RPC
- Read file back via filesystem → matches frontmatter + body
- Verify SHA returned matches computed

**E2 — Index update workflow:**
- Write note → upsert index entry
- Read index file → entry present with correct link
- Update note → upsert same entry → link updated

**E3 — Error propagation:**
- Invalid frontmatter → MCP error with details
- Path outside vault → MCP error `path_outside_vault`
- Stale update → MCP error `stale_update`

---

## Migration Plan

### Database
No DB migrations required in this PR. The chunker adoption (Phase 4) will add `indexed_at` in a future migration.

### Vault Files
**New files only.** Existing vault files are not back-migrated. This is intentional:
- Existing files may have ad-hoc frontmatter or none
- Mass migration risks breaking existing retrieval patterns
- Agents naturally transition to new format over time

**Optional future work:**
- Add a `migrate_to_okf` CLI command that adds frontmatter to legacy files
- Or, rely on the librarian to synthesize wiki entries and rewrite in OKF

### Backwards Compatibility
- Read tools (`vault_semantic_search`, `wiki_search`, etc.) are unchanged
- Old vault files remain discoverable (chunker doesn't require frontmatter)
- No breaking changes for existing workflows

---

## Success Criteria

1. ✅ `vault_write_note` tool accepts valid OKF frontmatter and writes files
2. ✅ `vault_upsert_index_entry` tool updates INDEX.md files atomically
3. ✅ MCP handlers return structured errors (path_outside_vault, invalid_frontmatter, stale_update)
4. ✅ All D1–D5 unit tests pass
5. ✅ All E1–E3 integration tests pass
6. ✅ No regressions in existing MCP tools
7. ✅ Wiki layer remains unchanged (this PR only adds write surface)

---

## Open Questions

1. **Should `updated_at` be required on NEW files?**
   - Proposal: Yes, require it from day one (explicit timestamps better than implicit)
   - Alternative: Optional on new, required on edits

2. **Should `vault_upsert_index_entry` create missing index files?**
   - Proposal: No, reject with `index_not_found` → let user decide whether to create
   - Alternative: Auto-create with a template

3. **Chunker adoption timing:**
   - Proposal: Separate PR, after this validates the write path
   - Alternative: Include in this PR (bloats scope)

4. **Back-migration approach:**
   - Proposal: Defer; new files only for now
   - Alternative: Add a `migrate_to_okf` CLI command in this PR

---

## Dependencies

**Required:**
- None (blocks on nothing, enables the wiki layer)

**Optional (Future):**
- Chunker adoption (Phase 4) → requires this PR's OKF struct
- Wiki layer librarian synthesis → requires chunker metadata

---

## Risks

1. **Frontmatter validation is too strict**
   - Mitigation: Start with enum validation, relax to allow custom values if requested

2. **Stale update check is too aggressive**
   - Mitigation: Add `force: boolean` flag to bypass (for migration scripts)

3. **Index file corruption risk**
   - Mitigation: Write atomically (temp + rename), validate before commit

4. **MCP tool naming conflicts**
   - Mitigation: Prefix with `vault_` for write tools, keep existing names for read tools

---

## Post-Merge Steps

1. Update `curated-thoughts-operations` skill with new tool usage patterns
2. Add example calls to `docs/mcp-tools.md`
3. Update backlog (mark P1 MCP write path items as done)
4. Consider removing `ct-memory-eval` server once Tessera validates the native write path
5. Begin chunker adoption PR (Phase 4)

---

## References

- `procedures/curated-thoughts-improvement-backlog.md` (P1 MCP write path items)
- `curated-thoughts-vision.md` (north star)
- `@equationalapplications/okf` (OKF spec)
- `docs/superpowers/specs/2026-08-26-fix-run-wiki-heal-source-ref-contract.md` (recent PR pattern)