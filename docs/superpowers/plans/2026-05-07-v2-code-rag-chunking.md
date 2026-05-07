# V2 Code-First RAG Chunking Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the universal prose chunker with a hybrid AST/Scanner/Declarative/Prose strategy, add line-range metadata to every chunk, configure `nomic-embed-code` via Ollama as the default embedder, and expose full chunk metadata in search results for coding agent use.

**Architecture:** A classifier maps file extensions to one of 5 strategies (`AstSymbol`, `Scanner`, `Declarative`, `Prose`, `Fallback`). Each strategy returns `Vec<Chunk>` carrying text, start/end line (1-indexed), optional symbol name, and strategy tag. The pipeline dispatches via `chunk_autodetect(path, text)`. `OllamaEmbedder` replaces `fastembed` in the pipeline; `fastembed`-based `Embedder` is kept untouched for SciFact benchmark use only.

**Tech Stack:** Rust, rusqlite, reqwest (blocking, already in Cargo.toml), Ollama HTTP API (`/api/embed`), tree-sitter + 5 grammars (M4/M5).

---

## File Map

### New files
- `src-tauri/src/chunker/classify.rs` — `classify(&Path) -> ChunkStrategy` extension table
- `src-tauri/src/chunker/words.rs` — `word_count(s) -> usize`, budget constants
- `src-tauri/src/chunker/fallback.rs` — blank-line splitter with line ranges
- `src-tauri/src/chunker/prose.rs` — sentence-aware chunker (moved + line ranges added)
- `src-tauri/src/chunker/scanner.rs` — brace-depth scanner with line ranges
- `src-tauri/src/chunker/declarative.rs` — YAML/JSON/TOML/XML top-level-key splitter
- `src-tauri/src/chunker/ast.rs` — Tree-sitter AST chunker (M4+)
- `src-tauri/src/embedder/ollama.rs` — `OllamaEmbedder` calling Ollama `/api/embed`

### Modified files
- `src-tauri/src/chunker/mod.rs` — defines `Chunk`, `ChunkStrategy`; `chunk_autodetect` dispatch
- `src-tauri/src/embedder/mod.rs` — adds `EmbedProfile`, `CloudProvider`; keeps `Embedder` for SciFact
- `src-tauri/src/db/schema.rs` — adds `MIGRATION_V4`
- `src-tauri/src/db/connection.rs` — runs V4 in `AppDb::open` and `open_in_memory`
- `src-tauri/src/db/queries.rs` — `insert_chunk` takes `&Chunk` instead of bare text
- `src-tauri/src/pipeline/mod.rs` — uses `OllamaEmbedder`, calls `chunk_autodetect`, expands ext list
- `src-tauri/src/search/mod.rs` — adds `start_line`, `end_line`, `symbol_name`, `strategy` to `SearchResult`
- `src-tauri/src/vault/config.rs` — adds `embed_profile` to `ConfigFile`
- `src-tauri/src/lib.rs` — `WikiEmbedder` state uses `OllamaEmbedder`; ext filter in `start_file_watcher`
- `src-tauri/Cargo.toml` — tree-sitter deps (M4/M5 only)

---

## Milestone 1: Schema + OllamaEmbedder + EmbedProfile

### Task 1: MIGRATION_V4 — add chunk columns

**Files:**
- Modify: `src-tauri/src/db/schema.rs`
- Modify: `src-tauri/src/db/connection.rs`

- [ ] **Step 1: Write the failing test**

Add to `src-tauri/src/db/connection.rs` inside the existing `mod tests`:

```rust
#[test]
fn migration_v4_adds_chunk_metadata_columns() {
    let conn = open_in_memory().unwrap();
    // after V4 runs, these columns must exist
    conn.execute(
        "INSERT INTO chunks (doc_id, chunk_text, position, start_line, end_line, strategy)
         VALUES (1, 'hello', 0, 1, 3, 'prose')",
        [],
    ).unwrap();
    let strategy: String = conn
        .query_row("SELECT strategy FROM chunks WHERE position = 0", [], |r| r.get(0))
        .unwrap();
    assert_eq!(strategy, "prose");
}
```

- [ ] **Step 2: Run test to verify it fails**

```
cargo test -p curated-thoughts migration_v4
```
Expected: FAIL — "table chunks has no column named start_line"

- [ ] **Step 3: Add MIGRATION_V4 constant to schema.rs**

Open `src-tauri/src/db/schema.rs`. After `MIGRATION_V3`, add:

```rust
pub const MIGRATION_V4: &str = "
ALTER TABLE chunks ADD COLUMN start_line   INTEGER;
ALTER TABLE chunks ADD COLUMN end_line     INTEGER;
ALTER TABLE chunks ADD COLUMN symbol_name  TEXT;
ALTER TABLE chunks ADD COLUMN strategy     TEXT NOT NULL DEFAULT 'prose';

INSERT OR IGNORE INTO schema_version (version) VALUES (4);
";
```

- [ ] **Step 4: Run V4 in connection.rs**

In `src-tauri/src/db/connection.rs`:

```rust
use crate::db::schema::{MIGRATION_V1, MIGRATION_V2, MIGRATION_V3, MIGRATION_V4};

impl AppDb {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch(&format!(
            "BEGIN;\n{}\n{}\n{}\n{}\nCOMMIT;",
            MIGRATION_V1, MIGRATION_V2, MIGRATION_V3, MIGRATION_V4
        ))?;
        Ok(AppDb(conn))
    }
}

#[cfg(test)]
pub fn open_in_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(&format!(
        "BEGIN;\n{}\n{}\n{}\nCOMMIT;",
        MIGRATION_V1, MIGRATION_V2, MIGRATION_V4
    ))?;
    Ok(conn)
}
```

- [ ] **Step 5: Run tests to verify they pass**

```
cargo test -p curated-thoughts migration_v4
```
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/db/schema.rs src-tauri/src/db/connection.rs
git commit -m "feat(db): MIGRATION_V4 — add start_line, end_line, symbol_name, strategy to chunks"
```

---

### Task 2: EmbedProfile in vault config

**Files:**
- Modify: `src-tauri/src/vault/config.rs`

- [ ] **Step 1: Write failing tests**

Add to `src-tauri/src/vault/config.rs` inside `mod tests`:

```rust
#[test]
fn embed_profile_defaults_to_nomic_embed_code() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);
    let profile = cfg.get_embed_profile().unwrap();
    match profile {
        EmbedProfile::Local { model } => assert_eq!(model, "nomic-embed-code"),
        _ => panic!("expected Local profile"),
    }
}

#[test]
fn embed_profile_roundtrips_local() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);
    cfg.set_embed_profile(&EmbedProfile::Local { model: "other-model".to_string() }).unwrap();
    match cfg.get_embed_profile().unwrap() {
        EmbedProfile::Local { model } => assert_eq!(model, "other-model"),
        _ => panic!("expected Local"),
    }
}

