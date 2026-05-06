# Curated Thoughts — Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Scaffold the Curated Thoughts Tauri 2.x desktop app with a Rust backend (SQLite schema, vault config, file watcher, Ollama setup), a 4-step setup wizard, and a 3-panel app shell skeleton.

**Architecture:** Tauri 2.x wraps a React 18 + TypeScript frontend and a Rust backend. Rust owns all file I/O, SQLite, file watching, and Ollama management. Frontend communicates via typed `invoke()` calls and Tauri events. Foundation ends with a running app that shows wizard on first launch, then 3-panel shell.

**Tech Stack:** Tauri 2.x, React 18 + TypeScript, Vite, Rust, rusqlite (bundled), notify 6, reqwest 0.12 (blocking), Vitest 1.x, @testing-library/react

---

## File Map

| File | Responsibility |
|---|---|
| `src-tauri/src/main.rs` | Tauri entry point — calls `lib::run()` |
| `src-tauri/src/lib.rs` | App builder, command registration, state management |
| `src-tauri/src/db/mod.rs` | DB module re-exports |
| `src-tauri/src/db/connection.rs` | Open SQLite, run migrations, `AppDb` state type |
| `src-tauri/src/db/schema.rs` | Migration SQL for all tables |
| `src-tauri/src/vault/mod.rs` | Vault module re-exports |
| `src-tauri/src/vault/config.rs` | Read/write vault root path to `~/.brain/config.json`; Tauri commands |
| `src-tauri/src/watcher/mod.rs` | Watcher module re-exports |
| `src-tauri/src/watcher/fs_watcher.rs` | notify-based watcher; emits `vault-event` to frontend |
| `src-tauri/src/setup/mod.rs` | Setup module re-exports |
| `src-tauri/src/setup/ollama.rs` | Detect Ollama, pull model with progress; Tauri commands |
| `src/main.tsx` | React root mount |
| `src/App.tsx` | Setup gate: wizard if not ready, shell if ready |
| `src/lib/tauri.ts` | Typed `invoke()` wrappers for all Rust commands |
| `src/lib/events.ts` | Typed Tauri event listeners |
| `src/hooks/useSetupStatus.ts` | Checks vault path + Ollama status, returns `{ needsSetup, loading }` |
| `src/components/setup/SetupWizard.tsx` | 4-step wizard container with step state |
| `src/components/setup/StepWelcome.tsx` | Step 1: welcome screen |
| `src/components/setup/StepOllama.tsx` | Step 2: Ollama detection + model pull with progress bar |
| `src/components/setup/StepVaultPicker.tsx` | Step 3: system folder picker via dialog plugin |
| `src/components/setup/StepDone.tsx` | Step 4: completion screen, calls `onComplete` |
| `src/components/shell/AppShell.tsx` | 3-panel flex layout container |
| `src/components/shell/Sidebar.tsx` | Left panel: search input + folder tree placeholder + review badge |
| `src/components/shell/EditorPane.tsx` | Center panel: editor placeholder |
| `src/components/shell/RelatedNotes.tsx` | Right panel: related notes placeholder |
| `src/index.css` | Layout styles for shell and wizard |
| `src/test-setup.ts` | Vitest global mocks for Tauri invoke and events |

---

### Task 1: Tauri project scaffold ✅

**Files:**
- Create: `src-tauri/Cargo.toml`
- Create: `package.json`
- Create: `vite.config.ts`
- Create: `src-tauri/tauri.conf.json`
- Create: `src/test-setup.ts`

Completed. Requires Rust/cargo to verify build.

---

### Task 2: SQLite connection + schema (Rust)

**Files:**
- Create: `src-tauri/src/db/mod.rs`
- Create: `src-tauri/src/db/connection.rs`
- Create: `src-tauri/src/db/schema.rs`

**Prerequisite:** Rust toolchain installed (`rustup.rs`)

- [ ] **Step 1: Create `src-tauri/src/db/schema.rs`**

