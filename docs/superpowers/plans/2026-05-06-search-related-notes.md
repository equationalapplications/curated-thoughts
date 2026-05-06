# Search + Related Notes — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire the search bar and Related Notes panel using cosine similarity over stored FastEmbed vectors — no new dependencies required.

**Architecture:** A new `search` Rust module exposes two pure functions: `semantic_search` (embed query → cosine over all chunk vectors → top N) and `related_chunks` (average doc vectors → cosine over other docs → top N). Two Tauri commands expose these to React. The Sidebar handles debounced search input; AppShell holds `selectedDoc` state that flows to RelatedNotes. Vectors already stored as little-endian f32 BLOBs in the `embeddings` table.

**Tech Stack:** Rust, rusqlite, fastembed (already in Cargo.toml), React 18, TypeScript

---

## File Map

| File | Action | Responsibility |
|---|---|---|
| `src-tauri/src/search/mod.rs` | Create | `cosine_similarity`, `bytes_to_f32`, `semantic_search`, `related_chunks`, unit tests |
| `src-tauri/src/lib.rs` | Modify | Add `mod search`, `search_vault` and `get_related_chunks` Tauri commands |
| `src/lib/tauri.ts` | Modify | Add `SearchResult` interface, `searchVault`, `getRelatedChunks` wrappers |
| `src/hooks/useSearch.ts` | Create | Debounced search hook (300 ms) |
| `src/hooks/useRelatedChunks.ts` | Create | Fetch related chunks when selectedDoc changes |
| `src/components/shell/SearchResults.tsx` | Create | List of search hit cards |
| `src/components/shell/Sidebar.tsx` | Modify | Wire search input → `useSearch`, render `SearchResults` |
| `src/components/shell/RelatedNotes.tsx` | Modify | Accept `selectedDoc` prop, call `useRelatedChunks` |
| `src/components/shell/AppShell.tsx` | Modify | Hold `selectedDoc` state, pass down |
| `src/index.css` | Modify | Styles for search results + related chunks |
| `src/test-setup.ts` | Modify | Mock `search_vault` and `get_related_chunks` |

---

### Task 1: Rust search module

**Files:**
- Create: `src-tauri/src/search/mod.rs`

- [ ] **Step 1: Create `src-tauri/src/search/mod.rs` with helpers + search functions + tests**

