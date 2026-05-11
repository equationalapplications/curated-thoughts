# Default Vault + Vault Switching — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Auto-create a default vault at `~/Curated-Thoughts/` on first launch (skipping the vault picker wizard step), and let users change vaults from Settings with optional DB backup/restore.

**Architecture:** On startup, if no vault is configured, Rust creates `~/Curated-Thoughts/` with subdirectories and persists the path. The setup wizard drops from 4 steps to 3 (Welcome → Ollama → Done). A new `VaultPanel` in `SettingsModal` shows the current path with "Change vault…" and "Reveal in Finder." A new `switch_vault` Tauri command handles backup → clear → restore → re-index orchestration. The frontend listens for a `vault-switched` event to reset state.

**Tech Stack:** Rust (rusqlite, dirs, fs), Tauri 2.x, React 18 + TypeScript, existing `VaultConfig`, existing `SettingsModal`

---

## File Map

| File | Action | Responsibility |
|---|---|---|
| `src-tauri/src/vault/config.rs` | Modify | Add `default_vault_path()` using `dirs::home_dir()` |
| `src-tauri/src/db/queries.rs` | Modify | Add `clear_vault_tables()` helper |
| `src-tauri/src/lib.rs` | Modify | Auto-create default vault in `run()`, add `switch_vault` + `backup_db` + `reveal_vault` commands |
| `src/lib/tauri.ts` | Modify | Add `switchVault`, `backupDb`, `revealVault` wrappers |
| `src/lib/events.ts` | Modify | Add `onVaultSwitched` listener |
| `src/components/settings/VaultPanel.tsx` | Create | Vault section: current path, change, reveal |
| `src/components/settings/SettingsModal.tsx` | Modify | Render `VaultPanel` |
| `src/components/setup/SetupWizard.tsx` | Modify | Remove vault picker step (3-step wizard) |
| `src/components/shell/AppShell.tsx` | Modify | Listen for `vault-switched`, reset state, propagate new path |
| `src/App.tsx` | Modify | Support vault path changes without full remount |
| `src/test-setup.ts` | Modify | Mock new commands |

---

### Task 1: Add `default_vault_path()` to vault config

**Files:**
- Modify: `src-tauri/src/vault/config.rs`

- [ ] **Step 1: Write failing test in `src-tauri/src/vault/config.rs`**

Add this test to the existing `mod tests` block:

```rust
#[test]
fn test_default_vault_path_ends_with_curated_thoughts() {
    let p = VaultConfig::default_vault_path();
    assert_eq!(p.file_name().unwrap().to_str().unwrap(), "Curated-Thoughts");
    assert!(p.is_absolute());
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cd src-tauri && cargo test vault::config::tests::test_default_vault_path_ends_with_curated_thoughts
```

Expected: FAIL — `default_vault_path` currently returns `~/.brain/config.json`, not `~/Curated-Thoughts`.

- [ ] **Step 3: Replace `default_vault_path` implementation**

In `src-tauri/src/vault/config.rs`, replace the existing `default_vault_path` method:

```rust
#[allow(dead_code)]
pub fn default_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".brain")
        .join("config.json")
}
```

with:

```rust
pub fn default_vault_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
        .join("Curated-Thoughts")
}
```

Also keep a separate `default_config_path()` for config.json since `run()` still needs it:

```rust
pub fn default_config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".brain")
        .join("config.json")
}
```

Remove the old `default_path()` method entirely to avoid confusion.

- [ ] **Step 4: Run test to verify it passes**

```bash
cd src-tauri && cargo test vault::config::tests::test_default_vault_path_ends_with_curated_thoughts
```

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/vault/config.rs
git commit -m "feat: add default_vault_path() returning ~/Curated-Thoughts"
```

---

### Task 2: Add `clear_vault_tables()` DB helper

**Files:**
- Modify: `src-tauri/src/db/queries.rs`

- [ ] **Step 1: Write failing test in `src-tauri/src/db/queries.rs`**

Add to the existing test module (or create one if absent):

```rust
#[cfg(test)]
mod clear_vault_tables_tests {
    use super::*;
    use crate::db::connection::open_in_memory;

