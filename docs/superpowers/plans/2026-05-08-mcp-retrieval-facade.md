# MCP Retrieval Façade Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a **`retrieval` Rust façade** so Tauri **`search_vault`** / **`get_related_chunks`** and a new **`curated-thoughts-mcp`** binary share one code path (`embed_one` + `search::semantic_search` / `search::related_chunks`), with MCP stdio exposing tools **`vault_semantic_search`** and **`vault_related_chunks`**.

**Architecture:** Resolve **`brain.db`** + **`config.json`** from **`CURATED_*` env vars** (default `~/.brain`). Open SQLite **read-only** for MCP; reuse the writable **`Mutex<AppDb>`** connection inside Tauri. Parse **`EmbedProfile`** via existing **`vault::VaultConfig`**. MCP uses optional Cargo feature **`mcp-server`** pulling **`rmcp`** (official SDK) **stdio transport** — default `cargo build` stays lean.

**Tech Stack:** `rusqlite` `OpenFlags::SQLITE_OPEN_READ_ONLY`, **`rmcp`** 1.x with **`macros`** + **`transport-io`**, **`tokio`**, **`schemars`**, serde JSON for tool payloads. Tests set **`CURATED_EMBED_STUB=constant8`** for deterministic 8-D vectors (already in `embedder/mod.rs`).

**Spec:** `docs/superpowers/specs/2026-05-08-mcp-retrieval-facade-design.md`

---

## File Map

| Action | Path | Responsibility |
|--------|------|----------------|
| Create | `src-tauri/src/retrieval/mod.rs` | Resolve brain paths from env; `open_read_only`; façade `semantic_search_chunks` / `related_chunks_facade`; re-export callers need |
| Modify | `src-tauri/src/lib.rs` | `pub mod retrieval;`; thin `search_vault` / `get_related_chunks` delegating to façade |
| Modify | `src-tauri/Cargo.toml` | Optional feature `mcp-server`; deps **`rmcp`**, **`tokio`**, **`schemars`**; `[[bin]]` `curated-thoughts-mcp` `required-features` |
| Create | `src-tauri/src/bin/curated_thoughts_mcp.rs` | `#[tokio::main]` + `rmcp` stdio serve; two `#[tool]` handlers |
| Create | `src-tauri/tests/retrieval_facade.rs` | Integration tests: temp brain dir + stub embed + façade calls |
| Modify | `README.md` | MCP section: build command, Cursor `mcpServers` snippet, **`CURATED_BRAIN_DIR`**, **`CURATED_BRAIN_DB`**, **`CURATED_BRAIN_CONFIG`**, **`CURATED_EMBED_STUB`** for tests only |

---

## Task 1: Retrieval façade module + tests (`retrieval`)

**Files:**
- Create: `src-tauri/src/retrieval/mod.rs`
- Modify: `src-tauri/src/lib.rs` — add line `pub mod retrieval;` alongside other `pub mod` entries (`pub mod search` is already public; retrieval may stay `pub mod retrieval`).
- Create: `src-tauri/tests/retrieval_facade.rs`

### Step 1: Add module file with env resolution + read-only open

Create `src-tauri/src/retrieval/mod.rs`:

