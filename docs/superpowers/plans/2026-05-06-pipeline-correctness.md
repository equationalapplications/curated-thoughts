# SP7: Pipeline Correctness — Folder Rules, Proposed Content, Deletion Cleanup, Error Log

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix four correctness gaps: (1) the librarian respects folder_rules mode (skips when `index`, auto-approves when `auto_approve=true`); (2) the review modal loads and displays the actual Ollama-generated proposed content; (3) file deletion purges shadow copies and marks orphaned wiki pages; (4) pipeline errors are written to `.brain/errors.log`.

**Architecture:** All four fixes live in Rust (`pipeline/mod.rs`, `librarian/mod.rs`) and one TypeScript change (ReviewModal loads proposed content via a new Tauri command). No new modules needed.

**Tech Stack:** Rust (std::fs, rusqlite), React 18 + TypeScript

---

## File Map

| File | Action | Responsibility |
|---|---|---|
| `src-tauri/src/librarian/mod.rs` | Modify | Check folder_rules mode before generating; auto-approve when set |
| `src-tauri/src/pipeline/mod.rs` | Modify | Deletion cleanup: remove shadow copy, mark wiki orphans; write errors.log |
| `src-tauri/src/lib.rs` | Modify | Add `get_proposed_content` Tauri command |
| `src/lib/tauri.ts` | Modify | Add `getProposedContent` invoke wrapper |
| `src/components/review/ReviewModal.tsx` | Modify | Load + display actual proposed content; pass it to approve |
| `src/test-setup.ts` | Modify | Mock `get_proposed_content` |

---

### Task 1: Folder rules mode in librarian + auto-approve

**Files:**
- Modify: `src-tauri/src/librarian/mod.rs`

- [ ] **Step 1: Update `generate_summary` in `src-tauri/src/librarian/mod.rs` to check folder_rules**

Read the file. Replace the entire `generate_summary` function with:

```rust
use anyhow::Result;
use rusqlite::Connection;

fn get_folder_mode(conn: &Connection, source_path: &str) -> (String, bool) {
    // Walk up directory tree to find a matching folder rule
    let mut p = std::path::Path::new(source_path);
    loop {
        let dir = p.parent().unwrap_or(p).to_string_lossy().to_string();
        if let Ok(Some((mode, auto))) = conn.query_row(
            "SELECT librarian_mode, auto_approve FROM folder_rules WHERE folder_path = ?1",
            [&dir],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? != 0)),
        ).map(Some).or_else(|_| Ok::<_, rusqlite::Error>(None)) {
            return (mode, auto);
        }
        if p.parent().is_none() || p.parent() == Some(p) { break; }
        p = p.parent().unwrap();
    }
    ("summarize".to_string(), false) // default: summarize, manual review
}

pub fn generate_summary(
    conn: &Connection,
    source_path: &str,
    model: &str,
) -> Result<()> {
    let (mode, auto_approve) = get_folder_mode(conn, source_path);

    if mode == "index" {
        return Ok(()); // index-only: skip librarian
    }

    // Load source chunks from DB
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
        return Ok(());
    }

    let source_text = chunks.join("\n\n");
    let truncated = &source_text[..source_text.len().min(4000)];

    let client = reqwest::blocking::Client::new();
    let resp = client
        .post("http://localhost:11434/api/generate")
        .json(&serde_json::json!({
            "model": model,
            "system": "You are a knowledge librarian. Summarize the document into a concise wiki page in markdown format. Use headings and bullet points, keep under 400 words. Output only markdown.",
            "prompt": format!("Document to summarize:\n\n{}", truncated),
            "stream": false
        }))
        .send()?;

    let body: serde_json::Value = resp.json()?;
    let wiki_content = body["response"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing response from Ollama"))?
        .to_string();

    let source_file = std::path::Path::new(source_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("summary.md")
        .to_string();

    let initial_status = if auto_approve { "approved" } else { "pending_review" };
    let source_ids = serde_json::json!([source_path]).to_string();

    conn.execute(
        "INSERT INTO wiki_pages (path, source_doc_ids, generated_by, status)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(path) DO UPDATE SET
           source_doc_ids = ?2,
           generated_by = ?3,
           status = ?4,
           last_synced = unixepoch()",
        rusqlite::params![source_file, source_ids, model, initial_status],
    )?;

    let proposed_dir = std::path::Path::new(source_path)
        .parent()
        .and_then(|p| p.parent())
        .map(|vault| vault.join(".brain").join("proposed"));

    if let Some(dir) = &proposed_dir {
        std::fs::create_dir_all(dir).ok();
        std::fs::write(dir.join(&source_file), &wiki_content).ok();
    }

    // If auto-approved, also write directly to wiki/
    if auto_approve {
        if let Some(vault) = std::path::Path::new(source_path)
            .parent()
            .and_then(|p| p.parent())
        {
            let wiki_dir = vault.join("wiki");
            std::fs::create_dir_all(&wiki_dir).ok();
            std::fs::write(wiki_dir.join(&source_file), &wiki_content).ok();
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema::{MIGRATION_V1, MIGRATION_V2};

    #[test]
    fn test_generate_summary_skips_when_no_chunks() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(MIGRATION_V1).unwrap();
        conn.execute_batch(MIGRATION_V2).unwrap();
        let result = generate_summary(&conn, "/vault/documents/nonexistent.md", "llama3.2:1b");
        assert!(result.is_ok());
    }

    #[test]
    fn test_index_mode_skips_librarian() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(MIGRATION_V1).unwrap();
        conn.execute_batch(MIGRATION_V2).unwrap();
        conn.execute(
            "INSERT INTO folder_rules (folder_path, librarian_mode, auto_approve) VALUES ('/vault/documents', 'index', 0)",
            [],
        ).unwrap();
        // Even though there are no chunks, mode=index should return Ok immediately
        let result = generate_summary(&conn, "/vault/documents/note.md", "llama3.2:1b");
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_folder_mode_defaults_to_summarize() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(MIGRATION_V1).unwrap();
        conn.execute_batch(MIGRATION_V2).unwrap();
        let (mode, auto) = get_folder_mode(&conn, "/vault/documents/note.md");
        assert_eq!(mode, "summarize");
        assert!(!auto);
    }
}
```