```rust
// embeddings table is omitted here — added in Sub-project 2 when sqlite-vec is integrated.
pub const MIGRATION_V1: &str = "
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS documents (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    path            TEXT    NOT NULL UNIQUE,
    hash            TEXT    NOT NULL,
    tier            TEXT    NOT NULL CHECK(tier IN ('user_doc', 'wiki')),
    folder_rules_id INTEGER,
    last_indexed    INTEGER,
    status          TEXT    NOT NULL DEFAULT 'pending'
                    CHECK(status IN ('pending', 'indexed', 'error', 'orphaned'))
);

CREATE TABLE IF NOT EXISTS chunks (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    doc_id     INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    chunk_text TEXT    NOT NULL,
    position   INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS wiki_pages (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    path           TEXT    NOT NULL UNIQUE,
    source_doc_ids TEXT    NOT NULL DEFAULT '[]',
    generated_by   TEXT    NOT NULL,
    last_synced    INTEGER,
    status         TEXT    NOT NULL DEFAULT 'pending_review'
                   CHECK(status IN ('pending_review', 'approved', 'rejected'))
);

CREATE TABLE IF NOT EXISTS folder_rules (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    folder_path     TEXT    NOT NULL UNIQUE,
    librarian_mode  TEXT    NOT NULL DEFAULT 'index'
                    CHECK(librarian_mode IN ('index', 'summarize', 'synthesize')),
    provider_override TEXT,
    model_override    TEXT,
    auto_approve      INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS schema_version (
    version INTEGER PRIMARY KEY
);

INSERT OR IGNORE INTO schema_version (version) VALUES (1);
";
```

- [ ] **Step 2: Write failing tests in `src-tauri/src/db/connection.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_db_initializes_with_schema_version() {
        let conn = open_in_memory().unwrap();
        let version: i64 = conn
            .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 1);
    }

    #[test]
    fn test_all_tables_exist() {
        let conn = open_in_memory().unwrap();
        for table in &["documents", "chunks", "wiki_pages", "folder_rules"] {
            let count: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "table '{}' not found in schema", table);
        }
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

```bash
cd src-tauri && cargo test db::connection
```

Expected: `error[E0425]: cannot find function 'open_in_memory' in this scope`

- [ ] **Step 4: Implement `src-tauri/src/db/connection.rs`**

```rust
use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;
use crate::db::schema::MIGRATION_V1;

pub struct AppDb(pub Connection);

impl AppDb {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch(MIGRATION_V1)?;
        Ok(AppDb(conn))
    }
}

pub fn open_in_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(MIGRATION_V1)?;
    Ok(conn)
}
```

- [ ] **Step 5: Create `src-tauri/src/db/mod.rs`**

```rust
pub mod connection;
pub mod schema;

pub use connection::AppDb;
```

- [ ] **Step 6: Run tests to verify they pass**

```bash
cd src-tauri && cargo test db::
```

Expected:
```
test db::connection::tests::test_db_initializes_with_schema_version ... ok
test db::connection::tests::test_all_tables_exist ... ok
```

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src/db/
git commit -m "feat: add SQLite connection with V1 schema migration"
```

---

### Task 3: Vault config (Rust)

**Files:**
- Create: `src-tauri/src/vault/mod.rs`
- Create: `src-tauri/src/vault/config.rs`

- [ ] **Step 1: Write failing tests in `src-tauri/src/vault/config.rs`**

```rust
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
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cd src-tauri && cargo test vault::config
```

Expected: `error[E0422]: cannot find struct 'VaultConfig' in this scope`

- [ ] **Step 3: Implement `src-tauri/src/vault/config.rs`**

```rust
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

#[derive(Serialize, Deserialize, Default)]
struct ConfigFile {
    vault_path: Option<String>,
}

pub struct VaultConfig {
    config_path: PathBuf,
}

impl VaultConfig {
    pub fn new(config_path: PathBuf) -> Self {
        VaultConfig { config_path }
    }

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
}
```

- [ ] **Step 4: Create `src-tauri/src/vault/mod.rs`**

```rust
pub mod config;
pub use config::VaultConfig;
```

- [ ] **Step 5: Run tests to verify they pass**

```bash
cd src-tauri && cargo test vault::
```

Expected:
```
test vault::config::tests::test_get_returns_none_when_file_absent ... ok
test vault::config::tests::test_set_then_get_roundtrip ... ok
test vault::config::tests::test_set_overwrites_existing_path ... ok
```

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/vault/
git commit -m "feat: add vault config persistence with get/set"
```

---

### Task 4: File watcher (Rust)

**Files:**
- Create: `src-tauri/src/watcher/mod.rs`
- Create: `src-tauri/src/watcher/fs_watcher.rs`

- [ ] **Step 1: Write failing tests in `src-tauri/src/watcher/fs_watcher.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, sync::mpsc, time::Duration};
    use tempfile::TempDir;

    #[test]
    fn test_watcher_detects_new_file() {
        let tmp = TempDir::new().unwrap();
        let (tx, rx) = mpsc::channel::<VaultEvent>();
        start_watcher(tmp.path().to_path_buf(), move |e| { tx.send(e).ok(); }).unwrap();

        fs::write(tmp.path().join("note.md"), "hello").unwrap();

        let event = rx.recv_timeout(Duration::from_secs(3)).expect("no event received");
        assert!(matches!(event, VaultEvent::Added(_)));
    }

    #[test]
    fn test_watcher_detects_deleted_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("note.md");
        fs::write(&path, "hello").unwrap();

        let (tx, rx) = mpsc::channel::<VaultEvent>();
        start_watcher(tmp.path().to_path_buf(), move |e| { tx.send(e).ok(); }).unwrap();

        std::thread::sleep(Duration::from_millis(200));
        fs::remove_file(&path).unwrap();

        let event = rx.recv_timeout(Duration::from_secs(3)).expect("no delete event");
        assert!(matches!(event, VaultEvent::Deleted(_)));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cd src-tauri && cargo test watcher::
