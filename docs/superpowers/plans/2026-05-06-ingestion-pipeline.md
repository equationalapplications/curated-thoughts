# Ingestion Pipeline — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire file-watcher events into a background ingestion pipeline that chunks text files, generates FastEmbed embeddings, and persists them to SQLite — so every document dropped into the vault is indexed automatically.

**Architecture:** A `PipelineWorker` runs on a dedicated background thread. `lib.rs` creates an `mpsc` channel on startup; the watcher callback sends `PipelineJob` messages through it. The worker owns the SQLite connection and the FastEmbed model (both non-`Send` friendly objects stay on one thread). Embeddings are stored as little-endian `f32` BLOBs in a new `embeddings` table. Cosine similarity search is done in Rust (load vectors, compute). Sub-project 3 can swap in `sqlite-vec` when the vault grows large.

**Tech Stack:** Rust, rusqlite (bundled), fastembed 4.x (AllMiniLML6V2, 384-dim, 23 MB), sha2, hex, tokio (already in deps)

---

## File Map

| File | Responsibility |
|---|---|
| `src-tauri/src/db/schema.rs` | Add `MIGRATION_V2`: `embeddings` table + schema version bump |
| `src-tauri/src/db/connection.rs` | Apply V2 migration on open |
| `src-tauri/src/db/queries.rs` | All DB read/write helpers used by pipeline |
| `src-tauri/src/chunker/mod.rs` | Split text into ~500-word chunks with 50-word overlap |
| `src-tauri/src/embedder/mod.rs` | Thin wrapper: init `TextEmbedding`, generate `Vec<Vec<f32>>` |
| `src-tauri/src/pipeline/mod.rs` | `PipelineJob` enum + `PipelineWorker` struct + `start_pipeline` fn |
| `src-tauri/src/lib.rs` | Add `PipelineTx` state, wire watcher → pipeline, add status command |
| `src/lib/tauri.ts` | Add `getIndexingStatus()` invoke wrapper |
| `src/hooks/useIndexingStatus.ts` | Poll indexing status every 2 s |
| `src/components/shell/IndexingStatus.tsx` | Badge in sidebar: "Indexing…" / "N docs indexed" |
| `src/components/shell/Sidebar.tsx` | Render `<IndexingStatus>` below search bar |

---

### Task 1: Add embeddings table (Migration V2)

**Files:**
- Modify: `src-tauri/src/db/schema.rs`
- Modify: `src-tauri/src/db/connection.rs`

- [ ] **Step 1: Add `MIGRATION_V2` to `src-tauri/src/db/schema.rs`**

Append after the existing `MIGRATION_V1` constant:

```rust
pub const MIGRATION_V2: &str = "
CREATE TABLE IF NOT EXISTS embeddings (
    id       INTEGER PRIMARY KEY AUTOINCREMENT,
    chunk_id INTEGER NOT NULL REFERENCES chunks(id) ON DELETE CASCADE,
    vector   BLOB    NOT NULL
);

INSERT OR IGNORE INTO schema_version (version) VALUES (2);
";
```

- [ ] **Step 2: Apply V2 in `src-tauri/src/db/connection.rs`**

Update `AppDb::open` and `open_in_memory` to also run `MIGRATION_V2`:

```rust
use crate::db::schema::{MIGRATION_V1, MIGRATION_V2};

pub struct AppDb(pub Connection);

impl AppDb {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch(MIGRATION_V1)?;
        conn.execute_batch(MIGRATION_V2)?;
        Ok(AppDb(conn))
    }
}

#[cfg(test)]
pub fn open_in_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(MIGRATION_V1)?;
    conn.execute_batch(MIGRATION_V2)?;
    Ok(conn)
}
```

- [ ] **Step 3: Add test for embeddings table**

Add to the `#[cfg(test)]` block in `src-tauri/src/db/connection.rs`:

```rust
#[test]
fn test_embeddings_table_exists() {
    let conn = open_in_memory().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='embeddings'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn test_schema_version_is_2() {
    let conn = open_in_memory().unwrap();
    let max_version: i64 = conn
        .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(max_version, 2);
}
```

- [ ] **Step 4: Run tests**