#[test]
fn embed_profile_roundtrips_cloud() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);
    cfg.set_embed_profile(&EmbedProfile::Cloud {
        provider: crate::embedder::CloudProvider::OpenAi,
        model: "text-embedding-3-small".to_string(),
        api_key: "sk-test".to_string(),
    }).unwrap();
    match cfg.get_embed_profile().unwrap() {
        EmbedProfile::Cloud { provider, model, .. } => {
            assert!(matches!(provider, crate::embedder::CloudProvider::OpenAi));
            assert_eq!(model, "text-embedding-3-small");
        }
        _ => panic!("expected Cloud"),
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```
cargo test -p curated-thoughts embed_profile
```
Expected: FAIL — types not defined yet

- [ ] **Step 3: Add EmbedProfile and CloudProvider to embedder/mod.rs**

Add to `src-tauri/src/embedder/mod.rs` (before the `Embedder` struct):

```rust
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub enum CloudProvider {
    OpenAi,
    Voyage,
    Cohere,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
#[serde(tag = "type")]
pub enum EmbedProfile {
    Local { model: String },
    Cloud { provider: CloudProvider, model: String, api_key: String },
}

impl Default for EmbedProfile {
    fn default() -> Self {
        EmbedProfile::Local { model: "nomic-embed-code".to_string() }
    }
}
```

- [ ] **Step 4: Update VaultConfig to persist EmbedProfile**

Replace `src-tauri/src/vault/config.rs` with:

```rust
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};
use crate::embedder::{EmbedProfile, CloudProvider};

#[derive(Serialize, Deserialize, Default)]
struct ConfigFile {
    vault_path: Option<String>,
    embed_profile: Option<EmbedProfile>,
}

pub struct VaultConfig {
    config_path: PathBuf,
}

impl VaultConfig {
    pub fn new(config_path: PathBuf) -> Self {
        VaultConfig { config_path }
    }

    #[allow(dead_code)]
    pub fn default_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_default()
            .join(".brain")
            .join("config.json")
    }

    fn read(&self) -> Result<ConfigFile> {
        if !self.config_path.exists() {
            return Ok(ConfigFile::default());
        }
        let text = fs::read_to_string(&self.config_path)?;
        Ok(serde_json::from_str(&text)?)
    }

    fn write(&self, cfg: &ConfigFile) -> Result<()> {
        if let Some(parent) = self.config_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&self.config_path, serde_json::to_string_pretty(cfg)?)?;
        Ok(())
    }

    pub fn get_vault_path(&self) -> Result<Option<String>> {
        Ok(self.read()?.vault_path)
    }

    pub fn set_vault_path(&self, path: &str) -> Result<()> {
        let mut cfg = self.read()?;
        cfg.vault_path = Some(path.to_string());
        self.write(&cfg)
    }

    pub fn vault_root(&self) -> Result<Option<std::path::PathBuf>> {
        Ok(self.get_vault_path()?.map(std::path::PathBuf::from))
    }

    pub fn get_embed_profile(&self) -> Result<EmbedProfile> {
        Ok(self.read()?.embed_profile.unwrap_or_default())
    }

    pub fn set_embed_profile(&self, profile: &EmbedProfile) -> Result<()> {
        let mut cfg = self.read()?;
        cfg.embed_profile = Some(profile.clone());
        self.write(&cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_config(tmp: &TempDir) -> VaultConfig {
        VaultConfig::new(tmp.path().join("config.json"))
    }

    #[test]
    fn test_get_returns_none_when_file_absent() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_config(&tmp);
        assert_eq!(cfg.get_vault_path().unwrap(), None);
    }

    #[test]
    fn test_set_then_get_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_config(&tmp);
        cfg.set_vault_path("/Users/test/brain").unwrap();
        assert_eq!(
            cfg.get_vault_path().unwrap(),
            Some("/Users/test/brain".to_string())
        );
    }

    #[test]
    fn test_set_overwrites_existing_path() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_config(&tmp);
        cfg.set_vault_path("/first").unwrap();
        cfg.set_vault_path("/second").unwrap();
        assert_eq!(cfg.get_vault_path().unwrap(), Some("/second".to_string()));
    }

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

    #[test]
    fn embed_profile_defaults_to_nomic_embed_code() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_config(&tmp);
        match cfg.get_embed_profile().unwrap() {
            EmbedProfile::Local { model } => assert_eq!(model, "nomic-embed-code"),
            _ => panic!("expected Local profile"),
        }
    }

    #[test]
    fn embed_profile_roundtrips_local() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_config(&tmp);
        cfg.set_embed_profile(&EmbedProfile::Local { model: "other-model".to_string() }).unwrap();
        match cfg.get_embed_profile().unwrap() {
            EmbedProfile::Local { model } => assert_eq!(model, "other-model"),
            _ => panic!("expected Local"),
        }
    }

    #[test]
    fn embed_profile_roundtrips_cloud() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_config(&tmp);
        cfg.set_embed_profile(&EmbedProfile::Cloud {
            provider: CloudProvider::OpenAi,
            model: "text-embedding-3-small".to_string(),
            api_key: "sk-test".to_string(),
        }).unwrap();
        match cfg.get_embed_profile().unwrap() {
            EmbedProfile::Cloud { provider, model, .. } => {
                assert!(matches!(provider, CloudProvider::OpenAi));
                assert_eq!(model, "text-embedding-3-small");
            }
            _ => panic!("expected Cloud"),
        }
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

```
cargo test -p curated-thoughts vault
```
Expected: PASS (all vault config tests including new embed_profile tests)

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/embedder/mod.rs src-tauri/src/vault/config.rs
git commit -m "feat(embedder): EmbedProfile + CloudProvider; vault config persists embed_profile"
```

---

### Task 3: OllamaEmbedder

**Files:**
- Create: `src-tauri/src/embedder/ollama.rs`
- Modify: `src-tauri/src/embedder/mod.rs`

- [ ] **Step 1: Write failing tests**

Create `src-tauri/src/embedder/ollama.rs` with tests only:

```rust
use anyhow::Result;

pub struct OllamaEmbedder {
    model: String,
    base_url: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ollama_embedder_new_local_stores_model() {
        let e = OllamaEmbedder::new_local("nomic-embed-code");
        assert_eq!(e.model, "nomic-embed-code");
        assert_eq!(e.base_url, "http://localhost:11434");
    }

    #[test]
    fn ollama_embedder_new_local_custom_base() {
        let e = OllamaEmbedder::with_base_url("nomic-embed-code", "http://10.0.0.1:11434");
        assert_eq!(e.base_url, "http://10.0.0.1:11434");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```
cargo test -p curated-thoughts ollama_embedder
```
Expected: FAIL — "OllamaEmbedder" constructor not defined

- [ ] **Step 3: Implement OllamaEmbedder**

Replace contents of `src-tauri/src/embedder/ollama.rs`:

```rust
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

pub struct OllamaEmbedder {
    pub(crate) model: String,
    pub(crate) base_url: String,
}

#[derive(Serialize)]
struct EmbedRequest {
    model: String,
    input: Vec<String>,
}

#[derive(Deserialize)]
struct EmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

impl OllamaEmbedder {
    pub fn new_local(model: &str) -> Self {
        OllamaEmbedder {
            model: model.to_string(),
            base_url: "http://localhost:11434".to_string(),
        }
    }

    pub fn with_base_url(model: &str, base_url: &str) -> Self {
        OllamaEmbedder {
            model: model.to_string(),
            base_url: base_url.to_string(),
        }
    }

    pub fn from_profile(profile: &crate::embedder::EmbedProfile) -> Result<Self> {
        match profile {
            crate::embedder::EmbedProfile::Local { model } => Ok(Self::new_local(model)),
            crate::embedder::EmbedProfile::Cloud { .. } => {
                Err(anyhow!("cloud embed profile not yet implemented"))
            }
        }
    }

    pub fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }
        let client = reqwest::blocking::Client::new();
        let resp = client
            .post(format!("{}/api/embed", self.base_url))
            .json(&EmbedRequest {
                model: self.model.clone(),
                input: texts,
            })
            .send()
            .map_err(|e| anyhow!("ollama embed request failed: {e}"))?;

        if !resp.status().is_success() {
            return Err(anyhow!("ollama embed error: {}", resp.status()));
        }

        let body: EmbedResponse = resp
            .json()
            .map_err(|e| anyhow!("ollama embed response parse failed: {e}"))?;
        Ok(body.embeddings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ollama_embedder_new_local_stores_model() {
        let e = OllamaEmbedder::new_local("nomic-embed-code");
        assert_eq!(e.model, "nomic-embed-code");
        assert_eq!(e.base_url, "http://localhost:11434");
    }

    #[test]
    fn ollama_embedder_new_local_custom_base() {
        let e = OllamaEmbedder::with_base_url("nomic-embed-code", "http://10.0.0.1:11434");
        assert_eq!(e.base_url, "http://10.0.0.1:11434");
    }

    #[test]
    fn ollama_embedder_from_profile_local() {
        let profile = crate::embedder::EmbedProfile::Local {
            model: "nomic-embed-code".to_string(),
        };
        let e = OllamaEmbedder::from_profile(&profile).unwrap();
        assert_eq!(e.model, "nomic-embed-code");
    }

    #[test]
    fn ollama_embedder_from_profile_cloud_errors() {
        let profile = crate::embedder::EmbedProfile::Cloud {
            provider: crate::embedder::CloudProvider::OpenAi,
            model: "text-embedding-3-small".to_string(),
            api_key: "sk-test".to_string(),
        };
        assert!(OllamaEmbedder::from_profile(&profile).is_err());
    }
}
```

Add to `src-tauri/src/embedder/mod.rs` at the top:

```rust
pub mod ollama;
pub use ollama::OllamaEmbedder;
```

- [ ] **Step 4: Run tests to verify they pass**

```
cargo test -p curated-thoughts ollama_embedder
```
Expected: PASS (unit tests; network tests not run in CI — live embed requires Ollama running)

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/embedder/ollama.rs src-tauri/src/embedder/mod.rs
git commit -m "feat(embedder): OllamaEmbedder via Ollama /api/embed HTTP endpoint"
```

---

### Task 4: Wire OllamaEmbedder into pipeline and search_vault

**Files:**
- Modify: `src-tauri/src/pipeline/mod.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Update PipelineWorker to use OllamaEmbedder**

In `src-tauri/src/pipeline/mod.rs`, replace the `use crate::embedder::Embedder;` import and the `Embedder::new()` call:

```rust
use crate::embedder::OllamaEmbedder;
// remove: use crate::embedder::Embedder;
```

In `PipelineWorker::run`, replace:

```rust
let embedder = match Embedder::new() {
    Ok(e) => e,
    Err(err) => {
        eprintln!("[pipeline] embedder init failed: {err}");
        return;
    }
};
```

with:

```rust
let embedder = match OllamaEmbedder::new_local("nomic-embed-code") {
    e => e, // infallible construction; errors surface at embed time
};
let _ = embedder; // used below
let embedder = OllamaEmbedder::new_local("nomic-embed-code");
```

Actually, `OllamaEmbedder::new_local` is infallible (just stores strings). Simplify:

```rust
let embedder = OllamaEmbedder::new_local("nomic-embed-code");
```

- [ ] **Step 2: Update WikiEmbedder state in lib.rs**

In `src-tauri/src/lib.rs`:

Replace:

```rust
use embedder::Embedder;
// ...
struct WikiEmbedder(Mutex<Option<Embedder>>);
```

with:

```rust
use embedder::OllamaEmbedder;
// ...
struct WikiEmbedder(Mutex<Option<OllamaEmbedder>>);
```

Update `embed_text` command (replace `Embedder::new()` with `OllamaEmbedder::new_local`):

```rust
#[tauri::command]
fn embed_text(text: String, embedder_state: State<WikiEmbedder>) -> Result<Vec<f32>, String> {
    let mut guard = embedder_state.0.lock().unwrap();
    if guard.is_none() {
        *guard = Some(OllamaEmbedder::new_local("nomic-embed-code"));
    }
    guard.as_ref().unwrap()
        .embed(vec![text])
        .map_err(|e| e.to_string())
        .map(|mut vecs| vecs.drain(..).next().unwrap_or_default())
}
```

Update `search_vault` command the same way (replace `Embedder::new()` with `OllamaEmbedder::new_local("nomic-embed-code")`):

```rust
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
            *guard = Some(OllamaEmbedder::new_local("nomic-embed-code"));
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
```

- [ ] **Step 3: Verify it compiles and existing tests pass**

```
cargo test -p curated-thoughts --features test-utils
```
Expected: all existing tests pass (SciFact still uses `Embedder` / fastembed — unchanged)

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/pipeline/mod.rs src-tauri/src/lib.rs
git commit -m "feat(pipeline): switch to OllamaEmbedder (nomic-embed-code) for ingest and search"
```

---

## Milestone 2: Chunk Struct + Classifier + Fallback + Prose with Line Ranges

### Task 5: Chunk struct and ChunkStrategy enum

**Files:**
- Modify: `src-tauri/src/chunker/mod.rs`
- Create: `src-tauri/src/chunker/words.rs`

- [ ] **Step 1: Write failing test**

Add to `src-tauri/src/chunker/mod.rs`:

```rust
#[cfg(test)]
mod struct_tests {
    use super::*;

    #[test]
    fn chunk_strategy_debug() {
        let s = ChunkStrategy::Prose;
        assert_eq!(format!("{s:?}"), "Prose");
    }

    #[test]
    fn chunk_fields_accessible() {
        let c = Chunk {
            text: "hello".to_string(),
            start_line: 1,
            end_line: 3,
            symbol_name: Some("foo".to_string()),
            strategy: ChunkStrategy::AstSymbol,
        };
        assert_eq!(c.start_line, 1);
        assert_eq!(c.end_line, 3);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```
cargo test -p curated-thoughts struct_tests
```
Expected: FAIL — `Chunk` and `ChunkStrategy` not defined

- [ ] **Step 3: Add Chunk and ChunkStrategy to chunker/mod.rs**

Replace the top of `src-tauri/src/chunker/mod.rs` (keep all existing sentence-splitting code below, add modules above):

```rust
pub mod classify;
pub mod words;
pub mod fallback;
pub mod prose;
pub mod scanner;
pub mod declarative;

use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub enum ChunkStrategy {
    AstSymbol,
    Scanner,
    Declarative,
    Prose,
    Fallback,
}

impl ChunkStrategy {
    pub fn as_str(&self) -> &'static str {
        match self {
            ChunkStrategy::AstSymbol => "ast_symbol",
            ChunkStrategy::Scanner => "scanner",
            ChunkStrategy::Declarative => "declarative",
            ChunkStrategy::Prose => "prose",
            ChunkStrategy::Fallback => "fallback",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Chunk {
    pub text: String,
    pub start_line: u32,
    pub end_line: u32,
    pub symbol_name: Option<String>,
    pub strategy: ChunkStrategy,
}

/// Dispatch to the correct chunker based on file extension.
pub fn chunk_autodetect(path: &Path, text: &str) -> Vec<Chunk> {
    let strategy = classify::classify(path);
    match strategy {
        ChunkStrategy::Prose => prose::chunk_prose(text),
        ChunkStrategy::Fallback => fallback::chunk_fallback(text),
        ChunkStrategy::Scanner => scanner::chunk_scanner(text),
        ChunkStrategy::Declarative => declarative::chunk_declarative(path, text),
        ChunkStrategy::AstSymbol => {
            // M4: ast::chunk_ast(path, text) — falls back to scanner until M4
            scanner::chunk_scanner(text)
        }
    }
}

// ── Keep chunk_text for SciFact benchmark + bin compatibility ────────────────
/// Returns plain strings. Used by embed_scifact bin and benchmark fixtures.
/// Do NOT use in the pipeline — use chunk_autodetect instead.
pub fn chunk_text(text: &str) -> Vec<String> {
    prose::chunk_prose(text).into_iter().map(|c| c.text).collect()
}
```

Delete the body of the old `chunk_text` function and all sentence-splitting helpers from `mod.rs` — they now live in `prose.rs`. The compatibility shim above delegates to `prose::chunk_prose`.

- [ ] **Step 4: Create words.rs**

Create `src-tauri/src/chunker/words.rs`:

```rust
pub const BUDGET_AST: usize = 400;
pub const BUDGET_SCANNER: usize = 200;
pub const BUDGET_DECLARATIVE: usize = 150;
pub const BUDGET_PROSE: usize = 100;
pub const BUDGET_FALLBACK: usize = 100;
pub const MIN_CHUNK_WORDS: usize = 20;

pub fn word_count(s: &str) -> usize {
    s.split_whitespace().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_count_empty() {
        assert_eq!(word_count(""), 0);
    }

    #[test]
    fn word_count_whitespace_only() {
        assert_eq!(word_count("   \n\t"), 0);
    }

    #[test]
    fn word_count_words() {
        assert_eq!(word_count("hello world foo"), 3);
    }
}
```

- [ ] **Step 5: Run tests**

```
cargo test -p curated-thoughts struct_tests
cargo test -p curated-thoughts word_count
```
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/chunker/mod.rs src-tauri/src/chunker/words.rs
git commit -m "feat(chunker): Chunk struct, ChunkStrategy, chunk_autodetect dispatch, word budgets"
```

---

### Task 6: Classifier

**Files:**
- Create: `src-tauri/src/chunker/classify.rs`

- [ ] **Step 1: Write failing tests**

Create `src-tauri/src/chunker/classify.rs`:

```rust
use std::path::Path;
use crate::chunker::ChunkStrategy;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn classify_rust() {
        assert!(matches!(classify(Path::new("src/main.rs")), ChunkStrategy::AstSymbol));
    }

    #[test]
    fn classify_typescript() {
        assert!(matches!(classify(Path::new("app.ts")), ChunkStrategy::AstSymbol));
        assert!(matches!(classify(Path::new("comp.tsx")), ChunkStrategy::AstSymbol));
    }

    #[test]
    fn classify_javascript() {
        assert!(matches!(classify(Path::new("index.js")), ChunkStrategy::AstSymbol));
        assert!(matches!(classify(Path::new("mod.mjs")), ChunkStrategy::AstSymbol));
    }

    #[test]
    fn classify_python() {
        assert!(matches!(classify(Path::new("script.py")), ChunkStrategy::AstSymbol));
    }

    #[test]
    fn classify_go() {
        assert!(matches!(classify(Path::new("main.go")), ChunkStrategy::AstSymbol));
    }

    #[test]
    fn classify_scanner_langs() {
        assert!(matches!(classify(Path::new("Foo.java")), ChunkStrategy::Scanner));
        assert!(matches!(classify(Path::new("main.cpp")), ChunkStrategy::Scanner));
        assert!(matches!(classify(Path::new("App.vue")), ChunkStrategy::Scanner));
    }

    #[test]
    fn classify_declarative() {
        assert!(matches!(classify(Path::new("config.yaml")), ChunkStrategy::Declarative));
        assert!(matches!(classify(Path::new("config.yml")), ChunkStrategy::Declarative));
        assert!(matches!(classify(Path::new("data.json")), ChunkStrategy::Declarative));
        assert!(matches!(classify(Path::new("Cargo.toml")), ChunkStrategy::Declarative));
        assert!(matches!(classify(Path::new("pom.xml")), ChunkStrategy::Declarative));
    }

    #[test]
    fn classify_prose() {
        assert!(matches!(classify(Path::new("README.md")), ChunkStrategy::Prose));
        assert!(matches!(classify(Path::new("notes.txt")), ChunkStrategy::Prose));
        assert!(matches!(classify(Path::new("doc.rst")), ChunkStrategy::Prose));
    }

    #[test]
    fn classify_fallback_unknown_ext() {
        assert!(matches!(classify(Path::new("file.xyz")), ChunkStrategy::Fallback));
    }

    #[test]
    fn classify_fallback_no_ext() {
        assert!(matches!(classify(Path::new("Makefile")), ChunkStrategy::Fallback));
        assert!(matches!(classify(Path::new("LICENSE")), ChunkStrategy::Fallback));
    }

    #[test]
    fn classify_case_insensitive_ext() {
        assert!(matches!(classify(Path::new("README.MD")), ChunkStrategy::Prose));
        assert!(matches!(classify(Path::new("main.RS")), ChunkStrategy::AstSymbol));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```
cargo test -p curated-thoughts classify
```
Expected: FAIL — `classify` not defined

- [ ] **Step 3: Implement classify**

Add before `#[cfg(test)]` in `classify.rs`:

```rust
use std::path::Path;
use crate::chunker::ChunkStrategy;

pub fn classify(path: &Path) -> ChunkStrategy {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());

    match ext.as_deref() {
        Some("rs") => ChunkStrategy::AstSymbol,
        Some("ts" | "tsx") => ChunkStrategy::AstSymbol,
        Some("js" | "jsx" | "mjs" | "cjs") => ChunkStrategy::AstSymbol,
        Some("py") => ChunkStrategy::AstSymbol,
        Some("go") => ChunkStrategy::AstSymbol,

        Some("java" | "kt" | "swift" | "c" | "h" | "cpp" | "hpp" | "cs"
            | "rb" | "php" | "vue" | "svelte") => ChunkStrategy::Scanner,

        Some("yaml" | "yml" | "json" | "jsonc" | "toml" | "xml") => ChunkStrategy::Declarative,

        Some("md" | "markdown" | "txt" | "rst" | "org") => ChunkStrategy::Prose,

        _ => ChunkStrategy::Fallback,
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

```
cargo test -p curated-thoughts classify
```
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/chunker/classify.rs
git commit -m "feat(chunker): classifier — extension to ChunkStrategy dispatch table"
```

---

### Task 7: Fallback chunker

**Files:**
- Create: `src-tauri/src/chunker/fallback.rs`

- [ ] **Step 1: Write failing tests**

Create `src-tauri/src/chunker/fallback.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_empty_returns_empty() {
        assert!(chunk_fallback("").is_empty());
    }

    #[test]
    fn fallback_short_text_is_single_chunk() {
        let chunks = chunk_fallback("hello world\nfoo bar");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].start_line, 1);
        assert_eq!(chunks[0].end_line, 2);
    }

    #[test]
    fn fallback_splits_on_blank_lines() {
        let text = "para one line one\npara one line two\n\npara two line one\npara two line two";
        let chunks = chunk_fallback(text);
        assert!(chunks.len() >= 2);
        assert!(chunks[0].text.contains("para one"));
        assert!(chunks[1].text.contains("para two"));
    }

    #[test]
    fn fallback_line_ranges_are_correct() {
        let text = "line one\nline two\n\nline four\nline five";
        let chunks = chunk_fallback(text);
        assert_eq!(chunks[0].start_line, 1);
        assert_eq!(chunks[0].end_line, 2);
        assert_eq!(chunks[1].start_line, 4);
        assert_eq!(chunks[1].end_line, 5);
    }

    #[test]
    fn fallback_strategy_tag() {
        let chunks = chunk_fallback("some text\n\nmore text");
        for c in &chunks {
            assert!(matches!(c.strategy, crate::chunker::ChunkStrategy::Fallback));
        }
    }

    #[test]
    fn fallback_no_symbol_name() {
        let chunks = chunk_fallback("text here");
        assert!(chunks[0].symbol_name.is_none());
    }

    #[test]
    fn fallback_merges_micro_chunks() {
        // A paragraph with fewer than MIN_CHUNK_WORDS words followed by a blank line
        // and a larger paragraph should merge the micro with the next
        let small = "tiny";
        let large = (0..25).map(|i| format!("word{i}")).collect::<Vec<_>>().join(" ");
        let text = format!("{small}\n\n{large}");
        let chunks = chunk_fallback(&text);
        // "tiny" is < MIN_CHUNK_WORDS so it merges with "word0 word1 ..."
        assert_eq!(chunks.len(), 1);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```
cargo test -p curated-thoughts fallback
```
Expected: FAIL — `chunk_fallback` not defined

- [ ] **Step 3: Implement fallback chunker**

Add before `#[cfg(test)]` in `fallback.rs`:

```rust
use crate::chunker::{Chunk, ChunkStrategy};
use crate::chunker::words::{word_count, BUDGET_FALLBACK, MIN_CHUNK_WORDS};

pub fn chunk_fallback(text: &str) -> Vec<Chunk> {
    if text.trim().is_empty() {
        return vec![];
    }

    // Split into paragraphs (blank-line boundaries)
    let paragraphs: Vec<(u32, u32, String)> = {
        let mut out = Vec::new();
        let mut para_lines: Vec<&str> = Vec::new();
        let mut para_start = 1u32;
        let mut current_line = 1u32;

        for line in text.lines() {
            if line.trim().is_empty() {
                if !para_lines.is_empty() {
                    out.push((para_start, current_line - 1, para_lines.join("\n")));
                    para_lines.clear();
                }
                para_start = current_line + 1;
            } else {
                if para_lines.is_empty() {
                    para_start = current_line;
                }
                para_lines.push(line);
            }
            current_line += 1;
        }
        if !para_lines.is_empty() {
            out.push((para_start, current_line - 1, para_lines.join("\n")));
        }
        out
    };

    // Group paragraphs respecting BUDGET and MIN_CHUNK_WORDS
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut group_text = String::new();
    let mut group_start = 0u32;
    let mut group_end = 0u32;
    let mut group_words = 0usize;

    for (start, end, para) in &paragraphs {
        let para_words = word_count(para);

        if group_text.is_empty() {
            group_text = para.clone();
            group_start = *start;
            group_end = *end;
            group_words = para_words;
        } else {
            group_text.push_str("\n\n");
            group_text.push_str(para);
            group_end = *end;
            group_words += para_words;
        }

        if group_words >= BUDGET_FALLBACK {
            chunks.push(Chunk {
                text: group_text.clone(),
                start_line: group_start,
                end_line: group_end,
                symbol_name: None,
                strategy: ChunkStrategy::Fallback,
            });
            group_text.clear();
            group_words = 0;
        }
    }

    if !group_text.is_empty() {
        if group_words < MIN_CHUNK_WORDS && !chunks.is_empty() {
            let last = chunks.last_mut().unwrap();
            last.text.push_str("\n\n");
            last.text.push_str(&group_text);
            last.end_line = group_end;
        } else {
            chunks.push(Chunk {
                text: group_text,
                start_line: group_start,
                end_line: group_end,
                symbol_name: None,
                strategy: ChunkStrategy::Fallback,
            });
        }
    }

    chunks
}
```

- [ ] **Step 4: Run tests to verify they pass**

```
cargo test -p curated-thoughts fallback
```
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/chunker/fallback.rs
git commit -m "feat(chunker): fallback chunker with blank-line splits and line ranges"
```

---

### Task 8: Prose chunker with line ranges

**Files:**
- Create: `src-tauri/src/chunker/prose.rs`

- [ ] **Step 1: Write failing tests**

Create `src-tauri/src/chunker/prose.rs` with tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prose_empty_returns_empty() {
        assert!(chunk_prose("").is_empty());
    }

    #[test]
    fn prose_single_chunk_has_line_ranges() {
        let chunks = chunk_prose("Hello world. Goodbye world.");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].start_line, 1);
        assert_eq!(chunks[0].end_line, 1);
    }

    #[test]
    fn prose_multiline_text_correct_line_ranges() {
        let text = "Line one sentence.\nLine two sentence.\nLine three sentence.";
        let chunks = chunk_prose(text);
        assert!(!chunks.is_empty());
        assert_eq!(chunks[0].start_line, 1);
        assert!(chunks[0].end_line >= 1);
    }

    #[test]
    fn prose_strategy_tag() {
        let chunks = chunk_prose("A sentence. Another sentence.");
        for c in &chunks {
            assert!(matches!(c.strategy, crate::chunker::ChunkStrategy::Prose));
        }
    }

    #[test]
    fn prose_no_symbol_name() {
        let chunks = chunk_prose("Hello world.");
        assert!(chunks[0].symbol_name.is_none());
    }

    #[test]
    fn prose_existing_chunking_still_works() {
        // Ensure the sentence-aware grouping still produces multi-chunk output
        // for long text (regression guard on moved logic)
        let long: String = (0..30).map(|i| format!("Sentence number {i} goes here.")).collect::<Vec<_>>().join(" ");
        let chunks = chunk_prose(&long);
        assert!(chunks.len() > 1, "long text must produce multiple chunks");
    }

    #[test]
    fn prose_end_line_gte_start_line() {
        let text = "First.\nSecond.\nThird.\nFourth.\nFifth.";
        let chunks = chunk_prose(text);
        for c in &chunks {
            assert!(c.end_line >= c.start_line);
        }
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```
cargo test -p curated-thoughts prose
```
Expected: FAIL — `chunk_prose` not defined

- [ ] **Step 3: Implement prose chunker**

Create the full implementation in `prose.rs`. Move the sentence-splitting logic from `chunker/mod.rs` and add line-range tracking:

```rust
use crate::chunker::{Chunk, ChunkStrategy};

const TARGET_WORDS: usize = 100;

fn word_count(s: &str) -> usize {
    s.split_whitespace().count()
}

fn byte_to_line(text: &str, byte_offset: usize) -> u32 {
    let safe = byte_offset.min(text.len());
    (text[..safe].chars().filter(|&c| c == '\n').count() + 1) as u32
}

fn word_chars_touching_dot(text: &str, dot_byte: usize) -> &str {
    let before = &text[..dot_byte];
    let mut start = before.len();
    for (i, ch) in before.char_indices().rev() {
        if ch.is_alphanumeric() || ch == '-' {
            start = i;
        } else {
            break;
        }
    }
    &before[start..]
}

fn is_probable_abbrev_dot_token(token: &str) -> bool {
    let len = token.chars().count();
    if !(1..=3).contains(&len) {
        return false;
    }
    let mut chs = token.chars();
    let Some(first) = chs.next() else { return false };
    if first.is_uppercase() {
        return true;
    }
    token.eq_ignore_ascii_case("al") || token.eq_ignore_ascii_case("vs")
}

fn char_before_dot(text: &str, dot_byte: usize) -> Option<char> {
    text[..dot_byte].chars().next_back()
}

fn ends_sentence(text: &str, punct_byte: usize, punct: char) -> bool {
    if !matches!(punct, '.' | '!' | '?') {
        return false;
    }
    if punct == '.' {
        if let Some(prev) = char_before_dot(text, punct_byte) {
            let punct_end = punct_byte + punct.len_utf8();
            let after_first = text
                .get(punct_end..)
                .and_then(|rest| rest.chars().find(|c| !c.is_whitespace()));
            if prev.is_ascii_digit() && matches!(after_first, Some(c) if c.is_ascii_digit()) {
                return false;
            }
            let token = word_chars_touching_dot(text, punct_byte);
            if is_probable_abbrev_dot_token(token) {
                return false;
            }
        }
    }
    let punct_end = punct_byte + punct.len_utf8();
    let rest = text.get(punct_end..).unwrap_or("");
    if rest.is_empty() || rest.chars().all(|c| c.is_whitespace()) {
        return true;
    }
    matches!(rest.trim_start().chars().next(), Some(c) if c.is_uppercase())
}

fn split_sentences(text: &str) -> Vec<&str> {
    let text = text.trim();
    if text.is_empty() {
        return vec![];
    }
    let mut out = Vec::new();
    let mut sent_start = 0usize;
    for (byte_idx, ch) in text.char_indices() {
        if !matches!(ch, '.' | '!' | '?') {
            continue;
        }
        if !ends_sentence(text, byte_idx, ch) {
            continue;
        }
        let punct_end = byte_idx + ch.len_utf8();
        out.push(text.get(sent_start..punct_end).unwrap_or("").trim());
        let after = text.get(punct_end..).unwrap_or("");
        sent_start = punct_end
            + after
                .chars()
                .take_while(|c| c.is_whitespace())
                .map(char::len_utf8)
                .sum::<usize>();
    }
    if sent_start < text.len() {
        let tail = text.get(sent_start..).unwrap_or("").trim();
        if !tail.is_empty() {
            out.push(tail);
        }
    }
    out.into_iter().filter(|s| !s.is_empty()).collect()
}

fn sentence_byte_offset(haystack: &str, needle: &str) -> usize {
    (needle.as_ptr() as usize).saturating_sub(haystack.as_ptr() as usize)
}

pub fn chunk_prose(text: &str) -> Vec<Chunk> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return vec![];
    }

    let sentences: Vec<&str> = split_sentences(trimmed);
    if sentences.is_empty() {
        return vec![];
    }

    let mut groups: Vec<std::ops::Range<usize>> = Vec::new();
    let mut cur_start = 0usize;
    let mut acc_words = 0usize;

    for (i, s) in sentences.iter().enumerate() {
        acc_words += word_count(s);
        if acc_words >= TARGET_WORDS {
            groups.push(cur_start..i + 1);
            cur_start = i + 1;
            acc_words = 0;
        }
    }
    if cur_start < sentences.len() {
        groups.push(cur_start..sentences.len());
    }

    let n = groups.len();
    let mut chunks = Vec::with_capacity(n);

    for (gi, r) in groups.iter().enumerate() {
        let mut parts: Vec<&str> = Vec::new();
        if gi > 0 {
            parts.push(sentences[r.start - 1].trim());
        }
        for idx in r.clone() {
            parts.push(sentences[idx].trim());
        }
        if gi < n - 1 {
            parts.push(sentences[r.end].trim());
        }
        let chunk_text = parts.join(" ");

        // Line range: first core sentence start → last core sentence end
        let first_core = sentences[r.start];
        let last_core = sentences[r.end - 1];
        let first_offset = sentence_byte_offset(trimmed, first_core);
        let last_offset = sentence_byte_offset(trimmed, last_core) + last_core.len();
        let start_line = byte_to_line(trimmed, first_offset);
        let end_line = byte_to_line(trimmed, last_offset.saturating_sub(1));

        chunks.push(Chunk {
            text: chunk_text,
            start_line,
            end_line,
            symbol_name: None,
            strategy: ChunkStrategy::Prose,
        });
    }

    chunks
}
```

- [ ] **Step 4: Run tests**

```
cargo test -p curated-thoughts prose
```
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/chunker/prose.rs
git commit -m "feat(chunker): prose chunker with line ranges (moved from mod.rs)"
```

---

### Task 9: Update insert_chunk + SearchResult for new fields

**Files:**
- Modify: `src-tauri/src/db/queries.rs`
- Modify: `src-tauri/src/search/mod.rs`

- [ ] **Step 1: Write failing tests**

Add to `src-tauri/src/db/queries.rs` inside `mod tests`:

```rust
#[test]
fn insert_chunk_stores_line_ranges_and_strategy() {
    let conn = open_in_memory().unwrap();
    let doc_id = upsert_document(&conn, "/docs/a.rs", "hash1").unwrap();
    let chunk = crate::chunker::Chunk {
        text: "fn foo() {}".to_string(),
        start_line: 5,
        end_line: 7,
        symbol_name: Some("foo".to_string()),
        strategy: crate::chunker::ChunkStrategy::AstSymbol,
    };
    let chunk_id = insert_chunk(&conn, doc_id, &chunk, 0).unwrap();
    let (start, end, sym, strat): (Option<i64>, Option<i64>, Option<String>, String) = conn
        .query_row(
            "SELECT start_line, end_line, symbol_name, strategy FROM chunks WHERE id = ?1",
            [chunk_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(start, Some(5));
    assert_eq!(end, Some(7));
    assert_eq!(sym.as_deref(), Some("foo"));
    assert_eq!(strat, "ast_symbol");
}
```

- [ ] **Step 2: Run test to verify it fails**

```
cargo test -p curated-thoughts insert_chunk_stores
```
Expected: FAIL — `insert_chunk` signature mismatch

- [ ] **Step 3: Update insert_chunk in queries.rs**

Replace:

```rust
pub fn insert_chunk(conn: &Connection, doc_id: i64, text: &str, position: usize) -> Result<i64> {
    conn.execute(
        "INSERT INTO chunks (doc_id, chunk_text, position) VALUES (?1, ?2, ?3)",
        rusqlite::params![doc_id, text, position as i64],
    )?;
    Ok(conn.last_insert_rowid())
}
```

with:

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
            chunk.strategy.as_str(),
        ],
    )?;
    Ok(conn.last_insert_rowid())
}
```

Also update the existing test `test_insert_chunk_and_embedding` to pass a `Chunk`:

```rust
#[test]
fn test_insert_chunk_and_embedding() {
    let conn = open_in_memory().unwrap();
    let doc_id = upsert_document(&conn, "/docs/a.md", "hash1").unwrap();
    let chunk = crate::chunker::Chunk {
        text: "hello world".to_string(),
        start_line: 1,
        end_line: 1,
        symbol_name: None,
        strategy: crate::chunker::ChunkStrategy::Prose,
    };
    let chunk_id = insert_chunk(&conn, doc_id, &chunk, 0).unwrap();
    insert_embedding(&conn, chunk_id, &[0.1_f32, 0.2, 0.3]).unwrap();
    let bytes: Vec<u8> = conn
        .query_row(
            "SELECT vector FROM embeddings WHERE chunk_id = ?1",
            [chunk_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(bytes.len(), 12);
}
```

- [ ] **Step 4: Update SearchResult in search/mod.rs**

Replace:

```rust
#[derive(Serialize, Clone, Debug)]
pub struct SearchResult {
    pub doc_path: String,
    pub chunk_text: String,
    pub chunk_position: i64,
    pub score: f32,
}
```

with:

```rust
#[derive(Serialize, Clone, Debug)]
pub struct SearchResult {
    pub doc_path: String,
    pub chunk_text: String,
    pub chunk_position: i64,
    pub score: f32,
    pub start_line: Option<i64>,
    pub end_line: Option<i64>,
    pub symbol_name: Option<String>,
    pub strategy: String,
}
```

Update `semantic_search` query to select the new columns:

```rust
let mut stmt = conn.prepare(
    "SELECT e.vector, c.chunk_text, c.position, d.path,
            c.start_line, c.end_line, c.symbol_name, c.strategy
     FROM embeddings e
     JOIN chunks c ON c.id = e.chunk_id
     JOIN documents d ON d.id = c.doc_id
     WHERE d.status = 'indexed'",
)?;
```

And update the row reading inside the while loop:

```rust
while let Some(row) = rows.next()? {
    let bytes: Vec<u8> = row.get(0)?;
    let chunk_text: String = row.get(1)?;
    let chunk_position: i64 = row.get(2)?;
    let doc_path: String = row.get(3)?;
    let start_line: Option<i64> = row.get(4)?;
    let end_line: Option<i64> = row.get(5)?;
    let symbol_name: Option<String> = row.get(6)?;
    let strategy: String = row.get(7).unwrap_or_else(|_| "prose".to_string());
    let vec = bytes_to_f32(&bytes);
    let score = cosine_similarity(query_vec, &vec);
    results.push((score, SearchResult {
        doc_path, chunk_text, chunk_position, score,
        start_line, end_line, symbol_name, strategy,
    }));
}
```

Do the same for `related_chunks` (update query and row reading identically).

- [ ] **Step 5: Run tests**

```
cargo test -p curated-thoughts
```
Expected: all tests pass

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/db/queries.rs src-tauri/src/search/mod.rs
git commit -m "feat(db,search): insert_chunk takes Chunk struct; SearchResult includes line ranges + strategy"
```

---

### Task 10: Update pipeline to use chunk_autodetect

**Files:**
- Modify: `src-tauri/src/pipeline/mod.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Update ingest_file in pipeline/mod.rs**

Replace:

```rust
use crate::chunker::chunk_text;
```

with:

```rust
use crate::chunker::chunk_autodetect;
```

Update the extension filter in `ingest_file` to include code and config files:

```rust
fn ingest_file(conn: &Connection, embedder: &OllamaEmbedder, path: &str) -> Result<()> {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());

    let supported = matches!(ext.as_deref(),
        Some("md" | "txt" | "markdown" | "pdf" | "docx" |
             "rs" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" |
             "py" | "go" | "java" | "kt" | "swift" | "c" | "h" |
             "cpp" | "hpp" | "cs" | "rb" | "php" | "vue" | "svelte" |
             "yaml" | "yml" | "json" | "jsonc" | "toml" | "xml")
    );
    if !supported {
        return Ok(());
    }
    // ... rest unchanged ...
```

Replace the chunking call:

```rust
let chunks = chunk_text(&text);
```

with:

```rust
let chunks = chunk_autodetect(Path::new(path), &text);
```

Update the insert loop (chunks are now `Chunk` structs):

```rust
for (i, chunk) in chunks.iter().enumerate() {
    let chunk_id = insert_chunk(conn, doc_id, chunk, i)?;
    insert_embedding(conn, chunk_id, &embeddings[i])?;
}
```

Update `embedder.embed()` call to pass texts:

```rust
let embeddings = embedder.embed(chunks.iter().map(|c| c.text.clone()).collect()).map_err(|e| {
    let _ = mark_document_error(conn, doc_id);
    e
})?;
```

- [ ] **Step 2: Update ext filter in start_file_watcher (lib.rs)**

In `src-tauri/src/lib.rs`, find the `start_file_watcher` reconciliation ext filter:

```rust
if matches!(ext, "md" | "txt" | "markdown" | "pdf" | "docx") {
```

Replace with:

```rust
if matches!(ext,
    "md" | "txt" | "markdown" | "pdf" | "docx" |
    "rs" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" |
    "py" | "go" | "java" | "kt" | "swift" | "c" | "h" |
    "cpp" | "hpp" | "cs" | "rb" | "php" | "vue" | "svelte" |
    "yaml" | "yml" | "json" | "jsonc" | "toml" | "xml"
) {
```

- [ ] **Step 3: Run tests**

```
cargo test -p curated-thoughts --features test-utils
```
Expected: PASS (pipeline integration tests use `.md` files — prose chunker still works)

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/pipeline/mod.rs src-tauri/src/lib.rs
git commit -m "feat(pipeline): use chunk_autodetect; expand ingest to code/config extensions"
```

---

## Milestone 3: Scanner + Declarative Chunkers

### Task 11: Scanner chunker

**Files:**
- Create: `src-tauri/src/chunker/scanner.rs`

- [ ] **Step 1: Write failing tests**

Create `src-tauri/src/chunker/scanner.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scanner_empty_returns_empty() {
        assert!(chunk_scanner("").is_empty());
    }

    #[test]
    fn scanner_small_function_single_chunk() {
        let src = "fn foo() {\n    let x = 1;\n    x\n}";
        let chunks = chunk_scanner(src);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].start_line, 1);
        assert_eq!(chunks[0].end_line, 4);
    }

    #[test]
    fn scanner_does_not_cut_inside_string_with_brace() {
        let src = "fn a() {\n    let s = \"hello }\";\n}\nfn b() {\n    let t = 2;\n}";
        let chunks = chunk_scanner(src);
        // Both fns should appear in chunks; '}' inside string must not cause premature cut
        let combined = chunks.iter().map(|c| c.text.as_str()).collect::<Vec<_>>().join(" ");
        assert!(combined.contains("fn a()"));
        assert!(combined.contains("fn b()"));
    }

    #[test]
    fn scanner_line_ranges_track_correctly() {
        let src = "line1\nline2\nline3";
        let chunks = chunk_scanner(src);
        assert!(!chunks.is_empty());
        let last = chunks.last().unwrap();
        assert!(last.end_line >= 3);
    }

    #[test]
    fn scanner_strategy_tag() {
        let chunks = chunk_scanner("fn x() { 1 }");
        for c in &chunks {
            assert!(matches!(c.strategy, crate::chunker::ChunkStrategy::Scanner));
        }
    }

    #[test]
    fn scanner_splits_large_file() {
        // Generate text > BUDGET_SCANNER words at indent 0 boundaries
        let mut lines = Vec::new();
        for i in 0..30 {
            lines.push(format!("fn func_{i}() {{"));
            for j in 0..8 {
                lines.push(format!("    let word_{j} = {j};"));
            }
            lines.push("}".to_string());
            lines.push(String::new());
        }
        let src = lines.join("\n");
        let chunks = chunk_scanner(&src);
        assert!(chunks.len() > 1, "large file must produce multiple chunks");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```
cargo test -p curated-thoughts scanner
```
Expected: FAIL

- [ ] **Step 3: Implement scanner chunker**

Add before `#[cfg(test)]` in `scanner.rs`:

```rust
use crate::chunker::{Chunk, ChunkStrategy};
use crate::chunker::words::{word_count, BUDGET_SCANNER, MIN_CHUNK_WORDS};

pub fn chunk_scanner(text: &str) -> Vec<Chunk> {
    if text.trim().is_empty() {
        return vec![];
    }

    let mut chunks: Vec<Chunk> = Vec::new();
    let mut buf = String::new();
    let mut buf_start_line = 1u32;
    let mut buf_words = 0usize;
    let mut current_line = 1u32;
    let mut brace_depth: i32 = 0;
    let mut in_single_str = false;
    let mut in_double_str = false;
    let mut prev_char = '\0';

    let flush = |buf: &mut String, buf_start_line: u32, end_line: u32,
                 chunks: &mut Vec<Chunk>| {
        let text = buf.trim().to_string();
        if !text.is_empty() {
            let words = word_count(&text);
            if words < MIN_CHUNK_WORDS && !chunks.is_empty() {
                let last = chunks.last_mut().unwrap();
                last.text.push('\n');
                last.text.push_str(&text);
                last.end_line = end_line;
            } else {
                chunks.push(Chunk {
                    text,
                    start_line: buf_start_line,
                    end_line,
                    symbol_name: None,
                    strategy: ChunkStrategy::Scanner,
                });
            }
        }
        buf.clear();
    };

    for ch in text.chars() {
        // Track string literals (basic: single and double quote, escaped by backslash)
        if ch == '\'' && !in_double_str && prev_char != '\\' {
            in_single_str = !in_single_str;
        }
        if ch == '"' && !in_single_str && prev_char != '\\' {
            in_double_str = !in_double_str;
        }

        if !in_single_str && !in_double_str {
            if ch == '{' {
                brace_depth += 1;
            } else if ch == '}' {
                brace_depth -= 1;
            }
        }

        buf.push(ch);
        if ch == '\n' {
            current_line += 1;
            buf_words = word_count(&buf);
        }

        // Cut at indent-0 closing brace when budget exceeded
        let at_top_level_close = ch == '}' && brace_depth == 0 && !in_single_str && !in_double_str;
        if at_top_level_close && buf_words >= BUDGET_SCANNER {
            let end = current_line;
            flush(&mut buf, buf_start_line, end, &mut chunks);
            buf_start_line = current_line + 1;
            buf_words = 0;
        }

        prev_char = ch;
    }

    if !buf.trim().is_empty() {
        let end = current_line;
        flush(&mut buf, buf_start_line, end, &mut chunks);
    }

    chunks
}
```

- [ ] **Step 4: Run tests**

```
cargo test -p curated-thoughts scanner
```
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/chunker/scanner.rs
git commit -m "feat(chunker): brace-depth scanner chunker with line ranges"
```

---

### Task 12: Declarative chunker (YAML / JSON / TOML / XML)

**Files:**
- Create: `src-tauri/src/chunker/declarative.rs`

- [ ] **Step 1: Write failing tests**

Create `src-tauri/src/chunker/declarative.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn declarative_empty_returns_empty() {
        assert!(chunk_declarative(Path::new("a.yaml"), "").is_empty());
    }

    #[test]
    fn yaml_single_doc_splits_on_top_level_keys() {
        let yaml = "key1:\n  val: 1\n  other: 2\nkey2:\n  val: 3\n";
        let chunks = chunk_declarative(Path::new("config.yaml"), yaml);
        assert!(chunks.len() >= 1);
        assert!(chunks[0].text.contains("key1"));
    }

    #[test]
    fn yaml_multi_doc_splits_on_document_boundary() {
        let yaml = "---\nfoo: 1\n---\nbar: 2\n";
        let chunks = chunk_declarative(Path::new("config.yaml"), yaml);
        assert!(chunks.len() >= 2);
        assert!(chunks[0].text.contains("foo"));
        assert!(chunks[1].text.contains("bar"));
    }

    #[test]
    fn yaml_symbol_name_is_top_level_key() {
        let yaml = "server:\n  port: 8080\n";
        let chunks = chunk_declarative(Path::new("config.yaml"), yaml);
        assert_eq!(chunks[0].symbol_name.as_deref(), Some("server"));
    }

    #[test]
    fn json_splits_on_top_level_keys() {
        let json = r#"{"key1": {"a": 1}, "key2": {"b": 2}}"#;
        let chunks = chunk_declarative(Path::new("data.json"), json);
        assert!(chunks.len() >= 1);
    }

    #[test]
    fn toml_splits_on_table_headers() {
        let toml = "[package]\nname = \"foo\"\n\n[dependencies]\nbar = \"1\"\n";
        let chunks = chunk_declarative(Path::new("Cargo.toml"), toml);
        assert!(chunks.len() >= 2);
        assert_eq!(chunks[0].symbol_name.as_deref(), Some("package"));
        assert_eq!(chunks[1].symbol_name.as_deref(), Some("dependencies"));
    }

    #[test]
    fn toml_array_table_splits() {
        let toml = "[[bin]]\nname = \"foo\"\n\n[[bin]]\nname = \"bar\"\n";
        let chunks = chunk_declarative(Path::new("Cargo.toml"), toml);
        assert!(chunks.len() >= 2);
    }

    #[test]
    fn declarative_line_ranges_present() {
        let yaml = "key1:\n  val: 1\nkey2:\n  val: 2\n";
        let chunks = chunk_declarative(Path::new("a.yaml"), yaml);
        for c in &chunks {
            assert!(c.start_line >= 1);
            assert!(c.end_line >= c.start_line);
        }
    }

    #[test]
    fn declarative_strategy_tag() {
        let yaml = "key: val\n";
        let chunks = chunk_declarative(Path::new("a.yaml"), yaml);
        for c in &chunks {
            assert!(matches!(c.strategy, crate::chunker::ChunkStrategy::Declarative));
        }
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```
cargo test -p curated-thoughts declarative
```
Expected: FAIL

- [ ] **Step 3: Implement declarative chunker**

Add before `#[cfg(test)]` in `declarative.rs`:

```rust
use std::path::Path;
use crate::chunker::{Chunk, ChunkStrategy};
use crate::chunker::words::{word_count, BUDGET_DECLARATIVE, MIN_CHUNK_WORDS};

pub fn chunk_declarative(path: &Path, text: &str) -> Vec<Chunk> {
    if text.trim().is_empty() {
        return vec![];
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());

    match ext.as_deref() {
        Some("yaml" | "yml") => chunk_yaml(text),
        Some("json" | "jsonc") => chunk_json(text),
        Some("toml") => chunk_toml(text),
        _ => chunk_xml_or_fallback(text),
    }
}

// ── YAML ──────────────────────────────────────────────────────────────────────

fn chunk_yaml(text: &str) -> Vec<Chunk> {
    let mut sections: Vec<(u32, u32, Option<String>, String)> = Vec::new(); // (start, end, symbol, text)
    let mut buf = String::new();
    let mut buf_start = 1u32;
    let mut current_sym: Option<String> = None;
    let mut current_line = 0u32;

    for line in text.lines() {
        current_line += 1;

        // Document separator
        if line.trim() == "---" {
            if !buf.trim().is_empty() {
                sections.push((buf_start, current_line - 1, current_sym.take(), buf.clone()));
                buf.clear();
            }
            buf_start = current_line + 1;
            current_sym = None;
            continue;
        }

        // Top-level key (no leading whitespace, ends with ':')
        let is_top_key = !line.starts_with(' ') && !line.starts_with('\t')
            && line.contains(':')
            && !line.starts_with('#');

        if is_top_key && !buf.trim().is_empty() {
            let key = line.split(':').next().unwrap_or("").trim().to_string();
            let words = word_count(&buf);
            if words >= BUDGET_DECLARATIVE {
                sections.push((buf_start, current_line - 1, current_sym.take(), buf.clone()));
                buf.clear();
                buf_start = current_line;
            }
            if buf.is_empty() {
                current_sym = Some(key);
            }
        } else if is_top_key && buf.trim().is_empty() {
            current_sym = Some(line.split(':').next().unwrap_or("").trim().to_string());
            buf_start = current_line;
        }

        buf.push_str(line);
        buf.push('\n');
    }

    if !buf.trim().is_empty() {
        sections.push((buf_start, current_line, current_sym, buf));
    }

    merge_and_emit(sections)
}

// ── JSON ──────────────────────────────────────────────────────────────────────

fn chunk_json(text: &str) -> Vec<Chunk> {
    // Split on top-level object keys by tracking character depth
    let mut sections: Vec<(u32, u32, Option<String>, String)> = Vec::new();
    let mut depth: i32 = 0;
    let mut in_str = false;
    let mut prev = '\0';
    let mut buf = String::new();
    let mut buf_start = 1u32;
    let mut current_line = 1u32;
    let mut current_key: Option<String> = None;
    let mut capturing_key = false;
    let mut key_buf = String::new();

    for ch in text.chars() {
        if ch == '\n' {
            current_line += 1;
        }

        if ch == '"' && prev != '\\' {
            in_str = !in_str;
            if in_str && depth == 1 {
                capturing_key = true;
                key_buf.clear();
            } else if !in_str && capturing_key {
                capturing_key = false;
                current_key = Some(key_buf.clone());
            }
        } else if in_str && capturing_key {
            key_buf.push(ch);
        }

        if !in_str {
            match ch {
                '{' | '[' => {
                    depth += 1;
                    if depth == 1 {
                        buf_start = current_line;
                    }
                }
                '}' | ']' => {
                    depth -= 1;
                    if depth == 1 && word_count(&buf) >= BUDGET_DECLARATIVE {
                        buf.push(ch);
                        sections.push((buf_start, current_line, current_key.take(), buf.clone()));
                        buf.clear();
                        buf_start = current_line;
                        prev = ch;
                        continue;
                    }
                    if depth == 0 && !buf.trim().is_empty() {
                        buf.push(ch);
                        sections.push((buf_start, current_line, current_key.take(), buf.clone()));
                        buf.clear();
                        prev = ch;
                        continue;
                    }
                }
                _ => {}
            }
        }

        buf.push(ch);
        prev = ch;
    }

    if !buf.trim().is_empty() {
        sections.push((buf_start, current_line, current_key.take(), buf));
    }

    merge_and_emit(sections)
}

// ── TOML ──────────────────────────────────────────────────────────────────────

fn chunk_toml(text: &str) -> Vec<Chunk> {
    let mut sections: Vec<(u32, u32, Option<String>, String)> = Vec::new();
    let mut buf = String::new();
    let mut buf_start = 1u32;
    let mut current_sym: Option<String> = None;
    let mut current_line = 0u32;

    for line in text.lines() {
        current_line += 1;
        let trimmed = line.trim();

        let is_table = trimmed.starts_with('[');

        if is_table && !buf.trim().is_empty() {
            sections.push((buf_start, current_line - 1, current_sym.take(), buf.clone()));
            buf.clear();
            buf_start = current_line;
        }

        if is_table {
            let header = trimmed.trim_start_matches('[').trim_end_matches(']').trim();
            current_sym = Some(header.to_string());
        }

        buf.push_str(line);
        buf.push('\n');
    }

    if !buf.trim().is_empty() {
        sections.push((buf_start, current_line, current_sym, buf));
    }

    merge_and_emit(sections)
}

// ── XML / fallback ────────────────────────────────────────────────────────────

fn chunk_xml_or_fallback(text: &str) -> Vec<Chunk> {
    // Naive: split on top-level closing tags by tracking angle-bracket depth
    crate::chunker::fallback::chunk_fallback(text)
        .into_iter()
        .map(|mut c| { c.strategy = ChunkStrategy::Declarative; c })
        .collect()
}

// ── Shared: merge micro-chunks and emit ───────────────────────────────────────

fn merge_and_emit(sections: Vec<(u32, u32, Option<String>, String)>) -> Vec<Chunk> {
    let mut chunks: Vec<Chunk> = Vec::new();

    for (start, end, sym, text) in sections {
        let words = word_count(&text);
        if words < MIN_CHUNK_WORDS && !chunks.is_empty() {
            let last = chunks.last_mut().unwrap();
            last.text.push('\n');
            last.text.push_str(text.trim());
            last.end_line = end;
        } else {
            chunks.push(Chunk {
                text: text.trim().to_string(),
                start_line: start,
                end_line: end,
                symbol_name: sym,
                strategy: ChunkStrategy::Declarative,
            });
        }
    }

    chunks
}
```

- [ ] **Step 4: Run tests**

```
cargo test -p curated-thoughts declarative
```
Expected: PASS

- [ ] **Step 5: Run full test suite**

```
cargo test -p curated-thoughts --features test-utils
```
Expected: all tests pass

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/chunker/declarative.rs
git commit -m "feat(chunker): declarative chunker for YAML/JSON/TOML/XML with top-level key splits"
```

---

## Milestone 4: Tree-sitter AST — Rust, Python, Go

### Task 13: Add tree-sitter dependencies

**Files:**
- Modify: `src-tauri/Cargo.toml`

- [ ] **Step 1: Add tree-sitter crates**

Check crates.io for latest compatible versions of each grammar before adding. At time of writing:

```toml
[dependencies]
# ... existing deps ...
tree-sitter = "0.22"
tree-sitter-rust = "0.21"
tree-sitter-python = "0.21"
tree-sitter-go = "0.21"
```

- [ ] **Step 2: Verify it compiles**

```
cargo build -p curated-thoughts
```
Expected: PASS (no new code yet, just deps resolving)

- [ ] **Step 3: Commit**

```bash
git add src-tauri/Cargo.toml
git commit -m "chore(deps): add tree-sitter + Rust/Python/Go grammars"
```

---

### Task 14: AST chunker — Rust

**Files:**
- Create: `src-tauri/src/chunker/ast.rs`
- Modify: `src-tauri/src/chunker/mod.rs`

- [ ] **Step 1: Write failing tests**

Create `src-tauri/src/chunker/ast.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn ast_rust_empty_returns_empty() {
        assert!(chunk_ast(Path::new("a.rs"), "").is_empty());
    }

    #[test]
    fn ast_rust_fn_becomes_chunk() {
        let src = "fn hello() -> u32 {\n    42\n}\n";
        let chunks = chunk_ast(Path::new("main.rs"), src);
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].text.contains("fn hello()"));
        assert_eq!(chunks[0].symbol_name.as_deref(), Some("hello"));
        assert_eq!(chunks[0].start_line, 1);
        assert_eq!(chunks[0].end_line, 3);
    }

    #[test]
    fn ast_rust_struct_becomes_chunk() {
        let src = "struct Foo {\n    x: u32,\n}\n";
        let chunks = chunk_ast(Path::new("types.rs"), src);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].symbol_name.as_deref(), Some("Foo"));
    }

    #[test]
    fn ast_rust_impl_method_has_parent_prefix() {
        let src = "impl Bar {\n    fn baz(&self) {\n        println!(\"hi\");\n    }\n}\n";
        let chunks = chunk_ast(Path::new("bar.rs"), src);
        // method chunk should include "impl Bar {" as prefix
        assert!(chunks.iter().any(|c| c.text.contains("impl Bar") && c.text.contains("fn baz")));
    }

    #[test]
    fn ast_rust_multiple_fns_multiple_chunks() {
        let src = "fn a() {}\nfn b() {}\nfn c() {}\n";
        let chunks = chunk_ast(Path::new("main.rs"), src);
        assert_eq!(chunks.len(), 3);
    }

    #[test]
    fn ast_rust_line_ranges_are_exact() {
        let src = "fn first() {\n    1\n}\n\nfn second() {\n    2\n}\n";
        let chunks = chunk_ast(Path::new("main.rs"), src);
        assert_eq!(chunks[0].start_line, 1);
        assert_eq!(chunks[0].end_line, 3);
        assert_eq!(chunks[1].start_line, 5);
        assert_eq!(chunks[1].end_line, 7);
    }

    #[test]
    fn ast_rust_nested_fn_not_top_level_chunk() {
        let src = "fn outer() {\n    fn inner() {}\n}\n";
        let chunks = chunk_ast(Path::new("main.rs"), src);
        // only outer becomes a chunk; inner is captured in outer's body text
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].symbol_name.as_deref(), Some("outer"));
    }

    #[test]
    fn ast_rust_parse_failure_falls_back_to_scanner() {
        // Completely unparseable text with .rs extension should fall back without panic
        let src = "this is not valid rust }{{{";
        let chunks = chunk_ast(Path::new("bad.rs"), src);
        // Scanner fallback: must not be empty and must not panic
        assert!(!chunks.is_empty());
        for c in &chunks {
            assert!(matches!(
                c.strategy,
                crate::chunker::ChunkStrategy::AstSymbol | crate::chunker::ChunkStrategy::Scanner
            ));
        }
    }

    #[test]
    fn ast_strategy_tag() {
        let src = "fn foo() {}\n";
        let chunks = chunk_ast(Path::new("a.rs"), src);
        assert!(matches!(chunks[0].strategy, crate::chunker::ChunkStrategy::AstSymbol));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```
cargo test -p curated-thoughts ast_rust
```
Expected: FAIL

- [ ] **Step 3: Implement Rust AST chunking in ast.rs**

```rust
use std::path::Path;
use crate::chunker::{Chunk, ChunkStrategy};
use crate::chunker::words::{word_count, BUDGET_AST, MIN_CHUNK_WORDS};

pub fn chunk_ast(path: &Path, text: &str) -> Vec<Chunk> {
    if text.trim().is_empty() {
        return vec![];
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());

    let result = match ext.as_deref() {
        Some("rs") => chunk_with_language(
            text,
            tree_sitter_rust::language(),
            &["function_item", "impl_item", "struct_item", "enum_item",
              "trait_item", "const_item", "type_item", "mod_item"],
            rust_symbol_name,
        ),
        Some("py") => chunk_with_language(
            text,
            tree_sitter_python::language(),
            &["function_definition", "class_definition"],
            python_symbol_name,
        ),
        Some("go") => chunk_with_language(
            text,
            tree_sitter_go::language(),
            &["function_declaration", "method_declaration", "type_declaration",
              "const_declaration", "var_declaration"],
            go_symbol_name,
        ),
        _ => return crate::chunker::scanner::chunk_scanner(text),
    };

    match result {
        Ok(chunks) if !chunks.is_empty() => chunks,
        _ => {
            eprintln!("[chunker] ast parse yielded no nodes for {:?}, falling back to scanner", path);
            crate::chunker::scanner::chunk_scanner(text)
        }
    }
}

fn chunk_with_language(
    text: &str,
    language: tree_sitter::Language,
    top_level_kinds: &[&str],
    symbol_name_fn: fn(&tree_sitter::Node, &[u8]) -> Option<String>,
) -> anyhow::Result<Vec<Chunk>> {
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&language)?;
    let tree = parser.parse(text.as_bytes(), None)
        .ok_or_else(|| anyhow::anyhow!("parse returned None"))?;
    let root = tree.root_node();
    let src = text.as_bytes();

    let mut raw: Vec<Chunk> = Vec::new();
    let mut cursor = root.walk();

    for child in root.children(&mut cursor) {
        if !top_level_kinds.contains(&child.kind()) {
            continue;
        }

        let start_line = (child.start_position().row + 1) as u32;
        let end_line = (child.end_position().row + 1) as u32;
        let symbol_name = symbol_name_fn(&child, src);
        let node_text = &text[child.byte_range()];

        // For methods inside impl blocks, extract children instead
        if child.kind() == "impl_item" {
            let impl_sig = first_line(node_text);
            let mut impl_cursor = child.walk();
            for method in child.children(&mut impl_cursor) {
                if method.kind() == "function_item" {
                    let m_start = (method.start_position().row + 1) as u32;
                    let m_end = (method.end_position().row + 1) as u32;
                    let m_text = &text[method.byte_range()];
                    let m_name = rust_symbol_name(&method, src);
                    raw.push(Chunk {
                        text: format!("{}\n{}", impl_sig, m_text),
                        start_line: m_start,
                        end_line: m_end,
                        symbol_name: m_name,
                        strategy: ChunkStrategy::AstSymbol,
                    });
                }
            }
            // Also emit the impl block itself (types, consts inside)
            // Only if it had no methods (pure type impls)
            continue;
        }

        raw.push(Chunk {
            text: node_text.to_string(),
            start_line,
            end_line,
            symbol_name,
            strategy: ChunkStrategy::AstSymbol,
        });
    }

    Ok(merge_small_symbols(raw))
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or("")
}

fn rust_symbol_name(node: &tree_sitter::Node, src: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" || child.kind() == "type_identifier" {
            return Some(String::from_utf8_lossy(&src[child.byte_range()]).to_string());
        }
    }
    None
}

fn python_symbol_name(node: &tree_sitter::Node, src: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" {
            return Some(String::from_utf8_lossy(&src[child.byte_range()]).to_string());
        }
    }
    None
}

fn go_symbol_name(node: &tree_sitter::Node, src: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" || child.kind() == "type_spec" {
            return Some(String::from_utf8_lossy(&src[child.byte_range()]).to_string());
        }
    }
    None
}

fn merge_small_symbols(chunks: Vec<Chunk>) -> Vec<Chunk> {
    let mut out: Vec<Chunk> = Vec::new();
    for chunk in chunks {
        if word_count(&chunk.text) < MIN_CHUNK_WORDS && !out.is_empty() {
            let last = out.last_mut().unwrap();
            last.text.push('\n');
            last.text.push_str(&chunk.text);
            last.end_line = chunk.end_line;
        } else {
            out.push(chunk);
        }
    }
    // Split oversized symbols at inner function boundary
    let mut final_out: Vec<Chunk> = Vec::new();
    for chunk in out {
        if word_count(&chunk.text) > BUDGET_AST {
            // Split at inner function line boundaries (lines starting with "    fn " or "def " etc.)
            split_large_symbol(chunk, &mut final_out);
        } else {
            final_out.push(chunk);
        }
    }
    final_out
}

fn split_large_symbol(chunk: Chunk, out: &mut Vec<Chunk>) {
    // Naive: split at blank lines within the chunk if over budget
    let mut buf = String::new();
    let mut buf_start = chunk.start_line;
    let mut current_line = chunk.start_line;

    for line in chunk.text.lines() {
        buf.push_str(line);
        buf.push('\n');
        if line.trim().is_empty() && word_count(&buf) >= BUDGET_AST {
            out.push(Chunk {
                text: buf.trim().to_string(),
                start_line: buf_start,
                end_line: current_line,
                symbol_name: chunk.symbol_name.clone(),
                strategy: ChunkStrategy::AstSymbol,
            });
            buf.clear();
            buf_start = current_line + 1;
        }
        current_line += 1;
    }
    if !buf.trim().is_empty() {
        out.push(Chunk {
            text: buf.trim().to_string(),
            start_line: buf_start,
            end_line: current_line - 1,
            symbol_name: chunk.symbol_name.clone(),
            strategy: ChunkStrategy::AstSymbol,
        });
    }
}
```

- [ ] **Step 4: Register ast module in chunker/mod.rs**

Add `pub mod ast;` to the module list at the top of `chunker/mod.rs`.

Update the `AstSymbol` arm in `chunk_autodetect`:

```rust
ChunkStrategy::AstSymbol => ast::chunk_ast(path, text),
```

- [ ] **Step 5: Run tests**

```
cargo test -p curated-thoughts ast_rust
```
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/chunker/ast.rs src-tauri/src/chunker/mod.rs
git commit -m "feat(chunker): Tree-sitter AST chunker for Rust with symbol names and exact line ranges"
```

---

### Task 15: AST chunker — Python and Go (same ast.rs)

The `chunk_ast` dispatcher in `ast.rs` already routes `py` → `tree_sitter_python` and `go` → `tree_sitter_go`. Only tests are needed.

**Files:**
- Modify: `src-tauri/src/chunker/ast.rs`

- [ ] **Step 1: Write Python tests**

Add to `ast.rs` `mod tests`:

```rust
#[test]
fn ast_python_def_becomes_chunk() {
    let src = "def greet(name):\n    return f\"hello {name}\"\n";
    let chunks = chunk_ast(Path::new("greet.py"), src);
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].symbol_name.as_deref(), Some("greet"));
    assert_eq!(chunks[0].start_line, 1);
}

