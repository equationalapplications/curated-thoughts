# Vault Folder Structure Refactoring

**Date:** 2026-08-27
**Status:** SPEC
**Author:** Kurt VanDusen
**Related Specs:**
- `2026-05-05-second-brain-app-design.md` — original design with documents/ immutability
- `2026-05-11-default-vault-and-vault-switching-design.md` — vault creation/switching
- `2026-08-26-mcp-write-path-okf-frontmatter.md` — current write path (to be corrected)

---

## Problem

The current vault folder structure enforces **no immutability guarantee** for source files:

```
curated-thoughts/
├── documents/        ← App CAN write here (via ["."])
├── wiki/
└── .brain/
```

The v2 MCP write path spec removed the original documents/ immutability contract by using `safe_vault_path(..., ["."], PathMode::MayCreate)`, which permits writes anywhere in the vault. This breaks the LLM Wiki pattern where source files are immutable and segregated from curated wiki content.

**Why this matters:**
- Source files should be user-owned truth (no silent app mutations)
- Wiki pages should be app-managed (organized, indexed)
- The separation prevents accidental data loss and makes the system predictable

---

## Solution

Rename folders to make the contract explicit, then enforce immutability at the Rust layer.

### New Folder Structure

```
<user-chosen-name>/                ← e.g., ~/curated-thoughts/ (default)
├── immutable-source-files/        ← Documents (read-only to app)
├── wiki/                          ← Wiki pages (app-managed)
└── .brain/                        ← Hidden app state
    ├── converted/                 ← Chunk cache
    └── proposed/                  ← Proposals
```

### Folder Contracts

| Folder | Contract | Reads Allowed? | Writes Allowed? |
|--------|----------|----------------|-----------------|
| `immutable-source-files/` | User documents, never mutated by app | ✓ | ✗ |
| `wiki/` | Curated wiki pages, app-managed | ✓ | ✓ |
| `.brain/` | App state, hidden | ✓ | ✓ |
| `.brain/converted/` | Chunk cache | ✓ | ✓ |
| `.brain/proposed/` | Proposed pages | ✓ | ✓ |

### Parent Folder Renaming

Users can rename the parent folder at any time without breaking the app:

**Behavior on vault missing:**
1. Detect configured vault path does not exist
2. Show prompt: "Vault not found at configured path. Please select your vault folder to continue."
3. Open folder picker
4. Validate structure on selection:
   - ✓ `immutable-source-files/` exists
   - ✓ `wiki/` exists
   - ✓ `.brain/` exists (optional for migration)
5. Update config: `vault_root = "<user-selected-path>"`
6. Continue launch

**No auto-detection** — manual re-prompt only (Option B from design discussion).

---

## Implementation

### Phase 1: Rust Layer Enforcement

**File:** `src-tauri/src/vault/safe_path.rs`

**Current code:**
```rust
fn allowed() -> &'static [&'static str] {
    &["documents", "wiki", ".brain/proposed"]  // ← documents is writable
}
```

**New code:**
```rust
// Reads: both folders accessible
const READABLE_SUBDIRS: &[&str] = &["immutable-source-files", "wiki"];

// Writes: wiki only
const WRITABLE_SUBDIRS: &[&str] = &["wiki"];

// Proposed pages: wiki + .brain/proposed
const PROPOSED_SUBDIRS: &[&str] = &["wiki", ".brain/proposed"];
```

**Update `safe_vault_path` callers:**

| Caller | Old | New |
|--------|-----|-----|
| `write_note` (okf/write.rs) | `["."]` | `WRITABLE_SUBDIRS` |
| `upsert_index_entry` (okf/write.rs) | `["."]` | `WRITABLE_SUBDIRS` |
| Read commands | `["documents", "wiki", ...]` | `READABLE_SUBDIRS` |
| Proposals | `["wiki", ".brain/proposed"]` | `PROPOSED_SUBDIRS` |