```rust
use anyhow::Result;
use rusqlite::Connection;
use serde::Serialize;

#[derive(Serialize, Clone, Debug)]
pub struct SearchResult {
    pub doc_path: String,
    pub chunk_text: String,
    pub chunk_position: i64,
    pub score: f32,
}

pub fn bytes_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
}

pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    (dot / (norm_a * norm_b)).clamp(-1.0, 1.0)
}

/// Load all indexed chunk vectors; compute cosine against query_vec; return top `limit`.
pub fn semantic_search(
    conn: &Connection,
    query_vec: &[f32],
    limit: usize,
) -> Result<Vec<SearchResult>> {
    let mut stmt = conn.prepare(
        "SELECT e.vector, c.chunk_text, c.position, d.path
         FROM embeddings e
         JOIN chunks c ON c.id = e.chunk_id
         JOIN documents d ON d.id = c.doc_id
         WHERE d.status = 'indexed'",
    )?;

    let mut results: Vec<(f32, SearchResult)> = Vec::new();
    let mut rows = stmt.query([])?;

    while let Some(row) = rows.next()? {
        let bytes: Vec<u8> = row.get(0)?;
        let chunk_text: String = row.get(1)?;
        let chunk_position: i64 = row.get(2)?;
        let doc_path: String = row.get(3)?;
        let vec = bytes_to_f32(&bytes);
        let score = cosine_similarity(query_vec, &vec);
        results.push((score, SearchResult { doc_path, chunk_text, chunk_position, score }));
    }

    results.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    Ok(results.into_iter().take(limit).map(|(_, r)| r).collect())
}

/// Average the vectors for `doc_path`; find top `limit` similar chunks from OTHER docs.
pub fn related_chunks(
    conn: &Connection,
    doc_path: &str,
    limit: usize,
) -> Result<Vec<SearchResult>> {
    // Collect vectors for the source document
    let doc_vecs: Vec<Vec<f32>> = {
        let mut stmt = conn.prepare(
            "SELECT e.vector FROM embeddings e
             JOIN chunks c ON c.id = e.chunk_id
             JOIN documents d ON d.id = c.doc_id
             WHERE d.path = ?1 AND d.status = 'indexed'",
        )?;
        let mut rows = stmt.query([doc_path])?;
        let mut vecs = Vec::new();
        while let Some(row) = rows.next()? {
            let bytes: Vec<u8> = row.get(0)?;
            vecs.push(bytes_to_f32(&bytes));
        }
        vecs
    };

    if doc_vecs.is_empty() {
        return Ok(vec![]);
    }

    // Average to get a single document embedding
    let dim = doc_vecs[0].len();
    let mut avg = vec![0.0_f32; dim];
    for v in &doc_vecs {
        for (a, b) in avg.iter_mut().zip(v) {
            *a += b;
        }
    }
    let n = doc_vecs.len() as f32;
    avg.iter_mut().for_each(|x| *x /= n);

    // Score all chunks from other documents
    let mut stmt = conn.prepare(
        "SELECT e.vector, c.chunk_text, c.position, d.path
         FROM embeddings e
         JOIN chunks c ON c.id = e.chunk_id
         JOIN documents d ON d.id = c.doc_id
         WHERE d.path != ?1 AND d.status = 'indexed'",
    )?;

    let mut results: Vec<(f32, SearchResult)> = Vec::new();
    let mut rows = stmt.query([doc_path])?;

    while let Some(row) = rows.next()? {
        let bytes: Vec<u8> = row.get(0)?;
        let chunk_text: String = row.get(1)?;
        let chunk_position: i64 = row.get(2)?;
        let doc_path_r: String = row.get(3)?;
        let vec = bytes_to_f32(&bytes);
        let score = cosine_similarity(&avg, &vec);
        results.push((score, SearchResult {
            doc_path: doc_path_r,
            chunk_text,
            chunk_position,
            score,
        }));
    }

    results.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    Ok(results.into_iter().take(limit).map(|(_, r)| r).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_identical_vectors() {
        let v = vec![1.0_f32, 2.0, 3.0];
        assert!((cosine_similarity(&v, &v) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_orthogonal_vectors() {
        let a = vec![1.0_f32, 0.0];
        let b = vec![0.0_f32, 1.0];
        assert!(cosine_similarity(&a, &b).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_empty() {
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
    }

    #[test]
    fn test_cosine_dimension_mismatch() {
        assert_eq!(cosine_similarity(&[1.0], &[1.0, 2.0]), 0.0);
    }

    #[test]
    fn test_bytes_to_f32_roundtrip() {
        let original = vec![1.5_f32, -2.0, 0.0, 100.0];
        let bytes: Vec<u8> = original.iter().flat_map(|f| f.to_le_bytes()).collect();
        assert_eq!(bytes_to_f32(&bytes), original);
    }

    #[test]
    fn test_bytes_to_f32_empty() {
        assert!(bytes_to_f32(&[]).is_empty());
    }

    #[test]
    fn test_bytes_to_f32_ignores_trailing_incomplete_chunk() {
        // 5 bytes → only 1 complete f32 (4 bytes), last byte ignored
        let bytes = vec![0x00, 0x00, 0x80, 0x3f, 0xff];
        let result = bytes_to_f32(&bytes);
        assert_eq!(result.len(), 1);
        assert!((result[0] - 1.0_f32).abs() < 1e-6);
    }
}
```

- [ ] **Step 2: Add `mod search;` to the top of `src-tauri/src/lib.rs`**

Read lib.rs, then add `mod search;` after the other `mod` declarations at the top.

- [ ] **Step 3: Run tests**

```bash
source "$HOME/.cargo/env" && cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/src-tauri && cargo test search:: 2>&1 | tail -10
```

Expected: 7 tests pass.

- [ ] **Step 4: Commit**

```bash
cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts
git add src-tauri/src/search/ src-tauri/src/lib.rs
git commit -m "feat: add semantic search and related-chunks Rust module"
```

---

### Task 2: Wire search Tauri commands into lib.rs

**Files:**
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Add two Tauri commands to `src-tauri/src/lib.rs`**

Read lib.rs first. Add these two commands after the `get_related_chunks` / Ollama section (before `pub fn run()`):

