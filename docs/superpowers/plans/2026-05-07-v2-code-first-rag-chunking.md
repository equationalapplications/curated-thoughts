# V2 Code-First RAG Chunking — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement `docs/superpowers/specs/2026-05-07-v2-code-rag-chunking-design.md`: persist **`Chunk`** metadata (**line spans**, **`symbol_name`**, **`strategy` tag**), evolve **`chunk_autodetect` → `Vec<Chunk>`**, add **`EmbedProfile`** + **`OllamaEmbedder`** for production ingest/search while **keeping FastEmbed `Embedder`** for frozen-vector benchmarks, extend **`SearchResult`** + TypeScript + minimal UI, then add **Tree-sitter AstSymbol** phases.

**Architecture:** SQLite **`MIGRATION_V4`** adds columns. Chunkers emit **`Chunk`** with **1-indexed inclusive** line ranges via a shared **`lines_for_byte_span`** helper on the original source `&str`. Classification grows from today’s **`ChunkStrategy`** toward the spec’s **`AstSymbol(lang)` | `Scanner` | …** — **scanner stays `code_like.rs`** until renamed. Pipeline embeds **`chunk.text`** vectors; query embed dimension must match stored blobs.

**Tech Stack:** Rust (edition 2021), rusqlite, serde/serde_json, reqwest (blocking), Ollama HTTP **`POST /api/embed`**, FastEmbed (benchmark-only), Tree-sitter + grammar crates (later milestones).

**Authoritative spec:** [2026-05-07-v2-code-rag-chunking-design.md](../specs/2026-05-07-v2-code-rag-chunking-design.md)

**Retired plan replaced by this document.** Useful extracts from the old plan: Ollama embed JSON body **`{ "model": "<name>", "input": ["...", ...] }`**, response **`{ "embeddings": [[f32, ...], ...] }`**; vault **`ConfigFile`** gains **`embed_profile: Option<EmbedProfile>`** with **`#[serde(tag = "type")]`** on **`EmbedProfile`**; **`Cloud`** profile may **`anyhow!`** “not implemented” until a later task.

**Commands:** Run Rust tests from **`src-tauri/`** (`cd src-tauri && cargo test …`). Package name is **`curated-thoughts`**.

---

## File map (creates / modifies)

| Path | Responsibility |
|------|------------------|
| `src-tauri/src/db/schema.rs` | **`MIGRATION_V4`** |
| `src-tauri/src/db/connection.rs` | Run V4; fix **`open_in_memory`** to apply **V1–V4** (today skips **V3**) |
| `src-tauri/src/db/queries.rs` | **`insert_chunk`** writes metadata columns |
| `src-tauri/src/chunker/mod.rs` | **`Chunk`**, **`ChunkStrategyTag`**, **`chunk_autodetect` → Vec<Chunk>**, span helper |
| `src-tauri/src/chunker/classify.rs` | Later: extend toward **`AstSymbol(Lang)`**; initially map extensions to **tags** only |
| `src-tauri/src/chunker/prose.rs` | Return **`Vec<Chunk>`** + line ranges |
| `src-tauri/src/chunker/fallback.rs` | Return **`Vec<Chunk>`** + ranges |
| `src-tauri/src/chunker/code_like.rs` | Return **`Vec<Chunk>`**, **`strategy: Scanner`** tag |
| `src-tauri/src/chunker/declarative.rs` | Return **`Vec<Chunk>`**, **`symbol_name`** where spec says |
| `src-tauri/src/chunker/limits.rs` | Optional word-budget helpers per §5 |
| `src-tauri/src/embedder/mod.rs` | **`CloudProvider`**, **`EmbedProfile`**, **`pub mod ollama`** |
| `src-tauri/src/embedder/ollama.rs` | **`OllamaEmbedder`** |
| `src-tauri/src/vault/config.rs` | **`get_embed_profile` / `set_embed_profile`** |
| `src-tauri/src/pipeline/mod.rs` | **`Vec<Chunk>`**, **`insert_chunk`**, embedder selection |
| `src-tauri/src/search/mod.rs` | **`SearchResult`** fields + SQL selects |
| `src-tauri/src/lib.rs` | **`WikiEmbedder`**, **`search_vault`** typing |
| `src/lib/tauri.ts` | **`SearchResult`** interface |
| `src/components/shell/SearchResults.tsx`, `RelatedNotes.tsx` | Optional **`path:line`** snippet |
| `src-tauri/tests/*.rs` using **`chunk_text`** | Keep **`chunk_text()`** shim returning **`Vec<String>`** for benches — **do not** point SciFact at Ollama |