```

Expected: `error[E0422]: cannot find enum 'VaultEvent' in this scope`

- [ ] **Step 3: Implement `src-tauri/src/watcher/fs_watcher.rs`**

```rust
use anyhow::Result;
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::{path::PathBuf, sync::mpsc, thread};

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind", content = "path")]
pub enum VaultEvent {
    Added(String),
    Modified(String),
    Deleted(String),
}

pub fn start_watcher<F>(vault_path: PathBuf, callback: F) -> Result<()>
where
    F: Fn(VaultEvent) + Send + 'static,
{
    let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
    let mut watcher = RecommendedWatcher::new(tx, Config::default())?;
    watcher.watch(&vault_path, RecursiveMode::Recursive)?;

    thread::spawn(move || {
        let _keep = watcher;
        for result in rx {
            let Ok(event) = result else { continue };
            for path in event.paths {
                let path_str = path.to_string_lossy().to_string();
                let vault_event = match event.kind {
                    EventKind::Create(_) => VaultEvent::Added(path_str),
                    EventKind::Modify(_) => VaultEvent::Modified(path_str),
                    EventKind::Remove(_) => VaultEvent::Deleted(path_str),
                    _ => continue,
                };
                callback(vault_event);
            }
        }
    });

    Ok(())
}
```

- [ ] **Step 4: Create `src-tauri/src/watcher/mod.rs`**

```rust
pub mod fs_watcher;
pub use fs_watcher::{start_watcher, VaultEvent};
```

- [ ] **Step 5: Run tests to verify they pass**

```bash
cd src-tauri && cargo test watcher::
```

Expected:
```
test watcher::fs_watcher::tests::test_watcher_detects_new_file ... ok
test watcher::fs_watcher::tests::test_watcher_detects_deleted_file ... ok
```

If tests are flaky on macOS due to FSEvents debounce, increase the `recv_timeout` to 5 seconds.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/watcher/
git commit -m "feat: add notify-based file watcher with Added/Modified/Deleted events"
```

---

### Task 5: Ollama setup logic (Rust)

**Files:**
- Create: `src-tauri/src/setup/mod.rs`
- Create: `src-tauri/src/setup/ollama.rs`

- [ ] **Step 1: Write failing tests in `src-tauri/src/setup/ollama.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_models_response_extracts_names() {
        let json = r#"{"models":[{"name":"llama3.2:3b"},{"name":"phi4-mini:latest"}]}"#;
        let models = parse_models_response(json).unwrap();
        assert_eq!(models, vec!["llama3.2:3b", "phi4-mini:latest"]);
    }

    #[test]
    fn test_parse_models_response_empty() {
        let json = r#"{"models":[]}"#;
        let models = parse_models_response(json).unwrap();
        assert!(models.is_empty());
    }

    #[test]
    fn test_parse_models_response_invalid_json_errors() {
        assert!(parse_models_response("not json").is_err());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cd src-tauri && cargo test setup::ollama
```

Expected: `error[E0425]: cannot find function 'parse_models_response' in this scope`

- [ ] **Step 3: Implement `src-tauri/src/setup/ollama.rs`**

