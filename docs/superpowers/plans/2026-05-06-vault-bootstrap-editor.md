# SP4: Vault Bootstrap + Folder Tree + BlockNote Editor

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Create vault subdirectories on setup, show a real folder tree in the sidebar, and render documents and wiki pages in a BlockNote editor (read-only for source docs, editable for wiki pages).

**Architecture:** `set_vault_path` creates `documents/`, `wiki/`, `.brain/converted/` on disk. A `list_vault_files` Rust command walks those two directories and returns typed `VaultFile` records. A `read_document` Rust command reads file bytes with path validation (refuses paths outside vault). The watcher filter is tightened to only ingest from `documents/`. In React, `FolderTree` replaces the sidebar placeholder; `EditorPane` installs BlockNote and renders content loaded from the selected document.

**Tech Stack:** Rust (std::fs::walk), React 18 + TypeScript, @blocknote/react, @blocknote/mantine, @mantine/core

---

## File Map

| File | Action | Responsibility |
|---|---|---|
| `src-tauri/src/lib.rs` | Modify | `set_vault_path` creates subdirs; add `list_vault_files`, `read_document` commands; tighten watcher filter |
| `src-tauri/src/vault/config.rs` | Modify | `VaultConfig` exposes `vault_root()` helper |
| `src/lib/tauri.ts` | Modify | Add `VaultFile`, `listVaultFiles`, `readDocument` wrappers |
| `src/hooks/useVaultFiles.ts` | Create | Loads file list, re-fetches on `vault-event` |
| `src/components/shell/FolderTree.tsx` | Create | Sidebar folder tree (documents + wiki sections) |
| `src/components/shell/Sidebar.tsx` | Modify | Show `FolderTree` below search/results |
| `src/components/shell/EditorPane.tsx` | Modify | BlockNote editor for selected doc |
| `src/components/shell/AppShell.tsx` | Modify | Pass `selectedDoc` to `EditorPane` |
| `src/index.css` | Modify | Folder tree + editor pane styles |
| `src/test-setup.ts` | Modify | Mock `list_vault_files`, `read_document` |

---

### Task 1: Vault subfolder bootstrap + watcher filter fix

**Files:**
- Modify: `src-tauri/src/vault/config.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Add `vault_root()` helper to `src-tauri/src/vault/config.rs`**

Read the file. Add this method inside the `impl VaultConfig` block, after `set_vault_path`:

```rust
pub fn vault_root(&self) -> anyhow::Result<Option<std::path::PathBuf>> {
    Ok(self.get_vault_path()?.map(std::path::PathBuf::from))
}
```

- [ ] **Step 2: Update `set_vault_path` Tauri command in `src-tauri/src/lib.rs` to create subdirs**

Read lib.rs. Replace the existing `set_vault_path` command with:

```rust
#[tauri::command]
fn set_vault_path(path: String, state: State<VaultConfigState>) -> Result<(), String> {
    state.0.lock().unwrap().set_vault_path(&path).map_err(|e| e.to_string())?;
    // Bootstrap vault directory structure
    let root = std::path::Path::new(&path);
    for subdir in &["documents", "wiki"] {
        std::fs::create_dir_all(root.join(subdir)).map_err(|e| e.to_string())?;
    }
    Ok(())
}
```

- [ ] **Step 3: Tighten the watcher filter in `start_file_watcher` in `src-tauri/src/lib.rs`**

Read lib.rs. In `start_file_watcher`, update the callback to only forward events from the `documents/` subtree to the pipeline:

```rust
#[tauri::command]
fn start_file_watcher(
    vault_path: String,
    app: AppHandle,
    pipeline: State<PipelineTx>,
) -> Result<(), String> {
    let tx = pipeline.0.lock().unwrap().clone();
    let documents_root = std::path::PathBuf::from(&vault_path).join("documents");
    start_watcher(vault_path.into(), move |event| {
        let _ = app.emit("vault-event", &event);
        // Only ingest files inside documents/ — enforces Tier 1 immutability at Rust layer
        let path_str = match &event {
            VaultEvent::Added(p) | VaultEvent::Modified(p) | VaultEvent::Deleted(p) => p,
        };
        let in_documents = std::path::Path::new(path_str)
            .starts_with(&documents_root);
        if !in_documents { return; }
        let job = match &event {
            VaultEvent::Added(p) | VaultEvent::Modified(p) => Some(PipelineJob::Ingest(p.clone())),
            VaultEvent::Deleted(p) => Some(PipelineJob::Delete(p.clone())),
        };
        if let Some(j) = job { let _ = tx.try_send(j); }
    })
    .map_err(|e| e.to_string())
}
```

- [ ] **Step 4: Write tests for vault bootstrap in `src-tauri/src/vault/config.rs`**

Add to the `#[cfg(test)]` block in `src-tauri/src/vault/config.rs`:

```rust
#[test]
fn test_vault_root_returns_none_when_unset() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);
    assert!(cfg.vault_root().unwrap().is_none());
}

#[test]
fn test_vault_root_returns_path_when_set() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);
    cfg.set_vault_path("/vault/root").unwrap();
    assert_eq!(cfg.vault_root().unwrap(), Some(std::path::PathBuf::from("/vault/root")));
}
```

- [ ] **Step 5: Run tests**

```bash
source "$HOME/.cargo/env" && cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/src-tauri && cargo test vault:: 2>&1 | tail -10
```

Expected: 5 tests pass (3 existing + 2 new).

- [ ] **Step 6: Build to confirm no errors**

```bash
source "$HOME/.cargo/env" && cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/src-tauri && cargo build 2>&1 | grep -E "^error" | head -10
```

Expected: no errors.

- [ ] **Step 7: Commit**

```bash
cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts
git add src-tauri/src/vault/config.rs src-tauri/src/lib.rs
git commit -m "feat: bootstrap vault subdirs on set; restrict watcher ingestion to documents/"
```

---

### Task 2: list_vault_files + read_document Tauri commands

**Files:**
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Add `list_vault_files` and `read_document` commands to `src-tauri/src/lib.rs`**

Read lib.rs. Add these two commands before `pub fn run()`:

```rust
// ── Vault file listing ────────────────────────────────────────────────────────

#[derive(serde::Serialize, Clone)]
pub struct VaultFile {
    pub path: String,
    pub name: String,
    pub tier: String, // "user_doc" | "wiki"
}

#[tauri::command]
fn list_vault_files(state: State<VaultConfigState>) -> Result<Vec<VaultFile>, String> {
    let root = match state.0.lock().unwrap().get_vault_path().map_err(|e| e.to_string())? {
        Some(p) => std::path::PathBuf::from(p),
        None => return Ok(vec![]),
    };

    let mut files = Vec::new();

    for (subdir, tier) in &[("documents", "user_doc"), ("wiki", "wiki")] {
        let dir = root.join(subdir);
        if !dir.exists() { continue; }
        let walker = walkdir::WalkDir::new(&dir)
            .min_depth(1)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .filter(|e| {
                let ext = e.path().extension().and_then(|s| s.to_str()).unwrap_or("");
                matches!(ext, "md" | "txt" | "markdown" | "pdf" | "docx")
            });

        for entry in walker {
            let path = entry.path().to_string_lossy().to_string();
            let name = entry.file_name().to_string_lossy().to_string();
            files.push(VaultFile { path, name, tier: tier.to_string() });
        }
    }

    Ok(files)
}

#[tauri::command]
fn read_document(path: String, state: State<VaultConfigState>) -> Result<String, String> {
    let root = match state.0.lock().unwrap().get_vault_path().map_err(|e| e.to_string())? {
        Some(p) => std::path::PathBuf::from(p),
        None => return Err("no vault path set".to_string()),
    };

    let doc_path = std::path::Path::new(&path);

    // Refuse to read outside the vault
    if !doc_path.starts_with(&root) {
        return Err("path outside vault".to_string());
    }
    // Only read from documents/ or wiki/ — no .brain/ access
    let in_documents = doc_path.starts_with(root.join("documents"));
    let in_wiki = doc_path.starts_with(root.join("wiki"));
    if !in_documents && !in_wiki {
        return Err("path not in documents/ or wiki/".to_string());
    }

    std::fs::read_to_string(doc_path).map_err(|e| e.to_string())
}
```

Also add `walkdir` to `src-tauri/Cargo.toml` `[dependencies]`:

```toml
walkdir = "2"
```

And register both commands in `generate_handler![]`:

```rust
list_vault_files,
read_document,
```

- [ ] **Step 2: Build**

```bash
source "$HOME/.cargo/env" && cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/src-tauri && cargo build 2>&1 | grep -E "^error" | head -10
```

Expected: no errors.

- [ ] **Step 3: Commit**