---

## Shared helper (implement once, use in all chunkers)

Add to **`src-tauri/src/chunker/mod.rs`** (or `chunker/span.rs`):

```rust
/// 1-indexed inclusive lines for `source[start_byte..end_byte]` (byte indices must be on char boundaries).
pub fn lines_for_byte_span(source: &str, start_byte: usize, end_byte: usize) -> (u32, u32) {
    let len = source.len();
    let start_byte = start_byte.min(len);
    let mut end_byte = end_byte.min(len);
    if end_byte < start_byte {
        end_byte = start_byte;
    }
    let start_line = 1 + source[..start_byte].bytes().filter(|&b| b == b'\n').count() as u32;
    let end_line = 1 + source[..end_byte].bytes().filter(|&b| b == b'\n').count() as u32;
    (start_line, end_line.max(start_line))
}
```

Chunkers must record **`start_byte` / `end_byte`** while slicing **the same `source` string** passed into ingest (use **lossy UTF-8 or extracted text** consistently for PDF/DOCX).

---

## Milestone M1 — Schema V4 + `Chunk` + pipeline (FastEmbed OK)

### Task 1: `MIGRATION_V4` + fix `open_in_memory`

**Files:** `src-tauri/src/db/schema.rs`, `src-tauri/src/db/connection.rs`

- [ ] **Step 1:** Append to **`schema.rs`** after **`MIGRATION_V3`**:

```rust
pub const MIGRATION_V4: &str = r"
ALTER TABLE chunks ADD COLUMN start_line   INTEGER NOT NULL DEFAULT 1;
ALTER TABLE chunks ADD COLUMN end_line     INTEGER NOT NULL DEFAULT 1;
ALTER TABLE chunks ADD COLUMN symbol_name  TEXT;
ALTER TABLE chunks ADD COLUMN strategy     TEXT NOT NULL DEFAULT 'prose';

INSERT OR IGNORE INTO schema_version (version) VALUES (4);
";
```

(Pre-release: **`NOT NULL DEFAULT 1`** avoids nullable line spam; re-ingest fills real spans.)

- [ ] **Step 2:** In **`connection.rs`**, import **`MIGRATION_V4`** and add it to **`AppDb::open`** batch after V3.

- [ ] **Step 3:** Fix **`open_in_memory`** to run **`MIGRATION_V1` through `MIGRATION_V4`** in order (today it omits **V3**, which diverges from disk **`AppDb::open`**).

- [ ] **Step 4:** Update **`test_schema_version_is_2`** → assert **`MAX(version) == 4`** (rename test to **`test_schema_version_is_4`**).

- [ ] **Step 5:** Add test **`migration_v4_chunk_columns_roundtrip`** inserting a row with **`start_line`**, **`end_line`**, **`strategy`**, **`symbol_name`**.

Run:

```bash
cd src-tauri && cargo test db::connection --lib
```

Expected: PASS.

- [ ] **Step 6:** Commit: `feat(db): add MIGRATION_V4 chunk metadata columns`

---

### Task 2: `Chunk`, `ChunkStrategyTag`, `insert_chunk`

**Files:** `src-tauri/src/chunker/mod.rs`, `src-tauri/src/db/queries.rs`, `src-tauri/src/pipeline/mod.rs` (minimal wiring)

- [ ] **Step 1:** Define in **`chunker/mod.rs`**:

```rust
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ChunkStrategyTag {
    Prose,
    Scanner,
    Declarative,
    Fallback,
    // AstSymbolRust, ... added with Tree-sitter milestones
}

impl ChunkStrategyTag {
    pub fn as_db_str(&self) -> &'static str {
        match self {
            ChunkStrategyTag::Prose => "prose",
            ChunkStrategyTag::Scanner => "scanner",
            ChunkStrategyTag::Declarative => "declarative",
            ChunkStrategyTag::Fallback => "fallback",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Chunk {
    pub text: String,
    pub start_line: u32,
    pub end_line: u32,
    pub symbol_name: Option<String>,
    pub strategy: ChunkStrategyTag,
}
```

- [ ] **Step 2:** Replace **`insert_chunk`** in **`queries.rs`**:

```rust
pub fn insert_chunk(conn: &Connection, doc_id: i64, chunk: &crate::chunker::Chunk, position: usize) -> Result<i64> {
    conn.execute(
        "INSERT INTO chunks (doc_id, chunk_text, position, start_line, end_line, symbol_name, strategy)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            doc_id,
            chunk.text,
            position as i64,
            chunk.start_line as i64,
            chunk.end_line as i64,
            chunk.symbol_name,
            chunk.strategy.as_db_str(),
        ],
    )?;
    Ok(conn.last_insert_rowid())
}
```

- [ ] **Step 3:** Add unit test in **`queries.rs`** `mod tests` inserting **`Chunk`** with metadata and selecting it back.

Run:

```bash
cd src-tauri && cargo test db::queries --lib
```

Expected: PASS.

- [ ] **Step 4:** Commit: `feat(chunk): add Chunk type and persist metadata in insert_chunk`

---

### Task 3: Prose + Fallback + CodeLike + Declarative → `Vec<Chunk>`

**Files:** `chunker/prose.rs`, `chunker/fallback.rs`, `chunker/code_like.rs`, `chunker/declarative.rs`, `chunker/mod.rs`

- [ ] **Step 1:** Change **`pub fn chunk_text(text: &str) -> Vec<String>`** to delegate:

```rust
pub fn chunk_text(text: &str) -> Vec<String> {
    chunk_prose_chunks(text).into_iter().map(|c| c.text).collect()
}
```

Implement **`chunk_prose_chunks(text: &str) -> Vec<Chunk>`**: reuse sentence grouping logic; for each joined chunk string, find its **byte span** inside **`text`** (search first occurrence of the chunk’s core substring or carry spans during grouping — prefer **carrying byte ranges** while iterating sentences to avoid ambiguity).

Set **`strategy: ChunkStrategyTag::Prose`**, **`symbol_name: None`**, **`lines_for_byte_span(text, start, end)`**.

- [ ] **Step 2:** **`chunk_fallback`** → **`chunk_fallback_chunks -> Vec<Chunk>`** with **`ChunkStrategyTag::Fallback`**.

- [ ] **Step 3:** **`chunk_code_like`** → **`chunk_code_like_chunks`**, **`strategy: ChunkStrategyTag::Scanner`**.

- [ ] **Step 4:** **`chunk_declarative`** → returns **`Vec<Chunk>`**, **`ChunkStrategyTag::Declarative`**, fill **`symbol_name`** per spec §7.3.

- [ ] **Step 5:** Update **`chunk_autodetect`**:

```rust
pub fn chunk_autodetect(path: &Path, text: &str) -> Vec<Chunk> {
    match classify(path) {
        ChunkStrategy::Prose => prose::chunk_prose_chunks(text),
        ChunkStrategy::CodeLike => code_like::chunk_code_like_chunks(text),
        ChunkStrategy::Declarative => declarative::chunk_declarative_chunks(path, text),
        ChunkStrategy::Fallback => fallback::chunk_fallback_chunks(text),
    }
}
```

- [ ] **Step 6:** Update **`pipeline/mod.rs`** ingest loop:

```rust
let chunks = chunk_autodetect(Path::new(path), &text);
for (i, chunk) in chunks.iter().enumerate() {
    let chunk_id = insert_chunk(conn, doc_id, chunk, i)?;
    insert_embedding(conn, chunk_id, vector)?;
}
```

Use **`chunk.text`** for **`embedder.embed`**.

- [ ] **Step 7:** Fix **`src-tauri/tests/helpers/recall_bench.rs`** and any code using **`chunk_text`** — still valid via shim.

Run:

```bash
cd src-tauri && cargo test --lib
cd src-tauri && cargo test --features test-utils --test pipeline
```

Expected: PASS.

- [ ] **Step 8:** Commit: `feat(chunk): emit Chunk with line ranges from all strategies`

---

## Milestone M2 — `EmbedProfile` + `OllamaEmbedder` + vault config + pipeline default

### Task 4: `EmbedProfile` + vault persistence

**Files:** `src-tauri/src/embedder/mod.rs`, `src-tauri/src/vault/config.rs`

- [ ] **Step 1:** Add enums per spec §8:

```rust
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum CloudProvider {
    OpenAi,
    Voyage,
    Cohere,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EmbedProfile {
    Local { model: String },
    Cloud {
        provider: CloudProvider,
        model: String,
        api_key: String,
    },
}

impl Default for EmbedProfile {
    fn default() -> Self {
        EmbedProfile::Local {
            model: "nomic-embed-code".to_string(),
        }
    }
}
```

- [ ] **Step 2:** Extend **`ConfigFile`** with **`embed_profile: Option<EmbedProfile>`**. **`get_embed_profile`** returns **`EmbedProfile::default()`** when absent.

- [ ] **Step 3:** Tests: **`embed_profile_defaults`**, **`embed_profile_roundtrip_local`**, **`embed_profile_roundtrip_cloud`** (serialize/deserialize JSON).

Run:

```bash
cd src-tauri && cargo test vault::config --lib
```

Expected: PASS.

- [ ] **Step 4:** Commit: `feat(config): persist EmbedProfile with nomic-embed-code default`

---

### Task 5: `OllamaEmbedder`

**Files:** `src-tauri/src/embedder/ollama.rs`, `src-tauri/src/embedder/mod.rs`

- [ ] **Step 1:** Implement **`OllamaEmbedder`** exactly as retired plan Task 3 (**`new_local`**, **`with_base_url`**, **`from_profile`** rejecting Cloud until implemented, **`embed`** POST **`{}/api/embed`**).

- [ ] **Step 2:** Unit tests without network (constructors + **`from_profile`** errors). Optional: **`mockito`** fake **`/api/embed`** returning JSON (**gate behind `cfg(test)`** or separate test).

Run:

```bash
cd src-tauri && cargo test embedder::ollama --lib
```

Expected: PASS.

- [ ] **Step 3:** Commit: `feat(embedder): add OllamaEmbedder for local models`

---

### Task 6: Pipeline + `WikiEmbedder` use profile

**Files:** `src-tauri/src/pipeline/mod.rs`, `src-tauri/src/lib.rs`

- [ ] **Step 1:** **`PipelineWorker`** currently has no **`VaultConfig`**. Thread **`EmbedProfile`** (or **`Arc<VaultConfig>`**) into **`PipelineWorker::new`** / **`start_pipeline`**: read profile when spawning worker (from same **`config.json`** path **`VaultConfig`** uses).

- [ ] **Step 2:** At runtime select embed implementation:

```rust
use anyhow::{anyhow, Result};
use crate::embedder::{EmbedProfile, OllamaEmbedder};

fn embed_batch(profile: &EmbedProfile, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
    match profile {
        EmbedProfile::Local { model } => {
            let o = OllamaEmbedder::new_local(model);
            o.embed(texts)
        }
        EmbedProfile::Cloud { .. } => Err(anyhow!("cloud embed not implemented")),
    }
}
```

Call **`embed_batch`** from the pipeline worker (or inline the **`match`** in **`ingest_file`**).

- [ ] **Step 3:** **`WikiEmbedder`** in **`lib.rs`** should use the **same** Ollama path for **`embed_text`** when profile is Local.

- [ ] **Step 4:** Document in **`docs/benchmarks/README.md`** or spec pointer: **SciFact / slow-tests still use FastEmbed `Embedder`** — do not import **`OllamaEmbedder`** inside **`tests/scifact.rs`**.

Run:

```bash
cd src-tauri && cargo test --features test-utils --test pipeline
```

Expected: PASS (may require Ollama + model locally for live embed; CI should mock or skip — add **`#[ignore]`** live test if needed).

- [ ] **Step 5:** Commit: `feat(pipeline): embed ingests via Ollama from EmbedProfile`

---

## Milestone M3 — Search + TS + UI

### Task 7: `SearchResult` + SQL

**Files:** `src-tauri/src/search/mod.rs`, `src/lib/tauri.ts`, `SearchResults.tsx`, `RelatedNotes.tsx`

- [ ] **Step 1:** Extend **`SearchResult`**:

```rust
pub struct SearchResult {
    pub doc_path: String,
    pub chunk_text: String,
    pub chunk_position: i64,
    pub score: f32,
    pub start_line: i64,
    pub end_line: i64,
    pub symbol_name: Option<String>,
    pub strategy: String,
}
```

- [ ] **Step 2:** SQL selects **`c.start_line, c.end_line, c.symbol_name, c.strategy`** in **`semantic_search`** and **`related_chunks`**.

- [ ] **Step 3:** Mirror fields in **`src/lib/tauri.ts`**.

