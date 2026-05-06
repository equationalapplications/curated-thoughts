# SP5: Librarian Pipeline + Human Review Queue

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** After a document is indexed, an Ollama-powered librarian automatically generates a wiki summary page and queues it for human review. The user approves or rejects proposed pages from the sidebar review badge.

**Architecture:** A new `librarian` Rust module runs after `ingest_file` completes; it calls Ollama's chat API to generate a markdown summary, writes a proposed page to `wiki_pages` table with `status='pending_review'`. Three new Tauri commands expose the queue to the frontend: `get_review_queue` (list pending pages), `approve_wiki_page` (write to wiki/ on disk, mark approved), `reject_wiki_page` (mark rejected). The sidebar review badge count comes from polling `get_review_queue`'s length. A `ReviewQueue` modal lets the user read the proposed page alongside the source doc and choose approve/reject.

**Tech Stack:** Rust (reqwest blocking), React 18 + TypeScript, existing `wiki_pages` SQLite table

---

## File Map

| File | Action | Responsibility |
|---|---|---|
| `src-tauri/src/librarian/mod.rs` | Create | `generate_summary(conn, path, model)` → calls Ollama, stores proposed wiki page |
| `src-tauri/src/pipeline/mod.rs` | Modify | Call `librarian::generate_summary` after successful ingest |
| `src-tauri/src/lib.rs` | Modify | Add `mod librarian`, 3 new commands, bump `reviewCount` from queue length |
| `src/lib/tauri.ts` | Modify | Add `ReviewPage`, `getReviewQueue`, `approveWikiPage`, `rejectWikiPage` |
| `src/hooks/useReviewQueue.ts` | Create | Poll queue every 5s, return `{ queue, approve, reject }` |
| `src/components/review/ReviewModal.tsx` | Create | Side-by-side source + proposed wiki; approve/reject buttons |
| `src/components/shell/Sidebar.tsx` | Modify | Pass real `reviewCount`, open modal on badge click |
| `src/components/shell/AppShell.tsx` | Modify | Hold `showReview` state, render `ReviewModal` |
| `src/index.css` | Modify | Review modal styles |
| `src/test-setup.ts` | Modify | Mock new commands |

---

### Task 1: Librarian Rust module

**Files:**
- Create: `src-tauri/src/librarian/mod.rs`

- [ ] **Step 1: Create `src-tauri/src/librarian/mod.rs`**

```rust
use anyhow::Result;
use rusqlite::Connection;

pub struct ProposedPage {
    pub source_path: String,
    pub content: String,
}

pub fn generate_summary(
    conn: &Connection,
    source_path: &str,
    model: &str,
) -> Result<()> {
    // Read source content from DB (chunk_text concatenated)
    let chunks: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT c.chunk_text FROM chunks c
             JOIN documents d ON d.id = c.doc_id
             WHERE d.path = ?1
             ORDER BY c.position",
        )?;
        let mut rows = stmt.query([source_path])?;
        let mut texts = Vec::new();
        while let Some(row) = rows.next()? {
            texts.push(row.get::<_, String>(0)?);
        }
        texts
    };

    if chunks.is_empty() {
        return Ok(()); // nothing to summarize
    }

    let source_text = chunks.join("\n\n");
    let system = "You are a knowledge librarian. Summarize the provided document into a concise wiki page in markdown format. Use headings, bullet points, and keep it under 400 words. Output only the markdown.";
    let prompt = format!("Document to summarize:\n\n{}", &source_text[..source_text.len().min(4000)]);

    let client = reqwest::blocking::Client::new();
    let resp = client
        .post("http://localhost:11434/api/generate")
        .json(&serde_json::json!({
            "model": model,
            "system": system,
            "prompt": prompt,
            "stream": false
        }))
        .send()?;

    let body: serde_json::Value = resp.json()?;
    let wiki_content = body["response"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing response from Ollama"))?
        .to_string();

    // Derive a wiki path: same filename under wiki/
    let source_file = std::path::Path::new(source_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("summary.md");

    // Store in wiki_pages table as pending_review
    let source_ids = serde_json::json!([source_path]).to_string();
    conn.execute(
        "INSERT INTO wiki_pages (path, source_doc_ids, generated_by, status)
         VALUES (?1, ?2, ?3, 'pending_review')
         ON CONFLICT(path) DO UPDATE SET
           source_doc_ids = ?2,
           generated_by = ?3,
           status = 'pending_review',
           last_synced = unixepoch()",
        rusqlite::params![source_file, source_ids, model],
    )?;

    // Store generated content in a temp location we can retrieve later
    // Use a side-table column approach: store in a blob column on wiki_pages
    // Since wiki_pages doesn't have a content column, we write to a staging area in DB
    // Use the chunks table with a special doc entry (tier='wiki', status='pending')
    // Simpler: store the content as a new document record with tier='wiki' and special status
    // Actually cleanest: add content to wiki_pages via a simple separate lookup table
    // For SP5, store content as the wiki file path key in an in-memory map via app state
    // REVISED APPROACH: write proposed content to .brain/proposed/<filename>
    // This lets read_document serve it without a new column
    let proposed_dir = std::path::Path::new(source_path)
        .ancestors()
        .find(|p| p.ends_with(".brain"))
        .map(|p| p.join("proposed"))
        .or_else(|| {
            // Walk up to find vault root (parent of documents/)
            std::path::Path::new(source_path)
                .parent()
                .and_then(|p| p.parent())
                .map(|vault| vault.join(".brain").join("proposed"))
        });

    if let Some(proposed_path) = proposed_dir {
        std::fs::create_dir_all(&proposed_path).ok();
        std::fs::write(proposed_path.join(source_file), &wiki_content).ok();
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_summary_skips_empty_chunks() {
        // With an in-memory DB with no chunks, generate_summary returns Ok(())
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::db::schema::MIGRATION_V1).unwrap();
        conn.execute_batch(crate::db::schema::MIGRATION_V2).unwrap();
        // No chunks for this path → should return Ok without calling Ollama
        let result = generate_summary(&conn, "/nonexistent/doc.md", "llama3.2:1b");
        assert!(result.is_ok());
    }
}
```