```bash
source "$HOME/.cargo/env" && cd src-tauri && cargo test db::
```

Expected: 4 tests pass (2 existing + 2 new).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/db/
git commit -m "feat: add embeddings table via migration V2"
```

---

### Task 2: DB query helpers

**Files:**
- Create: `src-tauri/src/db/queries.rs`
- Modify: `src-tauri/src/db/mod.rs`

- [ ] **Step 1: Create `src-tauri/src/db/queries.rs`**

```rust
use anyhow::Result;
use rusqlite::Connection;

pub struct DocRow {
    pub id: i64,
    pub hash: String,
    pub status: String,
}

pub fn upsert_document(conn: &Connection, path: &str, hash: &str) -> Result<i64> {
    conn.execute(
        "INSERT INTO documents (path, hash, tier, status)
         VALUES (?1, ?2, 'user_doc', 'pending')
         ON CONFLICT(path) DO UPDATE SET hash = ?2, status = 'pending'",
        rusqlite::params![path, hash],
    )?;
    Ok(conn.query_row(
        "SELECT id FROM documents WHERE path = ?1",
        [path],
        |r| r.get(0),
    )?)
}

pub fn get_document_by_path(conn: &Connection, path: &str) -> Result<Option<DocRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, hash, status FROM documents WHERE path = ?1",
    )?;
    let mut rows = stmt.query([path])?;
    if let Some(row) = rows.next()? {
        Ok(Some(DocRow {
            id: row.get(0)?,
            hash: row.get(1)?,
            status: row.get(2)?,
        }))
    } else {
        Ok(None)
    }
}

pub fn delete_document_chunks(conn: &Connection, doc_id: i64) -> Result<()> {
    conn.execute("DELETE FROM chunks WHERE doc_id = ?1", [doc_id])?;
    Ok(())
}

pub fn insert_chunk(conn: &Connection, doc_id: i64, text: &str, position: usize) -> Result<i64> {
    conn.execute(
        "INSERT INTO chunks (doc_id, chunk_text, position) VALUES (?1, ?2, ?3)",
        rusqlite::params![doc_id, text, position as i64],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn insert_embedding(conn: &Connection, chunk_id: i64, vector: &[f32]) -> Result<()> {
    let bytes: Vec<u8> = vector
        .iter()
        .flat_map(|f| f.to_le_bytes())
        .collect();
    conn.execute(
        "INSERT INTO embeddings (chunk_id, vector) VALUES (?1, ?2)",
        rusqlite::params![chunk_id, bytes],
    )?;
    Ok(())
}

pub fn mark_document_indexed(conn: &Connection, doc_id: i64) -> Result<()> {
    conn.execute(
        "UPDATE documents SET status = 'indexed', last_indexed = unixepoch() WHERE id = ?1",
        [doc_id],
    )?;
    Ok(())
}

pub fn mark_document_error(conn: &Connection, doc_id: i64) -> Result<()> {
    conn.execute(
        "UPDATE documents SET status = 'error' WHERE id = ?1",
        [doc_id],
    )?;
    Ok(())
}

pub fn delete_document(conn: &Connection, path: &str) -> Result<()> {
    // Chunks and embeddings cascade via FK
    conn.execute("DELETE FROM documents WHERE path = ?1", [path])?;
    Ok(())
}

pub fn count_indexed_documents(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM documents WHERE status = 'indexed'",
        [],
        |r| r.get(0),
    )?)
}