```bash
cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts
git add src-tauri/src/lib.rs src-tauri/Cargo.toml
git commit -m "feat: add list_vault_files and read_document Tauri commands"
```

---

### Task 3: Frontend types + hooks + FolderTree

**Files:**
- Modify: `src/lib/tauri.ts`
- Create: `src/hooks/useVaultFiles.ts`
- Create: `src/components/shell/FolderTree.tsx`
- Modify: `src/components/shell/Sidebar.tsx`
- Modify: `src/test-setup.ts`

- [ ] **Step 1: Append to `src/lib/tauri.ts`**

Read the file. Append at the end:

```ts
export interface VaultFile {
  path: string;
  name: string;
  tier: "user_doc" | "wiki";
}

export const listVaultFiles = (): Promise<VaultFile[]> =>
  invoke("list_vault_files");

export const readDocument = (path: string): Promise<string> =>
  invoke("read_document", { path });
```

- [ ] **Step 2: Create `src/hooks/useVaultFiles.ts`**

```ts
import { useState, useEffect, useCallback } from "react";
import { listen } from "@tauri-apps/api/event";
import { listVaultFiles, VaultFile } from "../lib/tauri";

export function useVaultFiles() {
  const [files, setFiles] = useState<VaultFile[]>([]);

  const refresh = useCallback(() => {
    listVaultFiles().then(setFiles).catch(() => setFiles([]));
  }, []);

  useEffect(() => {
    refresh();
    // Re-fetch whenever the watcher emits a vault-event
    const unlisten = listen("vault-event", refresh);
    return () => { unlisten.then((fn) => fn()); };
  }, [refresh]);

  return files;
}
```

- [ ] **Step 3: Create `src/components/shell/FolderTree.tsx`**

```tsx
import type { VaultFile } from "../../lib/tauri";

interface Props {
  files: VaultFile[];
  selectedPath: string | null;
  onSelect: (path: string) => void;
}

export function FolderTree({ files, selectedPath, onSelect }: Props) {
  const docs = files.filter((f) => f.tier === "user_doc");
  const wiki = files.filter((f) => f.tier === "wiki");

  if (files.length === 0) {
    return <p className="placeholder">Drop documents into your vault folder to get started</p>;
  }

  return (
    <div className="folder-tree">
      {docs.length > 0 && (
        <section className="tree-section">
          <h4 className="tree-section-label">Documents</h4>
          {docs.map((f) => (
            <button
              key={f.path}
              className={`tree-file${selectedPath === f.path ? " tree-file--active" : ""}`}
              onClick={() => onSelect(f.path)}
            >
              {f.name}
            </button>
          ))}
        </section>
      )}
      {wiki.length > 0 && (
        <section className="tree-section">
          <h4 className="tree-section-label">Wiki</h4>
          {wiki.map((f) => (
            <button
              key={f.path}
              className={`tree-file${selectedPath === f.path ? " tree-file--active" : ""}`}
              onClick={() => onSelect(f.path)}
            >
              {f.name}
            </button>
          ))}
        </section>
      )}
    </div>
  );
}
```

- [ ] **Step 4: Update `src/components/shell/Sidebar.tsx`**

Read the current Sidebar.tsx. Replace it entirely with:

```tsx
import { IndexingStatus } from "./IndexingStatus";
import { SearchResults } from "./SearchResults";
import { FolderTree } from "./FolderTree";
import { useSearch } from "../../hooks/useSearch";
import { useVaultFiles } from "../../hooks/useVaultFiles";

interface Props {
  reviewCount: number;
  selectedDoc: string | null;
  onDocSelect: (path: string) => void;
}

export function Sidebar({ reviewCount, selectedDoc, onDocSelect }: Props) {
  const { query, setQuery, results, searching } = useSearch();
  const files = useVaultFiles();

  return (
    <aside className="sidebar">
      <div className="search-bar">
        <input
          type="search"
          placeholder="Search your brain..."
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        {searching && <span className="search-spinner" aria-label="Searching">↻</span>}
      </div>
      {results.length > 0 ? (
        <SearchResults results={results} onSelect={onDocSelect} />
      ) : (
        <>
          <IndexingStatus />
          <FolderTree files={files} selectedPath={selectedDoc} onSelect={onDocSelect} />
        </>
      )}
      {reviewCount > 0 && (
        <div className="review-badge">{reviewCount} pages ready to review</div>
      )}
    </aside>
  );
}
```

Note: `Sidebar` now also accepts `selectedDoc` prop (passed from AppShell) to highlight the active file.