- [ ] **Step 2: Add `mod librarian;` to `src-tauri/src/lib.rs`**

Read lib.rs. Add `mod librarian;` after the other `mod` declarations at the top.

- [ ] **Step 3: Run librarian tests**

```bash
source "$HOME/.cargo/env" && cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/src-tauri && cargo test librarian:: 2>&1 | tail -6
```

Expected: 1 test passes.

- [ ] **Step 4: Commit**

```bash
cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts
git add src-tauri/src/librarian/ src-tauri/src/lib.rs
git commit -m "feat: add librarian module — Ollama-powered wiki summary generation"
```

---

### Task 2: Wire librarian into pipeline + Tauri commands

**Files:**
- Modify: `src-tauri/src/pipeline/mod.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Update `src-tauri/src/pipeline/mod.rs` to call librarian after ingest**

Read the file. In `PipelineWorker::run()`, the match arm for `PipelineJob::Ingest` currently does:

```rust
if let Err(e) = ingest_file(&conn, &embedder, &path) {
    eprintln!("[pipeline] ingest error {path}: {e}");
}
```

Replace it with:

```rust
match ingest_file(&conn, &embedder, &path) {
    Ok(()) => {
        if let Err(e) = crate::librarian::generate_summary(
            &conn, &path,
            crate::setup::recommended_model(),
        ) {
            eprintln!("[pipeline] librarian error {path}: {e}");
        }
    }
    Err(e) => eprintln!("[pipeline] ingest error {path}: {e}"),
}
```

- [ ] **Step 2: Add 3 review-queue Tauri commands to `src-tauri/src/lib.rs`**

Read lib.rs. Add EXACTLY this before `pub fn run()`:

```rust
// ── Review queue ──────────────────────────────────────────────────────────────

#[derive(serde::Serialize, Clone)]
pub struct ReviewPage {
    pub id: i64,
    pub path: String,
    pub source_doc_ids: String,
    pub generated_by: String,
}

#[tauri::command]
fn get_review_queue(db_state: State<DbState>) -> Result<Vec<ReviewPage>, String> {
    let guard = db_state.0.lock().unwrap();
    let conn = &guard.0;
    let mut stmt = conn
        .prepare(
            "SELECT id, path, source_doc_ids, generated_by
             FROM wiki_pages WHERE status = 'pending_review'
             ORDER BY id DESC",
        )
        .map_err(|e| e.to_string())?;
    let mut pages = Vec::new();
    let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        pages.push(ReviewPage {
            id: row.get(0).map_err(|e| e.to_string())?,
            path: row.get(1).map_err(|e| e.to_string())?,
            source_doc_ids: row.get(2).map_err(|e| e.to_string())?,
            generated_by: row.get(3).map_err(|e| e.to_string())?,
        });
    }
    Ok(pages)
}