pub fn count_pending_documents(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM documents WHERE status = 'pending'",
        [],
        |r| r.get(0),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::open_in_memory;

    #[test]
    fn test_upsert_document_creates_and_updates() {
        let conn = open_in_memory().unwrap();
        let id1 = upsert_document(&conn, "/docs/note.md", "abc123").unwrap();
        let id2 = upsert_document(&conn, "/docs/note.md", "def456").unwrap();
        assert_eq!(id1, id2, "upsert must return same id");
        let doc = get_document_by_path(&conn, "/docs/note.md").unwrap().unwrap();
        assert_eq!(doc.hash, "def456");
    }

    #[test]
    fn test_insert_chunk_and_embedding() {
        let conn = open_in_memory().unwrap();
        let doc_id = upsert_document(&conn, "/docs/a.md", "hash1").unwrap();
        let chunk_id = insert_chunk(&conn, doc_id, "hello world", 0).unwrap();
        insert_embedding(&conn, chunk_id, &[0.1_f32, 0.2, 0.3]).unwrap();

        let bytes: Vec<u8> = conn
            .query_row(
                "SELECT vector FROM embeddings WHERE chunk_id = ?1",
                [chunk_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(bytes.len(), 12); // 3 × 4 bytes
    }

    #[test]
    fn test_delete_document_cascades() {
        let conn = open_in_memory().unwrap();
        let doc_id = upsert_document(&conn, "/docs/b.md", "hash2").unwrap();
        let chunk_id = insert_chunk(&conn, doc_id, "text", 0).unwrap();
        insert_embedding(&conn, chunk_id, &[1.0_f32]).unwrap();
        delete_document(&conn, "/docs/b.md").unwrap();

        let doc = get_document_by_path(&conn, "/docs/b.md").unwrap();
        assert!(doc.is_none());
        let emb_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM embeddings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(emb_count, 0);
    }

    #[test]
    fn test_count_documents() {
        let conn = open_in_memory().unwrap();
        let id = upsert_document(&conn, "/docs/c.md", "hash3").unwrap();
        assert_eq!(count_pending_documents(&conn).unwrap(), 1);
        mark_document_indexed(&conn, id).unwrap();
        assert_eq!(count_indexed_documents(&conn).unwrap(), 1);
        assert_eq!(count_pending_documents(&conn).unwrap(), 0);
    }
}
```

- [ ] **Step 2: Export from `src-tauri/src/db/mod.rs`**

Replace the file contents:

```rust
pub mod connection;
pub mod queries;
pub mod schema;

pub use connection::AppDb;
pub use queries::*;
```

- [ ] **Step 3: Run tests**

```bash
source "$HOME/.cargo/env" && cd src-tauri && cargo test db::
```

Expected: 8 tests pass (4 existing + 4 new).

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/db/
git commit -m "feat: add DB query helpers for ingestion pipeline"
```

---

### Task 3: Text chunker

**Files:**
- Create: `src-tauri/src/chunker/mod.rs`

- [ ] **Step 1: Create `src-tauri/src/chunker/mod.rs`**

```rust
const CHUNK_WORDS: usize = 500;
const OVERLAP_WORDS: usize = 50;

pub fn chunk_text(text: &str) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return vec![];
    }
    let mut chunks = Vec::new();
    let mut start = 0;
    loop {
        let end = (start + CHUNK_WORDS).min(words.len());
        let chunk = words[start..end].join(" ");
        if !chunk.trim().is_empty() {
            chunks.push(chunk);
        }
        if end == words.len() {
            break;
        }
        start = end.saturating_sub(OVERLAP_WORDS);
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_text_returns_no_chunks() {
        assert!(chunk_text("").is_empty());
        assert!(chunk_text("   ").is_empty());
    }

    #[test]
    fn test_short_text_is_single_chunk() {
        let chunks = chunk_text("hello world this is a short sentence");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "hello world this is a short sentence");
    }

    #[test]
    fn test_long_text_splits_into_multiple_chunks() {
        let word = "word ";
        let text = word.repeat(1100);
        let chunks = chunk_text(&text);
        assert!(chunks.len() >= 2, "expected multiple chunks, got {}", chunks.len());
    }

    #[test]
    fn test_chunks_have_overlap() {
        let words: Vec<String> = (0..600).map(|i| format!("w{}", i)).collect();
        let text = words.join(" ");
        let chunks = chunk_text(&text);
        assert_eq!(chunks.len(), 2);
        // Last 50 words of chunk 0 should appear at start of chunk 1
        let last_of_first: Vec<&str> = chunks[0].split_whitespace().rev().take(50).collect::<Vec<_>>().into_iter().rev().collect();
        let first_of_second: Vec<&str> = chunks[1].split_whitespace().take(50).collect();
        assert_eq!(last_of_first, first_of_second);
    }

    #[test]
    fn test_chunk_max_word_count() {
        let text = "word ".repeat(1200);
        let chunks = chunk_text(&text);
        for chunk in &chunks {
            let word_count = chunk.split_whitespace().count();
            assert!(word_count <= 500, "chunk has {} words", word_count);
        }
    }
}
```

- [ ] **Step 2: Declare module in `src-tauri/src/lib.rs`**

Add `mod chunker;` after the existing `mod setup;` line.

- [ ] **Step 3: Run tests**

```bash
source "$HOME/.cargo/env" && cd src-tauri && cargo test chunker::
```

Expected: 5 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/chunker/
git add src-tauri/src/lib.rs
git commit -m "feat: add word-based text chunker with 500-word chunks and 50-word overlap"
```

---

### Task 4: FastEmbed embedder

**Files:**
- Modify: `src-tauri/Cargo.toml`
- Create: `src-tauri/src/embedder/mod.rs`

- [ ] **Step 1: Add fastembed and sha2 to `src-tauri/Cargo.toml`**

```toml
fastembed = { version = "4", default-features = false, features = ["ort-download-binaries"] }
sha2 = "0.10"
hex = "0.4"
```

- [ ] **Step 2: Write the failing test first in `src-tauri/src/embedder/mod.rs`**

```rust
use anyhow::Result;
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

pub struct Embedder {
    model: TextEmbedding,
}

impl Embedder {
    pub fn new() -> Result<Self> {
        let model = TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::AllMiniLML6V2)
        )?;
        Ok(Embedder { model })
    }

    pub fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        Ok(self.model.embed(texts, None)?)
    }

    pub fn dimensions() -> usize {
        384
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_embed_returns_correct_dimensions() {
        let embedder = Embedder::new().expect("model init");
        let vecs = embedder.embed(vec!["hello world".to_string()]).unwrap();
        assert_eq!(vecs.len(), 1);
        assert_eq!(vecs[0].len(), Embedder::dimensions());
    }

    #[test]
    fn test_embed_multiple_texts() {
        let embedder = Embedder::new().expect("model init");
        let vecs = embedder
            .embed(vec!["first sentence".to_string(), "second sentence".to_string()])
            .unwrap();
        assert_eq!(vecs.len(), 2);
    }

    #[test]
    fn test_similar_texts_have_high_cosine_similarity() {
        let embedder = Embedder::new().expect("model init");
        let vecs = embedder
            .embed(vec![
                "the cat sat on the mat".to_string(),
                "a cat was sitting on the mat".to_string(),
                "quantum physics and thermodynamics".to_string(),
            ])
            .unwrap();
        let sim_similar = cosine_similarity(&vecs[0], &vecs[1]);
        let sim_different = cosine_similarity(&vecs[0], &vecs[2]);
        assert!(sim_similar > sim_different, "similar texts should be closer");
    }

    fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
        let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        dot / (norm_a * norm_b)
    }
}
```

- [ ] **Step 3: Declare module in `src-tauri/src/lib.rs`**

Add `mod embedder;` after `mod chunker;`.

- [ ] **Step 4: Run tests (downloads ~23 MB model on first run)**

```bash
source "$HOME/.cargo/env" && cd src-tauri && cargo test embedder:: -- --nocapture 2>&1
```

Expected: 3 tests pass. First run downloads `all-MiniLM-L6-v2` to `~/.cache/huggingface/`.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/Cargo.toml src-tauri/src/embedder/ src-tauri/src/lib.rs
git commit -m "feat: add FastEmbed embedder wrapping AllMiniLML6V2 (384-dim)"
```