```rust
// ── Search commands ───────────────────────────────────────────────────────────

#[tauri::command]
fn search_vault(
    query: String,
    limit: usize,
    db_state: State<DbState>,
    embedder_state: State<WikiEmbedder>,
) -> Result<Vec<search::SearchResult>, String> {
    let query_vec = {
        let mut guard = embedder_state.0.lock().unwrap();
        if guard.is_none() {
            *guard = Some(Embedder::new().map_err(|e| e.to_string())?);
        }
        guard
            .as_ref()
            .unwrap()
            .embed(vec![query])
            .map_err(|e| e.to_string())?
            .into_iter()
            .next()
            .unwrap_or_default()
    };
    let guard = db_state.0.lock().unwrap();
    search::semantic_search(&guard.0, &query_vec, limit.clamp(1, 50))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_related_chunks(
    doc_path: String,
    limit: usize,
    db_state: State<DbState>,
) -> Result<Vec<search::SearchResult>, String> {
    let guard = db_state.0.lock().unwrap();
    search::related_chunks(&guard.0, &doc_path, limit.clamp(1, 10))
        .map_err(|e| e.to_string())
}
```

Also add both to the `invoke_handler!` macro in `run()`:

```rust
search_vault,
get_related_chunks,
```

- [ ] **Step 2: Verify build**

```bash
source "$HOME/.cargo/env" && cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/src-tauri && cargo build 2>&1 | grep -E "^error" | head -10
```

Expected: no errors.

- [ ] **Step 3: Run all Rust tests to confirm nothing regressed**

```bash
source "$HOME/.cargo/env" && cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts/src-tauri && cargo test 2>&1 | grep -E "test result|FAILED"
```

Expected: all pass.

- [ ] **Step 4: Commit**

```bash
cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts
git add src-tauri/src/lib.rs
git commit -m "feat: expose search_vault and get_related_chunks as Tauri commands"
```

---

### Task 3: Frontend types + invoke wrappers + hooks

**Files:**
- Modify: `src/lib/tauri.ts`
- Create: `src/hooks/useSearch.ts`
- Create: `src/hooks/useRelatedChunks.ts`
- Modify: `src/test-setup.ts`

- [ ] **Step 1: Append to `src/lib/tauri.ts`**

Read the file, then append at the end:

```ts
export interface SearchResult {
  doc_path: string;
  chunk_text: string;
  chunk_position: number;
  score: number;
}

export const searchVault = (query: string, limit = 10): Promise<SearchResult[]> =>
  invoke("search_vault", { query, limit });

export const getRelatedChunks = (docPath: string, limit = 5): Promise<SearchResult[]> =>
  invoke("get_related_chunks", { docPath, limit });
```

- [ ] **Step 2: Create `src/hooks/useSearch.ts`**

```ts
import { useState, useEffect, useRef } from "react";
import { searchVault, SearchResult } from "../lib/tauri";

const DEBOUNCE_MS = 300;

export function useSearch() {
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<SearchResult[]>([]);
  const [searching, setSearching] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    if (timer.current) clearTimeout(timer.current);
    if (!query.trim()) {
      setResults([]);
      return;
    }
    timer.current = setTimeout(async () => {
      setSearching(true);
      try {
        setResults(await searchVault(query));
      } catch {
        setResults([]);
      } finally {
        setSearching(false);
      }
    }, DEBOUNCE_MS);
    return () => {
      if (timer.current) clearTimeout(timer.current);
    };
  }, [query]);

  return { query, setQuery, results, searching };
}
```

- [ ] **Step 3: Create `src/hooks/useRelatedChunks.ts`**

```ts
import { useState, useEffect } from "react";
import { getRelatedChunks, SearchResult } from "../lib/tauri";

export function useRelatedChunks(docPath: string | null): SearchResult[] {
  const [chunks, setChunks] = useState<SearchResult[]>([]);

  useEffect(() => {
    if (!docPath) {
      setChunks([]);
      return;
    }
    getRelatedChunks(docPath).then(setChunks).catch(() => setChunks([]));
  }, [docPath]);

  return chunks;
}
```

- [ ] **Step 4: Update `src/test-setup.ts` — add mocks for search commands**

Read the file. In the `invoke` mock, add two new cases before `return Promise.resolve(null)`:

```ts
if (cmd === "search_vault") return Promise.resolve([]);
if (cmd === "get_related_chunks") return Promise.resolve([]);
```

