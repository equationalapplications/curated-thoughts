# SP6: Pandoc Ingestion + Settings Panel + Folder Rules

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ingest PDF/DOCX files via pandoc; add a settings panel accessible from a header toolbar that lets users configure per-folder librarian rules (index / summarize / synthesize + auto-approve toggle).

**Architecture:** `ingest_file` in `pipeline/mod.rs` checks file extension — for `.pdf`/`.docx`/`.odt` it shells out to `pandoc` and writes a shadow copy to `.brain/converted/`, then uses that markdown as the text. A `folder_rules` module exposes CRUD Tauri commands. The app gains a thin header bar (`AppHeader`) with a settings gear button; clicking it opens a `SettingsModal` containing a `FolderRulesPanel`.

**Tech Stack:** Rust (std::process::Command for pandoc), React 18 + TypeScript, existing `folder_rules` SQLite table

---

## File Map

| File | Action | Responsibility |
|---|---|---|
| `src-tauri/src/pipeline/mod.rs` | Modify | Try pandoc conversion before chunking for non-markdown files |
| `src-tauri/src/lib.rs` | Modify | Add `FolderRule` struct + 3 CRUD commands; add `get_folder_rule_for_path` |
| `src/lib/tauri.ts` | Modify | Add `FolderRule`, `getFolderRules`, `setFolderRule`, `deleteFolderRule` |
| `src/components/shell/AppHeader.tsx` | Create | Thin header bar with app title + settings gear button |
| `src/components/settings/SettingsModal.tsx` | Create | Modal container with close button |
| `src/components/settings/FolderRulesPanel.tsx` | Create | List of folder rules + add/edit/delete UI |
| `src/components/shell/AppShell.tsx` | Modify | Add `AppHeader`, hold `showSettings` state |
| `src/index.css` | Modify | Header + settings styles |
| `src/test-setup.ts` | Modify | Mock new commands |

---

### Task 1: Pandoc integration in pipeline

**Files:**
- Modify: `src-tauri/src/pipeline/mod.rs`

- [ ] **Step 1: Add `try_pandoc_convert` helper + update `ingest_file` in `src-tauri/src/pipeline/mod.rs`**

Read the file. Add this helper function before `ingest_file`:

```rust
/// Convert a non-markdown file to markdown via pandoc.
/// Returns the path to the converted .md file in .brain/converted/.
fn try_pandoc_convert(source_path: &str) -> Option<std::path::PathBuf> {
    let p = std::path::Path::new(source_path);
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
    if matches!(ext, "md" | "txt" | "markdown") {
        return None; // no conversion needed
    }
    if !matches!(ext, "pdf" | "docx" | "odt" | "doc" | "rtf") {
        return None; // unsupported
    }

    // .brain/converted/ lives two levels up: <vault>/documents/<file> → <vault>/.brain/converted/
    let converted_dir = p
        .parent()
        .and_then(|d| d.parent())
        .map(|vault| vault.join(".brain").join("converted"))?;

    std::fs::create_dir_all(&converted_dir).ok()?;

    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("doc");
    let out_path = converted_dir.join(format!("{}.md", stem));

    let status = std::process::Command::new("pandoc")
        .args([source_path, "-o", out_path.to_str()?, "--to", "markdown"])
        .status()
        .ok()?;

    if status.success() { Some(out_path) } else { None }
}
```

Then update `ingest_file` to use the converted path when available. Find the line in `ingest_file` that reads:

```rust
let bytes = std::fs::read(path)?;
```

Replace it with:

```rust
let read_path = try_pandoc_convert(path)
    .map(|p| p.to_string_lossy().to_string())
    .unwrap_or_else(|| path.to_string());

let bytes = std::fs::read(&read_path)?;
```

Also remove the early extension filter at the top of `ingest_file` (since pandoc now handles non-markdown). Find:

```rust
if !matches!(ext, "md" | "txt" | "markdown") {
    return Ok(());
}
```

Replace with:

```rust
// Supported: native markdown + pandoc-convertible formats
if !matches!(ext, "md" | "txt" | "markdown" | "pdf" | "docx" | "odt" | "doc" | "rtf") {
    return Ok(());
}
```