---

### Task 5: Hashing utility

**Files:**
- Create: `src-tauri/src/hasher.rs`

- [ ] **Step 1: Create `src-tauri/src/hasher.rs`**

```rust
use sha2::{Digest, Sha256};

pub fn hash_bytes(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_is_deterministic() {
        assert_eq!(hash_bytes(b"hello"), hash_bytes(b"hello"));
    }

    #[test]
    fn test_different_inputs_produce_different_hashes() {
        assert_ne!(hash_bytes(b"hello"), hash_bytes(b"world"));
    }

    #[test]
    fn test_hash_is_64_hex_chars() {
        assert_eq!(hash_bytes(b"test").len(), 64);
    }
}
```

- [ ] **Step 2: Declare module in `src-tauri/src/lib.rs`**

Add `mod hasher;` after `mod embedder;`.

- [ ] **Step 3: Run tests**

```bash
source "$HOME/.cargo/env" && cd src-tauri && cargo test hasher::
```

Expected: 3 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/hasher.rs src-tauri/src/lib.rs
git commit -m "feat: add SHA-256 hashing utility for change detection"
```

---

### Task 6: Pipeline worker

**Files:**
- Create: `src-tauri/src/pipeline/mod.rs`

- [ ] **Step 1: Create `src-tauri/src/pipeline/mod.rs`**

```rust
use anyhow::Result;
use rusqlite::Connection;
use std::{path::{Path, PathBuf}, sync::mpsc};