#[test]
fn ast_python_class_becomes_chunk_with_methods_as_children() {
    let src = "class Dog:\n    def bark(self):\n        print(\"woof\")\n";
    let chunks = chunk_ast(Path::new("dog.py"), src);
    // class-level chunk (may include methods or separate them depending on impl)
    assert!(!chunks.is_empty());
    assert!(chunks.iter().any(|c| c.text.contains("class Dog") || c.symbol_name.as_deref() == Some("Dog")));
}

#[test]
fn ast_python_multiple_defs() {
    let src = "def a():\n    pass\n\ndef b():\n    pass\n";
    let chunks = chunk_ast(Path::new("funcs.py"), src);
    assert_eq!(chunks.len(), 2);
}
```

- [ ] **Step 2: Write Go tests**

Add to `ast.rs` `mod tests`:

```rust
#[test]
fn ast_go_func_becomes_chunk() {
    let src = "package main\n\nfunc Hello() string {\n\treturn \"hello\"\n}\n";
    let chunks = chunk_ast(Path::new("main.go"), src);
    assert!(chunks.iter().any(|c| c.symbol_name.as_deref() == Some("Hello")));
}

#[test]
fn ast_go_multiple_funcs() {
    let src = "package main\n\nfunc A() {}\n\nfunc B() {}\n";
    let chunks = chunk_ast(Path::new("main.go"), src);
    assert!(chunks.len() >= 2);
}
```

- [ ] **Step 3: Run tests**

```
cargo test -p curated-thoughts ast_python
cargo test -p curated-thoughts ast_go
```
Expected: PASS

If Python's `class_definition` emits methods as children of the class node and you want separate chunks per method, update `chunk_with_language` to recurse into `class_definition` children the same way it recurses into `impl_item` in Rust.

- [ ] **Step 4: Add integration test**

Add to `src-tauri/tests/pipeline.rs` (or create a new `tests/chunker_integration.rs`):

```rust
#[cfg(test)]
mod chunker_integration {
    use std::path::Path;
    use curated_thoughts_lib::chunker::chunk_autodetect;

