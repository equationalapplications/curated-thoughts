# PR #101 — MCP Write Path + OKF Frontmatter — Implementation Checklist

**Branch:** `feature/mcp-write-path-okf-frontmatter`
**Spec:** `docs/superpowers/specs/2026-08-26-mcp-write-path-okf-frontmatter.md`
**Status:** Ready to implement

---

## Phase 1: Core Write Path

### 1.1 Add OKF module
- [ ] Create `src-tauri/src/okf/mod.rs`
- [ ] Add `OkfFrontmatter` struct with serde derives
- [ ] Add `EntityType` enum (fact|task|event|concept|doc)
- [ ] Add `validate_frontmatter(frontmatter: &OkfFrontmatter) -> Result<(), String>`
- [ ] Add unit tests D1 (OKF validation)

### 1.2 Add `vault_write_note` command
- [ ] Add `vault_write_note` Tauri command in `src-tauri/src/lib.rs`
- [ ] Implement path validation (reject `../`, absolute paths)
- [ ] Implement stale update check (`updated_at` ≤ file mtime)
- [ ] Implement atomic write (temp file + rename)
- [ ] Return `WriteNoteResult` with path and SHA256
- [ ] Add unit tests D2 (new file) and D3 (stale update)

### 1.3 Add `vault_upsert_index_entry` command
- [ ] Add `vault_upsert_index_entry` Tauri command in `src-tauri/src/lib.rs`
- [ ] Implement index file read
- [ ] Implement entry lookup by regex (`^## {entry_name}`)
- [ ] Implement append or update logic
- [ ] Implement atomic write (temp file + rename)
- [ ] Return `UpsertResult` with `appended: bool` and `line_number`
- [ ] Add unit tests D4 (new entry) and D5 (update entry)

### 1.4 Add error types
- [ ] Add `WriteNoteError` enum in `src-tauri/src/okf/mod.rs`:
  - `PathOutsideVault`
  - `InvalidFrontmatter`
  - `StaleUpdate`
  - `WriteError`
- [ ] Add `UpsertError` enum:
  - `IndexNotFound`
  - `InvalidMetadata`

---

## Phase 2: MCP Tool Registration

### 2.1 Register Tauri commands
- [ ] Register `vault_write_note` in `src-tauri/src/lib.rs` via `#[tauri::command]`
- [ ] Register `vault_upsert_index_entry` in `src-tauri/src/lib.rs` via `#[tauri::command]`

### 2.2 Add MCP tool handlers
- [ ] Add `vault_write_note` handler in `tools/src/bin/curated_thoughts_mcp.rs`
- [ ] Add `vault_upsert_index_entry` handler in `tools/src/bin/curated_thoughts_mcp.rs`
- [ ] Add tool schemas to MCP `tools/list` response

### 2.3 Add integration tests
- [ ] Add E1 test (MCP roundtrip: write → read → verify SHA)
- [ ] Add E2 test (index update workflow)
- [ ] Add E3 test (error propagation)

---

## Phase 3: Documentation

### 3.1 Update docs
- [ ] Update `docs/mcp-tools.md` with new tools
- [ ] Add examples for both tools
- [ ] Update `procedures/curated-thoughts-improvement-backlog.md`:
  - Mark "Add a write tool to the Curated Thoughts MCP server" as done
  - Mark "Adopt OKF frontmatter on all new vault files" as done

### 3.2 Add migration note
- [ ] Add note to README or docs: "New files only, no back-migration"
- [ ] Document `okf_version: 0.1` and `profile: llm-wiki/1` contract

---

## Phase 4: Chunker Adoption (Separate PR)

### 4.1 Extract OKF metadata into chunks
- [ ] Add `indexed_at` column to `chunks` table (MIGRATION_V13)
- [ ] Modify chunker to parse OKF frontmatter
- [ ] Populate `chunk.symbol_name` from `frontmatter.title` (first chunk)
- [ ] Populate `chunk.tags` from `frontmatter.tags` (all chunks)
- [ ] Add `chunk.strategy = "okf_document"` for OKF files
- [ ] Add tests for metadata extraction

---

## Verification Commands

```bash
# Run unit tests
cargo test -p tauri-app-lib --lib --features test-utils okf
cargo test -p tauri-app-lib --lib --features test-utils vault_write_note
cargo test -p tauri-app-lib --lib --features test-utils vault_upsert_index_entry

# Run integration tests
cargo test -p curated_thoughts_mcp

# Verify MCP tool registration
./tools/target/debug/curated_thoughts_mcp --help | grep vault_write_note
./tools/target/debug/curated_thoughts_mcp --help | grep vault_upsert_index_entry

# Build binary
cargo build --release --bin curated_thoughts_mcp
```

---

## Files to Change

**New files:**
- `src-tauri/src/okf/mod.rs` (OKF module)
- `src-tauri/tests/okf_frontmatter.rs` (unit tests)
- `src-tauri/tests/mcp_write_path.rs` (integration tests)

**Modified files:**
- `src-tauri/src/lib.rs` (add commands)
- `tools/src/bin/curated_thoughts_mcp.rs` (register MCP handlers)
- `docs/mcp-tools.md` (add tool docs)
- `procedures/curated-thoughts-improvement-backlog.md` (mark items done)

---

## Commit Strategy

### Commit 1: OKF module + validation
- `feat(okf): add OkfFrontmatter struct and validation`
- Adds `src-tauri/src/okf/mod.rs`
- Adds D1 tests

### Commit 2: `vault_write_note` command
- `feat(mcp): add vault_write_note Tauri command`
- Adds D2 and D3 tests

### Commit 3: `vault_upsert_index_entry` command
- `feat(mcp): add vault_upsert_index_entry Tauri command`
- Adds D4 and D5 tests

### Commit 4: MCP tool registration
- `feat(mcp): register write tools in curated_thoughts_mcp`
- Registers handlers in `tools/src/bin/curated_thoughts_mcp.rs`
- Adds E1, E2, E3 integration tests

### Commit 5: Documentation
- `docs(mcp): document vault_write_note and vault_upsert_index_entry`
- Updates `docs/mcp-tools.md` and backlog

---

## Blocking Decisions

**Q1: Should `updated_at` be required on NEW files?**
- [ ] Decide: Required (proposal) or Optional (alternative)
- [ ] Update spec accordingly

**Q2: Should `vault_upsert_index_entry` create missing index files?**
- [ ] Decide: Reject with `index_not_found` (proposal) or Auto-create (alternative)
- [ ] Update spec accordingly

---

## Notes

- This PR does NOT modify the wiki layer or chunker
- Backwards compatible: read tools unchanged, old vault files unchanged
- Phase 4 (chunker adoption) is a separate PR