- [ ] **Step 5: Run tests**

```bash
npm test 2>&1 | tail -6
```

Expected: 6 tests pass (no new test file — hooks tested via integration in later tasks).

- [ ] **Step 6: Commit**

```bash
cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts
git add src/lib/tauri.ts src/hooks/useSearch.ts src/hooks/useRelatedChunks.ts src/test-setup.ts
git commit -m "feat: add search/related-chunks invoke wrappers and debounced hooks"
```

---

### Task 4: SearchResults component + Sidebar update

**Files:**
- Create: `src/components/shell/SearchResults.tsx`
- Modify: `src/components/shell/Sidebar.tsx`

- [ ] **Step 1: Create `src/components/shell/SearchResults.tsx`**

```tsx
import type { SearchResult } from "../../lib/tauri";

interface Props {
  results: SearchResult[];
  onSelect: (path: string) => void;
}

export function SearchResults({ results, onSelect }: Props) {
  if (results.length === 0) return null;
  return (
    <div className="search-results">
      {results.map((r, i) => (
        <button
          key={i}
          className="search-result"
          onClick={() => onSelect(r.doc_path)}
        >
          <span className="search-result-path">
            {r.doc_path.split("/").at(-1)}
          </span>
          <span className="search-result-snippet">
            {r.chunk_text.slice(0, 120)}…
          </span>
        </button>
      ))}
    </div>
  );
}
```

- [ ] **Step 2: Replace `src/components/shell/Sidebar.tsx`**

```tsx
import { IndexingStatus } from "./IndexingStatus";
import { SearchResults } from "./SearchResults";
import { useSearch } from "../../hooks/useSearch";

interface Props {
  reviewCount: number;
  onDocSelect: (path: string) => void;
}

export function Sidebar({ reviewCount, onDocSelect }: Props) {
  const { query, setQuery, results, searching } = useSearch();

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
          <div className="folder-tree">
            <p className="placeholder">Documents will appear here</p>
          </div>
        </>
      )}
      {reviewCount > 0 && (
        <div className="review-badge">{reviewCount} pages ready to review</div>
      )}
    </aside>
  );
}
```

- [ ] **Step 3: Update test for SetupWizard to keep passing**

The SetupWizard test renders the wizard which uses `useSetupStatus`. It doesn't render Sidebar. Run tests to confirm:

```bash
npm test 2>&1 | tail -6
```

Expected: 6 tests pass.

- [ ] **Step 4: Commit**

```bash
cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts
git add src/components/shell/SearchResults.tsx src/components/shell/Sidebar.tsx
git commit -m "feat: add SearchResults component and wire search into Sidebar"
```

---

### Task 5: RelatedNotes + AppShell + CSS

**Files:**
- Modify: `src/components/shell/RelatedNotes.tsx`
- Modify: `src/components/shell/AppShell.tsx`
- Modify: `src/index.css`

- [ ] **Step 1: Replace `src/components/shell/RelatedNotes.tsx`**

```tsx
import { useRelatedChunks } from "../../hooks/useRelatedChunks";

interface Props {
  selectedDoc: string | null;
}

export function RelatedNotes({ selectedDoc }: Props) {
  const chunks = useRelatedChunks(selectedDoc);

  return (
    <aside className="related-notes">
      <h3>Related Notes</h3>
      {chunks.length === 0 ? (
        <p className="placeholder">
          {selectedDoc ? "No related notes found" : "Select a document to see related notes"}
        </p>
      ) : (
        <div className="related-chunks">
          {chunks.map((chunk, i) => (
            <div key={i} className="related-chunk">
              <span className="related-chunk-path">
                {chunk.doc_path.split("/").at(-1)}
              </span>
              <p className="related-chunk-text">
                {chunk.chunk_text.slice(0, 200)}
                {chunk.chunk_text.length > 200 ? "…" : ""}
              </p>
              <span className="related-chunk-score">
                {Math.round(chunk.score * 100)}% similar
              </span>
            </div>
          ))}
        </div>
      )}
    </aside>
  );
}
```

- [ ] **Step 2: Replace `src/components/shell/AppShell.tsx`**