use crate::chunker::chunk_text;
use crate::db::queries::{
    delete_document, delete_document_chunks, get_document_by_path, insert_chunk,
    insert_embedding, mark_document_error, mark_document_indexed, upsert_document,
};
use crate::embedder::Embedder;
use crate::hasher::hash_bytes;

#[derive(Debug, Clone)]
pub enum PipelineJob {
    Ingest(String),
    Delete(String),
}

pub struct PipelineWorker {
    db_path: PathBuf,
    rx: mpsc::Receiver<PipelineJob>,
}

impl PipelineWorker {
    pub fn new(db_path: PathBuf, rx: mpsc::Receiver<PipelineJob>) -> Self {
        PipelineWorker { db_path, rx }
    }

    pub fn run(self) {
        let embedder = match Embedder::new() {
            Ok(e) => e,
            Err(err) => {
                eprintln!("[pipeline] embedder init failed: {err}");
                return;
            }
        };
        let conn = match Connection::open(&self.db_path) {
            Ok(c) => c,
            Err(err) => {
                eprintln!("[pipeline] db open failed: {err}");
                return;
            }
        };

        for job in self.rx {
            match job {
                PipelineJob::Ingest(path) => {
                    if let Err(e) = ingest_file(&conn, &embedder, &path) {
                        eprintln!("[pipeline] ingest error {path}: {e}");
                    }
                }
                PipelineJob::Delete(path) => {
                    if let Err(e) = delete_document(&conn, &path) {
                        eprintln!("[pipeline] delete error {path}: {e}");
                    }
                }
            }
        }
    }
}

fn ingest_file(conn: &Connection, embedder: &Embedder, path: &str) -> Result<()> {
    // Only index plain text / markdown for now
    let p = Path::new(path);
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
    if !matches!(ext, "md" | "txt" | "markdown") {
        return Ok(());
    }

    let bytes = std::fs::read(path)?;
    let hash = hash_bytes(&bytes);

    // Skip if content unchanged
    if let Some(doc) = get_document_by_path(conn, path)? {
        if doc.hash == hash && doc.status == "indexed" {
            return Ok(());
        }
        // Content changed — remove old chunks (cascade deletes embeddings)
        delete_document_chunks(conn, doc.id)?;
    }

    let text = String::from_utf8_lossy(&bytes).to_string();
    let doc_id = upsert_document(conn, path, &hash)?;

    let chunks = chunk_text(&text);
    if chunks.is_empty() {
        mark_document_indexed(conn, doc_id)?;
        return Ok(());
    }

    let embeddings = embedder.embed(chunks.clone()).map_err(|e| {
        let _ = mark_document_error(conn, doc_id);
        e
    })?;

    for (i, (chunk, vector)) in chunks.iter().zip(embeddings.iter()).enumerate() {
        let chunk_id = insert_chunk(conn, doc_id, chunk, i)?;
        insert_embedding(conn, chunk_id, vector)?;
    }

    mark_document_indexed(conn, doc_id)?;
    Ok(())
}

