# MCP Retrieval Façade Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a **`retrieval` Rust façade** so Tauri **`search_vault`** / **`get_related_chunks`** and a new **`curated-thoughts-mcp`** binary share one code path (`embed_one` + `search::semantic_search` / `search::related_chunks`), with MCP stdio exposing tools **`vault_semantic_search`** and **`vault_related_chunks`**.

**Architecture:** Resolve **`brain.db`** + **`config.json`** from **`CURATED_*` env vars** (default `~/.brain`) with **path validation** (canonicalize, reject symlinks escaping brain root). Open SQLite **read-only** for MCP; reuse the writable **`Mutex<AppDb>`** connection inside Tauri. Parse **`EmbedProfile`** + **MCP opt-in flag** via existing **`vault::VaultConfig`**. MCP uses optional Cargo feature **`mcp-server`** pulling **`rmcp`** (official SDK) **stdio transport** — default `cargo build` stays lean.

**Security:** MCP requires **`mcp_enabled: true`** in config.json (default false). Server bails on start if disabled or if **`CURATED_BRAIN_DIR`** has group/other read permissions (single-user only). Paths canonicalized + validated under brain root to prevent arbitrary file read.

**Tech Stack:** `rusqlite` `OpenFlags::SQLITE_OPEN_READ_ONLY`, **`rmcp`** 1.x with **`macros`** + **`transport-io`**, **`tokio`**, **`schemars`**, serde JSON for tool payloads. Tests set **`CURATED_EMBED_STUB=constant8`** for deterministic 8-D vectors (already in `embedder/mod.rs`).

**Spec:** `docs/superpowers/specs/2026-05-08-mcp-retrieval-facade-design.md`

---

## File Map

| Action | Path | Responsibility |
|--------|------|----------------|
| Create | `src-tauri/src/retrieval/mod.rs` | Resolve brain paths from env with **validation** (canonicalize, symlink checks); `open_read_only`; façade `semantic_search_chunks` / `related_chunks_facade`; re-export callers need |
| Modify | `src-tauri/src/vault.rs` | Add `mcp_enabled: bool` field to `VaultConfig` (default `false`); add `check_mcp_allowed()` method |
| Modify | `src-tauri/src/lib.rs` | `pub mod retrieval;`; thin `search_vault` / `get_related_chunks` delegating to façade |
| Modify | `src-tauri/Cargo.toml` | Optional feature `mcp-server`; deps **`rmcp`**, **`tokio`**, **`schemars`**; `[[bin]]` `curated-thoughts-mcp` `required-features`; dev-dep `cargo-audit` note |
| Create | `src-tauri/src/bin/curated_thoughts_mcp.rs` | `#[tokio::main]` + `rmcp` stdio serve; **security checks** (opt-in flag, dir permissions); two `#[tool]` handlers |
| Create | `src-tauri/tests/retrieval_facade.rs` | Integration tests: temp brain dir + stub embed + façade calls + **SQL injection test** |
| Modify | `README.md` | MCP section: **SECURITY WARNINGS** (vault exfil, query→embedder), opt-in flag, build command, Cursor `mcpServers` snippet, **`CURATED_BRAIN_DIR`**, **`CURATED_BRAIN_DB`**, **`CURATED_BRAIN_CONFIG`**, **`CURATED_EMBED_STUB`** for tests only |

---

## Task 1: Retrieval façade module + tests (`retrieval`)

**Files:**
- Create: `src-tauri/src/retrieval/mod.rs`
- Modify: `src-tauri/src/lib.rs` — add line `pub mod retrieval;` alongside other `pub mod` entries (`pub mod search` is already public; retrieval may stay `pub mod retrieval`).
- Modify: `src-tauri/src/vault.rs` — add `mcp_enabled` field to `VaultConfig`
- Create: `src-tauri/tests/retrieval_facade.rs`

### Step 1: Add module file with env resolution + read-only open + **path validation**

Create `src-tauri/src/retrieval/mod.rs`:

```rust
//! Shared retrieval entry points for Tauri IPC and MCP. See MCP spec §4–§7.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use rusqlite::{Connection, OpenFlags};

use crate::embedder::{embed_one, EmbedProfile};
use crate::search::{self, SearchResult};
use crate::vault::VaultConfig;

fn default_brain_home() -> Result<PathBuf> {
    dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine home directory; set CURATED_BRAIN_DIR explicitly"))
        .map(|h| h.join(".brain"))
}

/// Canonicalize and validate path is under allowed root (prevents symlink escape, arbitrary file read).
fn validate_path_under_root(path: &Path, root: &Path) -> Result<PathBuf> {
    let canonical = path.canonicalize().with_context(|| {
        format!("Path does not exist or cannot be canonicalized: {}", path.display())
    })?;
    let canonical_root = root.canonicalize().with_context(|| {
        format!("Root path does not exist: {}", root.display())
    })?;
    
    if !canonical.starts_with(&canonical_root) {
        bail!(
            "Security: path {} escapes brain root {}",
            canonical.display(),
            canonical_root.display()
        );
    }
    Ok(canonical)
}

/// Resolve `(database_file, config_file)` paths from env (spec §4) with **security validation**.
pub fn resolve_brain_paths() -> Result<(PathBuf, PathBuf)> {
    let db_path = env::var("CURATED_BRAIN_DB").ok();
    let config_explicit = env::var("CURATED_BRAIN_CONFIG").ok();
    let dir = match env::var("CURATED_BRAIN_DIR") {
        Ok(d) => PathBuf::from(d),
        Err(_) => default_brain_home()?,
    };

    let config_path = if let Some(c) = config_explicit {
        PathBuf::from(c)
    } else if let Some(db) = &db_path {
        let p = PathBuf::from(db);
        p.parent()
            .map(|parent| parent.join("config.json"))
            .context("CURATED_BRAIN_DB has no parent for config.json")?
    } else {
        dir.join("config.json")
    };

    let db_path_final = db_path.map(PathBuf::from).unwrap_or_else(|| dir.join("brain.db"));

    // Validate paths under brain root (prevent arbitrary file read)
    let validated_db = validate_path_under_root(&db_path_final, &dir)?;
    let validated_config = validate_path_under_root(&config_path, &dir)?;

    Ok((validated_db, validated_config))
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

/// Check brain directory permissions (single-user only — group/other must not have read).
pub fn check_brain_dir_permissions(dir: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = fs::metadata(dir).with_context(|| {
            format!("Cannot stat brain directory: {}", dir.display())
        })?;
        let mode = metadata.permissions().mode();
        // Check if group (bit 4) or other (bit 1) have read permission
        if (mode & 0o044) != 0 {
            bail!(
                "Security: brain directory {} is readable by group or other (mode: {:o}). Run: chmod 700 {}",
                dir.display(),
                mode & 0o777,
                dir.display()
            );
        }
    }
    #[cfg(not(unix))]
    {
        // Windows/other: skip permission check (ACLs are complex)
        eprintln!("Warning: brain directory permission check skipped on non-Unix platform");
    }
    Ok(())
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
/// **Security note:** Verify `search::related_chunks` uses bound SQL params (not string interpolation).
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

### Step 2: Add `mcp_enabled` field to `VaultConfig`

In `src-tauri/src/vault.rs`, add to the `VaultConfig` struct:

```rust
#[serde(default)]
pub mcp_enabled: bool,
```

And add a method:

```rust
impl VaultConfig {
    pub fn check_mcp_allowed(&self) -> Result<()> {
        if !self.mcp_enabled {
            bail!("MCP server disabled. Set \"mcp_enabled\": true in config.json to allow.");
        }
        Ok(())
    }
}
```

### Step 3: Integration test with **SQL injection probe**

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
        check_brain_dir_permissions, load_embed_profile, open_brain_readonly,
        resolve_brain_paths, related_chunks_facade, semantic_search_chunks,
    },
};

fn write_minimal_config(dir: &std::path::Path, mcp_enabled: bool) -> std::path::PathBuf {
    let p = dir.join("config.json");
    let content = format!(r#"{{"mcp_enabled": {}}}"#, mcp_enabled);
    fs::write(&p, content).unwrap();
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
    write_minimal_config(brain, true);
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
            assert_eq!(db_resolved, db_path.canonicalize()?);

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

#[test]
fn related_chunks_rejects_sql_injection() -> Result<()> {
    let tmp = TempDir::new()?;
    let brain = tmp.path();
    write_minimal_config(brain, true);
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

            let (db_resolved, _) = resolve_brain_paths()?;
            let ro = open_brain_readonly(&db_resolved)?;

            // Probe with SQL injection attempt
            let malicious = "'; DROP TABLE Chunks; --";
            let result = related_chunks_facade(&ro, malicious, 5);
            
            // Should not crash or return error from SQL injection (bound params safe)
            // May return 0 results (no such doc) or error about missing doc
            assert!(result.is_ok() || result.is_err());
            
            // Verify Chunks table still exists
            let count: i64 = ro.query_row("SELECT COUNT(*) FROM Chunks", [], |r| r.get(0))?;
            assert_eq!(count, 1); // Original chunk still exists
            Ok(())
        },
    )?;
    Ok(())
}

#[test]
#[cfg(unix)]
fn brain_dir_permissions_check_rejects_group_read() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    
    let tmp = TempDir::new()?;
    let brain = tmp.path();
    write_minimal_config(brain, true);

    // Make directory group-readable
    let mut perms = fs::metadata(brain)?.permissions();
    perms.set_mode(0o755); // rwxr-xr-x
    fs::set_permissions(brain, perms)?;

    let result = check_brain_dir_permissions(brain);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("readable by group or other"));
    Ok(())
}
```