- [ ] **Step 2: Write a test for try_pandoc_convert skipping markdown**

Add to `#[cfg(test)]` in `pipeline/mod.rs`:

```rust
#[test]
fn test_pandoc_skips_markdown_files() {
    // markdown files need no conversion
    assert!(try_pandoc_convert("/vault/documents/note.md").is_none());
    assert!(try_pandoc_convert("/vault/documents/note.txt").is_none());
}

#[test]
fn test_pandoc_skips_unsupported_extensions() {
    assert!(try_pandoc_convert("/vault/documents/image.png").is_none());
    assert!(try_pandoc_convert("/vault/documents/data.csv").is_none());
}
```

- [ ] **Step 3: Run tests**

```bash
source "$HOME/.cargo/env" && cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/src-tauri && cargo test pipeline:: 2>&1 | tail -8
```

Expected: tests pass (pandoc tests return None for markdown/unsupported).

- [ ] **Step 4: Build**

```bash
source "$HOME/.cargo/env" && cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/src-tauri && cargo build 2>&1 | grep "^error" | head -10
```

Expected: no errors.

- [ ] **Step 5: Commit**

```bash
cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts
git add src-tauri/src/pipeline/mod.rs
git commit -m "feat: convert PDF/DOCX via pandoc before chunking"
```

---

### Task 2: Folder rules Tauri commands

**Files:**
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Add folder rules struct + 3 Tauri commands to `src-tauri/src/lib.rs`**

Read lib.rs. Add EXACTLY this block before `pub fn run()`:

```rust
// ── Folder rules ──────────────────────────────────────────────────────────────

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct FolderRule {
    pub id: i64,
    pub folder_path: String,
    pub librarian_mode: String, // "index" | "summarize" | "synthesize"
    pub auto_approve: bool,
}

#[tauri::command]
fn get_folder_rules(db_state: State<DbState>) -> Result<Vec<FolderRule>, String> {
    let guard = db_state.0.lock().unwrap();
    let conn = &guard.0;
    let mut stmt = conn
        .prepare("SELECT id, folder_path, librarian_mode, auto_approve FROM folder_rules ORDER BY folder_path")
        .map_err(|e| e.to_string())?;
    let mut rules = Vec::new();
    let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        rules.push(FolderRule {
            id: row.get(0).map_err(|e| e.to_string())?,
            folder_path: row.get(1).map_err(|e| e.to_string())?,
            librarian_mode: row.get(2).map_err(|e| e.to_string())?,
            auto_approve: row.get::<_, i64>(3).map_err(|e| e.to_string())? != 0,
        });
    }
    Ok(rules)
}

#[tauri::command]
fn set_folder_rule(
    folder_path: String,
    librarian_mode: String,
    auto_approve: bool,
    db_state: State<DbState>,
) -> Result<(), String> {
    let auto_approve_int: i64 = if auto_approve { 1 } else { 0 };
    db_state
        .0
        .lock()
        .unwrap()
        .0
        .execute(
            "INSERT INTO folder_rules (folder_path, librarian_mode, auto_approve)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(folder_path) DO UPDATE SET
               librarian_mode = ?2,
               auto_approve = ?3",
            rusqlite::params![folder_path, librarian_mode, auto_approve_int],
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn delete_folder_rule(id: i64, db_state: State<DbState>) -> Result<(), String> {
    db_state
        .0
        .lock()
        .unwrap()
        .0
        .execute("DELETE FROM folder_rules WHERE id = ?1", [id])
        .map_err(|e| e.to_string())?;
    Ok(())
}
```

Register all 3 in `generate_handler![]`:
```rust
get_folder_rules,
set_folder_rule,
delete_folder_rule,
```

- [ ] **Step 2: Build + all tests**

```bash
source "$HOME/.cargo/env" && cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/src-tauri && cargo build 2>&1 | grep "^error" | head -5
source "$HOME/.cargo/env" && cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/src-tauri && cargo test 2>&1 | grep -E "test result|FAILED"
```

Expected: no errors, all tests pass.

- [ ] **Step 3: Commit**