/// Start the pipeline worker on a background thread.
/// Returns the sender end of the job channel.
pub fn start_pipeline(db_path: PathBuf) -> mpsc::SyncSender<PipelineJob> {
    let (tx, rx) = mpsc::sync_channel::<PipelineJob>(256);
    let worker = PipelineWorker::new(db_path, rx);
    std::thread::Builder::new()
        .name("pipeline-worker".to_string())
        .spawn(move || worker.run())
        .expect("spawn pipeline worker");
    tx
}
```

- [ ] **Step 2: Declare module in `src-tauri/src/lib.rs`**

Add `mod pipeline;` after `mod hasher;`.

- [ ] **Step 3: Verify build compiles**

```bash
source "$HOME/.cargo/env" && cd src-tauri && cargo build 2>&1 | grep -E "^error" | head -20
```

Expected: no errors (warnings about unused ok).

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/pipeline/ src-tauri/src/lib.rs
git commit -m "feat: add background pipeline worker (ingest/delete jobs)"
```

---

### Task 7: Wire pipeline into app + Tauri commands

**Files:**
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Replace `src-tauri/src/lib.rs` with wired version**

```rust
mod chunker;
mod db;
mod embedder;
mod hasher;
mod pipeline;
mod setup;
mod vault;
mod watcher;

use std::sync::{mpsc::SyncSender, Mutex};
use tauri::{AppHandle, Emitter, State};
use db::AppDb;
use pipeline::{start_pipeline, PipelineJob};
use setup::{check_ollama as ollama_check, list_local_models as ollama_models,
            pull_model as ollama_pull, recommended_model as ollama_recommended,
            start_ollama_server as ollama_start, OllamaStatus};
use vault::VaultConfig;
use watcher::{start_watcher, VaultEvent};

#[allow(dead_code)]
struct DbState(Mutex<AppDb>);
struct VaultConfigState(Mutex<VaultConfig>);
struct PipelineTx(Mutex<SyncSender<PipelineJob>>);

// ── Vault commands ────────────────────────────────────────────────────────────

#[tauri::command]
fn get_vault_path(state: State<VaultConfigState>) -> Result<Option<String>, String> {
    state.0.lock().unwrap().get_vault_path().map_err(|e| e.to_string())
}

#[tauri::command]
fn set_vault_path(path: String, state: State<VaultConfigState>) -> Result<(), String> {
    state.0.lock().unwrap().set_vault_path(&path).map_err(|e| e.to_string())
}

// ── Watcher commands ──────────────────────────────────────────────────────────

#[tauri::command]
fn start_file_watcher(
    vault_path: String,
    app: AppHandle,
    pipeline: State<PipelineTx>,
) -> Result<(), String> {
    let tx = pipeline.0.lock().unwrap().clone();
    start_watcher(vault_path.into(), move |event| {
        let _ = app.emit("vault-event", &event);
        let job = match &event {
            VaultEvent::Added(p) | VaultEvent::Modified(p) => {
                Some(PipelineJob::Ingest(p.clone()))
            }
            VaultEvent::Deleted(p) => Some(PipelineJob::Delete(p.clone())),
        };
        if let Some(j) = job {
            let _ = tx.try_send(j);
        }
    })
    .map_err(|e| e.to_string())
}

// ── Pipeline status ───────────────────────────────────────────────────────────

#[derive(serde::Serialize)]
pub struct IndexingStatus {
    pub indexed: i64,
    pub pending: i64,
}

#[tauri::command]
fn get_indexing_status(db_state: State<DbState>) -> Result<IndexingStatus, String> {
    let guard = db_state.0.lock().unwrap();
    let conn = &guard.0;
    let indexed = db::count_indexed_documents(conn).map_err(|e| e.to_string())?;
    let pending = db::count_pending_documents(conn).map_err(|e| e.to_string())?;
    Ok(IndexingStatus { indexed, pending })
}

// ── Ollama commands ───────────────────────────────────────────────────────────

#[tauri::command]
fn check_ollama() -> OllamaStatus { ollama_check() }

#[tauri::command]
fn list_local_models() -> Result<Vec<String>, String> {
    ollama_models().map_err(|e| e.to_string())
}

#[tauri::command]
fn get_recommended_model() -> String { ollama_recommended().to_string() }

#[tauri::command]
fn start_ollama_server() -> Result<(), String> {
    ollama_start().map_err(|e| e.to_string())
}

#[tauri::command]
fn pull_model(model_id: String, app: AppHandle) -> Result<(), String> {
    ollama_pull(&model_id, move |completed, total| {
        let _ = app.emit(
            "ollama-pull-progress",
            serde_json::json!({ "completed": completed, "total": total }),
        );
    })
    .map_err(|e| e.to_string())
}

// ── App entry ─────────────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let brain_dir = dirs::home_dir().unwrap_or_default().join(".brain");
    std::fs::create_dir_all(&brain_dir).ok();

    let db_path = brain_dir.join("brain.db");
    let db = AppDb::open(&db_path).expect("failed to open database");
    let config = VaultConfig::new(brain_dir.join("config.json"));
    let pipeline_tx = start_pipeline(db_path);

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_os::init())
        .manage(DbState(Mutex::new(db)))
        .manage(VaultConfigState(Mutex::new(config)))
        .manage(PipelineTx(Mutex::new(pipeline_tx)))
        .invoke_handler(tauri::generate_handler![
            get_vault_path,
            set_vault_path,
            start_file_watcher,
            get_indexing_status,
            check_ollama,
            list_local_models,
            pull_model,
            start_ollama_server,
            get_recommended_model,
        ])
        .run(tauri::generate_context!())
        .expect("error running Tauri application");
}
```