```rust
//! Shared retrieval entry points for Tauri IPC and MCP. See MCP spec §4–§7.

use std::env;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use rusqlite::{Connection, OpenFlags};

use crate::embedder::{embed_one, EmbedProfile};
use crate::search::{self, SearchResult};
use crate::vault::VaultConfig;

fn default_brain_home() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".brain")
}

/// Resolve `(database_file, config_file)` paths from env (spec §4).
pub fn resolve_brain_paths() -> Result<(PathBuf, PathBuf)> {
    let db_path = env::var("CURATED_BRAIN_DB").ok();
    let config_explicit = env::var("CURATED_BRAIN_CONFIG").ok();
    let dir = env::var("CURATED_BRAIN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| default_brain_home());

    let config_path = if let Some(c) = config_explicit {
        PathBuf::from(c)
    } else if let Some(db) = &db_path {
        db.parent()
            .map(|p| p.join("config.json"))
            .context("CURATED_BRAIN_DB has no parent for config.json")?
    } else {
        dir.join("config.json")
    };

    let db_path_final = db_path.map(PathBuf::from).unwrap_or_else(|| dir.join("brain.db"));

    Ok((db_path_final, config_path))
}

pub fn load_embed_profile(config_path: &Path) -> Result<EmbedProfile> {
    VaultConfig::new(config_path.to_path_buf()).get_embed_profile()
}

/// Open existing brain DB **read-only** for MCP (`SQLITE_OPEN_READ_ONLY`). No migrations.
pub fn open_brain_readonly(db_path: &Path) -> Result<Connection> {
    if !db_path.exists() {
        bail!(
            "brain.db not found at {}; set CURATED_BRAIN_DIR or CURATED_BRAIN_DB",
            db_path.display()
        );
    }
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    Ok(Connection::open_with_flags(db_path, flags)?)
}

/// Embed query text then run cosine search — same semantics as `search_vault` Tauri command.
pub fn semantic_search_chunks(
    conn: &Connection,
    profile: &EmbedProfile,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchResult>> {
    let clamped = limit.clamp(1, 50);
    let vec = embed_one(profile, query.to_string())?;
    search::semantic_search(conn, &vec, clamped)
}

/// Delegate to [`search::related_chunks`] with limit clamp matching Tauri (1–10 default path).
pub fn related_chunks_facade(
    conn: &Connection,
    doc_path: &str,
    limit: usize,
) -> Result<Vec<SearchResult>> {
    let clamped = limit.clamp(1, 10);
    search::related_chunks(conn, doc_path, clamped)
}
```

Wire `pub mod retrieval;` in `src-tauri/src/lib.rs` immediately after `pub mod search`.

### Step 2: Thin integration test (fixture DB + **`CURATED_EMBED_STUB`**)

Add dev-dependency in **`src-tauri/Cargo.toml`** under **`[dev-dependencies]`**:

```toml
temp-env = "0.7"
```

Create `src-tauri/tests/retrieval_facade.rs`:

```rust
//! Façade parity checks (`CURATED_EMBED_STUB=constant8` → deterministic 8-D vectors).

use std::fs;

use anyhow::Result;
use rusqlite::Connection;
use tempfile::TempDir;
use tauri_app_lib::{
    chunker::{Chunk, ChunkStrategyTag},
    db::{queries, AppDb},
    embedder::{embed_one, EmbedProfile},
    retrieval::{
        load_embed_profile, open_brain_readonly, resolve_brain_paths, semantic_search_chunks,
    },
};

fn write_minimal_config(dir: &std::path::Path) -> std::path::PathBuf {
    let p = dir.join("config.json");
    fs::write(&p, "{}").unwrap();
    p
}

fn seed_indexed_fixture(conn: &Connection) -> Result<()> {
    let doc_id = queries::upsert_document(conn, "/vault/documents/x.md", "hash1")?;
    let chunk = Chunk {
        text: "alpha beta gamma".into(),
        start_line: 1,
        end_line: 1,
        symbol_name: Some("foo".into()),
        strategy: ChunkStrategyTag::Prose,
    };
    let chunk_id = queries::insert_chunk(conn, doc_id, &chunk, 0)?;
    let v = embed_one(&EmbedProfile::default(), "q".into())?;
    queries::insert_embedding(conn, chunk_id, &v)?;
    queries::mark_document_indexed(conn, doc_id)?;
    Ok(())
}

#[test]
fn semantic_facade_reads_via_readonly_and_returns_symbol() -> Result<()> {
    let tmp = TempDir::new()?;
    let brain = tmp.path();
    write_minimal_config(brain);
    let db_path = brain.join("brain.db");
    let brain_s = brain.to_str().expect("UTF-8 temp path");

    temp_env::with_vars(
        [
            ("CURATED_EMBED_STUB", Some("constant8")),
            ("CURATED_BRAIN_DIR", Some(brain_s)),
        ],
        || -> Result<()> {
            {
                let db = AppDb::open(&db_path)?;
                seed_indexed_fixture(&db.0)?;
            }

            let (db_resolved, cfg_resolved) = resolve_brain_paths()?;
            assert_eq!(db_resolved, db_path);

            let profile = load_embed_profile(&cfg_resolved)?;
            let ro = open_brain_readonly(&db_resolved)?;

            let out = semantic_search_chunks(&ro, &profile, "q", 5)?;
            assert_eq!(out.len(), 1);
            assert_eq!(out[0].symbol_name.as_deref(), Some("foo"));
            assert_eq!(out[0].strategy, "prose");
            Ok(())
        },
    )?;
    Ok(())
}
```

