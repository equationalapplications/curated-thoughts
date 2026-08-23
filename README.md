[![GitHub Release](https://img.shields.io/github/v/release/equationalapplications/curated-thoughts)](https://github.com/equationalapplications/curated-thoughts/releases)
[![CI](https://github.com/equationalapplications/curated-thoughts/actions/workflows/ci.yml/badge.svg)](https://github.com/equationalapplications/curated-thoughts/actions/workflows/ci.yml)
[![Downloads](https://img.shields.io/github/downloads/equationalapplications/curated-thoughts/total)](https://github.com/equationalapplications/curated-thoughts/releases)
[![License](https://img.shields.io/github/license/equationalapplications/curated-thoughts)](LICENSE)
[![macOS](https://img.shields.io/badge/macOS-supported-success)](https://github.com/equationalapplications/curated-thoughts/releases)
[![Linux](https://img.shields.io/badge/Linux-supported-success)](https://github.com/equationalapplications/curated-thoughts/releases)
[![Windows](https://img.shields.io/badge/Windows-supported-success)](https://github.com/equationalapplications/curated-thoughts/releases)

# Curated Thoughts

Curated Thoughts is a privacy-first, local-first desktop second brain built with Tauri, React, and Rust.

Inspired by [Andrej Karpathy's LLM Wiki memory spec](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f) and powered by `@equationalapplications/react-llm-wiki`, this app is not just a file browser or a basic RAG tool. It is a **cognitive architecture** designed to help local LLMs build compounding, structured memory over time.

---

## 🧠 The Three-Tier Memory System

Curated Thoughts models AI memory biologically, moving information from raw input to crystallized knowledge:

1. **Working Memory (The Context):** The active UI state, conversation history, and current focus window. Fast, highly relevant, but volatile.
2. **Episodic Memory (The RAG Layer):** Raw recall. When you drop files into the vault, they are immediately chunked and embedded via local Fastembed into SQLite. This allows the LLM to semantically search exact quotes and track raw facts before deep synthesis occurs.
3. **Semantic Memory (The LLM Wiki):** The long-term truth. The system actively condenses raw facts into a curated, interlinked web of concepts and entities. This acts as a semantic wiki stored natively in SQLite (exportable as true `.md` files), allowing the LLM to naturally read, link, and traverse relationships.

---

## 🏗️ Architecture & Data Flow

The app strictly separates your source material from the generated AI memory, managed entirely by a background Rust engine called the **Active Librarian**.

- **`documents/` (The Immutable Vault):** Your source of truth. The local file watcher monitors this directory for PDFs, DOCX, and MD files. The UI never writes to this folder.
- **The Review Queue (Human-in-the-Loop):** The Active Librarian synthesizes new episodic data and proposes interconnected wiki pages. Humans must approve or edit these proposals before they are committed to long-term memory.
- **`.brain/` (The Mutable State):** The namespace-safe local storage containing the SQLite databases. This houses the embedded chunk rows (Episodic) and the generated Markdown wiki pages (Semantic), alongside your configuration files.

---

## ⚡ Key Features

### Bring Your Own Inference (BYOI)
The memory system seamlessly routes generation to your preferred engine. Spin up a local sidecar (like Ollama/Llama) for full offline privacy, or connect to external OpenAI-compatible APIs for heavy lifting. The frontend handles the wiki logic while the app supplies the `generateText` function.

### Unified MCP Agent Server
Curated Thoughts isn't just a standalone desktop app; it acts as a system-wide brain. The crate exposes a standard **stdio Model Context Protocol (MCP) server**. You can hook this vault directly into MCP-compliant clients (like Claude Desktop or Cursor), giving your favorite agents native access to your immutable documents, Fastembed RAG search, and the mutable wiki layer.

### Offline-First & Privacy Native
All parsing, chunking, local embeddings (Fastembed), and SQLite metadata operations happen strictly on your machine.

---

## 🚀 Local Development

### Install & Run
```bash
# Install frontend dependencies
pnpm install

# Run the desktop app in dev mode
pnpm run tauri dev

# Build the app for production
pnpm run build

```

### Project Structure

* `src/` — React frontend and app UI
* `src-tauri/` — Rust backend, file watcher, SQLite, sidecar integration
* `public/` — static assets
* `package.json` — frontend dependencies and scripts

### Recommended IDE Setup

* [VS Code](https://code.visualstudio.com/)
* [Tauri VS Code extension](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode)
* [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)

---

## 🤖 MCP Agent Server

The crate can expose a **stdio** [Model Context Protocol](https://modelcontextprotocol.io/) server for local agents. It reads the same brain layout as the desktop app (SQLite chunks and embeddings).

### Build the Server

Cargo needs the manifest at **`src-tauri/Cargo.toml`**. From the repository root (`curated-thoughts/`):

```bash
cargo build --manifest-path src-tauri/Cargo.toml -p curated-thoughts --features mcp-server --bin curated-thoughts-mcp

```

*The resulting binary will be located at `src-tauri/target/debug/curated-thoughts-mcp`.*

### Cursor / VS Code `mcpServers` snippet

Adjust the `command` path to your clone and build output to give your IDE access to your brain:

```json
{
  "mcpServers": {
    "curated-thoughts": {
      "command": "/path/to/curated-thoughts/src-tauri/target/debug/curated-thoughts-mcp",
      "env": {
        "CURATED_BRAIN_DIR": "/path/to/your/brain"
      }
    }
  }
}

```

### Environment Variables

| Variable | Purpose |
| --- | --- |
| **`CURATED_BRAIN_DIR`** | Brain home directory (expects `brain.db` and `config.json` there). If unset, defaults to **`~/.brain`** (`$HOME/.brain`), same as the app. |
| **`CURATED_BRAIN_DB`** | Optional explicit path to `brain.db` instead of `{brain_dir}/brain.db`. |
| **`CURATED_BRAIN_CONFIG`** | Optional explicit path to `config.json` when it is not beside the resolved DB. |

### Security Note

This is a **local stdio** server: any client you attach can invoke tools that return **indexed chunk text and metadata** from your brain database. Treat the MCP process and its environment as part of your **trust boundary**; do not point it at sensitive data you would not show to the agent.

---

## 🛠️ CLI Tools & Testing

### Bulk Re-index (`bulk_reindex` CLI)

When chunking logic (`ast_*` tags, prose heuristics) or embedding settings change, the pipeline normally **skips** files whose bytes are unchanged. Re-run chunking and embeddings for every indexed doc without touching files:

```bash
cargo run --manifest-path src-tauri/Cargo.toml --bin bulk_reindex -- --dry-run
cargo run --manifest-path src-tauri/Cargo.toml --bin bulk_reindex --

```

### Semantic Search Profiling

To measure mean query latency vs. chunk count (e.g., before adopting sqlite-vec / ANN):

```bash
CURATED_EMBED_STUB=constant8 cargo run --manifest-path src-tauri/Cargo.toml --release --bin semantic_search_profile -- 5000

```

### Integration Tests

End-to-end test spawns **`curated-thoughts-mcp`** and speaks MCP over stdin/stdout (uses **`CURATED_EMBED_STUB`**):

```bash
cargo test --manifest-path src-tauri/Cargo.toml -p curated-thoughts --features mcp-server --test mcp_integration

```

---

## 📦 Related Packages

Learn more about the Equational Applications memory and wiki packages powering this architecture:

* [`@equationalapplications/react-llm-wiki`](https://www.npmjs.com/package/@equationalapplications/react-llm-wiki) — React web support for local LLM Wiki memory.
* [`@equationalapplications/expo-llm-wiki`](https://www.npmjs.com/package/@equationalapplications/expo-llm-wiki) — Expo / React Native version with `expo-sqlite` adapter.
* [`@equationalapplications/core-llm-wiki`](https://www.npmjs.com/package/@equationalapplications/core-llm-wiki) — Framework-agnostic core logic.

---

## Connecting AI agents (MCP)

Release bundles include an MCP server (stdio JSON-RPC). Point any MCP client at:

    <install-dir>/curated-thoughts-mcp --mcp

(The sidecar is named `curated-thoughts-mcp` because Tauri requires a sidecar's
name to differ from the Cargo package name.)

Configuration lives in `~/.brain` (override with the `CURATED_BRAIN_DIR` env var).
The server speaks stdio only — tracing goes to stderr, protocol traffic on stdout.

Tool inventory and client examples: see `specs/curated-thoughts-mcp-coding-spec.md`.

Known limitation (Windows): agent-spawned sidecars may briefly flash a console window
unless the client passes `CREATE_NO_WINDOW`. Standard for MCP servers on Windows.

Developers: the `curated-thoughts-mcp` binary in the `tools/` crate remains the
manual/dev path (`cargo build --manifest-path tools/Cargo.toml --bin curated-thoughts-mcp`).

---

Made with ❤️ by Equational Applications LLC. [https://equationalapplications.com/](https://equationalapplications.com/)