Note: `DbState` needs to expose the inner `Connection`. Update `AppDb` in `connection.rs` to remove `#[allow(dead_code)]` — the field is now used in `get_indexing_status`.

- [ ] **Step 2: Remove `#[allow(dead_code)]` from `AppDb` in `connection.rs`**

The `0` field of `AppDb` is now accessed via `guard.0` in `get_indexing_status`. The allow is no longer needed.

Also update `db::count_indexed_documents` and `db::count_pending_documents` to be accessible — they're already `pub` in `queries.rs` and re-exported via `pub use queries::*` in `mod.rs`. Confirm `mod.rs` has `pub use queries::*`.

- [ ] **Step 3: Build**

```bash
source "$HOME/.cargo/env" && cd src-tauri && cargo build 2>&1 | grep -E "^error" | head -20
```

Expected: no errors. Fix any type mismatches (the `watcher::VaultEvent` import must match the enum variant names `Added/Modified/Deleted`).

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/lib.rs src-tauri/src/db/connection.rs
git commit -m "feat: wire pipeline into app — watcher events feed ingestion worker"
```

---

### Task 8: Frontend indexing status

**Files:**
- Modify: `src/lib/tauri.ts`
- Create: `src/hooks/useIndexingStatus.ts`
- Create: `src/components/shell/IndexingStatus.tsx`
- Modify: `src/components/shell/Sidebar.tsx`
- Modify: `src/index.css`

- [ ] **Step 1: Add `getIndexingStatus` to `src/lib/tauri.ts`**

Append:

```ts
export interface IndexingStatus {
  indexed: number;
  pending: number;
}

export const getIndexingStatus = (): Promise<IndexingStatus> =>
  invoke("get_indexing_status");
```

- [ ] **Step 2: Create `src/hooks/useIndexingStatus.ts`**

```ts
import { useEffect, useState } from "react";
import { getIndexingStatus, IndexingStatus } from "../lib/tauri";

const POLL_MS = 2000;

export function useIndexingStatus(): IndexingStatus {
  const [status, setStatus] = useState<IndexingStatus>({ indexed: 0, pending: 0 });

  useEffect(() => {
    const tick = () => getIndexingStatus().then(setStatus).catch(() => {});
    tick();
    const id = setInterval(tick, POLL_MS);
    return () => clearInterval(id);
  }, []);

  return status;
}
```

- [ ] **Step 3: Create `src/components/shell/IndexingStatus.tsx`**

```tsx
import { useIndexingStatus } from "../../hooks/useIndexingStatus";

export function IndexingStatus() {
  const { indexed, pending } = useIndexingStatus();

  if (pending > 0) {
    return (
      <div className="indexing-badge indexing-badge--busy">
        Indexing {pending} file{pending !== 1 ? "s" : ""}…
      </div>
    );
  }
  if (indexed === 0) return null;
  return (
    <div className="indexing-badge">
      {indexed} doc{indexed !== 1 ? "s" : ""} indexed
    </div>
  );
}
```

- [ ] **Step 4: Add to `src/components/shell/Sidebar.tsx`**

```tsx
import { IndexingStatus } from "./IndexingStatus";