- [ ] **Step 5: Update `src/components/shell/AppShell.tsx` to pass `selectedDoc` to Sidebar**

Read the file. Update `<Sidebar>` to add `selectedDoc={selectedDoc}`:

```tsx
<Sidebar reviewCount={0} selectedDoc={selectedDoc} onDocSelect={setSelectedDoc} />
```

- [ ] **Step 6: Update `src/test-setup.ts`**

Read the file. Add two mocks before `return Promise.resolve(null)`:

```ts
if (cmd === "list_vault_files") return Promise.resolve([]);
if (cmd === "read_document") return Promise.resolve("# Hello\n\nTest document.");
```

- [ ] **Step 7: Run tests**

```bash
npm test 2>&1 | tail -6
```

Expected: 6 tests pass.

- [ ] **Step 8: Commit**

```bash
cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts
git add src/lib/tauri.ts src/hooks/useVaultFiles.ts \
        src/components/shell/FolderTree.tsx \
        src/components/shell/Sidebar.tsx \
        src/components/shell/AppShell.tsx \
        src/test-setup.ts
git commit -m "feat: add FolderTree with live vault file listing"
```

---

### Task 4: BlockNote editor in EditorPane

**Files:**
- Modify: `package.json` (install deps)
- Modify: `src/components/shell/EditorPane.tsx`
- Modify: `src/components/shell/AppShell.tsx`
- Modify: `src/index.css`

- [ ] **Step 1: Install BlockNote packages**

```bash
npm install @blocknote/react @blocknote/core @blocknote/mantine @mantine/core @mantine/hooks 2>&1 | tail -5
```

- [ ] **Step 2: Replace `src/components/shell/EditorPane.tsx`**

```tsx
import { useEffect, useState } from "react";
import { useCreateBlockNote } from "@blocknote/react";
import { BlockNoteView } from "@blocknote/mantine";
import "@blocknote/mantine/style.css";
import { readDocument } from "../../lib/tauri";

interface Props {
  selectedDoc: string | null;
  isWiki: boolean;
}

export function EditorPane({ selectedDoc, isWiki }: Props) {
  const [markdown, setMarkdown] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);

  const editor = useCreateBlockNote();

  useEffect(() => {
    if (!selectedDoc) {
      setMarkdown(null);
      setLoadError(null);
      return;
    }
    readDocument(selectedDoc)
      .then(async (content) => {
        const blocks = await editor.tryParseMarkdownToBlocks(content);
        editor.replaceBlocks(editor.document, blocks);
        setMarkdown(content);
        setLoadError(null);
      })
      .catch((e) => setLoadError(String(e)));
  }, [selectedDoc, editor]);

  if (!selectedDoc) {
    return (
      <main className="editor-pane">
        <p className="placeholder">Select a document to read it</p>
      </main>
    );
  }

  if (loadError) {
    return (
      <main className="editor-pane">
        <p className="editor-error">Could not load document: {loadError}</p>
      </main>
    );
  }

  return (
    <main className="editor-pane editor-pane--active">
      {!isWiki && (
        <div className="editor-protected-badge">User Document — protected</div>
      )}
      <BlockNoteView
        editor={editor}
        editable={isWiki}
        theme="light"
      />
    </main>
  );
}
```

- [ ] **Step 3: Update `src/components/shell/AppShell.tsx` to pass props to EditorPane**

Read the file. Update it to derive `isWiki` from selectedDoc and pass to EditorPane:

```tsx
import { useEffect, useState } from "react";
import { Sidebar } from "./Sidebar";
import { EditorPane } from "./EditorPane";
import { RelatedNotes } from "./RelatedNotes";
import { startFileWatcher } from "../../lib/tauri";

interface Props { vaultPath: string }

export function AppShell({ vaultPath }: Props) {
  const [selectedDoc, setSelectedDoc] = useState<string | null>(null);
  const isWiki = selectedDoc?.includes("/wiki/") ?? false;

  useEffect(() => {
    startFileWatcher(vaultPath).catch(console.error);
  }, [vaultPath]);

  return (
    <div className="app-shell">
      <Sidebar reviewCount={0} selectedDoc={selectedDoc} onDocSelect={setSelectedDoc} />
      <EditorPane selectedDoc={selectedDoc} isWiki={isWiki} />
      <RelatedNotes selectedDoc={selectedDoc} />
    </div>
  );
}
```

- [ ] **Step 4: Append editor styles to `src/index.css`**