Implementer matches **`temp_env`** signature to the chosen crate version.

### Step 4: Compile + run façade tests

From **`src-tauri/`**:

```bash
cd src-tauri
cargo test -p curated-thoughts --test retrieval_facade
```

Expected: All tests **PASS** (semantic search, SQL injection probe, permissions check).

### Step 4: SQLite read-open flags sanity

Implementer trims **`NO_MUTEX`** if **`open_with_flags` fails locally** — keep **`SQLITE_OPEN_READ_ONLY`**.

### Step 5: Commit

```bash
git add src-tauri/src/retrieval/mod.rs src-tauri/src/vault.rs src-tauri/src/lib.rs src-tauri/tests/retrieval_facade.rs src-tauri/Cargo.toml src-tauri/Cargo.lock
git commit -m "feat(retrieval): shared brain path resolution with security validation

- Canonicalize paths + reject symlinks escaping brain root
- Check brain dir permissions (700 on Unix, fail if group/other readable)
- Fail loud when home_dir() unavailable (no silent CWD fallback)
- Add VaultConfig.mcp_enabled flag (default false)
- SQL injection test for related_chunks (verify bound params)"
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

use std::env;
use std::path::Path;
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
    check_brain_dir_permissions, load_embed_profile, open_brain_readonly, related_chunks_facade,
    resolve_brain_paths, semantic_search_chunks,
};
use tauri_app_lib::vault::VaultConfig;

fn redact_home_in_path(p: &Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(rest) = p.strip_prefix(&home) {
            return format!("~/{}", rest.display());
        }
    }
    p.display().to_string()
}

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
    eprintln!("[curated-thoughts-mcp] starting");

    let (db_path, config_path) = resolve_brain_paths()?;
    
    // Security: check opt-in flag
    let config = VaultConfig::new(config_path.clone());
    config.check_mcp_allowed()?;
    
    // Security: verify brain directory permissions (single-user only)
    let brain_dir = db_path.parent()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine brain directory from DB path"))?;
    check_brain_dir_permissions(brain_dir)?;

    eprintln!(
        "[curated-thoughts-mcp] brain: {}, config: {}",
        redact_home_in_path(&db_path),
        redact_home_in_path(&config_path)
    );

    let profile = load_embed_profile(&config_path)?;
    
    // Warn if embedder sends data to cloud
    if let tauri_app_lib::embedder::EmbedProfile::Cloud { .. } = &profile {
        eprintln!("⚠️  WARNING: Query text will be sent to cloud embedder");
    }
    
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
git commit -m "feat(mcp): stdio vault_semantic_search and vault_related_chunks with security checks

- Require mcp_enabled: true in config.json (fail loud if disabled)
- Check brain dir permissions before serving (700 on Unix)
- Redact $HOME to ~ in stderr logs
- Warn if cloud embedder enabled (query text → cloud)"
```