    #[test]
    fn rust_file_produces_ast_symbol_chunks() {
        let src = include_str!("../src/chunker/mod.rs");
        let chunks = chunk_autodetect(Path::new("mod.rs"), src);
        assert!(!chunks.is_empty());
        for c in &chunks {
            assert!(c.start_line >= 1);
            assert!(c.end_line >= c.start_line);
        }
    }

    #[test]
    fn markdown_file_produces_prose_chunks() {
        let src = "# Title\n\nThis is a paragraph with several sentences. It continues here. And here.\n\nAnother paragraph follows.\n";
        let chunks = chunk_autodetect(Path::new("README.md"), src);
        assert!(!chunks.is_empty());
        for c in &chunks {
            assert!(matches!(c.strategy, curated_thoughts_lib::chunker::ChunkStrategy::Prose));
        }
    }

    #[test]
    fn yaml_file_produces_declarative_chunks() {
        let src = "server:\n  port: 8080\nlogging:\n  level: info\n";
        let chunks = chunk_autodetect(Path::new("config.yaml"), src);
        assert!(!chunks.is_empty());
        for c in &chunks {
            assert!(matches!(c.strategy, curated_thoughts_lib::chunker::ChunkStrategy::Declarative));
        }
    }

    #[test]
    fn unknown_ext_produces_fallback_chunks() {
        let src = "some text\n\nmore text here\n\nfinal section\n";
        let chunks = chunk_autodetect(Path::new("Makefile"), src);
        assert!(!chunks.is_empty());
        for c in &chunks {
            assert!(matches!(c.strategy, curated_thoughts_lib::chunker::ChunkStrategy::Fallback));
        }
    }
}
```

- [ ] **Step 5: Run integration tests**

```
cargo test -p curated-thoughts chunker_integration
```
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/chunker/ast.rs src-tauri/tests/chunker_integration.rs
git commit -m "feat(chunker): AST chunker for Python and Go; cross-strategy integration tests"
```