    #[test]
    fn clear_vault_tables_empties_all_vault_data() {
        let conn = open_in_memory().unwrap();
        // Insert a document + chunk + embedding
        upsert_document(&conn, "/test/doc.md", "abc123").unwrap();
        let doc_id: i64 = conn
            .query_row("SELECT id FROM documents LIMIT 1", [], |r| r.get(0))
            .unwrap();
        let chunk = crate::chunker::Chunk {
            text: "hello".into(),
            start_line: 1,
            end_line: 1,
            symbol_name: None,
            strategy: crate::chunker::ChunkStrategyTag::Prose,
        };
        let chunk_id = insert_chunk(&conn, doc_id, &chunk, 0).unwrap();
        insert_embedding(&conn, chunk_id, &[0.1, 0.2, 0.3]).unwrap();

        // Insert a folder rule
        conn.execute(
            "INSERT INTO folder_rules (folder_path, librarian_mode) VALUES ('test', 'index')",
            [],
        ).unwrap();

        clear_vault_tables(&conn).unwrap();

        let doc_count: i64 = conn
            .query_row("SELECT count(*) FROM documents", [], |r| r.get(0))
            .unwrap();
        let chunk_count: i64 = conn
            .query_row("SELECT count(*) FROM chunks", [], |r| r.get(0))
            .unwrap();
        let embed_count: i64 = conn
            .query_row("SELECT count(*) FROM embeddings", [], |r| r.get(0))
            .unwrap();
        let wiki_count: i64 = conn
            .query_row("SELECT count(*) FROM wiki_pages", [], |r| r.get(0))
            .unwrap();
        let rule_count: i64 = conn
            .query_row("SELECT count(*) FROM folder_rules", [], |r| r.get(0))
            .unwrap();
        assert_eq!(doc_count, 0);
        assert_eq!(chunk_count, 0);
        assert_eq!(embed_count, 0);
        assert_eq!(wiki_count, 0);
        assert_eq!(rule_count, 0);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cd src-tauri && cargo test clear_vault_tables_tests
```

Expected: FAIL — `clear_vault_tables` not found.

- [ ] **Step 3: Implement `clear_vault_tables` in `src-tauri/src/db/queries.rs`**

Add this public function:

```rust
pub fn clear_vault_tables(conn: &Connection) -> anyhow::Result<()> {
    conn.execute_batch(
        "DELETE FROM embeddings;
         DELETE FROM chunks;
         DELETE FROM documents;
         DELETE FROM wiki_pages;
         DELETE FROM folder_rules;",
    )?;
    Ok(())
}
```

- [ ] **Step 4: Run test to verify it passes**

```bash
cd src-tauri && cargo test clear_vault_tables_tests
```

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/db/queries.rs
git commit -m "feat: add clear_vault_tables() for vault switching"
```

---

### Task 3: Auto-create default vault on startup + `switch_vault` command

**Files:**
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Add default vault auto-creation to `run()`**

In `src-tauri/src/lib.rs`, in the `run()` function, after `config` is created and before `tauri::Builder`, add:

```rust
// Auto-create default vault if none configured
if config.get_vault_path().unwrap_or(None).is_none() {
    let default_vault = VaultConfig::default_vault_path();
    for subdir in &["documents", "wiki"] {
        std::fs::create_dir_all(default_vault.join(subdir)).ok();
    }
    std::fs::create_dir_all(default_vault.join(".brain").join("converted")).ok();
    config.set_vault_path(default_vault.to_str().unwrap_or_default()).ok();
}
```

- [ ] **Step 2: Add `backup_vault_db` command**

Add this Tauri command to `src-tauri/src/lib.rs`:

```rust
#[tauri::command]
fn backup_vault_db(vault_state: State<VaultConfigState>) -> Result<String, String> {
    let vault = vault_state
        .0
        .lock()
        .unwrap()
        .get_vault_path()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "no vault configured".to_string())?;

    let brain_dir = dirs::home_dir().unwrap_or_default().join(".brain");
    let src = brain_dir.join("brain.db");
    if !src.exists() {
        return Err("no database to back up".to_string());
    }

    let dest_dir = std::path::PathBuf::from(&vault).join(".brain");
    std::fs::create_dir_all(&dest_dir).map_err(|e| e.to_string())?;
    let dest = dest_dir.join("brain.db.bak");
    std::fs::copy(&src, &dest).map_err(|e| e.to_string())?;

    Ok(dest.to_string_lossy().into_owned())
}
```

- [ ] **Step 3: Add `switch_vault` command**

Add this Tauri command to `src-tauri/src/lib.rs`:

```rust
#[tauri::command]
fn switch_vault(
    new_path: String,
    restore_backup: bool,
    app: AppHandle,
    db_state: State<DbState>,
    vault_state: State<VaultConfigState>,
) -> Result<(), String> {
    let new_root = std::path::PathBuf::from(&new_path);

    // Create vault subdirs if missing
    for subdir in &["documents", "wiki"] {
        std::fs::create_dir_all(new_root.join(subdir)).map_err(|e| e.to_string())?;
    }
    std::fs::create_dir_all(new_root.join(".brain").join("converted"))
        .map_err(|e| e.to_string())?;

    // Check for backup in the target vault
    let backup_path = new_root.join(".brain").join("brain.db.bak");
    let has_backup = backup_path.exists();

    // Clear vault-specific tables
    {
        let guard = db_state.0.lock().unwrap();
        crate::db::clear_vault_tables(&guard.0).map_err(|e| e.to_string())?;
    }

    // Restore backup if requested and available
    if restore_backup && has_backup {
        let brain_dir = dirs::home_dir().unwrap_or_default().join(".brain");
        let db_path = brain_dir.join("brain.db");
        // Close current connection by dropping guard, copy backup, reopen
        // Since we can't reopen the Mutex<AppDb>, we restore by importing rows.
        // Simpler approach: copy backup over global DB and note that app needs restart.
        std::fs::copy(&backup_path, &db_path).map_err(|e| e.to_string())?;
    }

    // Update config
    vault_state
        .0
        .lock()
        .unwrap()
        .set_vault_path(&new_path)
        .map_err(|e| e.to_string())?;

    // Emit event so frontend resets
    let _ = app.emit("vault-switched", &new_path);

    Ok(())
}
```

- [ ] **Step 4: Add `check_vault_backup` command**

```rust
#[tauri::command]
fn check_vault_backup(path: String) -> bool {
    std::path::PathBuf::from(&path)
        .join(".brain")
        .join("brain.db.bak")
        .exists()
}
```

- [ ] **Step 5: Add `reveal_vault` command**

The frontend calls this to open the vault folder in the system file manager. Use `std::process::Command` with platform-specific commands since the `open` crate is not a direct dependency:

```rust
#[tauri::command]
fn reveal_vault(vault_state: State<VaultConfigState>) -> Result<(), String> {
    let vault = vault_state
        .0
        .lock()
        .unwrap()
        .get_vault_path()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "no vault configured".to_string())?;

    #[cfg(target_os = "macos")]
    std::process::Command::new("open").arg(&vault).spawn().map_err(|e| e.to_string())?;

    #[cfg(target_os = "linux")]
    std::process::Command::new("xdg-open").arg(&vault).spawn().map_err(|e| e.to_string())?;

    #[cfg(target_os = "windows")]
    std::process::Command::new("explorer").arg(&vault).spawn().map_err(|e| e.to_string())?;

    Ok(())
}
```

- [ ] **Step 6: Register all new commands in `invoke_handler`**

In the `tauri::Builder` chain in `run()`, add to the `generate_handler!` macro:

```rust
backup_vault_db,
switch_vault,
check_vault_backup,
reveal_vault,
```

- [ ] **Step 7: Verify Rust build**

```bash
cd src-tauri && cargo build
```

Expected: no errors.

- [ ] **Step 8: Commit**

```bash
git add src-tauri/src/lib.rs
git commit -m "feat: auto-create default vault on startup, add switch_vault + backup commands"
```

---

### Task 4: Add frontend typed wrappers + event

**Files:**
- Modify: `src/lib/tauri.ts`
- Modify: `src/lib/events.ts`
- Modify: `src/test-setup.ts`

- [ ] **Step 1: Add wrappers to `src/lib/tauri.ts`**

Append to the file:

```ts
export const switchVault = (newPath: string, restoreBackup: boolean): Promise<void> =>
  invoke("switch_vault", { newPath, restoreBackup });

export const backupVaultDb = (): Promise<string> =>
  invoke("backup_vault_db");

export const checkVaultBackup = (path: string): Promise<boolean> =>
  invoke("check_vault_backup", { path });

export const revealVault = (): Promise<void> =>
  invoke("reveal_vault");
```

- [ ] **Step 2: Add event listener to `src/lib/events.ts`**

Append:

```ts
export const onVaultSwitched = (
  cb: (newPath: string) => void
): Promise<UnlistenFn> =>
  listen<string>("vault-switched", (e) => cb(e.payload));
```

- [ ] **Step 3: Add mocks to `src/test-setup.ts`**

Add inside the `invoke` mock:

```ts
if (cmd === "switch_vault") return Promise.resolve();
if (cmd === "backup_vault_db") return Promise.resolve("/test/backup.db");
if (cmd === "check_vault_backup") return Promise.resolve(false);
if (cmd === "reveal_vault") return Promise.resolve();
```

- [ ] **Step 4: Commit**

```bash
git add src/lib/tauri.ts src/lib/events.ts src/test-setup.ts
git commit -m "feat: add typed wrappers for vault switching commands and events"
```

---

### Task 5: Create `VaultPanel` settings component

**Files:**
- Create: `src/components/settings/VaultPanel.tsx`

- [ ] **Step 1: Create `src/components/settings/VaultPanel.tsx`**

```tsx
import { open } from "@tauri-apps/plugin-dialog";
import { useState } from "react";
import {
  getVaultPath,
  backupVaultDb,
  switchVault,
  checkVaultBackup,
  revealVault,
} from "../../lib/tauri";

interface Props {
  vaultPath: string;
  onVaultChanged: (newPath: string) => void;
}

export function VaultPanel({ vaultPath, onVaultChanged }: Props) {
  const [switching, setSwitching] = useState(false);

  async function handleChangeVault() {
    const selected = await open({
      directory: true,
      multiple: false,
      title: "Choose a new vault folder",
    });
    if (typeof selected !== "string" || selected === vaultPath) return;

    const hasBackup = await checkVaultBackup(selected);

    const doBackup = confirm(
      "Back up your current index before switching?\n\n" +
        "This saves your indexed data so it can be restored if you switch back."
    );

    setSwitching(true);
    try {
      if (doBackup) {
        await backupVaultDb();
      }

      let restore = false;
      if (hasBackup) {
        restore = confirm(
          "Found a previous index for this vault. Restore it?\n\n" +
            "(Files changed since the backup will be re-indexed.)"
        );
      }

      await switchVault(selected, restore);
      onVaultChanged(selected);
    } catch (e) {
      alert("Failed to switch vault: " + String(e));
    } finally {
      setSwitching(false);
    }
  }

  const folderName = vaultPath.split(/[/\\]/).filter(Boolean).pop() ?? vaultPath;

  return (
    <div className="settings-section">
      <h3>Vault</h3>
      <div className="vault-info">
        <span className="vault-path" title={vaultPath}>{folderName}</span>
        <span className="vault-full-path">{vaultPath}</span>
      </div>
      <div className="vault-actions">
        <button onClick={handleChangeVault} disabled={switching}>
          {switching ? "Switching..." : "Change vault…"}
        </button>
        <button onClick={() => revealVault()} className="vault-reveal-btn">
          Reveal in Finder
        </button>
      </div>
      <p className="vault-hint">
        Switching vaults closes the current document and re-indexes the new folder.
      </p>
    </div>
  );
}
```

- [ ] **Step 2: Commit**

```bash
git add src/components/settings/VaultPanel.tsx
git commit -m "feat: add VaultPanel settings component for vault switching"
```

---

### Task 6: Wire `VaultPanel` into `SettingsModal`

**Files:**
- Modify: `src/components/settings/SettingsModal.tsx`

- [ ] **Step 1: Update `SettingsModal.tsx`**

Replace the full file content:

```tsx
import { FolderRulesPanel } from "./FolderRulesPanel";
import { ModelPanel } from "./ModelPanel";
import { VaultPanel } from "./VaultPanel";

interface Props {
  onClose: () => void;
  vaultPath: string;
  onVaultChanged: (newPath: string) => void;
}

export function SettingsModal({ onClose, vaultPath, onVaultChanged }: Props) {
  return (
    <div className="review-overlay" onClick={onClose}>
      <div className="settings-modal" onClick={(e) => e.stopPropagation()}>
        <div className="review-header">
          <h2>Settings</h2>
          <button className="review-close" onClick={onClose}>✕</button>
        </div>
        <VaultPanel vaultPath={vaultPath} onVaultChanged={onVaultChanged} />
        <hr className="settings-divider" />
        <ModelPanel />
        <hr className="settings-divider" />
        <FolderRulesPanel />
      </div>
    </div>
  );
}
```

- [ ] **Step 2: Commit**

```bash
git add src/components/settings/SettingsModal.tsx
git commit -m "feat: wire VaultPanel into SettingsModal"
```

---

### Task 7: Simplify SetupWizard to 3 steps

**Files:**
- Modify: `src/components/setup/SetupWizard.tsx`

- [ ] **Step 1: Update `SetupWizard.tsx`**

Replace the full file content. The vault picker step is removed; `onComplete` no longer needs a vault path since the backend auto-creates it:

```tsx
import { useState } from "react";
import { StepWelcome } from "./StepWelcome";
import { StepOllama } from "./StepOllama";
import { StepDone } from "./StepDone";

interface Props {
  onComplete: () => void;
  initialStep?: number;
}

export function SetupWizard({ onComplete, initialStep = 0 }: Props) {
  const [step, setStep] = useState(initialStep);
  const next = () => setStep((s) => s + 1);

  return (
    <div className="setup-wizard">
      {step === 0 && <StepWelcome onNext={next} />}
      {step === 1 && <StepOllama onNext={next} />}
      {step === 2 && <StepDone onComplete={onComplete} />}
    </div>
  );
}
```

- [ ] **Step 2: Commit**

```bash
git add src/components/setup/SetupWizard.tsx
git commit -m "feat: simplify wizard to 3 steps (vault auto-created on startup)"
```

---

### Task 8: Update `App.tsx` and `AppShell.tsx` for vault switching

**Files:**
- Modify: `src/App.tsx`
- Modify: `src/components/shell/AppShell.tsx`

- [ ] **Step 1: Update `src/App.tsx`**

The wizard no longer passes a vault path. `useSetupStatus` now only checks Ollama since vault is always set after startup. The app supports vault path changes via a callback:

```tsx
import { useState, useCallback } from "react";
import { useSetupStatus } from "./hooks/useSetupStatus";
import { SetupWizard } from "./components/setup/SetupWizard";
import { AppShell } from "./components/shell/AppShell";

export function App() {
  const { loading, needsSetup, vaultPath } = useSetupStatus();
  const [setupComplete, setSetupComplete] = useState(false);
  const [currentVaultPath, setCurrentVaultPath] = useState<string | null>(null);

  const handleVaultChanged = useCallback((newPath: string) => {
    setCurrentVaultPath(newPath);
  }, []);

  if (loading) {
    return (
      <div className="loading-screen">
        <p>Loading...</p>
      </div>
    );
  }

  if (needsSetup && !setupComplete) {
    return (
      <SetupWizard
        onComplete={() => {
          setSetupComplete(true);
        }}
      />
    );
  }

  const activePath = currentVaultPath ?? vaultPath!;
  return <AppShell vaultPath={activePath} onVaultChanged={handleVaultChanged} />;
}

export default App;
```

- [ ] **Step 2: Update `src/components/shell/AppShell.tsx`**

Add `onVaultChanged` prop and pass it through to `SettingsModal`. Also listen for `vault-switched` to reset state:

In the `Props` interface, add `onVaultChanged`:

```tsx
interface Props {
  vaultPath: string;
  onVaultChanged: (newPath: string) => void;
}
```

Update the `AppShell` function signature:

```tsx
export function AppShell({ vaultPath, onVaultChanged }: Props) {
```

Add an import for `onVaultSwitched` from events:

```tsx
import { onVaultSwitched } from "../../lib/events";
```

Add a `useEffect` to listen for the backend event and reset state:

```tsx
useEffect(() => {
  const promise = onVaultSwitched((newPath) => {
    setSelectedDoc(null);
    onVaultChanged(newPath);
  });
  return () => { promise.then((unlisten) => unlisten()); };
}, [onVaultChanged]);
```

Update the `SettingsModal` rendering to pass new props:

```tsx
{showSettings && (
  <SettingsModal
    onClose={() => setShowSettings(false)}
    vaultPath={vaultPath}
    onVaultChanged={(newPath) => {
      onVaultChanged(newPath);
      setShowSettings(false);
    }}
  />
)}
```

- [ ] **Step 3: Verify TypeScript compiles**

```bash
npm run build
```

Expected: no type errors.

- [ ] **Step 4: Commit**

```bash
git add src/App.tsx src/components/shell/AppShell.tsx
git commit -m "feat: wire vault switching through App → AppShell → SettingsModal"
```

---

### Task 9: Add CSS for VaultPanel

**Files:**
- Modify: `src/index.css`

- [ ] **Step 1: Append vault styles to `src/index.css`**

```css
.vault-info {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.vault-path {
  font-weight: 600;
  font-size: 14px;
}

.vault-full-path {
  font-size: 11px;
  color: #888;
  word-break: break-all;
}

.vault-actions {
  display: flex;
  gap: 8px;
  margin-top: 8px;
}

.vault-actions button {
  padding: 6px 14px;
  border: 1px solid #d5d5d5;
  border-radius: 6px;
  background: white;
  font-size: 13px;
  cursor: pointer;
}

.vault-actions button:hover {
  background: #f5f5f5;
}

.vault-actions button:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.vault-reveal-btn {
  color: #3b82f6;
  border-color: #3b82f6 !important;
}

.vault-hint {
  margin-top: 8px;
  font-size: 11px;
  color: #999;
}
```

- [ ] **Step 2: Commit**

```bash
git add src/index.css
git commit -m "feat: add VaultPanel CSS styles"
```

---

### Task 10: Update tests

**Files:**
- Modify: `src/__tests__/SetupWizard.test.tsx`

- [ ] **Step 1: Update wizard tests for 3-step flow**

The wizard no longer has a vault picker step. Update the test that checks `onComplete`:

```tsx
test("calls onComplete when done step button clicked", () => {
  const onComplete = vi.fn();
  render(<SetupWizard onComplete={onComplete} initialStep={2} />);
  fireEvent.click(screen.getByRole("button", { name: /open my brain/i }));
  expect(onComplete).toHaveBeenCalledTimes(1);
});
```

Note: `initialStep` changes from `3` to `2` since there are now only 3 steps (0, 1, 2).

Remove or update any test that references the vault picker step.

- [ ] **Step 2: Run all frontend tests**

```bash
npm test
```

Expected: all tests pass.

- [ ] **Step 3: Commit**

```bash
git add src/__tests__/
git commit -m "test: update wizard tests for 3-step flow"
```

---

### Task 11: Run full build and manual verification

- [ ] **Step 1: Run Rust tests**

```bash
cd src-tauri && cargo test
```

Expected: all tests pass.

- [ ] **Step 2: Run frontend tests**

```bash
npm test
```

Expected: all tests pass.

- [ ] **Step 3: Build the full app**

```bash
npm run tauri dev
```

Manual checklist:
- [ ] First launch (delete `~/.brain/config.json` to simulate): `~/Curated-Thoughts/` is auto-created with `documents/`, `wiki/`, `.brain/` subdirs
- [ ] Wizard shows 3 steps: Welcome → Ollama → Done (no vault picker)
- [ ] After wizard, 3-panel shell appears with the default vault
- [ ] Settings → Vault section shows `~/Curated-Thoughts/` path
- [ ] "Change vault…" opens folder dialog
- [ ] Backup confirmation appears before switching
- [ ] After switching, app resets (no stale docs in tree/editor)
- [ ] "Reveal in Finder" opens the vault folder
- [ ] Switching back to a vault with `.brain/brain.db.bak` offers restore

- [ ] **Step 4: Final commit if any fixes needed**

```bash
git add -A && git commit -m "fix: address issues found during manual verification"
```