```rust
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::process::Command;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct OllamaStatus {
    pub installed: bool,
    pub running: bool,
    pub models: Vec<String>,
}

#[derive(Deserialize)]
struct OllamaListResponse {
    models: Vec<OllamaModel>,
}

#[derive(Deserialize)]
struct OllamaModel {
    name: String,
}

pub fn parse_models_response(json: &str) -> Result<Vec<String>> {
    let resp: OllamaListResponse = serde_json::from_str(json)?;
    Ok(resp.models.into_iter().map(|m| m.name).collect())
}

pub fn check_ollama() -> OllamaStatus {
    let installed = Command::new("ollama").arg("--version").output().is_ok();

    if !installed {
        return OllamaStatus { installed: false, running: false, models: vec![] };
    }

    match reqwest::blocking::get("http://localhost:11434/api/tags") {
        Ok(resp) if resp.status().is_success() => {
            let text = resp.text().unwrap_or_default();
            let models = parse_models_response(&text).unwrap_or_default();
            OllamaStatus { installed: true, running: true, models }
        }
        _ => OllamaStatus { installed: true, running: false, models: vec![] },
    }
}

pub fn list_local_models() -> Result<Vec<String>> {
    let text = reqwest::blocking::get("http://localhost:11434/api/tags")?.text()?;
    parse_models_response(&text)
}

pub fn pull_model<F>(model_id: &str, on_progress: F) -> Result<()>
where
    F: Fn(u64, u64),
{
    use std::io::{BufRead, BufReader};

    let client = reqwest::blocking::Client::new();
    let resp = client
        .post("http://localhost:11434/api/pull")
        .json(&serde_json::json!({ "name": model_id }))
        .send()?;

    let reader = BufReader::new(resp);
    for line in reader.lines() {
        let line = line?;
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&line) {
            let completed = val["completed"].as_u64().unwrap_or(0);
            let total = val["total"].as_u64().unwrap_or(1);
            on_progress(completed, total);
        }
    }
    Ok(())
}
```

- [ ] **Step 4: Create `src-tauri/src/setup/mod.rs`**

```rust
pub mod ollama;
pub use ollama::{check_ollama, list_local_models, pull_model, OllamaStatus};
```

- [ ] **Step 5: Run tests to verify they pass**

```bash
cd src-tauri && cargo test setup::
```

Expected:
```
test setup::ollama::tests::test_parse_models_response_extracts_names ... ok
test setup::ollama::tests::test_parse_models_response_empty ... ok
test setup::ollama::tests::test_parse_models_response_invalid_json_errors ... ok
```

- [ ] **Step 6: Verify full Rust build passes**

```bash
cd src-tauri && cargo build
```

Expected: no errors or warnings about unused imports.

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src/setup/
git commit -m "feat: add Ollama detection and model pull with streaming progress"
```

---

### Task 6: Wire Rust modules into `lib.rs`

**Files:**
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Replace `src-tauri/src/lib.rs` with the full wired version**

```rust
mod db;
mod vault;
mod watcher;
mod setup;

use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};
use db::AppDb;
use vault::VaultConfig;
use setup::{check_ollama as ollama_check, list_local_models as ollama_models,
            pull_model as ollama_pull, OllamaStatus};
use watcher::start_watcher;

struct DbState(Mutex<AppDb>);
struct VaultConfigState(Mutex<VaultConfig>);

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
fn start_file_watcher(vault_path: String, app: AppHandle) -> Result<(), String> {
    start_watcher(vault_path.into(), move |event| {
        let _ = app.emit("vault-event", &event);
    })
    .map_err(|e| e.to_string())
}

// ── Ollama commands ───────────────────────────────────────────────────────────

#[tauri::command]
fn check_ollama() -> OllamaStatus {
    ollama_check()
}

#[tauri::command]
fn list_local_models() -> Result<Vec<String>, String> {
    ollama_models().map_err(|e| e.to_string())
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

    let db = AppDb::open(&brain_dir.join("brain.db")).expect("failed to open database");
    let config = VaultConfig::new(brain_dir.join("config.json"));

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_os::init())
        .manage(DbState(Mutex::new(db)))
        .manage(VaultConfigState(Mutex::new(config)))
        .invoke_handler(tauri::generate_handler![
            get_vault_path,
            set_vault_path,
            start_file_watcher,
            check_ollama,
            list_local_models,
            pull_model,
        ])
        .run(tauri::generate_context!())
        .expect("error running Tauri application");
}
```

- [ ] **Step 2: Verify full Rust build passes**

```bash
cd src-tauri && cargo build
```

Expected: no errors.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/lib.rs
git commit -m "feat: wire all Rust modules and Tauri commands into lib.rs"
```

---

### Task 7: Typed frontend bridge

**Files:**
- Create: `src/lib/tauri.ts`
- Create: `src/lib/events.ts`

- [ ] **Step 1: Create `src/lib/tauri.ts`**

```ts
import { invoke } from "@tauri-apps/api/core";

export interface OllamaStatus {
  installed: boolean;
  running: boolean;
  models: string[];
}

export const getVaultPath = (): Promise<string | null> =>
  invoke("get_vault_path");

export const setVaultPath = (path: string): Promise<void> =>
  invoke("set_vault_path", { path });

export const checkOllama = (): Promise<OllamaStatus> =>
  invoke("check_ollama");

export const listLocalModels = (): Promise<string[]> =>
  invoke("list_local_models");

export const pullModel = (modelId: string): Promise<void> =>
  invoke("pull_model", { modelId });

export const startFileWatcher = (vaultPath: string): Promise<void> =>
  invoke("start_file_watcher", { vaultPath });
```