Implementer matches **`temp_env`** signature to the chosen crate version (**`[&[(&str, Option<&str>)]`** or **`HashMap`** overload per docs).

### Step 3: Compile + run façade test only

From **`src-tauri/`**:

```bash
cd src-tauri
cargo test -p curated-thoughts --test retrieval_facade
```

Expected: **`semantic_facade_reads_via_readonly_and_returns_symbol`** **PASS**.

### Step 4: SQLite read-open flags sanity

Implementer trims **`NO_MUTEX`** if **`open_with_flags` fails locally** — keep **`SQLITE_OPEN_READ_ONLY`**.

### Step 5: Commit

```bash
git add src-tauri/src/retrieval/mod.rs src-tauri/src/lib.rs src-tauri/tests/retrieval_facade.rs src-tauri/Cargo.toml src-tauri/Cargo.lock
git commit -m "feat(retrieval): shared brain path resolution and search façade"
```

---

## Task 2: Wire Tauri `search_vault` / `get_related_chunks` through façade

**Files:**
- Modify: `src-tauri/src/lib.rs` — **`search_vault`** and **`get_related_chunks`** bodies only

### Step 1: Replace duplicated embed + search bodies

Locate:

```rust
fn search_vault(
    ...
) -> Result<Vec<search::SearchResult>, String> {
    let profile = cfg...get_embed_profile()...
    let query_vec = crate::embedder::embed_one(&profile, query).map_err(|e| e.to_string())?;
    let guard = db_state.0.lock().unwrap();
    search::semantic_search(&guard.0, &query_vec, limit.clamp(1, 50))
```

Replace **`query_vec`** path with façade:

```rust
    let guard = db_state.0.lock().unwrap();
    retrieval::semantic_search_chunks(&guard.0, &profile, &query, limit)
        .map_err(|e| e.to_string())
```

And **`get_related_chunks`** replace inner `related_chunks(...)`:

```rust
    retrieval::related_chunks_facade(&guard.0, &doc_path, limit)
        .map_err(|e| e.to_string())
```

Add `use crate::retrieval;` near top imports or qualify `crate::retrieval::`.

### Step 2: Run existing tests touching search

```bash
cd src-tauri
cargo test -p curated-thoughts search -- --nocapture 2>&1 | tail -30
cargo test -p curated-thoughts --features test-utils 2>&1 | tail -40
```

Expected: Same pass count as before (no retrieval regression).

### Step 3: Commit

```bash
git add src-tauri/src/lib.rs
git commit -m "refactor(tauri): route search commands through retrieval façade"
```

---

## Task 3: Cargo feature **`mcp-server`** + MCP binary scaffolding

**Files:**
- Modify: `src-tauri/Cargo.toml`

### Step 1: Merge into **`[features]`** and **`[dependencies]`** (no duplicate table headers)

In **`src-tauri/Cargo.toml`**, extend the **existing** `[features]` table with:

```toml
mcp-server = ["dep:rmcp", "dep:schemars"]
```

Add **optional** dependencies (same `[dependencies]` table as `tauri`, `serde`, etc.):

```toml
rmcp = { version = "1", optional = true, features = ["macros", "transport-io"] }
schemars = { version = "0.8", optional = true, features = ["derive"] }
```

Do **not** add a second **`[dependencies]`** header. **`tokio`** is already present transitively; only add it explicitly if **`rmcp`** build errors require **`full` features** (follow compiler errors).

Add:

```toml
[[bin]]
name = "curated-thoughts-mcp"
path = "src/bin/curated_thoughts_mcp.rs"
required-features = ["mcp-server"]
```