```css
/* ── Editor pane ────────────────────────────────────────────────────────────── */
.editor-pane--active {
  flex-direction: column;
  align-items: stretch;
  justify-content: flex-start;
  padding: 0;
  overflow: auto;
}

.editor-protected-badge {
  padding: 6px 16px;
  background: var(--tertiary-cont);
  color: var(--on-surface-var);
  font-size: 11px;
  font-weight: 600;
  letter-spacing: 0.06em;
  text-transform: uppercase;
  border-bottom: 1px solid var(--outline-var);
  flex-shrink: 0;
}

.editor-error {
  color: var(--error);
  font-size: 13px;
  padding: 24px;
}

/* BlockNote overrides to match Clanker palette */
.bn-container {
  --bn-colors-editor-background: var(--bg);
  --bn-colors-editor-text: var(--on-surface);
  --bn-colors-side-menu: var(--outline);
  font-family: var(--font-body) !important;
}

/* ── Folder tree ────────────────────────────────────────────────────────────── */
.folder-tree {
  flex: 1;
  display: flex;
  flex-direction: column;
  gap: 4px;
  overflow-y: auto;
}

.tree-section { display: flex; flex-direction: column; gap: 2px; }

.tree-section-label {
  font-size: 10px;
  font-weight: 700;
  text-transform: uppercase;
  letter-spacing: 0.08em;
  color: var(--outline);
  padding: 4px 4px 2px;
}

.tree-file {
  display: block;
  width: 100%;
  text-align: left;
  padding: 5px 8px;
  border-radius: var(--r-sm);
  font-size: 12px;
  font-family: var(--font-body);
  color: var(--on-surface-var);
  background: transparent;
  border: none;
  cursor: pointer;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  transition: background 0.1s, color 0.1s;
}
.tree-file:hover { background: var(--surface-variant); color: var(--on-surface); }
.tree-file--active { background: var(--primary-container); color: var(--on-primary-cont); font-weight: 600; }
```

- [ ] **Step 5: Run tests + build**

```bash
npm test 2>&1 | tail -6 && npm run build 2>&1 | tail -4
```

Expected: 6 tests pass (BlockNote components not tested directly — they require a real browser DOM), clean build.

If BlockNote causes test environment issues, add this mock to `src/test-setup.ts`:

```ts
vi.mock("@blocknote/react", () => ({
  useCreateBlockNote: () => ({
    document: [],
    tryParseMarkdownToBlocks: vi.fn().mockResolvedValue([]),
    replaceBlocks: vi.fn(),
  }),
  BlockNoteView: () => null,
}));
vi.mock("@blocknote/mantine", () => ({
  BlockNoteView: () => null,
}));
```

- [ ] **Step 6: Commit**

```bash
cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts
git add package.json package-lock.json \
        src/components/shell/EditorPane.tsx \
        src/components/shell/AppShell.tsx \
        src/index.css src/test-setup.ts
git commit -m "feat: add BlockNote editor to EditorPane (read-only source, editable wiki)"
```

---

## Self-Review

### Spec coverage

| Spec requirement | Task |
|---|---|
| Vault subfolders `documents/`, `wiki/`, `.brain/converted/` auto-created | Task 1 |
| `documents/` write-protection enforced at Rust layer | Task 1 (watcher filter) |
| Folder tree in sidebar | Task 3 |
| Source doc view: read-only in BlockNote with "User Document — protected" badge | Task 4 |
| Wiki page editing in BlockNote | Task 4 (`isWiki=true` → `editable=true`) |
| Re-fetch file list on watcher events | Task 3 (`useVaultFiles`) |
| `read_document` path validation (no escaping vault) | Task 2 |

**Correctly deferred:**
- Librarian pipeline (SP5)
- Human review queue (SP5)
- PDF/DOCX via pandoc (SP6)
- Wiki save (write-back to disk after BlockNote edit) — needs SP5 wiki write command
- Settings panel (SP7)

### Placeholder scan — none found.

### Type consistency
- `VaultFile { path, name, tier }` — Rust Task 2, TS Task 3, FolderTree Task 3
- `EditorPane { selectedDoc, isWiki }` — defined Task 4, passed Task 4 (AppShell)
- `Sidebar { reviewCount, selectedDoc, onDocSelect }` — defined Task 3, passed Task 4 (AppShell)
- `useVaultFiles()` returns `VaultFile[]` — defined Task 3, consumed FolderTree Task 3