interface Props { reviewCount: number }

export function Sidebar({ reviewCount }: Props) {
  return (
    <aside className="sidebar">
      <div className="search-bar">
        <input type="search" placeholder="Search your brain..." />
      </div>
      <IndexingStatus />
      <div className="folder-tree">
        <p className="placeholder">Documents will appear here</p>
      </div>
      {reviewCount > 0 && (
        <div className="review-badge">{reviewCount} pages ready to review</div>
      )}
    </aside>
  );
}
```

- [ ] **Step 5: Add CSS to `src/index.css`**

Append:

```css
.indexing-badge {
  font-size: 11px;
  font-weight: 600;
  color: var(--on-surface-var);
  background: var(--elev-2);
  border: 1px solid var(--outline-var);
  padding: 5px 10px;
  border-radius: var(--r-pill);
  text-align: center;
  letter-spacing: 0.01em;
}
.indexing-badge--busy {
  background: var(--primary-container);
  color: var(--on-primary-cont);
  border-color: transparent;
  animation: pulse-badge 1.5s ease-in-out infinite;
}
@keyframes pulse-badge {
  0%, 100% { opacity: 1; }
  50%       { opacity: 0.65; }
}
```

- [ ] **Step 6: Update test mock for `get_indexing_status`**

In `src/test-setup.ts`, update the default `invoke` mock to handle the new command:

```ts
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn((cmd: string) => {
    if (cmd === "check_ollama") {
      return Promise.resolve({ installed: true, running: true, models: ["llama3.2:3b"] });
    }
    if (cmd === "get_recommended_model") {
      return Promise.resolve("llama3.2:3b");
    }
    if (cmd === "get_indexing_status") {
      return Promise.resolve({ indexed: 0, pending: 0 });
    }
    return Promise.resolve(null);
  }),
}));
```

- [ ] **Step 7: Run all tests**

```bash
npm test
```

Expected: 6 tests pass.

- [ ] **Step 8: Build frontend**

```bash
npm run build
```

Expected: clean build.

- [ ] **Step 9: Commit**

```bash
git add src/lib/tauri.ts src/hooks/useIndexingStatus.ts \
        src/components/shell/IndexingStatus.tsx \
        src/components/shell/Sidebar.tsx src/index.css src/test-setup.ts
git commit -m "feat: add indexing status badge to sidebar (polls every 2s)"
```

---

## Self-Review

### Spec coverage

| Spec requirement | Task |
|---|---|
| File watcher → pipeline | Task 7 (watcher callback sends PipelineJob) |
| CHUNK ~512 tokens | Task 3 (500 words ≈ 512 tokens) |
| EMBED via FastEmbed | Task 4 + Task 6 |
| Store in DB | Task 1 (schema) + Task 2 (queries) + Task 6 (pipeline) |
| Change detection (hash) | Task 5 + Task 6 |
| Deleted file → cascade | Task 2 (`delete_document` + FK cascade) + Task 7 (watcher Delete event) |
| Pipeline status to frontend | Task 7 (`get_indexing_status`) + Task 8 (UI badge) |

**Out of scope (Sub-project 3+):**
- PDF/DOCX → markdown via pandoc
- Cosine similarity search endpoint
- LLM summarization
- Human review queue
- sqlite-vec swap-in

### Placeholder scan — none found. All steps contain exact code.

### Type consistency
- `PipelineJob::Ingest(String)` / `PipelineJob::Delete(String)` — consistent across Tasks 6, 7
- `VaultEvent::Added(p)` / `VaultEvent::Modified(p)` / `VaultEvent::Deleted(p)` — match `fs_watcher.rs` definition
- `IndexingStatus { indexed, pending }` — matches Rust struct and TS interface in Tasks 7, 8
- `count_indexed_documents` / `count_pending_documents` — defined in Task 2, used in Task 7
- `start_pipeline` — defined Task 6, called Task 7