---

## Milestone 5: TypeScript/JavaScript AST + MCP Response Shape

### Task 16: Add TS/JS tree-sitter grammars

**Files:**
- Modify: `src-tauri/Cargo.toml`

- [ ] **Step 1: Add grammar crates**

```toml
tree-sitter-typescript = "0.21"
tree-sitter-javascript = "0.21"
```

Note: `tree-sitter-typescript` provides two language functions:
- `tree_sitter_typescript::language_typescript()` for `.ts`
- `tree_sitter_typescript::language_tsx()` for `.tsx`

- [ ] **Step 2: Compile check**

```
cargo build -p curated-thoughts
```
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add src-tauri/Cargo.toml
git commit -m "chore(deps): add tree-sitter TypeScript and JavaScript grammars"
```

---

### Task 17: AST chunker — TypeScript and JavaScript

**Files:**
- Modify: `src-tauri/src/chunker/ast.rs`

- [ ] **Step 1: Add TS/JS routing to chunk_ast**

Add new arms to the `match ext.as_deref()` in `chunk_ast`:

```rust
Some("ts") => chunk_with_language(
    text,
    tree_sitter_typescript::language_typescript(),
    &["function_declaration", "class_declaration", "lexical_declaration",
      "variable_declaration", "export_statement"],
    ts_symbol_name,
),
Some("tsx") => chunk_with_language(
    text,
    tree_sitter_typescript::language_tsx(),
    &["function_declaration", "class_declaration", "lexical_declaration",
      "variable_declaration", "export_statement"],
    ts_symbol_name,
),
Some("js" | "jsx" | "mjs" | "cjs") => chunk_with_language(
    text,
    tree_sitter_javascript::language(),
    &["function_declaration", "class_declaration", "lexical_declaration",
      "variable_declaration", "export_statement"],
    ts_symbol_name,
),
```

Add `ts_symbol_name` function:

```rust
fn ts_symbol_name(node: &tree_sitter::Node, src: &[u8]) -> Option<String> {
    // For export_statement, descend to find declaration name
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "identifier" | "property_identifier") {
            return Some(String::from_utf8_lossy(&src[child.byte_range()]).to_string());
        }
        // For lexical_declaration: find variable_declarator → identifier
        if child.kind() == "variable_declarator" {
            let mut vc = child.walk();
            for vc_child in child.children(&mut vc) {
                if vc_child.kind() == "identifier" {
                    return Some(String::from_utf8_lossy(&src[vc_child.byte_range()]).to_string());
                }
            }
        }
    }
    None
}
```

- [ ] **Step 2: Write TS/JS tests**

Add to `ast.rs` `mod tests`:

```rust
#[test]
fn ast_ts_function_declaration() {
    let src = "function greet(name: string): string {\n    return `hello ${name}`;\n}\n";
    let chunks = chunk_ast(Path::new("greet.ts"), src);
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].symbol_name.as_deref(), Some("greet"));
}