- [ ] **Step 2: Create `src/lib/events.ts`**

```ts
import { listen, UnlistenFn } from "@tauri-apps/api/event";

export interface VaultEvent {
  kind: "Added" | "Modified" | "Deleted";
  path: string;
}

export interface PullProgress {
  completed: number;
  total: number;
}

export const onVaultEvent = (
  cb: (event: VaultEvent) => void
): Promise<UnlistenFn> =>
  listen<VaultEvent>("vault-event", (e) => cb(e.payload));

export const onPullProgress = (
  cb: (progress: PullProgress) => void
): Promise<UnlistenFn> =>
  listen<PullProgress>("ollama-pull-progress", (e) => cb(e.payload));
```

- [ ] **Step 3: Commit**

```bash
git add src/lib/
git commit -m "feat: add typed Tauri invoke wrappers and event listeners"
```

---

### Task 8: `useSetupStatus` hook

**Files:**
- Create: `src/hooks/useSetupStatus.ts`
- Create: `src/__tests__/useSetupStatus.test.ts`

- [ ] **Step 1: Write failing tests**

```ts
// src/__tests__/useSetupStatus.test.ts
import { renderHook, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { useSetupStatus } from "../hooks/useSetupStatus";

const mockInvoke = invoke as ReturnType<typeof vi.fn>;

test("needsSetup true when vault path is null", async () => {
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd === "get_vault_path") return Promise.resolve(null);
    if (cmd === "check_ollama") return Promise.resolve({ installed: true, running: true, models: ["llama3.2:3b"] });
    return Promise.resolve(null);
  });
  const { result } = renderHook(() => useSetupStatus());
  await waitFor(() => expect(result.current.loading).toBe(false));
  expect(result.current.needsSetup).toBe(true);
});

test("needsSetup false when vault set and Ollama running", async () => {
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd === "get_vault_path") return Promise.resolve("/Users/test/vault");
    if (cmd === "check_ollama") return Promise.resolve({ installed: true, running: true, models: ["llama3.2:3b"] });
    return Promise.resolve(null);
  });
  const { result } = renderHook(() => useSetupStatus());
  await waitFor(() => expect(result.current.loading).toBe(false));
  expect(result.current.needsSetup).toBe(false);
});

test("needsSetup true when Ollama not installed", async () => {
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd === "get_vault_path") return Promise.resolve("/Users/test/vault");
    if (cmd === "check_ollama") return Promise.resolve({ installed: false, running: false, models: [] });
    return Promise.resolve(null);
  });
  const { result } = renderHook(() => useSetupStatus());
  await waitFor(() => expect(result.current.loading).toBe(false));
  expect(result.current.needsSetup).toBe(true);
});
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
npm test -- useSetupStatus
```

Expected: `Cannot find module '../hooks/useSetupStatus'`

- [ ] **Step 3: Implement `src/hooks/useSetupStatus.ts`**

```ts
import { useEffect, useState } from "react";
import { checkOllama, getVaultPath } from "../lib/tauri";

export interface SetupStatus {
  loading: boolean;
  needsSetup: boolean;
  vaultPath: string | null;
  ollamaReady: boolean;
}

export function useSetupStatus(): SetupStatus {
  const [loading, setLoading] = useState(true);
  const [vaultPath, setVaultPath] = useState<string | null>(null);
  const [ollamaReady, setOllamaReady] = useState(false);

  useEffect(() => {
    Promise.all([getVaultPath(), checkOllama()])
      .then(([path, status]) => {
        setVaultPath(path);
        setOllamaReady(status.installed && status.running);
      })
      .finally(() => setLoading(false));
  }, []);

  return {
    loading,
    needsSetup: !vaultPath || !ollamaReady,
    vaultPath,
    ollamaReady,
  };
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
npm test -- useSetupStatus
```

Expected: 3 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/hooks/ src/__tests__/
git commit -m "feat: add useSetupStatus hook with vault and Ollama status detection"
```

---

### Task 9: Setup wizard

**Files:**
- Create: `src/components/setup/StepWelcome.tsx`
- Create: `src/components/setup/StepOllama.tsx`
- Create: `src/components/setup/StepVaultPicker.tsx`
- Create: `src/components/setup/StepDone.tsx`
- Create: `src/components/setup/SetupWizard.tsx`
- Create: `src/__tests__/SetupWizard.test.tsx`

- [ ] **Step 1: Write failing tests**

```tsx
// src/__tests__/SetupWizard.test.tsx
import { render, screen, fireEvent } from "@testing-library/react";
import { SetupWizard } from "../components/setup/SetupWizard";