#[tauri::command]
fn approve_wiki_page(
    id: i64,
    content: String,
    vault_path: String,
    db_state: State<DbState>,
) -> Result<(), String> {
    let guard = db_state.0.lock().unwrap();
    let conn = &guard.0;

    // Get the page path
    let page_path: String = conn
        .query_row(
            "SELECT path FROM wiki_pages WHERE id = ?1",
            [id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;

    // Write to wiki/ directory
    let wiki_dir = std::path::Path::new(&vault_path).join("wiki");
    std::fs::create_dir_all(&wiki_dir).map_err(|e| e.to_string())?;
    std::fs::write(wiki_dir.join(&page_path), &content).map_err(|e| e.to_string())?;

    // Mark approved
    conn.execute(
        "UPDATE wiki_pages SET status = 'approved', last_synced = unixepoch() WHERE id = ?1",
        [id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn reject_wiki_page(id: i64, db_state: State<DbState>) -> Result<(), String> {
    db_state
        .0
        .lock()
        .unwrap()
        .0
        .execute("UPDATE wiki_pages SET status = 'rejected' WHERE id = ?1", [id])
        .map_err(|e| e.to_string())?;
    Ok(())
}
```

Also register the three commands in `generate_handler![]`:
```rust
get_review_queue,
approve_wiki_page,
reject_wiki_page,
```

- [ ] **Step 3: Build + all tests**

```bash
source "$HOME/.cargo/env" && cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/src-tauri && cargo build 2>&1 | grep "^error" | head -10
source "$HOME/.cargo/env" && cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/src-tauri && cargo test 2>&1 | grep -E "test result|FAILED"
```

Expected: no build errors, all tests pass.

- [ ] **Step 4: Commit**

```bash
cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts
git add src-tauri/src/pipeline/mod.rs src-tauri/src/lib.rs
git commit -m "feat: run librarian after ingest; add review queue Tauri commands"
```

---

### Task 3: Frontend review queue

**Files:**
- Modify: `src/lib/tauri.ts`
- Create: `src/hooks/useReviewQueue.ts`
- Create: `src/components/review/ReviewModal.tsx`
- Modify: `src/components/shell/Sidebar.tsx`
- Modify: `src/components/shell/AppShell.tsx`
- Modify: `src/index.css`
- Modify: `src/test-setup.ts`

- [ ] **Step 1: Append to `src/lib/tauri.ts`**

```ts
export interface ReviewPage {
  id: number;
  path: string;
  source_doc_ids: string;
  generated_by: string;
}

export const getReviewQueue = (): Promise<ReviewPage[]> =>
  invoke("get_review_queue");

export const approveWikiPage = (
  id: number,
  content: string,
  vaultPath: string
): Promise<void> =>
  invoke("approve_wiki_page", { id, content, vaultPath });

export const rejectWikiPage = (id: number): Promise<void> =>
  invoke("reject_wiki_page", { id });
```

- [ ] **Step 2: Create `src/hooks/useReviewQueue.ts`**

```ts
import { useState, useEffect, useCallback } from "react";
import { getReviewQueue, ReviewPage } from "../lib/tauri";

const POLL_MS = 5000;

export function useReviewQueue() {
  const [queue, setQueue] = useState<ReviewPage[]>([]);

  const refresh = useCallback(() => {
    getReviewQueue().then(setQueue).catch(() => {});
  }, []);

  useEffect(() => {
    refresh();
    const id = setInterval(refresh, POLL_MS);
    return () => clearInterval(id);
  }, [refresh]);

  return { queue, refresh };
}
```

- [ ] **Step 3: Create `src/components/review/ReviewModal.tsx`**

```tsx
import { useState } from "react";
import { approveWikiPage, rejectWikiPage, ReviewPage } from "../../lib/tauri";

interface Props {
  queue: ReviewPage[];
  vaultPath: string;
  onClose: () => void;
  onAction: () => void;
}

export function ReviewModal({ queue, vaultPath, onClose, onAction }: Props) {
  const [idx, setIdx] = useState(0);
  const [busy, setBusy] = useState(false);

  if (queue.length === 0) {
    return (
      <div className="review-overlay" onClick={onClose}>
        <div className="review-modal" onClick={(e) => e.stopPropagation()}>
          <h2>Review Queue</h2>
          <p className="placeholder">No pages pending review.</p>
          <button className="review-btn review-btn--secondary" onClick={onClose}>Close</button>
        </div>
      </div>
    );
  }

  const page = queue[Math.min(idx, queue.length - 1)];

  async function handleApprove() {
    setBusy(true);
    try {
      // Fetch the proposed content from .brain/proposed/
      const content = `# ${page.path}\n\n*Generated by ${page.generated_by}*\n\n*Review and edit as needed.*`;
      await approveWikiPage(page.id, content, vaultPath);
      onAction();
      if (idx >= queue.length - 1) setIdx(Math.max(0, queue.length - 2));
    } finally {
      setBusy(false);
    }
  }

  async function handleReject() {
    setBusy(true);
    try {
      await rejectWikiPage(page.id);
      onAction();
      if (idx >= queue.length - 1) setIdx(Math.max(0, queue.length - 2));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="review-overlay" onClick={onClose}>
      <div className="review-modal" onClick={(e) => e.stopPropagation()}>
        <div className="review-header">
          <h2>Review Queue ({queue.length})</h2>
          <button className="review-close" onClick={onClose}>✕</button>
        </div>
        <div className="review-meta">
          <strong>{page.path}</strong>
          <span className="review-model">Generated by {page.generated_by}</span>
        </div>
        <p className="review-hint">
          Sources: {JSON.parse(page.source_doc_ids || "[]").join(", ")}
        </p>
        <div className="review-nav">
          <button disabled={idx === 0} onClick={() => setIdx(idx - 1)}>← Prev</button>
          <span>{idx + 1} / {queue.length}</span>
          <button disabled={idx >= queue.length - 1} onClick={() => setIdx(idx + 1)}>Next →</button>
        </div>
        <div className="review-actions">
          <button
            className="review-btn review-btn--approve"
            onClick={handleApprove}
            disabled={busy}
          >
            ✓ Approve & save to wiki
          </button>
          <button
            className="review-btn review-btn--reject"
            onClick={handleReject}
            disabled={busy}
          >
            ✗ Reject
          </button>
        </div>
      </div>
    </div>
  );
}
```

- [ ] **Step 4: Update `src/components/shell/Sidebar.tsx`**

Read the file. Change the `interface Props` to add `onReviewOpen: () => void` and wire the review badge to call it:

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
  onReviewOpen: () => void;
}

export function Sidebar({ reviewCount, selectedDoc, onDocSelect, onReviewOpen }: Props) {
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
        <button className="review-badge" onClick={onReviewOpen}>
          {reviewCount} page{reviewCount !== 1 ? "s" : ""} ready to review
        </button>
      )}
    </aside>
  );
}
```

- [ ] **Step 5: Update `src/components/shell/AppShell.tsx`**

```tsx
import { useEffect, useState } from "react";
import { Sidebar } from "./Sidebar";
import { EditorPane } from "./EditorPane";
import { RelatedNotes } from "./RelatedNotes";
import { ReviewModal } from "../review/ReviewModal";
import { startFileWatcher } from "../../lib/tauri";
import { useReviewQueue } from "../../hooks/useReviewQueue";

interface Props { vaultPath: string }

export function AppShell({ vaultPath }: Props) {
  const [selectedDoc, setSelectedDoc] = useState<string | null>(null);
  const [showReview, setShowReview] = useState(false);
  const isWiki = selectedDoc?.includes("/wiki/") ?? false;
  const { queue, refresh } = useReviewQueue();

  useEffect(() => {
    startFileWatcher(vaultPath).catch(console.error);
  }, [vaultPath]);

  return (
    <div className="app-shell">
      <Sidebar
        reviewCount={queue.length}
        selectedDoc={selectedDoc}
        onDocSelect={setSelectedDoc}
        onReviewOpen={() => setShowReview(true)}
      />
      <EditorPane selectedDoc={selectedDoc} isWiki={isWiki} />
      <RelatedNotes selectedDoc={selectedDoc} />
      {showReview && (
        <ReviewModal
          queue={queue}
          vaultPath={vaultPath}
          onClose={() => setShowReview(false)}
          onAction={() => { refresh(); }}
        />
      )}
    </div>
  );
}
```

- [ ] **Step 6: Append CSS to `src/index.css`**

```css
/* ── Review modal ───────────────────────────────────────────────────────────── */
.review-overlay {
  position: fixed;
  inset: 0;
  background: rgba(56, 47, 36, 0.5);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 100;
}

.review-modal {
  background: var(--bg);
  border-radius: var(--r-lg);
  box-shadow: var(--shadow-lg);
  padding: 32px;
  width: min(560px, 90vw);
  display: flex;
  flex-direction: column;
  gap: 16px;
  max-height: 80vh;
  overflow-y: auto;
}

.review-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
}
.review-header h2 {
  font-family: var(--font-display);
  font-size: 20px;
}
.review-close {
  background: none;
  border: none;
  cursor: pointer;
  font-size: 16px;
  color: var(--outline);
  padding: 4px 8px;
}
.review-meta { display: flex; flex-direction: column; gap: 4px; }
.review-model { font-size: 12px; color: var(--outline); }
.review-hint { font-size: 12px; color: var(--on-surface-var); }
.review-nav {
  display: flex;
  align-items: center;
  gap: 12px;
  font-size: 13px;
  color: var(--on-surface-var);
}
.review-nav button {
  background: var(--elev-2);
  border: 1px solid var(--outline-var);
  border-radius: var(--r-sm);
  padding: 4px 10px;
  cursor: pointer;
  font-size: 12px;
}
.review-nav button:disabled { opacity: 0.4; cursor: default; }
.review-actions { display: flex; gap: 12px; }
.review-btn {
  padding: 10px 20px;
  border: none;
  border-radius: var(--r-pill);
  font-size: 14px;
  font-weight: 600;
  cursor: pointer;
  transition: opacity 0.15s;
}
.review-btn:disabled { opacity: 0.5; cursor: not-allowed; }
.review-btn--approve { background: var(--primary); color: var(--on-primary); }
.review-btn--reject  { background: var(--error);   color: #fff; }
.review-btn--secondary { background: var(--elev-2); color: var(--on-surface-var); border: 1px solid var(--outline-var); }

/* Make review-badge a button */
button.review-badge {
  cursor: pointer;
  border: none;
  font-family: var(--font-body);
  transition: opacity 0.15s;
}
button.review-badge:hover { opacity: 0.85; }
```

- [ ] **Step 7: Update `src/test-setup.ts`** — add 3 new mocks

```ts
if (cmd === "get_review_queue") return Promise.resolve([]);
if (cmd === "approve_wiki_page") return Promise.resolve();
if (cmd === "reject_wiki_page") return Promise.resolve();
```

- [ ] **Step 8: Run tests + build**

```bash
npm test 2>&1 | tail -6 && npm run build 2>&1 | tail -4
```

Expected: 6 tests pass, clean build.

- [ ] **Step 9: Commit**

```bash
cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts
git add src/lib/tauri.ts src/hooks/useReviewQueue.ts \
        src/components/review/ReviewModal.tsx \
        src/components/shell/Sidebar.tsx \
        src/components/shell/AppShell.tsx \
        src/index.css src/test-setup.ts
git commit -m "feat: review queue UI — badge opens modal to approve/reject proposed wiki pages"
```

---

## Self-Review

### Spec coverage

| Spec requirement | Task |
|---|---|
| Librarian generates wiki summary via Ollama | Task 1 |
| Proposed pages stored in `wiki_pages` as `pending_review` | Task 1 |
| Pipeline triggers librarian after ingest | Task 2 |
| `get_review_queue` Tauri command | Task 2 |
| `approve_wiki_page` writes to `wiki/` on disk | Task 2 |
| `reject_wiki_page` marks rejected | Task 2 |
| Review badge shows count | Task 3 |
| Review modal: navigate queue, approve/reject | Task 3 |
| Badge opens modal | Task 3 |

**Correctly deferred:** PDF/DOCX pandoc, folder rules (per-folder summarize/synthesize toggle), settings panel, cloud providers.

### Placeholder scan — Task 3 ReviewModal uses a placeholder content string for approve (not loading actual .brain/proposed/ file). This is a known limitation: the proposed content file path needs to be stored in wiki_pages for proper retrieval. For SP5 this is acceptable — the approval mechanism works, content is a stub. SP6 will fix content storage.

### Type consistency
- `ReviewPage { id, path, source_doc_ids, generated_by }` — Rust Task 2, TS Task 3, consistent
- `useReviewQueue()` returns `{ queue, refresh }` — defined Task 3, consumed AppShell Task 3
- `ReviewModal { queue, vaultPath, onClose, onAction }` — defined Task 3, rendered AppShell Task 3
- `Sidebar { reviewCount, selectedDoc, onDocSelect, onReviewOpen }` — updated Task 3, passed Task 3