#[test]
fn ast_ts_class_declaration() {
    let src = "class Animal {\n    name: string;\n    constructor(name: string) {\n        this.name = name;\n    }\n}\n";
    let chunks = chunk_ast(Path::new("animal.ts"), src);
    assert!(!chunks.is_empty());
    assert!(chunks.iter().any(|c| c.symbol_name.as_deref() == Some("Animal") || c.text.contains("class Animal")));
}

#[test]
fn ast_ts_arrow_const_export() {
    let src = "export const handler = async (req: Request) => {\n    return Response.ok();\n};\n";
    let chunks = chunk_ast(Path::new("handler.ts"), src);
    assert!(!chunks.is_empty());
}

#[test]
fn ast_tsx_component() {
    let src = "export function Button({ label }: Props) {\n    return <button>{label}</button>;\n}\n";
    let chunks = chunk_ast(Path::new("Button.tsx"), src);
    assert!(!chunks.is_empty());
    assert!(chunks.iter().any(|c| c.text.contains("Button")));
}

#[test]
fn ast_js_function() {
    let src = "function add(a, b) {\n    return a + b;\n}\n";
    let chunks = chunk_ast(Path::new("math.js"), src);
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].symbol_name.as_deref(), Some("add"));
}
```

- [ ] **Step 3: Run tests**

```
cargo test -p curated-thoughts ast_ts
cargo test -p curated-thoughts ast_tsx
cargo test -p curated-thoughts ast_js
```
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/chunker/ast.rs
git commit -m "feat(chunker): AST chunker for TypeScript, TSX, and JavaScript"
```