```bash
cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts
git add src-tauri/src/lib.rs
git commit -m "feat: add folder rules CRUD Tauri commands"
```

---

### Task 3: Settings panel frontend

**Files:**
- Modify: `src/lib/tauri.ts`
- Create: `src/components/shell/AppHeader.tsx`
- Create: `src/components/settings/SettingsModal.tsx`
- Create: `src/components/settings/FolderRulesPanel.tsx`
- Modify: `src/components/shell/AppShell.tsx`
- Modify: `src/index.css`
- Modify: `src/test-setup.ts`

- [ ] **Step 1: Append to src/lib/tauri.ts**

```ts
export interface FolderRule {
  id: number;
  folder_path: string;
  librarian_mode: "index" | "summarize" | "synthesize";
  auto_approve: boolean;
}

export const getFolderRules = (): Promise<FolderRule[]> =>
  invoke("get_folder_rules");

export const setFolderRule = (
  folderPath: string,
  librarianMode: string,
  autoApprove: boolean
): Promise<void> =>
  invoke("set_folder_rule", { folderPath, librarianMode, autoApprove });

export const deleteFolderRule = (id: number): Promise<void> =>
  invoke("delete_folder_rule", { id });
```

- [ ] **Step 2: Create src/components/shell/AppHeader.tsx**

```tsx
interface Props {
  onSettingsOpen: () => void;
}

export function AppHeader({ onSettingsOpen }: Props) {
  return (
    <header className="app-header">
      <span className="app-header-title">Curated Thoughts</span>
      <button
        className="app-header-settings"
        onClick={onSettingsOpen}
        aria-label="Settings"
        title="Settings"
      >
        ⚙
      </button>
    </header>
  );
}
```

- [ ] **Step 3: Create src/components/settings/FolderRulesPanel.tsx**

```tsx
import { useState, useEffect } from "react";
import { getFolderRules, setFolderRule, deleteFolderRule, FolderRule } from "../../lib/tauri";

const MODES = ["index", "summarize", "synthesize"] as const;

export function FolderRulesPanel() {
  const [rules, setRules] = useState<FolderRule[]>([]);
  const [folderPath, setFolderPath] = useState("");
  const [mode, setMode] = useState<string>("index");
  const [autoApprove, setAutoApprove] = useState(false);
  const [saving, setSaving] = useState(false);

  const load = () => getFolderRules().then(setRules).catch(() => {});

  useEffect(() => { load(); }, []);

  async function handleAdd() {
    if (!folderPath.trim()) return;
    setSaving(true);
    try {
      await setFolderRule(folderPath.trim(), mode, autoApprove);
      setFolderPath("");
      setMode("index");
      setAutoApprove(false);
      await load();
    } finally {
      setSaving(false);
    }
  }

  async function handleDelete(id: number) {
    await deleteFolderRule(id);
    await load();
  }

  return (
    <div className="folder-rules-panel">
      <h3>Folder Rules</h3>
      <p className="settings-hint">Set how the librarian processes each folder.</p>

      {rules.length > 0 && (
        <div className="rules-list">
          {rules.map((r) => (
            <div key={r.id} className="rule-row">
              <span className="rule-path">{r.folder_path}</span>
              <span className="rule-mode">{r.librarian_mode}</span>
              {r.auto_approve && <span className="rule-auto">auto</span>}
              <button className="rule-delete" onClick={() => handleDelete(r.id)}>✕</button>
            </div>
          ))}
        </div>
      )}

      <div className="rule-form">
        <input
          type="text"
          placeholder="Folder path (e.g. /vault/documents/research)"
          value={folderPath}
          onChange={(e) => setFolderPath(e.target.value)}
          className="rule-input"
        />
        <select value={mode} onChange={(e) => setMode(e.target.value)} className="rule-select">
          {MODES.map((m) => <option key={m} value={m}>{m}</option>)}
        </select>
        <label className="rule-auto-label">
          <input
            type="checkbox"
            checked={autoApprove}
            onChange={(e) => setAutoApprove(e.target.checked)}
          />
          Auto-approve
        </label>
        <button
          className="rule-add-btn"
          onClick={handleAdd}
          disabled={saving || !folderPath.trim()}
        >
          {saving ? "Saving…" : "Add rule"}
        </button>
      </div>
    </div>
  );
}
```