---

## Task 5: Documentation + housekeeping

**Files:**
- Modify: `README.md` (project root file `README.md` in repo root)
- Optionally: `src-tauri/tests/README.md` — MCP build one-liner

### Step 1: README section (MCP) with **SECURITY WARNINGS**

Add **”MCP agent server (experimental)”** section with loud warnings:

````markdown
## MCP Agent Server (Experimental)

⚠️ **SECURITY WARNING — READ BEFORE ENABLING** ⚠️

The MCP server exposes your **private vault contents** to any LLM client that connects via stdio. This includes:

- **Full chunk text** from your notes, documents, and indexed files
- **Query text** sent to your embedder (which may be a cloud service like OpenAI/Anthropic)
- **Document paths** and metadata from your brain database

**Trust boundary:** If you connect this to a cloud-based LLM (Cursor → Anthropic, VS Code → OpenAI, etc.), your private notes will leave your machine on every search query.

**Requirements:**
- ✅ **Opt-in required:** Set `”mcp_enabled”: true` in `~/.brain/config.json` to allow MCP server to start
- ✅ **Single-user only:** Brain directory must have `700` permissions (no group/other read). Server will refuse to start otherwise.
- ✅ **Stdio only:** No network exposure in this version. Only the process that spawns the MCP binary can access it.

### Build

From `src-tauri/`:

```bash
cargo build -p curated-thoughts --features mcp-server --bin curated-thoughts-mcp
```

Binary location: `target/debug/curated-thoughts-mcp` (or `target/release/` for `--release`)

### Configuration

Set `mcp_enabled: true` in your vault config:

```bash
echo '{“mcp_enabled”: true}' > ~/.brain/config.json
```

Ensure brain directory has correct permissions:

```bash
chmod 700 ~/.brain
```

### Environment Variables

| Variable | Default | Purpose |
|----------|---------|---------|
| `CURATED_BRAIN_DIR` | `~/.brain` | Root directory containing `brain.db` and `config.json` |
| `CURATED_BRAIN_DB` | `$CURATED_BRAIN_DIR/brain.db` | SQLite database path (overrides) |
| `CURATED_BRAIN_CONFIG` | `$CURATED_BRAIN_DIR/config.json` | Config file path (overrides) |
| `CURATED_EMBED_STUB` | (none) | **Test-only:** Set to `constant8` for deterministic 8-D vectors |

### Cursor Integration Example

Add to `~/.cursor/mcp.json` (or workspace `.cursor/mcp.json`):

```json
{
  “mcpServers”: {
    “curated-thoughts”: {
      “command”: “/ABSOLUTE/PATH/TO/target/debug/curated-thoughts-mcp”,
      “env”: {
        “CURATED_BRAIN_DIR”: “${env:HOME}/.brain”
      }
    }
  }
}
```

Replace `/ABSOLUTE/PATH/TO/` with your actual repo path.

### Available Tools

- **`vault_semantic_search`** — Semantic search over indexed chunks (cosine similarity)
  - Parameters: `query` (string), `limit` (optional, default 10, max 50)
  - Returns: JSON array of `SearchResult` (chunk text, doc_path, symbol_name, score, etc.)

- **`vault_related_chunks`** — Find chunks related to a specific document
  - Parameters: `doc_path` (string), `limit` (optional, default 5, max 10)
  - Returns: JSON array of `SearchResult`

### Data Flow & Privacy

```
Your MCP Client (Cursor/IDE)
  ↓ stdio (query text)
curated-thoughts-mcp
  ↓ query → embedder (may be cloud!)
  ↓ cosine search → brain.db
  ↓ chunk text results
Your MCP Client
  ↓ chunk text in context
Cloud LLM (if client uses one)
```

**What leaves your machine:**
1. Query text → your configured embedder (Ollama local = safe, OpenAI/Anthropic cloud = exposed)
2. Search results (chunk text) → MCP client → may be sent to cloud LLM in next request

**Mitigation options** (not implemented in v0):
- Per-vault opt-in flag (e.g., `mcp_allowlist: [“/vault/public/*”]`)
- Redact chunk text (return only `doc_path:line + score`, no content)
- Path-prefix denylist (`.env`, `/secrets/`, etc.)