**Add path validation helper:**
```rust
/// Validates that a path is within allowed subdirs for the given mode.
/// Returns SafePathError::Outside if the path would violate the contract.
pub fn validate_path_mode(
    vault_root: &Path,
    user_path: &str,
    allowed_subdirs: &[&str],
    mode: PathMode,
) -> Result<PathBuf, SafePathError> {
    safe_vault_path(vault_root, user_path, allowed_subdirs, mode)
}
```

**Update tests:**
- Add tests proving writes to `immutable-source-files/` are rejected
- Add tests proving writes to `wiki/` succeed
- Update existing tests to use new folder names

---

### Phase 2: Migration on Upgrade

**Detect old structure:**
```rust
fn needs_migration(vault_root: &Path) -> bool {
    vault_root.join("documents").exists()
}
```

**Migration process:**
```rust
fn migrate_vault(vault_root: &Path) -> Result<(), MigrationError> {
    let old = vault_root.join("documents");
    let new = vault_root.join("immutable-source-files");

    if old.exists() && !new.exists() {
        std::fs::rename(&old, &new)?;
    }

    Ok(())
}
```

**UI notification:**
```
"Documents folder renamed to 'immutable-source-files' to match app conventions.
Your files are unchanged."
```

---

### Phase 3: Frontend Updates

**Update folder tree component:**
```tsx
// FolderTree.tsx
const FOLDERS = [
  { name: "immutable-source-files", label: "Source Files" },
  { name: "wiki", label: "Wiki Pages" },
];
```

**Update drag-drop handler:**
- Drop into `immutable-source-files/` → ingestion (read-only)
- Drop into `wiki/` → rejected (show error: "Drag source files into Source Files folder")

**Update vault settings:**
- Add "Relocate Vault" button (triggers re-prompt)
- Show current vault path
- Validate structure on manual selection

---

### Phase 4: Config Update

**Default vault location:**
```rust
// src-tauri/src/vault/config.rs
fn default_vault_path() -> PathBuf {
    dirs::home_dir()
        .expect("home directory not found")
        .join("curated-thoughts")
}
```

**Config schema:**
```rust
pub struct VaultConfig {
    pub vault_root: PathBuf,
    pub migrated_to_v2: bool,  // Track migration state
}
```

---

## Acceptance Criteria

### Phase 1 (Rust Layer)
- [ ] `WRITEABLE_SUBDIRS` excludes `immutable-source-files/`
- [ ] Test: Write to `immutable-source-files/` returns `SafePathError::Outside`
- [ ] Test: Write to `wiki/` succeeds
- [ ] Test: Read from both folders succeeds

### Phase 2 (Migration)
- [ ] Old vaults auto-migrate `documents/` → `immutable-source-files/`
- [ ] Migration is idempotent (can run multiple times)
- [ ] UI shows migration notification
- [ ] Migration flag set in config

### Phase 3 (Frontend)
- [ ] Folder tree shows "Source Files" and "Wiki Pages"
- [ ] Drag-drop into `immutable-source-files/` succeeds
- [ ] Drag-drop into `wiki/` shows error
- [ ] Vault relocation button works

### Phase 4 (Config)
- [ ] Default vault: `~/curated-thoughts/`
- [ ] Missing vault triggers re-prompt
- [ ] Re-prompt validates structure before accepting
- [ ] Config persists vault path

---

## Risks / Trade-offs

| Risk | Mitigation |
|------|------------|
| User has renamed parent folder | Re-prompt handles this gracefully |
| Migration fails mid-way | Backup original folder before rename |
| Frontend has hardcoded folder paths | Centralize folder names in constants |
| Existing shell scripts reference `documents/` | Document breaking change in changelog |

---

## Open Questions

**None** — resolved in design discussion.

---

## Relation to Prior Work

This refactoring corrects the anti-pattern introduced in:
- `2026-08-26-mcp-write-path-okf-frontmatter.md` (PR #101) — which removed documents/ immutability

The contract aligns with the original design:
- `2026-05-05-second-brain-app-design.md` — Tier 1 immutable documents, Tier 2 wiki pages