test("renders welcome step on mount", () => {
  render(<SetupWizard onComplete={vi.fn()} />);
  expect(screen.getByText(/your second brain/i)).toBeInTheDocument();
  expect(screen.getByRole("button", { name: /get started/i })).toBeInTheDocument();
});

test("clicking Get Started advances to Ollama step", () => {
  render(<SetupWizard onComplete={vi.fn()} />);
  fireEvent.click(screen.getByRole("button", { name: /get started/i }));
  expect(screen.getByText(/install ollama/i)).toBeInTheDocument();
});

test("calls onComplete when done step button clicked", () => {
  const onComplete = vi.fn();
  render(<SetupWizard onComplete={onComplete} initialStep={3} />);
  fireEvent.click(screen.getByRole("button", { name: /open my brain/i }));
  expect(onComplete).toHaveBeenCalledTimes(1);
});
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
npm test -- SetupWizard
```

Expected: `Cannot find module '../components/setup/SetupWizard'`

- [ ] **Step 3: Create `src/components/setup/StepWelcome.tsx`**

```tsx
interface Props { onNext: () => void }

export function StepWelcome({ onNext }: Props) {
  return (
    <div className="setup-step">
      <h1>Your Second Brain</h1>
      <p>Private by default. Your documents never leave your machine.</p>
      <button onClick={onNext}>Get Started</button>
    </div>
  );
}
```

- [ ] **Step 4: Create `src/components/setup/StepOllama.tsx`**

```tsx
import { useEffect, useState } from "react";
import { checkOllama, pullModel } from "../../lib/tauri";
import { onPullProgress } from "../../lib/events";

interface Props { onNext: () => void }

const DEFAULT_MODEL = "llama3.2:3b";

export function StepOllama({ onNext }: Props) {
  const [phase, setPhase] = useState<"checking" | "needs-install" | "pulling" | "ready" | "error">("checking");
  const [progress, setProgress] = useState(0);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  useEffect(() => {
    checkOllama().then((s) => {
      if (s.installed && s.running && s.models.length > 0) {
        setPhase("ready");
      } else if (!s.installed) {
        setPhase("needs-install");
      } else {
        startPull();
      }
    });
  }, []);

  async function startPull() {
    setPhase("pulling");
    setProgress(0);
    const unlisten = await onPullProgress(({ completed, total }) => {
      setProgress(total > 0 ? Math.round((completed / total) * 100) : 0);
    });
    try {
      await pullModel(DEFAULT_MODEL);
      setPhase("ready");
    } catch (e) {
      setErrorMsg(String(e));
      setPhase("error");
    } finally {
      unlisten();
    }
  }

  return (
    <div className="setup-step">
      <h2>Install Ollama</h2>
      {phase === "checking" && <p>Checking for Ollama...</p>}
      {phase === "needs-install" && (
        <>
          <p>Ollama is required for local AI processing. Download it from <strong>ollama.com</strong>, then click below.</p>
          <button onClick={() => startPull()}>I've installed Ollama</button>
        </>
      )}
      {phase === "pulling" && (
        <>
          <p>Downloading {DEFAULT_MODEL}... {progress}%</p>
          <progress value={progress} max={100} style={{ width: "100%" }} />
        </>
      )}
      {phase === "ready" && (
        <>
          <p>Ollama is ready.</p>
          <button onClick={onNext}>Continue</button>
        </>
      )}
      {phase === "error" && (
        <>
          <p style={{ color: "red" }}>Error: {errorMsg}</p>
          <button onClick={() => startPull()}>Retry</button>
        </>
      )}
    </div>
  );
}
```

- [ ] **Step 5: Create `src/components/setup/StepVaultPicker.tsx`**

```tsx
import { open } from "@tauri-apps/plugin-dialog";
import { useState } from "react";
import { setVaultPath } from "../../lib/tauri";

interface Props { onNext: (path: string) => void }

export function StepVaultPicker({ onNext }: Props) {
  const [selected, setSelected] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  async function pickFolder() {
    const path = await open({ directory: true, multiple: false, title: "Choose your vault folder" });
    if (typeof path === "string") setSelected(path);
  }

  async function confirm() {
    if (!selected) return;
    setSaving(true);
    await setVaultPath(selected);
    setSaving(false);
    onNext(selected);
  }

  return (
    <div className="setup-step">
      <h2>Choose Your Vault</h2>
      <p>Pick the folder where your documents live. The app will watch it for changes.</p>
      <button onClick={pickFolder}>Browse...</button>
      {selected && <p className="selected-path">{selected}</p>}
      <button onClick={confirm} disabled={!selected || saving}>
        {saving ? "Saving..." : "Confirm"}
      </button>
    </div>
  );
}
```

- [ ] **Step 6: Create `src/components/setup/StepDone.tsx`**

```tsx
interface Props { onComplete: () => void }