```tsx
import { useEffect, useState } from "react";
import { Sidebar } from "./Sidebar";
import { EditorPane } from "./EditorPane";
import { RelatedNotes } from "./RelatedNotes";
import { startFileWatcher } from "../../lib/tauri";

interface Props { vaultPath: string }

export function AppShell({ vaultPath }: Props) {
  const [selectedDoc, setSelectedDoc] = useState<string | null>(null);

  useEffect(() => {
    startFileWatcher(vaultPath).catch(console.error);
  }, [vaultPath]);

  return (
    <div className="app-shell">
      <Sidebar reviewCount={0} onDocSelect={setSelectedDoc} />
      <EditorPane />
      <RelatedNotes selectedDoc={selectedDoc} />
    </div>
  );
}
```

- [ ] **Step 3: Append to `src/index.css`**

```css
/* ── Search results ─────────────────────────────────────────────────────────── */
.search-results {
  display: flex;
  flex-direction: column;
  gap: 4px;
  overflow-y: auto;
  flex: 1;
}

.search-result {
  display: flex;
  flex-direction: column;
  gap: 3px;
  padding: 8px 10px;
  background: var(--elev-2);
  border: 1px solid var(--outline-var);
  border-radius: var(--r-md);
  cursor: pointer;
  text-align: left;
  transition: background 0.1s, border-color 0.1s;
  width: 100%;
}
.search-result:hover {
  background: var(--primary-container);
  border-color: transparent;
}
.search-result-path {
  font-size: 12px;
  font-weight: 600;
  color: var(--primary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.search-result-snippet {
  font-size: 11px;
  color: var(--on-surface-var);
  line-height: 1.4;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
}

.search-spinner {
  font-size: 13px;
  color: var(--outline);
  margin-left: 6px;
  animation: spin 0.8s linear infinite;
  display: inline-block;
}
@keyframes spin { to { transform: rotate(360deg); } }

/* ── Related chunks ─────────────────────────────────────────────────────────── */
.related-chunks {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.related-chunk {
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 10px;
  background: var(--elev-2);
  border: 1px solid var(--outline-var);
  border-radius: var(--r-md);
}
.related-chunk-path {
  font-size: 11px;
  font-weight: 600;
  color: var(--primary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.related-chunk-text {
  font-size: 12px;
  color: var(--on-surface-var);
  line-height: 1.5;
}
.related-chunk-score {
  font-size: 10px;
  color: var(--outline);
  letter-spacing: 0.03em;
}
```

- [ ] **Step 4: Run all tests + build**

```bash
npm test 2>&1 | tail -6 && npm run build 2>&1 | tail -4
```

Expected: 6 tests pass, clean build.

- [ ] **Step 5: Commit**

```bash
cd /Users/equationalapplications/code/src/github.com/equationalapplications/curated-thoughts
git add src/components/shell/RelatedNotes.tsx src/components/shell/AppShell.tsx src/index.css
git commit -m "feat: wire RelatedNotes with cosine-similar chunks and selectedDoc state"
```

---

## Self-Review

### Spec coverage

| Spec requirement | Task |
|---|---|
| Search: hybrid full-text + semantic | Task 1 (semantic only; FTS5 hybrid deferred — semantic alone is shippable) |
| Related Notes: top 5 cosine similar | Task 1 (`related_chunks`) + Task 5 |
| Search bar wired | Task 4 |
| RelatedNotes panel filled | Task 5 |
| selectedDoc state flows Sidebar → RelatedNotes | Task 5 (AppShell holds state) |

**Out of scope (correctly deferred):**
- FTS5 full-text index + BM25 hybrid scoring
- BlockNote editor (SP4)
- Folder tree real implementation
- Review queue (SP5)
- sqlite-vec swap for large vaults

### Placeholder scan — none found.

### Type consistency
- `SearchResult { doc_path, chunk_text, chunk_position, score }` — consistent across Task 1 (Rust), Task 3 (TS), Tasks 4-5 (components)
- `semantic_search(conn, &[f32], usize)` — defined Task 1, called Task 2
- `related_chunks(conn, &str, usize)` — defined Task 1, called Task 2
- `useSearch()` returns `{ query, setQuery, results, searching }` — defined Task 3, consumed Task 4
- `useRelatedChunks(docPath: string | null)` returns `SearchResult[]` — defined Task 3, consumed Task 5
- `Sidebar` props: `{ reviewCount, onDocSelect }` — defined Task 4, passed Task 5
- `RelatedNotes` props: `{ selectedDoc: string | null }` — defined Task 5, passed Task 5
