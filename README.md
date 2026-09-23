[![GitHub Release](https://img.shields.io/github/v/release/equationalapplications/curated-thoughts)](https://github.com/equationalapplications/curated-thoughts/releases)
[![CI](https://img.shields.io/github/actions/workflow/status/equationalapplications/curated-thoughts/ci.yml?branch=main)](https://github.com/equationalapplications/curated-thoughts/actions/workflows/ci.yml)
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

Nothing reaches the long-term wiki without passing the **human-verification gate**: the librarian's wiki proposals land in a review queue, and only your explicit approval commits them (see [Key Features](#-key-features)).

---

## 🏗️ Architecture & Data Flow

The app strictly separates your source material from the generated AI memory, managed entirely by a background Rust engine called the **Active Librarian**.

- **`documents/` (The Immutable Vault):** Your source of truth. The local file watcher monitors this directory for PDFs, DOCX, and MD files. The UI never writes to this folder.
- **The Review Queue (Human-in-the-Loop):** The Active Librarian synthesizes new episodic data and proposes interconnected wiki pages. Humans must approve or edit these proposals before they are committed to long-term memory — from the Review desk in the UI, the `ct proposals review` CLI, or the MCP proposal tools. When a source document is deleted, its provenance is recorded so pending and committed wiki content stays traceable to (or is marked stranded from) what it was built from.
- **`.brain/` (The Mutable State):** The namespace-safe local storage containing the SQLite databases. This houses the embedded chunk rows (Episodic) and the generated Markdown wiki pages (Semantic), alongside your configuration files. The vault walker and file watcher never ingest a `.brain/` directory. Backup and restore are crash-safe: a restore captures the knowledge replica's obligations before overwriting `brain.db` and re-syncs the replica afterward.

The repo is a single Cargo workspace with two packages: `curated-thoughts` in `src-tauri/` (the desktop app, whose binary doubles as the full MCP server) and `curated-thoughts-tools` in `tools/` (the `ct` headless CLI and helper binaries), sharing the same database layer.

---

## ⚡ Key Features

### Bring Your Own Inference (BYOI)
The memory system seamlessly routes generation to your preferred engine. Spin up a local sidecar (like Ollama/Llama) for full offline privacy, or connect to external OpenAI-compatible APIs for heavy lifting. The frontend handles the wiki logic while the app supplies the `generateText` function. An optional Jev classifier (Cloudflare Workers AI or any Jev-compatible endpoint) can type facts faster and more cheaply than your generation model when you run "Type untyped facts". It is off until you pick a provider in Settings → Models, is blocked in Strict privacy mode, and sends fact titles and bodies to the endpoint you configure.

### Human-in-the-Loop Verification
Proposals are gated, not automatic. Approve or reject wiki changes from the UI Review desk, interactively with `ct proposals review`, or in bulk with `ct approve`, or programmatically through the MCP `curated_proposal_decide` tool — every decision is stamped with who reviewed it. Librarian evidence that no longer anchors to a source chunk is re-graded (exported, then purged) by a one-time migration; `ct evidence regrade --yes` re-runs that pass on demand.

### Wiki Maintenance
**Settings → Maintenance** lists wiki drafts you can **Promote** into the wiki, runs a lint **health report**, and offers **Type untyped facts**, which assigns ontology types to facts that have none.

### Backup, Restore & Vault Switching
Restoring a backup is crash-safe and keeps the knowledge replica in sync. Switching to a different vault clears the knowledge layer (the wiki built from the old vault) — the app asks for confirmation first.

### Unified MCP Agent Server
Curated Thoughts isn't just a standalone desktop app; it acts as a system-wide brain. The app binary doubles as a standard **stdio Model Context Protocol (MCP) server** (`--mcp`). You can hook this vault directly into MCP-compliant clients (like Claude Desktop or Cursor), giving your favorite agents native access to your immutable documents, Fastembed RAG search, and the wiki layer — read tools for recall, search and graph traversal, plus write tools for vault notes, agent wisdom entries and proposal decisions. A separate read-only server is available for development (see [MCP Agent Server](#-mcp-agent-server)).

### Offline-First & Privacy Native
All parsing, chunking, local embeddings (Fastembed), and SQLite metadata operations happen strictly on your machine.

### Cross-Partition Wiki Graph
Wiki relationships are traversable across namespaces: `wiki_traverse_graph` scopes a walk to one namespace when given an `entityId`, and discovers edges across all of them when it is omitted.

---

## 🚀 Local Development

### Install & Run

On a fresh clone, Tauri's build script requires the MCP sidecar path to exist before any Rust build (including `pnpm tauri dev`). Create an empty placeholder once:

```bash
mkdir -p src-tauri/binaries
touch "src-tauri/binaries/curated-thoughts-mcp-$(rustc -vV | sed -n 's/^host: //p')"
# On Windows, append .exe to the placeholder name.
```

```bash
# Install frontend dependencies
pnpm install

# Run the desktop app in dev mode
pnpm tauri dev

# Build the desktop app for production
pnpm tauri build

# Frontend only: type-check, lint, unit tests
pnpm typecheck
pnpm lint
pnpm test
```

### Project Structure

* `src/` — React frontend and app UI (Settings, Review desk, Drafts panels)
* `src-tauri/` — Rust backend: file watcher, SQLite, librarian engine, MCP server
* `tools/` — the `ct` headless CLI and dev/ingest helper binaries
* `docs/superpowers/specs/` — design specs

### Recommended IDE Setup

* [VS Code](https://code.visualstudio.com/)
* [Tauri VS Code extension](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode)
* [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)

---

## 🤖 MCP Agent Server

The repo builds two **stdio** [Model Context Protocol](https://modelcontextprotocol.io/) servers. Both read the same brain layout as the desktop app (SQLite chunks and embeddings).

| Server | How to run | Tools |
| --- | --- | --- |
| **Full server** (what release bundles ship) | the app binary with `--mcp` | read + write (16) |
| **Read-only dev server** | `curated-thoughts-mcp` from the `tools` package | read-only (7) |

**Full server tools:** `vault_semantic_search`, `vault_related_chunks`, `wiki_search`, `wiki_context`, `wiki_get_ontology`, `wiki_traverse_graph`, `curated_recall_context`, `curated_get_wiki_entry`, `curated_search_code`, `curated_proposals_list` (read); `vault_write_note`, `vault_upsert_index_entry`, `curated_add_wisdom`, `curated_update_wisdom`, `curated_archive_wisdom`, `curated_proposal_decide` (write).

**Read-only dev server tools:** `vault_semantic_search`, `vault_related_chunks`, `curated_recall_context`, `curated_get_wiki_entry`, `curated_search_code`, `graph_neighbors`, `curated_superpowers_setup`.

The source of truth for both lists is the `#[tool(name = …)]` attributes in `src-tauri/src/mcp_server.rs` and `tools/src/bin/curated_thoughts_mcp.rs`.

### Build the Server

From the repository root (after the placeholder step in [Install & Run](#install--run)):

```bash
# Full server: the app binary with the MCP feature
cargo build -p curated-thoughts --features mcp-server --bin curated-thoughts

# Read-only dev server
cargo build -p curated-thoughts-tools --bin curated-thoughts-mcp
```

*The binaries land in `target/debug/`. Run the full server as `target/debug/curated-thoughts --mcp`.*

### Cursor / VS Code `mcpServers` snippet

Adjust the `command` path to your clone and build output to give your IDE access to your brain:

```json
{
  "mcpServers": {
    "curated-thoughts": {
      "command": "/path/to/curated-thoughts/target/debug/curated-thoughts",
      "args": ["--mcp"],
      "env": {
        "CURATED_BRAIN_DIR": "/path/to/your/brain"
      }
    }
  }
}
```

For the read-only dev server, use `target/debug/curated-thoughts-mcp` as the `command` and drop `args`.

### Environment Variables

| Variable | Purpose |
| --- | --- |
| **`CURATED_BRAIN_DIR`** | Brain home directory (expects `brain.db` and `config.json` there). If unset, defaults to **`~/.brain`** (`$HOME/.brain`), same as the app. The server exits at startup if `brain.db` is missing. |
| **`CURATED_BRAIN_DB`** | Optional explicit path to `brain.db` instead of `{brain_dir}/brain.db`. |
| **`CURATED_BRAIN_CONFIG`** | Optional explicit path to `config.json` when it is not beside the resolved DB. |

### Security Note

This is a **local stdio** server: any client you attach can invoke tools that return **indexed chunk text and metadata** from your brain database, and the full server's write tools can change vault notes, wisdom entries and proposal decisions. Treat the MCP process and its environment as part of your **trust boundary**; do not point it at sensitive data you would not show to the agent.

### Connecting release builds

Release bundles include the full server as a sidecar. Point any MCP client at:

```text
<install-dir>/curated-thoughts-mcp --mcp
```

(The sidecar is the app binary, copied under the name `curated-thoughts-mcp` because Tauri requires a sidecar's
name to differ from the Cargo package name. Don't confuse it with the read-only dev server of the same name.
The server speaks stdio only —
tracing goes to stderr, protocol traffic on stdout. Known limitation on
Windows: agent-spawned sidecars may briefly flash a console window unless the
client passes `CREATE_NO_WINDOW`.)

---

## 🛠️ CLI Tools & Testing

### The `ct` Headless CLI

The `tools` package builds `ct`, a headless operator surface for the same brain the GUI uses — useful for scripting, servers, and agents. It reads the same `CURATED_BRAIN_*` variables as the MCP server:

```bash
cargo build -p curated-thoughts-tools --bin ct
export PATH="$PWD/target/debug:$PATH"   # or call target/debug/ct directly

ct status                              # vault + database summary
ct search <query>                      # semantic search over indexed chunks
ct recall <prompt>                     # recall context (chunks + wiki entries)
ct code <query>                        # search code chunks
ct graph <symbol> [--dir callers|callees|both] [--hops N]   # code call-graph lookups
ct wiki list|get|forget|sweep          # inspect and curate the wiki layer
ct ingest --yes                        # (re-)ingest the vault
ct librarian run --yes                 # run a synthesis pass on demand
ct proposals list|show|review          # the human-verification gate, headless
ct approve <proposal-id> | --all --yes # approve without the interactive loop
ct evidence regrade --yes              # re-run the V20 evidence re-grade (idempotent)
ct trust [--list] [--revoke <path>]    # manage symlinks the ingest walker may follow
CURATED_VAULT_ROOT=/path/to/vault ct watch [--once] [--json]   # vault watcher daemon
```

### Bulk Re-index (`bulk_reindex` CLI)

When chunking logic (`ast_*` tags, prose heuristics) or embedding settings change, the pipeline normally **skips** files whose bytes are unchanged. Re-run chunking and embeddings for every indexed doc without touching files:

```bash
cargo run -p curated-thoughts-tools --bin bulk_reindex -- --dry-run
cargo run -p curated-thoughts-tools --bin bulk_reindex --
```

### Semantic Search Profiling

To measure mean query latency vs. chunk count (e.g., before adopting sqlite-vec / ANN):

```bash
CURATED_EMBED_STUB=constant8 cargo run --release -p curated-thoughts-tools --bin semantic_search_profile -- 5000
```

### Integration Tests

End-to-end test spawns the app binary with **`--mcp`** and speaks MCP over stdin/stdout (uses **`CURATED_EMBED_STUB`**). The tests skip silently unless **`CURATED_MCP_INTEGRATION_TESTS=1`** is set:

```bash
CURATED_MCP_INTEGRATION_TESTS=1 cargo test -p curated-thoughts --features mcp-server --test mcp_integration
```

---

## 📦 Related Packages

Learn more about the Equational Applications memory and wiki packages powering this architecture:

* [`@equationalapplications/react-llm-wiki`](https://www.npmjs.com/package/@equationalapplications/react-llm-wiki) — React web support for local LLM Wiki memory.
* [`@equationalapplications/expo-llm-wiki`](https://www.npmjs.com/package/@equationalapplications/expo-llm-wiki) — Expo / React Native version with `expo-sqlite` adapter.
* [`@equationalapplications/core-llm-wiki`](https://www.npmjs.com/package/@equationalapplications/core-llm-wiki) — Framework-agnostic core logic.

---

Made with ❤️ by Equational Applications LLC. [https://equationalapplications.com/](https://equationalapplications.com/)