export function StepDone({ onComplete }: Props) {
  return (
    <div className="setup-step">
      <h2>You're all set!</h2>
      <p>Your librarian is ready to start curating your thoughts.</p>
      <button onClick={onComplete}>Open My Brain</button>
    </div>
  );
}
```

- [ ] **Step 7: Create `src/components/setup/SetupWizard.tsx`**

```tsx
import { useState } from "react";
import { StepWelcome } from "./StepWelcome";
import { StepOllama } from "./StepOllama";
import { StepVaultPicker } from "./StepVaultPicker";
import { StepDone } from "./StepDone";

interface Props {
  onComplete: (vaultPath: string) => void;
  initialStep?: number;
}

export function SetupWizard({ onComplete, initialStep = 0 }: Props) {
  const [step, setStep] = useState(initialStep);
  const [vaultPath, setVaultPath] = useState<string>("");
  const next = () => setStep((s) => s + 1);

  return (
    <div className="setup-wizard">
      {step === 0 && <StepWelcome onNext={next} />}
      {step === 1 && <StepOllama onNext={next} />}
      {step === 2 && (
        <StepVaultPicker
          onNext={(path) => { setVaultPath(path); next(); }}
        />
      )}
      {step === 3 && <StepDone onComplete={() => onComplete(vaultPath)} />}
    </div>
  );
}
```

- [ ] **Step 8: Run tests to verify they pass**

```bash
npm test -- SetupWizard
```

Expected: 3 tests pass.

- [ ] **Step 9: Commit**

```bash
git add src/components/setup/ src/__tests__/SetupWizard.test.tsx
git commit -m "feat: add 4-step setup wizard with Ollama install and vault picker"
```

---

### Task 10: App shell skeleton + wiring

**Files:**
- Create: `src/components/shell/AppShell.tsx`
- Create: `src/components/shell/Sidebar.tsx`
- Create: `src/components/shell/EditorPane.tsx`
- Create: `src/components/shell/RelatedNotes.tsx`
- Modify: `src/App.tsx`
- Modify: `src/main.tsx`
- Modify: `src/index.css`

- [ ] **Step 1: Create `src/components/shell/Sidebar.tsx`**

```tsx
interface Props { reviewCount: number }