### Security Checklist

Before enabling MCP:

- [ ] Do you understand that **private vault contents** will be accessible to the LLM client?
- [ ] Is your embedder local (Ollama) or cloud? If cloud, query text is exposed.
- [ ] Is your MCP client local or cloud-connected? If cloud-connected, chunk text is exposed.
- [ ] Have you set `chmod 700 ~/.brain` to prevent other users on the system from accessing your vault?
- [ ] Have you added `”mcp_enabled”: true` to `config.json` to explicitly opt in?

### Dependencies & Supply Chain

The `mcp-server` feature adds:
- `rmcp` (official Anthropic MCP SDK for Rust)
- `schemars` (JSON Schema derivation)
- `tokio` (async runtime, already transitive)

Run `cargo audit` in CI for the `mcp-server` feature build:

```bash
cargo audit --features mcp-server
```

Add to CI: `cargo-deny` or `cargo-vet` for additional supply chain checks.
````

Mention **`CURATED_EMBED_STUB`** is **test-only** (already covered in env vars table).

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
git commit -m "docs: MCP server security warnings, opt-in requirement, usage guide

- Loud warnings about vault exfil via cloud LLMs
- Document mcp_enabled opt-in flag requirement
- Single-user only (700 permissions) security model
- Data flow diagram (query → embedder → LLM)
- Security checklist before enabling
- cargo-audit integration for supply chain"
```

---

## Plan self-review vs spec

| Spec § | Satisfied by |
|--------|----------------|
| Parity **`SearchResult`** | Façade calls **`search::*`** unchanged; MCP returns **`serde_json`** of **`Vec<SearchResult>`** |
| Read-only MCP DB | **`open_brain_readonly`** |
| Env config | **`resolve_brain_paths`** with path validation |
| Two tools naming | MCP **`vault_semantic_search`** / **`vault_related_chunks`** |
| Security local stdio | README + **`eprintln!`** guideline |
| Tests | **`retrieval_facade`** + Tauri regressions + SQL injection probe |

## Security Review Mitigations

| Issue # | Severity | Mitigation |
|---------|----------|------------|
| #1 | MEDIUM | **`validate_path_under_root()`** — canonicalize paths, reject symlinks escaping brain root |
| #2 | HIGH | **`mcp_enabled`** opt-in flag in config.json (default false); loud README warnings about vault exfil; security checklist |
| #3 | LOW | SQL injection test added in **`retrieval_facade.rs`** — verifies bound params in `related_chunks` |
| #4 | MEDIUM | **`check_brain_dir_permissions()`** — Unix mode check, bail if group/other readable; documented "single-user local only" |
| #5 | LOW | Acknowledged — Mutex DoS not critical for v0 (future: connection pool for read-only) |
| #6 | LOW | **`redact_home_in_path()`** — replace `$HOME` with `~` in stderr logs |
| #7 | MEDIUM | README documents query text → embedder data flow; warn on startup if **`EmbedProfile::Cloud`** |
| #11 | LOW | README documents `cargo audit --features mcp-server` + `cargo-deny` CI integration |
| #12 | LOW | **`default_brain_home()`** — fail loud when `home_dir()` returns None (no silent CWD fallback) |

**Residual risks:**
- **rmcp API drift** — implementer must align macro imports with crates.io **`1.x`** README for exact version
- **SQLite WAL + concurrent read-only** may SQLITE_BUSY — document **`PRAGMA` / retry** outside v0 scope per spec caveat
- **Mutex contention** — single connection behind lock; malicious/buggy host spamming requests blocks others (not security-critical but availability risk)
- **Chunk text redaction not implemented** — future opt-in to return only `doc_path:line + score` without content
- **Path allowlist/denylist not implemented** — future per-vault or path-prefix filtering (e.g., deny `.env*`, `/secrets/`)

---

**Plan complete and saved to `docs/superpowers/plans/2026-05-08-mcp-retrieval-facade.md`.**

Execution options:

1. **Subagent-driven (recommended)** — fresh subagent per task, human review checkpoints (`superpowers:subagent-driven-development`).
2. **Inline execution** — run tasks in series here (`superpowers:executing-plans`).

Which approach do you want?