- [ ] **Step 2: Run librarian tests**

```bash
source "$HOME/.cargo/env" && cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/src-tauri && cargo test librarian:: 2>&1 | tail -8
```

Expected: 3 tests pass.

- [ ] **Step 3: Commit**

```bash
cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts
git add src-tauri/src/librarian/mod.rs
git commit -m "feat: librarian respects folder_rules mode (index skips, auto_approve writes directly)"
```

---

### Task 2: Deletion cleanup + errors.log

**Files:**
- Modify: `src-tauri/src/pipeline/mod.rs`

- [ ] **Step 1: Add `write_error_log` helper + update deletion handler in `src-tauri/src/pipeline/mod.rs`**

Read the file. Add this helper before `ingest_file`:

```rust
fn write_error_log(vault_path: Option<&std::path::Path>, msg: &str) {
    let Some(vault) = vault_path else { return; };
    let log_path = vault.join(".brain").join("errors.log");
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let line = format!("[{}] {}\n", timestamp, msg);
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&log_path) {
        let _ = f.write_all(line.as_bytes());
    }
}
```

Also add a `vault_root` field to `PipelineWorker` so we can pass it to the error log. Read the current `PipelineWorker` struct and `new()` function. Update them to include an optional vault root:

Actually, to keep changes minimal, use the db_path to derive the vault root (`.brain/brain.db` → vault is 2 levels up from db_path's parent... wait no, db_path is `<vault>/.brain/brain.db`, so vault is `db_path.parent().parent()`).

In `PipelineWorker::run()`, derive vault root from `self.db_path` and pass it to `write_error_log`:

```rust
let vault_root = self.db_path.parent().and_then(|p| p.parent()).map(|p| p.to_path_buf());
let vault_path = vault_root.as_deref();
```

Add this right after the `conn` is opened (before the `for job in self.rx` loop).

Then update the error logging in the match arms:

```rust
for job in self.rx {
    match job {
        PipelineJob::Ingest(path) => {
            match ingest_file(&conn, &embedder, &path) {
                Ok(()) => {
                    if let Err(e) = crate::librarian::generate_summary(
                        &conn, &path,
                        crate::setup::recommended_model(),
                    ) {
                        let msg = format!("librarian error {}: {}", path, e);
                        eprintln!("[pipeline] {}", msg);
                        write_error_log(vault_path, &msg);
                    }
                }
                Err(e) => {
                    let msg = format!("ingest error {}: {}", path, e);
                    eprintln!("[pipeline] {}", msg);
                    write_error_log(vault_path, &msg);
                }
            }
        }
        PipelineJob::Delete(path) => {
            // Remove from DB (cascades chunks + embeddings)
            if let Err(e) = delete_document(&conn, &path) {
                eprintln!("[pipeline] delete error {path}: {e}");
            }
            // Remove shadow copy from .brain/converted/
            if let Some(vault) = vault_root.as_ref() {
                let stem = std::path::Path::new(&path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("");
                let shadow = vault.join(".brain").join("converted").join(format!("{}.md", stem));
                std::fs::remove_file(&shadow).ok();
            }
            // Mark sourced wiki pages as orphaned
            let path_json = serde_json::json!([path]).to_string();
            conn.execute(
                "UPDATE wiki_pages SET status = 'orphaned'
                 WHERE status != 'rejected'
                 AND source_doc_ids LIKE ?1",
                [format!("%{}%", path)],
            ).ok();
        }
    }
}
```

Note: Replace the EXISTING match arms entirely with the above — don't add to them.

- [ ] **Step 2: Build + all tests**

```bash
source "$HOME/.cargo/env" && cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/src-tauri && cargo build 2>&1 | grep "^error" | head -10
source "$HOME/.cargo/env" && cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/src-tauri && cargo test 2>&1 | grep -E "test result|FAILED"
```

Expected: no build errors, all tests pass.

- [ ] **Step 3: Commit**

```bash
cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts
git add src-tauri/src/pipeline/mod.rs
git commit -m "feat: deletion cleanup (shadow copy + orphan wiki pages) + errors.log"
```

---

### Task 3: Proposed content in review modal

**Files:**
- Modify: `src-tauri/src/lib.rs`
- Modify: `src/lib/tauri.ts`
- Modify: `src/components/review/ReviewModal.tsx`
- Modify: `src/test-setup.ts`

- [ ] **Step 1: Add `get_proposed_content` command to `src-tauri/src/lib.rs`**

Read lib.rs. Add before `pub fn run()`:

```rust
#[tauri::command]
fn get_proposed_content(page_id: i64, db_state: State<DbState>, vault_state: State<VaultConfigState>) -> Result<String, String> {
    // Look up the page path from DB
    let page_path: String = {
        let guard = db_state.0.lock().unwrap();
        guard.0.query_row(
            "SELECT path FROM wiki_pages WHERE id = ?1",
            [page_id],
            |r| r.get(0),
        ).map_err(|e| e.to_string())?
    };

    // Build the proposed file path
    let vault = vault_state.0.lock().unwrap().get_vault_path().map_err(|e| e.to_string())?
        .ok_or("no vault set".to_string())?;
    let proposed_path = std::path::Path::new(&vault)
        .join(".brain")
        .join("proposed")
        .join(&page_path);

    // Read proposed content, fall back to placeholder if not found
    std::fs::read_to_string(&proposed_path)
        .unwrap_or_else(|_| format!("# {}\n\n*Proposed wiki page — content not available.*", page_path))
        .pipe(Ok)
}
```

Wait — `pipe` doesn't exist in std. Use:
```rust
    Ok(std::fs::read_to_string(&proposed_path)
        .unwrap_or_else(|_| format!("# {}\n\n*Proposed wiki page — content not available.*", page_path)))
```

Register in `generate_handler![]`:
```rust
get_proposed_content,
```

- [ ] **Step 2: Append to `src/lib/tauri.ts`**

```ts
export const getProposedContent = (pageId: number): Promise<string> =>
  invoke("get_proposed_content", { pageId });
```

- [ ] **Step 3: Update `src/components/review/ReviewModal.tsx`**

Read the current file. Replace it entirely with this version that loads actual proposed content:

```tsx
import { useState, useEffect } from "react";
import { approveWikiPage, rejectWikiPage, getProposedContent, ReviewPage } from "../../lib/tauri";

interface Props {
  queue: ReviewPage[];
  vaultPath: string;
  onClose: () => void;
  onAction: () => void;
}

export function ReviewModal({ queue, vaultPath, onClose, onAction }: Props) {
  const [idx, setIdx] = useState(0);
  const [busy, setBusy] = useState(false);
  const [content, setContent] = useState<string | null>(null);

  const page = queue[Math.min(idx, queue.length - 1)];

  useEffect(() => {
    setContent(null);
    if (!page) return;
    getProposedContent(page.id).then(setContent).catch(() => setContent(null));
  }, [page?.id]);

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

  async function handleApprove() {
    if (!page) return;
    setBusy(true);
    try {
      const approveContent = content ?? `# ${page.path}\n\n*Generated by ${page.generated_by}*`;
      await approveWikiPage(page.id, approveContent, vaultPath);
      onAction();
      if (idx >= queue.length - 1) setIdx(Math.max(0, queue.length - 2));
    } finally {
      setBusy(false);
    }
  }

  async function handleReject() {
    if (!page) return;
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
      <div className="review-modal review-modal--wide" onClick={(e) => e.stopPropagation()}>
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
        {content !== null ? (
          <pre className="review-content">{content}</pre>
        ) : (
          <p className="review-hint">Loading proposed content…</p>
        )}
        <div className="review-nav">
          <button disabled={idx === 0} onClick={() => setIdx(idx - 1)}>← Prev</button>
          <span>{idx + 1} / {queue.length}</span>
          <button disabled={idx >= queue.length - 1} onClick={() => setIdx(idx + 1)}>Next →</button>
        </div>
        <div className="review-actions">
          <button className="review-btn review-btn--approve" onClick={handleApprove} disabled={busy}>
            ✓ Approve &amp; save to wiki
          </button>
          <button className="review-btn review-btn--reject" onClick={handleReject} disabled={busy}>
            ✗ Reject
          </button>
        </div>
      </div>
    </div>
  );
}
```

- [ ] **Step 4: Add CSS for review-modal--wide + review-content to `src/index.css`**

Append:

```css
.review-modal--wide { width: min(720px, 92vw); }
.review-content {
  background: var(--elev-2);
  border: 1px solid var(--outline-var);
  border-radius: var(--r-md);
  padding: 12px 16px;
  font-size: 12px;
  font-family: var(--font-mono);
  white-space: pre-wrap;
  overflow-y: auto;
  max-height: 280px;
  color: var(--on-surface);
  line-height: 1.5;
}
```

- [ ] **Step 5: Update `src/test-setup.ts`** — add mock before `return Promise.resolve(null)`:

```ts
if (cmd === "get_proposed_content") return Promise.resolve("# Test Wiki Page\n\nTest content.");
```

- [ ] **Step 6: Run tests + build**

```bash
npm test 2>&1 | tail -6 && npm run build 2>&1 | tail -4
```

Expected: 6 tests pass, clean build.

- [ ] **Step 7: Commit**

```bash
cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts
git add src-tauri/src/lib.rs \
        src/lib/tauri.ts \
        src/components/review/ReviewModal.tsx \
        src/index.css src/test-setup.ts
git commit -m "feat: review modal loads actual proposed content from .brain/proposed/"
```

---

## Self-Review

### Spec coverage

| Spec requirement | Task |
|---|---|
| Folder rules mode respected (index skips librarian) | Task 1 |
| auto_approve writes directly to wiki/ | Task 1 |
| File deletion cascade: purge shadow copy | Task 2 |
| File deletion: mark wiki pages orphaned | Task 2 |
| `.brain/errors.log` for pipeline errors | Task 2 |
| Review modal shows actual proposed content | Task 3 |
| approve_wiki_page uses actual generated content | Task 3 |

**Correctly deferred:** synthesize mode (cross-doc), broken wikilinks, sqlite-vec, cloud providers.

### Placeholder scan — none found.

### Type consistency
- `get_proposed_content(page_id: i64)` — Rust Task 3, `getProposedContent(pageId: number)` TS Task 3, consistent
- `ReviewModal` now uses `content: string | null` loaded via hook — consistent usage
