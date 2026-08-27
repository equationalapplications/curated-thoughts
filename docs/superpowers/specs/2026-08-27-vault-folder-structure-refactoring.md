# Vault Folder Structure Refactoring

**Date:** 2026-08-27
**Status:** SPEC (rev 2 — incorporates review round 1)
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

Note: "never mutated by app" means the *app/agent* never mutates files here. **User-initiated deletion remains allowed** (the user owns this tier) — see the `delete_vault_file` row in the caller table below.

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

**Current code** (verified at base commit `b37f197`): there is no central
allowed-lists function in the codebase. Each call site passes its own literal
subdir slice to `safe_vault_path`, and the MCP write path passes `["."]` —
i.e. the entire vault is writable:
```rust
// okf/write.rs:118 (write_note) — the hole this spec closes
safe_vault_path(vault_root, path, &["."], PathMode::MayCreate)
```

**New code:**
```rust
// Single source of truth for folder NAMES — consumed by the watcher,
// drop-copy, migration, and exposed to the frontend via a Tauri command.
pub const IMMUTABLE_DIR: &str = "immutable-source-files";
pub const WIKI_DIR: &str = "wiki";

// Reads: both folders accessible
const READABLE_SUBDIRS: &[&str] = &[IMMUTABLE_DIR, WIKI_DIR];

// Writes: wiki only
const WRITABLE_SUBDIRS: &[&str] = &[WIKI_DIR];

// Proposed pages: wiki + .brain/proposed
const PROPOSED_SUBDIRS: &[&str] = &[WIKI_DIR, ".brain/proposed"];
```

**Update `safe_vault_path` callers:**

Complete call-site inventory at base `b37f197` (line numbers will drift during
implementation; relocate by symbol). Rule: mutations → `WRITABLE_SUBDIRS`,
reads → `READABLE_SUBDIRS`, proposal writes → `PROPOSED_SUBDIRS`.

| Call site | Today | New |
|-----------|-------|-----|
| `write_note` incl. parent-dir bootstrap (okf/write.rs:118,137) | `["."]` MayCreate | `WRITABLE_SUBDIRS` |
| `upsert_index_entry` (okf/write.rs:332,340) | `["."]` MustExist | `READABLE_SUBDIRS` |
| `safe_vault_relative_path` — MCP tool dispatch (tool_dispatch.rs:50) | `["."]` MayCreate | split by tool: write tools → `WRITABLE_SUBDIRS`, read tools → `READABLE_SUBDIRS` |
| `read_document` (lib.rs:2201) | `["documents","wiki"]` MustExist | `READABLE_SUBDIRS` |
| `get_related_chunks` / `get_structural_neighbors` / `get_chunk_ids_for_wiki_entry` (lib.rs:1887/1964/2087) | `["."]` MustExist | `READABLE_SUBDIRS` |
| `run_wiki_forget` (lib.rs:1575) | `["documents","wiki"]` MayCreate | `WRITABLE_SUBDIRS` (mutates the wiki index) |
| `save_wiki_page` (lib.rs:2304) | `["wiki"]` MayCreate | unchanged ✓ |
| `delete_vault_file` (lib.rs:2332) | `["documents"]` MustExist | `READABLE_SUBDIRS` — user-initiated deletion of source files stays allowed; the USER owns the immutable tier, the APP doesn't mutate it |
| `unique_drop_destination` probe (lib.rs:2367–2379) | `["documents"]` | `&[IMMUTABLE_DIR]` |

**Non-`safe_vault_path` touchpoints that hardcode `"documents"`:**
- Watcher root + containment guard: `lib.rs:800–801` (`join("documents")`) and
  the `canonical.starts_with(&documents_root)` check (~lib.rs:980)
  → `join(IMMUTABLE_DIR)`
- OS drop-copy destination: `lib.rs:2413`
  (`create_dir_all(vault_root.join("documents"))`) → `join(IMMUTABLE_DIR)`

**One core, thin adapters** (PR #101 lesson): the constants above are the
single core. `tools/` consumes them via `pub use` from `src-tauri` (dep
direction `tools → src-tauri` already exists); the frontend gets them via one
Tauri command (e.g. `get_vault_layout`), never a second hardcoded copy.

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

### Containment invariants — pin, don't rewrite (review feedback #3)

`safe_vault_path` already satisfies the cross-platform separator concern; this
spec **pins the invariant** so no implementer "simplifies" it into string
matching:

1. Traversal rejection stays `Path::components()`-based — the existing match
   on `Component::ParentDir | Component::Prefix` (rejects `..` AND Windows
   drive prefixes like `C:foo`) must remain. Never replace with
   `user_path.contains("..")`.
2. Allowed-subdir matching stays canonicalize-based: each allowed subdir is
   canonicalized and containment is `PathBuf::starts_with` — which compares
   path **components**, not string prefixes — so `documents\..\wiki` inputs
   cannot string-match their way in.
3. MayCreate mode's existing filename hygiene checks (rejects `\` in the
   final component) stay; MustExist paths canonicalize, which resolves any
   separator ambiguity on Windows.

New regression tests:
- `wiki\..\immutable-source-files\x.md` → `SafePathError::Traversal`
- `immutable-source-files/sub/../x.md` → `SafePathError::Traversal`
  (middle `..` rejected even though it would resolve back inside — strictness
  IS the contract)