- [ ] **Step 4:** UI: show **`doc_path:start_line`** (or range) next to snippet.

Run:

```bash
cd src-tauri && cargo test search --lib
npm test
```

Expected: PASS.

- [ ] **Step 5:** Commit: `feat(search): return chunk line span and strategy`

---

## Milestone M4 — Classifier tags + word budgets (optional convergence)

- [ ] **Step 1:** Introduce **`Lang`** + **`IngestStrategy`** enum matching spec §4; **`classify_v2(path) -> IngestStrategy`**; map **`AstSymbol`** buckets **to `Scanner` chunks** until M5/M6 ship (**spec rollout note**).

- [ ] **Step 2:** Map **`IngestStrategy`** → **`ChunkStrategyTag`** on each **`Chunk`** (**`ast_symbol_rust`** etc. once AST lands).

- [ ] **Step 3:** Optionally add **`chunker/words.rs`** and tighten budgets toward §5.

- [ ] **Step 4:** Commit per logical unit.

---

## Milestone M5 — AstSymbol (Rust, Python, Go)

**Files:** `src-tauri/Cargo.toml`, **`src-tauri/src/chunker/ast.rs`**, **`classify.rs`**, **`mod.rs`**

- [ ] **Step 1:** Add dependencies: **`tree-sitter`**, **`tree-sitter-rust`**, **`tree-sitter-python`**, **`tree-sitter-go`** (versions from crates.io at implementation time).

- [ ] **Step 2:** Implement **`chunk_ast(lang, source) -> Vec<Chunk>`** with Tree-sitter queries per §7.1; **`symbol_name`** from node text; **`strategy`** tags **`AstSymbolRust`** etc.; on failure return **`chunk_code_like_chunks(source)`** and tag **`Scanner`** or keep requested ast tag + log — follow spec (**fallback to scanner**).

- [ ] **Step 3:** Wire **`classify`** so **`rs` / `py` / `go`** use AST path.

- [ ] **Step 4:** Integration tests with tiny fixtures under **`src-tauri/tests/fixtures/ast/`**.

- [ ] **Step 5:** Commit: `feat(chunk): Tree-sitter AstSymbol for Rust, Python, Go`

---

## Milestone M6 — AstSymbol (TypeScript, TSX, JavaScript)

- [ ] **Step 1:** Add **`tree-sitter-typescript`** (or split **`tree-sitter-javascript`** + TS grammar per crate layout).

- [ ] **Step 2:** Extend **`chunk_ast`** for TS/JS; handle **`tsx`** .

- [ ] **Step 3:** Tests for exported **`function`**, **`class`**, arrow **`const`**.

- [ ] **Step 4:** Commit: `feat(chunk): Tree-sitter AstSymbol for TS/TSX/JS`

---

## Verification matrix (before claiming done)

| Requirement (spec §) | Task |
|---------------------|------|
| §9 Schema | Task 1 |
| §6 Chunk struct | Task 2–3 |
| §3 Pipeline `Vec<Chunk>` | Task 3 |
| §8 EmbedProfile | Task 4–6 |
| §10 Search payload | Task 7 |
| §4 AstSymbol rollout | M4–M6 |
| §11 Benchmarks untouched | Never switch **`tests/scifact.rs`** to Ollama |
| Pre-release breaking OK | Nullable only **`symbol_name`**; wipe **`brain.db`** acceptable |

---

## Self-review (plan author)

1. **Spec coverage:** §0 baseline acknowledged; §7 chunker behaviors mapped to Tasks 3–6; §8 embedding Tasks 4–6; §10 Task 7; Tree-sitter §7.1 M5–M6.
2. **Placeholder scan:** No `TBD`; cloud embed explicitly “not implemented” path.
3. **Types:** **`ChunkStrategy`** (classifier) vs **`ChunkStrategyTag`** (persisted) — implementers must not confuse; rename in code if clearer (**`PersistedChunkStrategy`**).

---

## Execution handoff

**Plan complete and saved to `docs/superpowers/plans/2026-05-07-v2-code-first-rag-chunking.md`.**

**1. Subagent-Driven (recommended)** — dispatch a fresh subagent per task; spec then quality review between tasks (`superpowers:subagent-driven-development`).

**2. Inline Execution** — run tasks in this session with checkpoints (`superpowers:executing-plans`).

**Which approach do you want?**