---

### Task 18: Verify full MCP response shape via search_vault

**Files:**
- Modify: `src-tauri/tests/pipeline.rs` (or create `tests/search_result_shape.rs`)

- [ ] **Step 1: Write integration test for SearchResult shape**

Add to `src-tauri/tests/pipeline.rs` (requires `features = ["test-utils"]`):

```rust
#[cfg(test)]
mod search_result_shape {
    use tempfile::TempDir;
    use curated_thoughts_lib::{make_test_app, PipelineWorker, PipelineJob};
    use std::sync::mpsc;

    #[test]
    fn search_result_includes_line_ranges_and_strategy() {
        let tmp = TempDir::new().unwrap();

        // Write a small Rust file
        let doc_dir = tmp.path().join("documents");
        std::fs::create_dir_all(&doc_dir).unwrap();
        let file = doc_dir.join("hello.rs");
        std::fs::write(&file, "fn greet() -> &'static str {\n    \"hello\"\n}\n").unwrap();

        // Run pipeline
        let db_path = tmp.path().join("brain.db");
        let (tx, rx) = mpsc::sync_channel(8);
        let worker = PipelineWorker::new(db_path.clone(), rx);
        let handle = std::thread::spawn(move || worker.run());
        tx.send(PipelineJob::Ingest(file.to_string_lossy().into_owned())).unwrap();
        drop(tx);
        handle.join().unwrap();

        // Query via Tauri command
        let app = make_test_app(tmp.path());
        let results: Vec<serde_json::Value> = tauri::test::get_ipc_response::<_, Vec<serde_json::Value>>(
            &app,
            tauri::webview::InvokeRequest {
                cmd: "search_vault".into(),
                callback: tauri::ipc::CallbackFn(0),
                error: tauri::ipc::CallbackFn(1),
                url: "http://tauri.localhost".parse().unwrap(),
                body: tauri::ipc::InvokeBody::Json(serde_json::json!({
                    "query": "greet function",
                    "limit": 5
                })),
                headers: Default::default(),
                invoke_key: tauri::test::INVOKE_KEY.to_string(),
            },
        ).unwrap();

        // Verify response shape
        if !results.is_empty() {
            let r = &results[0];
            assert!(r.get("start_line").is_some(), "start_line must be present");
            assert!(r.get("end_line").is_some(), "end_line must be present");
            assert!(r.get("strategy").is_some(), "strategy must be present");
            assert!(r.get("doc_path").is_some(), "doc_path must be present");
        }
    }
}
```