- [ ] **Step 4: Create src/components/settings/SettingsModal.tsx**

```tsx
import { FolderRulesPanel } from "./FolderRulesPanel";

interface Props {
  onClose: () => void;
}

export function SettingsModal({ onClose }: Props) {
  return (
    <div className="review-overlay" onClick={onClose}>
      <div className="settings-modal" onClick={(e) => e.stopPropagation()}>
        <div className="review-header">
          <h2>Settings</h2>
          <button className="review-close" onClick={onClose}>✕</button>
        </div>
        <FolderRulesPanel />
      </div>
    </div>
  );
}
```

- [ ] **Step 5: Update src/components/shell/AppShell.tsx**

Read the file. Add `AppHeader` and `SettingsModal`. Replace entirely:

```tsx
import { useEffect, useState } from "react";
import { AppHeader } from "./AppHeader";
import { Sidebar } from "./Sidebar";
import { EditorPane } from "./EditorPane";
import { RelatedNotes } from "./RelatedNotes";
import { ReviewModal } from "../review/ReviewModal";
import { SettingsModal } from "../settings/SettingsModal";
import { startFileWatcher } from "../../lib/tauri";
import { useReviewQueue } from "../../hooks/useReviewQueue";

interface Props { vaultPath: string }

export function AppShell({ vaultPath }: Props) {
  const [selectedDoc, setSelectedDoc] = useState<string | null>(null);
  const [showReview, setShowReview] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const isWiki = selectedDoc?.includes("/wiki/") ?? false;
  const { queue, refresh } = useReviewQueue();

  useEffect(() => {
    startFileWatcher(vaultPath).catch(console.error);
  }, [vaultPath]);

  return (
    <div className="app-root">
      <AppHeader onSettingsOpen={() => setShowSettings(true)} />
      <div className="app-shell">
        <Sidebar
          reviewCount={queue.length}
          selectedDoc={selectedDoc}
          onDocSelect={setSelectedDoc}
          onReviewOpen={() => setShowReview(true)}
        />
        <EditorPane selectedDoc={selectedDoc} isWiki={isWiki} />
        <RelatedNotes selectedDoc={selectedDoc} />
      </div>
      {showReview && (
        <ReviewModal
          queue={queue}
          vaultPath={vaultPath}
          onClose={() => setShowReview(false)}
          onAction={() => { refresh(); }}
        />
      )}
      {showSettings && <SettingsModal onClose={() => setShowSettings(false)} />}
    </div>
  );
}
```

- [ ] **Step 6: Update src/test-setup.ts** — add 3 mocks before `return Promise.resolve(null)`:

```ts
if (cmd === "get_folder_rules") return Promise.resolve([]);
if (cmd === "set_folder_rule") return Promise.resolve();
if (cmd === "delete_folder_rule") return Promise.resolve();
```

- [ ] **Step 7: Append CSS to src/index.css**