- MayCreate final component containing `\` → `SafePathError::InvalidName`

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
    let new = vault_root.join(IMMUTABLE_DIR);

    match (old.exists(), new.exists()) {
        (true, false) => { std::fs::rename(&old, &new)?; Ok(()) }
        (false, _) => Ok(()), // nothing to migrate — idempotent
        (true, true) => Err(MigrationError::BothFoldersExist { old, new }),
    }
}
```

**UI notification (success):**
```
"Documents folder renamed to 'immutable-source-files' to match app conventions.
Your files are unchanged."
```

**UI on `BothFoldersExist` (review feedback #1) — BLOCKING dialog, not a
toast:** after Phase 1, `documents/` is no longer in `READABLE_SUBDIRS`, so
legacy files left in an un-migrated `documents/` become **invisible in-app** —
a silent data-disappearance, not a cosmetic issue. The app must abort
migration and show:
```
Migration blocked: both 'documents' and 'immutable-source-files' exist.
Move your files from 'documents' into 'immutable-source-files' manually,
then restart the app. (Files left in 'documents' are not visible to the app.)
```
The vault still opens for `wiki/` reads/writes; only migration is blocked.
Re-running after the user merges folders proceeds via the normal path.

---

### Phase 3: Frontend Updates

**Update folder tree component:**
```tsx
// FolderTree.tsx — names fetched from get_vault_layout, shown here for illustration
const FOLDERS = [
  { name: "immutable-source-files", label: "Source Files" },
  { name: "wiki", label: "Wiki Pages" },
];
```

**Drag & drop (review feedback #2 — decision: blanket routing IS the intended UX):**

Verified reality at base `b37f197`: ALL OS file drops are handled in Rust —
`on_window_event` → `copy_os_drop_paths_to_vault` (lib.rs:2400) — and copied
into `documents/` (lib.rs:2413 + `unique_drop_destination`, lib.rs:2359). The
React `onDragDropEvent` listener (AppShell.tsx:151) only drives the drop
overlay visual. There is **no per-folder drop targeting today**, and none is
added by this spec.

Contract going forward:
- Every OS drop lands in `immutable-source-files/` and flows through the
  chunk/embed/graph ingestion pipeline. No exceptions by file type.
- `wiki/` is app-managed: pages enter via librarian synthesis, the in-app
  editor, or MCP write tools — never via OS drag-drop. Rationale: manual
  `.md` import into `wiki/` would bypass chunking + graph building — exactly
  the class of drift this refactoring eliminates.
- If per-folder targeting is ever built (Tauri's `DragDropEvent::Drop` carries
  a `position` field for hit-testing), wiki targets MUST be rejected with:
  "Drag source files into Source Files — wiki pages are created by the app."
  This spec pins that contract now so the future feature inherits it.

Changes in this phase: retarget `copy_os_drop_paths_to_vault` +
`unique_drop_destination` to `IMMUTABLE_DIR`; update overlay copy ("Dropping
into Source Files…"). No frontend routing logic.

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
- [ ] `WRITABLE_SUBDIRS` excludes `immutable-source-files/`
- [ ] Test: Write to `immutable-source-files/` returns `SafePathError::Outside`
- [ ] Test: Write to `wiki/` succeeds
- [ ] Test: Read from both folders succeeds
- [ ] Test: `wiki\..\immutable-source-files\x.md` → `Traversal`; `sub/../x.md` → `Traversal`; MayCreate backslash filename → `InvalidName`

### Phase 2 (Migration)
- [ ] Old vaults auto-migrate `documents/` → `immutable-source-files/`
- [ ] Migration is idempotent (can run multiple times)
- [ ] UI shows migration notification
- [ ] Both folders exist → migration aborts with blocking dialog naming both paths
- [ ] Test: `(old exists, new exists)` returns `MigrationError::BothFoldersExist`

### Phase 3 (Frontend)
- [ ] Folder tree shows "Source Files" and "Wiki Pages"
- [ ] OS drop of any file → copied into `immutable-source-files/` and ingested (existing behavior, renamed destination)
- [ ] No code path accepts an OS drop into `wiki/` (grep gate: drop handling references `IMMUTABLE_DIR` only)
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
| Frontend has hardcoded folder paths | Folder names live ONLY in `IMMUTABLE_DIR`/`WIKI_DIR` (safe_path.rs); frontend fetches via Tauri command |
| Both `documents/` and `immutable-source-files/` exist pre-migration | Blocking `BothFoldersExist` dialog; legacy files never silently hidden |
| Existing shell scripts reference `documents/` | Document breaking change in changelog |

---

## Open Questions

**None open.** Resolved in design discussion, plus review round 1
(Kurt, Aug 27 2026):

1. **Both-folders migration edge case** → blocking `BothFoldersExist` dialog
   (Phase 2) — legacy files would otherwise be invisible post-Phase 1.
2. **Blanket drag-drop rejection intended UX?** → **Yes**: all OS drops route
   to `immutable-source-files/`; wiki is app-managed only (Phase 3). Manual
   `.md` import would bypass chunking + graph building.
3. **Windows `\` vs Unix `/` in subdir matching** → already Component-based +
   canonicalize containment in `safe_vault_path`; pinned as invariant with
   regression tests (Phase 1).

---

## Relation to Prior Work

This refactoring corrects the anti-pattern introduced in:
- `2026-08-26-mcp-write-path-okf-frontmatter.md` (PR #101) — which removed documents/ immutability

The contract aligns with the original design:
- `2026-05-05-second-brain-app-design.md` — Tier 1 immutable documents, Tier 2 wiki pages