export function Sidebar({ reviewCount }: Props) {
  return (
    <aside className="sidebar">
      <div className="search-bar">
        <input type="search" placeholder="Search your brain..." />
      </div>
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

- [ ] **Step 2: Create `src/components/shell/EditorPane.tsx`**

```tsx
export function EditorPane() {
  return (
    <main className="editor-pane">
      <p className="placeholder">Open a document to get started</p>
    </main>
  );
}
```

- [ ] **Step 3: Create `src/components/shell/RelatedNotes.tsx`**

```tsx
export function RelatedNotes() {
  return (
    <aside className="related-notes">
      <h3>Related Notes</h3>
      <p className="placeholder">Open a document to see related notes</p>
    </aside>
  );
}
```

- [ ] **Step 4: Create `src/components/shell/AppShell.tsx`**

```tsx
import { useEffect } from "react";
import { Sidebar } from "./Sidebar";
import { EditorPane } from "./EditorPane";
import { RelatedNotes } from "./RelatedNotes";
import { startFileWatcher } from "../../lib/tauri";

interface Props { vaultPath: string }

export function AppShell({ vaultPath }: Props) {
  useEffect(() => {
    startFileWatcher(vaultPath).catch(console.error);
  }, [vaultPath]);

  return (
    <div className="app-shell">
      <Sidebar reviewCount={0} />
      <EditorPane />
      <RelatedNotes />
    </div>
  );
}
```

- [ ] **Step 5: Replace `src/App.tsx`**

```tsx
import { useState } from "react";
import { useSetupStatus } from "./hooks/useSetupStatus";
import { SetupWizard } from "./components/setup/SetupWizard";
import { AppShell } from "./components/shell/AppShell";

export function App() {
  const { loading, needsSetup, vaultPath } = useSetupStatus();
  const [setupComplete, setSetupComplete] = useState(false);
  const [resolvedVaultPath, setResolvedVaultPath] = useState<string | null>(null);

  if (loading) {
    return (
      <div className="loading-screen">
        <p>Loading...</p>
      </div>
    );
  }

  if (needsSetup && !setupComplete) {
    return (
      <SetupWizard
        onComplete={(path: string) => {
          setResolvedVaultPath(path);
          setSetupComplete(true);
        }}
      />
    );
  }

  const activePath = resolvedVaultPath ?? vaultPath!;
  return <AppShell vaultPath={activePath} />;
}
```

- [ ] **Step 6: Replace `src/main.tsx`**

```tsx
import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./App";
import "./index.css";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
```

- [ ] **Step 7: Replace `src/index.css` with layout styles**

```css
*, *::before, *::after { box-sizing: border-box; margin: 0; padding: 0; }

body { font-family: system-ui, sans-serif; font-size: 14px; color: #1a1a1a; }

.loading-screen {
  display: flex;
  align-items: center;
  justify-content: center;
  height: 100vh;
  color: #999;
}

.app-shell {
  display: flex;
  height: 100vh;
  overflow: hidden;
}

.sidebar {
  width: 240px;
  flex-shrink: 0;
  border-right: 1px solid #e5e5e5;
  display: flex;
  flex-direction: column;
  padding: 12px;
  gap: 12px;
  overflow-y: auto;
}

.search-bar input {
  width: 100%;
  padding: 6px 10px;
  border: 1px solid #d5d5d5;
  border-radius: 6px;
  font-size: 13px;
}

.folder-tree { flex: 1; }

.review-badge {
  background: #f59e0b;
  color: white;
  padding: 6px 10px;
  border-radius: 6px;
  font-size: 12px;
  font-weight: 600;
  text-align: center;
}

.editor-pane {
  flex: 1;
  overflow: auto;
  display: flex;
  align-items: center;
  justify-content: center;
}

.related-notes {
  width: 260px;
  flex-shrink: 0;
  border-left: 1px solid #e5e5e5;
  padding: 12px;
  overflow-y: auto;
}

.related-notes h3 { margin-bottom: 8px; font-size: 13px; font-weight: 600; }

.placeholder { color: #aaa; font-size: 13px; }

.setup-wizard {
  display: flex;
  align-items: center;
  justify-content: center;
  height: 100vh;
  background: #fafafa;
}

.setup-step {
  max-width: 480px;
  width: 100%;
  padding: 40px;
  display: flex;
  flex-direction: column;
  gap: 16px;
  background: white;
  border-radius: 12px;
  box-shadow: 0 2px 16px rgba(0,0,0,0.08);
}

.setup-step h1 { font-size: 24px; }
.setup-step h2 { font-size: 20px; }

.setup-step button {
  padding: 10px 20px;
  background: #3b82f6;
  color: white;
  border: none;
  border-radius: 8px;
  font-size: 14px;
  font-weight: 600;
  cursor: pointer;
  align-self: flex-start;
}

.setup-step button:disabled {
  background: #d0d0d0;
  cursor: not-allowed;
}

.selected-path {
  font-size: 12px;
  color: #555;
  word-break: break-all;
}
```

- [ ] **Step 8: Run all tests**

```bash
npm test
```

Expected: all tests pass with no failures.

- [ ] **Step 9: Run the app and verify end-to-end manually**

```bash
npm run tauri dev
```

Manual checklist:
- [ ] App launches — setup wizard shows (first launch, no vault configured)
- [ ] "Your Second Brain" heading visible, "Get Started" button present
- [ ] Clicking "Get Started" advances to Ollama step
- [ ] Ollama step shows correct state (checking → ready if installed, or needs-install if not)
- [ ] Vault picker step opens a system folder dialog on click
- [ ] After selecting a folder, "Confirm" button becomes active
- [ ] Done step shows "Open My Brain" button
- [ ] Clicking "Open My Brain" shows 3-panel shell
- [ ] Shell has: left sidebar with search input, center placeholder, right "Related Notes" panel
- [ ] No console errors

- [ ] **Step 10: Commit**

```bash
git add src/components/shell/ src/App.tsx src/main.tsx src/index.css
git commit -m "feat: add 3-panel app shell and wire setup gate in App.tsx"
```

---

## Foundation complete

At this point the project has:
- Tauri 2.x app that launches on macOS/Windows/Linux
- SQLite database initializing with full schema on first run at `~/.brain/brain.db`
- Vault root path persisted to `~/.brain/config.json`
- File watcher active once vault is set, emitting events to React
- Ollama detection + model pull with progress streamed to wizard
- 4-step setup wizard on first launch
- 3-panel shell on subsequent launches

**Next plan:** Sub-project 2 — Ingestion Pipeline (file watcher events → chunk → embed via FastEmbed → sqlite-vec).