**Verify `rmcp` version / feature names** with `cargo add rmcp -p curated-thoughts --features macros,transport-io --dry-run` (adjust to match **`1.x`** published API).

### Step 2: Build binary (**should fail**: empty `main`)

```bash
cd src-tauri
cargo build -p curated-thoughts --features mcp-server --bin curated-thoughts-mcp
```

Expected: linker error **`main` not found** until Task 4.

### Step 3: Commit Cargo-only

```bash
git add src-tauri/Cargo.toml src-tauri/Cargo.lock
git commit -m "build: add mcp-server feature gate and curated-thoughts-mcp bin target"
```

---

## Task 4: Implement MCP stdio server (**`rmcp`** tools)

**Files:**
- Create: `src-tauri/src/bin/curated_thoughts_mcp.rs`

### Step 1: Skeleton `#[tool_router]` + main

Adapt from **`rmcp`** README (**Tools** § **Calculator**).

```rust
//! Curated Thoughts MCP — stdio. Build:
//! `cargo build -p curated-thoughts --features mcp-server --bin curated-thoughts-mcp`

use std::sync::{Arc, Mutex};

use anyhow::Result;
use rusqlite::Connection;

use rmcp::{
    handler::server::wrapper::Parameters,
    schemars::JsonSchema,
    tool,
    tool_router,
    ServiceExt,
    transport::stdio,
};
use serde::Deserialize;

use tauri_app_lib::embedder::EmbedProfile;
use tauri_app_lib::retrieval::{
    load_embed_profile, open_brain_readonly, related_chunks_facade, resolve_brain_paths,
    semantic_search_chunks,
};

#[derive(Debug, Deserialize, JsonSchema)]
struct SemanticParams {
    #[schemars(description = "Search query text (embedded with vault embed_profile)")]
    query: String,
    #[schemars(description = "Maximum chunks (default 10, clamped 1–50)")]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct RelatedParams {
    #[schemars(description = "Document path as stored in Indexed documents rows")]
    doc_path: String,
    #[schemars(description = "Max related chunks (default 5, clamped 1–10)")]
    limit: Option<usize>,
}

#[derive(Clone)]
struct CtServer {
    conn: Arc<Mutex<Connection>>,
    profile: EmbedProfile,
}

#[tool_router(server_handler)]
impl CtServer {
    #[tool(description = "Semantic vault search (cosine similarity over stored embeddings)")]
    fn vault_semantic_search(
        &self,
        Parameters(p): Parameters<SemanticParams>,
    ) -> Result<String, String> {
        let limit = p.limit.unwrap_or(10);
        let g = self.conn.lock().map_err(|e| e.to_string())?;
        let hits = semantic_search_chunks(&g, &self.profile, &p.query, limit).map_err(|e| {
            format!(
                "semantic_search failed — check Ollama/local embedder, indexed docs, CURATED_* paths: {}",
                e
            )
        })?;
        serde_json::to_string(&hits).map_err(|e| e.to_string())
    }

    #[tool(description = "Related chunks from other documents vs average embedding of doc_path")]
    fn vault_related_chunks(
        &self,
        Parameters(p): Parameters<RelatedParams>,
    ) -> Result<String, String> {
        let limit = p.limit.unwrap_or(5);
        let g = self.conn.lock().map_err(|e| e.to_string())?;
        let hits = related_chunks_facade(&g, &p.doc_path, limit)
            .map_err(|e| format!("related_chunks failed: {}", e))?;
        serde_json::to_string(&hits).map_err(|e| e.to_string())
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    eprintln!(
        "[curated-thoughts-mcp] starting (logging to stderr — never write diagnostics to stdout)"
    );

    let (db_path, config_path) = resolve_brain_paths()?;
    let profile = load_embed_profile(&config_path)?;
    let conn = open_brain_readonly(&db_path)?;

    let srv = CtServer {
        conn: Arc::new(Mutex::new(conn)),
        profile,
    };

    let handle = srv.serve(stdio()).await?;
    handle.waiting().await?;
    Ok(())
}
```

