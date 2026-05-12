# Default Vault + Vault Switching — Design Spec

**Date:** 2026-05-11
**Status:** Draft
**Stack:** Tauri 2.x (Rust backend), React 18 frontend, existing `VaultConfig` in `vault/config.rs`

---

## Overview

On first launch, the app automatically creates a default vault in the user's home directory and begins indexing immediately — no wizard step required to choose a folder. Users can change the vault later from Settings. A single global database at `~/.brain/brain.db` holds all indexed data. When switching vaults, the app offers to back up the current DB into the outgoing vault before clearing it and re-indexing the new vault.

---

## Problem

Today the setup wizard forces the user to pick a vault folder before the app is usable. This adds friction for first-time users who just want to try the app. Additionally, once setup completes, there is no way to change the vault without re-running the wizard or manually editing `config.json`.

---

## Design

### Default Vault on First Launch

**Location (all platforms):**

| Platform | Default path |
|---|---|
| macOS | `~/Curated-Thoughts/` |
| Linux | `~/Curated-Thoughts/` |
| Windows | `%USERPROFILE%\Curated-Thoughts\` |

All resolved via `dirs::home_dir().join("Curated-Thoughts")`.

**Auto-creation:** On startup, if `VaultConfig::get_vault_path()` returns `None`:
1. Compute `default_vault_path()` → `dirs::home_dir().join("Curated-Thoughts")`
2. Create subdirectories: `documents/`, `wiki/`, `.brain/`
3. Persist path via `set_vault_path()`
4. Continue startup (open global DB at `~/.brain/brain.db`, start watcher + pipeline)

**Wizard change:** The vault picker step (Step 3) is removed from the setup wizard. The wizard becomes: Welcome → Ollama → Done. An optional "Use a different folder?" link on the welcome step lets advanced users override before proceeding.

### Single Global Database

The SQLite database stays at `~/.brain/brain.db`. This keeps the architecture simple: one connection, one pipeline worker, no runtime DB swapping. The global config at `~/.brain/config.json` stores the active vault path and global preferences.

**Trade-off:** The DB contains indexed data (chunks, embeddings, wiki metadata) that is specific to one vault's files. When the user switches vaults, the old data is stale. Rather than silently mixing paths from different roots, the app **clears the DB and re-indexes the new vault** on switch.

### DB Backup on Vault Switch

To avoid losing expensive LLM/embedding work, the app offers to **back up the current `brain.db` into the outgoing vault** before clearing it:

1. User clicks "Change vault…" in Settings
2. Confirmation dialog: "Back up your current index before switching? This saves your indexed data to `{current_vault}/.brain/brain.db.bak` so it can be restored if you switch back."
   - **Back up and switch** (default) — copies `~/.brain/brain.db` → `{current_vault}/.brain/brain.db.bak`, then clears and re-indexes
   - **Switch without backup** — clears and re-indexes immediately
   - **Cancel**
3. On switching *back* to a vault that has a `.brain/brain.db.bak`, offer to **restore** it instead of re-indexing: "Found a previous index for this vault. Restore it? (Files changed since the backup will be re-indexed.)"
4. Restore copies the backup over the global DB, then reconciliation (hash check) picks up any files that changed since the backup was made.

### Vault Switching (Settings UI)

**Settings → Vault section:**
- **Current vault:** display path (truncated with tooltip for full path)
- **Reveal in Finder / Explorer** button
- **Change vault…** button → opens `open({ directory: true })` folder dialog
- Explainer text: "Switching vaults will re-index the new folder. You can back up your current index first."

**On switch (backend sequence):**
1. Stop file watcher
2. Stop pipeline worker
3. If user chose "back up": copy `~/.brain/brain.db` → `{old_vault}/.brain/brain.db.bak`
4. Clear all vault-specific tables in the global DB (documents, chunks, embeddings, wiki_pages, folder_rules)
5. Update `config.json` with new vault path
6. If new vault has a `.brain/brain.db.bak` and user chose "restore": copy it over the global DB, then run hash-based reconciliation
7. Create `documents/`, `wiki/`, `.brain/` in new vault if missing
8. Restart pipeline worker
9. Start watcher on new vault root (triggers re-indexing for any unindexed files)
10. Emit `vault-switched` event to frontend

**On switch (frontend sequence):**
1. Listen for `vault-switched` event
2. Clear active file / editor state
3. Re-fetch folder tree from new vault
4. Reset related notes panel
5. Show brief toast: "Switched to {folder name}"

### Header Affordance (optional, high discoverability)

Display the vault folder name (last path segment) in `AppHeader`. Clicking opens a small menu:
- Switch vault… (opens Settings → Vault)
- Reveal in Finder
- Settings

This is optional for v1 but recommended for discoverability since users may not think to look in Settings.

---

## Cloud Sync Consideration

`~/Curated-Thoughts/` is in the home directory, which avoids default iCloud/OneDrive document sync. However, users may still place it under a synced path when switching vaults. The `.brain/` subdirectory contains SQLite which can corrupt under concurrent writes from multiple machines.

**Mitigation:** If a future version detects the vault is inside a known sync folder, warn the user. For now, the home-directory default sidesteps the issue.

---

## Scope

**In scope:**
- `default_vault_path()` function using `dirs::home_dir()`
- Auto-creation of vault structure on first launch
- Remove vault picker from wizard
- Settings → Vault section with change capability
- DB backup to outgoing vault on switch (`brain.db.bak`)
- DB restore from incoming vault's backup (with reconciliation)
- Backend vault-switch logic (stop watcher, clear DB, restart pipeline)
- Frontend state reset on switch
- `vault-switched` Tauri event

**Out of scope (phase 2):**
- Recent vaults list / multi-vault library
- Header vault name affordance (nice-to-have, not required)
- Automatic cloud-sync detection and warnings

---

## Key Constraints

- `dirs::home_dir()` must be available; if it returns `None` on an unusual system, fall back to the current working directory.
- The global DB stays at `~/.brain/brain.db` — no runtime connection swapping needed. Clear + re-index is the switch mechanism.
- Backup files use `.brain/brain.db.bak` inside the vault — a single file copy, not a complex export.
- Vault changes that must clear or restore the global DB, stop/restart the watcher and pipeline, and reconcile state go through the `switch_vault` Tauri command. `set_vault_path` only updates persisted config and ensures the vault directory layout exists; callers must not use it as a substitute for `switch_vault`.
- Existing path-traversal hardening still applies: all file ops validate against the active vault root.