```css
/* ── App header ─────────────────────────────────────────────────────────────── */
.app-root { display: flex; flex-direction: column; height: 100vh; overflow: hidden; }

.app-header {
  height: 38px;
  flex-shrink: 0;
  background: var(--elev-1);
  border-bottom: 1px solid var(--outline-var);
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 0 14px;
  -webkit-app-region: drag;
}
.app-header-title {
  font-family: var(--font-display);
  font-size: 13px;
  font-weight: 600;
  color: var(--on-surface-var);
  letter-spacing: 0.01em;
}
.app-header-settings {
  background: none;
  border: none;
  cursor: pointer;
  font-size: 16px;
  color: var(--outline);
  padding: 4px 6px;
  border-radius: var(--r-sm);
  transition: color 0.15s, background 0.15s;
  -webkit-app-region: no-drag;
}
.app-header-settings:hover { color: var(--on-surface); background: var(--surface-variant); }

/* ── Settings modal ─────────────────────────────────────────────────────────── */
.settings-modal {
  background: var(--bg);
  border-radius: var(--r-lg);
  box-shadow: var(--shadow-lg);
  padding: 32px;
  width: min(540px, 90vw);
  max-height: 80vh;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 20px;
}
.settings-hint { font-size: 13px; color: var(--on-surface-var); }

/* ── Folder rules form ──────────────────────────────────────────────────────── */
.folder-rules-panel { display: flex; flex-direction: column; gap: 12px; }
.folder-rules-panel h3 { font-family: var(--font-display); font-size: 16px; }
.rules-list { display: flex; flex-direction: column; gap: 6px; }
.rule-row {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 10px;
  background: var(--elev-2);
  border-radius: var(--r-sm);
  font-size: 12px;
}
.rule-path { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--on-surface); }
.rule-mode { color: var(--primary); font-weight: 600; }
.rule-auto { font-size: 10px; background: var(--tertiary-cont); color: var(--on-surface-var); padding: 1px 6px; border-radius: 999px; }
.rule-delete { background: none; border: none; color: var(--outline); cursor: pointer; font-size: 13px; padding: 2px 4px; }
.rule-form { display: flex; flex-wrap: wrap; gap: 8px; align-items: center; }
.rule-input {
  flex: 1;
  min-width: 200px;
  padding: 7px 12px;
  border: 1.5px solid var(--outline-var);
  border-radius: var(--r-md);
  font-size: 13px;
  font-family: var(--font-body);
  background: var(--elev-2);
  color: var(--on-surface);
  outline: none;
}
.rule-input:focus { border-color: var(--primary); background: var(--bg); }
.rule-select {
  padding: 7px 10px;
  border: 1.5px solid var(--outline-var);
  border-radius: var(--r-md);
  font-size: 13px;
  font-family: var(--font-body);
  background: var(--elev-2);
  color: var(--on-surface);
  cursor: pointer;
}
.rule-auto-label { display: flex; align-items: center; gap: 6px; font-size: 13px; color: var(--on-surface-var); cursor: pointer; }
.rule-add-btn {
  padding: 8px 18px;
  background: var(--primary);
  color: var(--on-primary);
  border: none;
  border-radius: var(--r-pill);
  font-size: 13px;
  font-weight: 600;
  cursor: pointer;
}
.rule-add-btn:disabled { background: var(--surface-variant); color: var(--outline); cursor: not-allowed; }
```

- [ ] **Step 8: Run tests + build**

```bash
npm test 2>&1 | tail -6 && npm run build 2>&1 | tail -4
```

Expected: 6 tests pass, clean build.

- [ ] **Step 9: Commit**

```bash
cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts
git add src/lib/tauri.ts \
        src/components/shell/AppHeader.tsx \
        src/components/settings/SettingsModal.tsx \
        src/components/settings/FolderRulesPanel.tsx \
        src/components/shell/AppShell.tsx \
        src/index.css src/test-setup.ts
git commit -m "feat: settings panel with folder rules (index/summarize/synthesize + auto-approve)"
```

---

## Self-Review

### Spec coverage

| Spec requirement | Task |
|---|---|
| PDF/DOCX → markdown via pandoc | Task 1 |
| Store shadow copy in .brain/converted/ | Task 1 |
| Per-folder librarian_mode (index/summarize/synthesize) | Task 2 + 3 |
| auto_approve setting per folder | Task 2 + 3 |
| Settings panel | Task 3 |
| Folder rules UI (folder picker → mode → auto_approve) | Task 3 |

**Correctly deferred:**
- Cloud provider management (API keys, Anthropic/OpenAI)
- OS Keychain integration
- sqlite-vec swap
- Synthesize mode (cross-doc synthesis, still uses `summarize` for SP6)
- errors.log file

### Placeholder scan — none found.

### Type consistency
- `FolderRule { id, folder_path, librarian_mode, auto_approve }` — Rust Task 2, TS Task 3, consistent
- `AppHeader { onSettingsOpen }` — defined Task 3, passed AppShell Task 3
- `SettingsModal { onClose }` — defined Task 3, rendered AppShell Task 3
- `AppShell` adds `app-root` wrapper div + `AppHeader` — CSS updated in Task 3