**Cargo:** Ensure **`tokio`** with **`macros` + `rt`** is available when **`--features mcp-server`** builds (add explicit **`tokio`** dep under optional feature shim if rustc errors).

Adapt imports if **`rmcp 1.x`** renames **`JsonSchema`** path or **`transport::stdio()`** ctor — follow compiler errors.

**Semantics:** Returning **`serde_json`** string of **`Vec<SearchResult>`** satisfies MCP **`CallToolResult`** text content expectation; alternatively wrap in **`rmcp`** content builder if host expects structured JSON parts (follow **`rmcp` examples**).

### Step 3: Build MCP binary

```bash
cd src-tauri
cargo build -p curated-thoughts --features mcp-server --bin curated-thoughts-mcp
```

Expected: **`Finished dev`** without errors.

### Step 4: Manual smoke (**optional** Cursor / MCP inspector)

Configure host to invoke:

`/path/to/target/debug/curated-thoughts-mcp` env `CURATED_BRAIN_DIR=$HOME/.brain`

Call **`vault_semantic_search`** `{ "query": "test", "limit": 3 }` — expects JSON **`SearchResult[]`**.

### Step 5: Commit

```bash
git add src-tauri/src/bin/curated_thoughts_mcp.rs src-tauri/Cargo.toml src-tauri/Cargo.lock
git commit -m "feat(mcp): stdio vault_semantic_search and vault_related_chunks"
```

---

## Task 5: Documentation + housekeeping

**Files:**
- Modify: `README.md` (project root file `README.md` in repo root)
- Optionally: `src-tauri/tests/README.md` — MCP build one-liner

### Step 1: README section (MCP)

Add **“MCP agent server (experimental)”**:

- Build: **`cargo build -p curated-thoughts --features mcp-server --bin curated-thoughts-mcp`** from **`src-tauri/`**
- **`CURATED_BRAIN_DIR`** (default `~/.brain`)
- **`CURATED_BRAIN_DB`** / **`CURATED_BRAIN_CONFIG`** overrides
- Security bullets (stdin server, exposes chunk text — trust boundary)
- Example Cursor **`mcp.json`** fragment:

```json
{
  "mcpServers": {
    "curated-thoughts": {
      "command": "/ABS/PATH/target/debug/curated-thoughts-mcp",
      "env": {
        "CURATED_BRAIN_DIR": "${env:HOME}/.brain"
      }
    }
  }
}
```

Mention **`CURATED_EMBED_STUB`** is **test-only**.

### Step 2: Regression suite

```bash
cd src-tauri
cargo test -p curated-thoughts --test retrieval_facade
cargo build -p curated-thoughts --features mcp-server --bin curated-thoughts-mcp
cargo test -p curated-thoughts --features test-utils 2>&1 | tail -20
```

### Step 3: Commit docs

```bash
git add README.md src-tauri/tests/README.md
git commit -m "docs: MCP server usage and env vars"
```

---

## Plan self-review vs spec

| Spec § | Satisfied by |
|--------|----------------|
| Parity **`SearchResult`** | Façae calls **`search::*`** unchanged; MCP returns **`serde_json`** of **`Vec<SearchResult>`** |
| Read-only MCP DB | **`open_brain_readonly`** |
| Env config | **`resolve_brain_paths`** |
| Two tools naming | MCP **`vault_semantic_search`** / **`vault_related_chunks`** |
| Security local stdio | README + **`eprintln!`** guideline |
| Tests | **`retrieval_facade`** + Tauri regressions |

**Residual risk:** `rmcp` API drift — implementer aligns macro imports with crates.io **`1.x`** README for their exact version**.** SQLite WAL + concurrent read-only open may SQLITE_BUSY — document **`PRAGMA` / retry** outside v0 scope per spec caveat.

---

**Plan complete and saved to `docs/superpowers/plans/2026-05-08-mcp-retrieval-facade.md`.**

Execution options:

1. **Subagent-driven (recommended)** — fresh subagent per task, human review checkpoints (`superpowers:subagent-driven-development`).
2. **Inline execution** — run tasks in series here (`superpowers:executing-plans`).

Which approach do you want?