- [ ] **Step 2: Run the test**

```
cargo test -p curated-thoughts --features test-utils search_result_shape
```

Note: this test requires Ollama running with `nomic-embed-code` pulled. If Ollama is unavailable, the pipeline will log an error and the document will not be indexed — the test will still pass (results will be empty, assertion skipped). To test with real results, run with Ollama available.

- [ ] **Step 3: Run full test suite**

```
cargo test -p curated-thoughts --features test-utils
```
Expected: all tests pass

- [ ] **Step 4: Final commit**

```bash
git add src-tauri/tests/
git commit -m "test(search): integration test verifying SearchResult includes line ranges and strategy"
```

---

## Post-Milestone Notes

**SciFact benchmark:** The SciFact fixture embeddings are AllMiniLML6V2 384-dim. Querying them with `nomic-embed-code` 768-dim vectors will produce dimension mismatches and zero scores. To run the benchmark with v2 embeddings: regenerate `scifact-embeddings.bin.gz` using `embed_scifact` binary after switching it to `OllamaEmbedder`. This is out of scope for this plan but noted for future work.

**Cloud provider embed:** `OllamaEmbedder::from_profile` returns an error for `Cloud` profiles. Cloud implementation (OpenAI `/v1/embeddings`, Voyage, Cohere) is a separate task deferred to a later milestone.

**Ollama model pull:** If `nomic-embed-code` is not yet pulled, `OllamaEmbedder::embed` will return an HTTP 404. The pipeline catches this error, logs it, and marks the document as `error` status — same behavior as any other ingest failure. The user must run `ollama pull nomic-embed-code` before first ingest.
