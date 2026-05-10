[![GitHub Release](https://img.shields.io/github/v/release/equationalapplications/curated-thoughts)](https://github.com/equationalapplications/curated-thoughts/releases)
[![CI](https://github.com/equationalapplications/curated-thoughts/actions/workflows/ci.yml/badge.svg)](https://github.com/equationalapplications/curated-thoughts/actions/workflows/ci.yml)
[![Downloads](https://img.shields.io/github/downloads/equationalapplications/curated-thoughts/total)](https://github.com/equationalapplications/curated-thoughts/releases)
[![License](https://img.shields.io/github/license/equationalapplications/curated-thoughts)](LICENSE)
[![macOS](https://img.shields.io/badge/macOS-supported-success)](https://github.com/equationalapplications/curated-thoughts/releases)
[![Linux](https://img.shields.io/badge/Linux-supported-success)](https://github.com/equationalapplications/curated-thoughts/releases)

# Curated Thoughts

Curated Thoughts is a privacy-first, local-first desktop second brain built with Tauri, React, and Rust.

Inspired by [Andrej Karpathy's LLM Wiki memory spec](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f) and powered by `@equationalapplications/react-llm-wiki`, the app uses a local LLM Wiki experience to keep generated wiki entries aligned with source documents and newly ingested information.

## Overview

Curated Thoughts lets users drop files into a watched vault and automatically indexes them into a searchable knowledge base. A local Active Librarian processes documents into wiki pages, while the frontend keeps the file system isolated from direct writes.

Key app concepts:
- **Immutable source documents** in a watched `documents/` vault
- **Generated, reviewable wiki pages** in `wiki/`
- **Background ingestion pipeline** for document conversion, chunking, embedding, and synthesis
- **Local LLM-powered memory** via `@equationalapplications/react-llm-wiki`

## Why this project exists

This app applies the LLM Wiki idea to a desktop second brain: persistent episodic memory, semantic retrieval, and human-in-the-loop synthesis. It connects local file content, embeddings, and long-term memory into a unified experience.

## Rapid Development Phase

**Pre-release posture:** - **Breaking changes are acceptable** — schema reshaping, embedding dimension switches, wiping dev databases (`brain.db`), and non-additive Tauri/TS API changes do **not** require backward compatibility with prior milestones or lazy migration of legacy chunk rows.

## Architecture

Curated Thoughts separates user source material from generated wiki content and local metadata.

- `documents/`: immutable source files that are watched and never written by the UI.
- `wiki/`: generated and user-editable wiki pages produced by the Active Librarian.
- `.brain/`: local app state, including the SQLite database, converted document shadows, and ingestion metadata.

The ingestion pipeline works like this:
1. Watch files in the vault and detect changes.
2. Convert PDFs/DOCX/MD into normalized markdown.
3. Chunk documents and store them in SQLite.
4. Generate or reembed semantic vectors for retrieval.
5. Create proposed wiki pages for review, then write approved pages to `wiki/`.
6. Use the LLM Wiki package to power semantic search, related context, and long-term memory.

## User flows

- **Onboarding:** users are guided through setting up the vault, installing the local model sidecar, and choosing a watched folder.
- **Add documents:** place files in the watched `documents/` vault and the app detects them automatically.
- **Review queue:** generated wiki proposals are presented for approval or editing before they become part of the `wiki/` collection.
- **Search and related notes:** the UI surfaces semantic and full-text results, letting users explore related context from embeddings and wiki memory.

## What powers the LLM Wiki

Curated Thoughts draws on the same core design principles as the Equational Applications LLM Wiki packages:

- **Bring Your Own Inference (BYOI):** the app supplies a `generateText` function while the wiki package owns prompt construction, JSON parsing, and memory writes.
- **Namespace-safe SQLite:** all wiki tables are prefixed to avoid collisions with other databases.
- **Multi-entity support:** multiple independent "brains" can coexist in one database.
- **Semantic retrieval + keyword fallback:** embeddings provide cosine search, with MiniSearch fallback when offline or embedding is unavailable.
- **Offline-first behavior:** local search works without network access, while optional embedding enables richer semantic results.

## Package links

Learn more about the Equational Applications memory and wiki packages:

- [`@equationalapplications/react-llm-wiki`](https://www.npmjs.com/package/@equationalapplications/react-llm-wiki) — React web support for local LLM Wiki memory.
- [`@equationalapplications/expo-llm-wiki`](https://www.npmjs.com/package/@equationalapplications/expo-llm-wiki) — Expo / React Native version with `expo-sqlite` adapter.
- [`@equationalapplications/core-llm-wiki`](https://www.npmjs.com/package/@equationalapplications/core-llm-wiki) — framework-agnostic core logic for Node or browser environments.
- [`expo-llm-wiki` GitHub repo](https://github.com/equationalapplications/expo-llm-wiki)

## Local development

### Install

```bash
npm install
```

### Run the app

```bash
npm run tauri dev
```

### Build

```bash
npm run build
```

## MCP agent server (experimental)

The crate can expose a **stdio** [Model Context Protocol](https://modelcontextprotocol.io/) server for local agents. It reads the same brain layout as the desktop app (SQLite chunks and embeddings).

### Build

Cargo needs the manifest at **`src-tauri/Cargo.toml`**. Either change into that crate **or** pass **`--manifest-path`** from the repository root (`curated-thoughts/`):

```bash
cd src-tauri
cargo build -p curated-thoughts --features mcp-server --bin curated-thoughts-mcp
```

```bash
# from repository root
cargo build --manifest-path src-tauri/Cargo.toml -p curated-thoughts --features mcp-server --bin curated-thoughts-mcp
```

With that manifest path, Cargo’s default target directory is **`src-tauri/target`**. After a debug build, the binary is:

- **`src-tauri/target/debug/curated-thoughts-mcp`** (from the repo root), or
- **`target/debug/curated-thoughts-mcp`** when your working directory is `src-tauri/`.

### Environment

| Variable | Purpose |
| --- | --- |
| **`CURATED_BRAIN_DIR`** | Brain home directory (expects `brain.db` and `config.json` there). If unset, defaults to **`~/.brain`** (`$HOME/.brain`), same as the app. |
| **`CURATED_BRAIN_DB`** | Optional explicit path to `brain.db` instead of `{brain_dir}/brain.db`. |
| **`CURATED_BRAIN_CONFIG`** | Optional explicit path to `config.json` when it is not beside the resolved DB. |

### Integration tests (stdio + `vault_*` tools)

End-to-end test spawns **`curated-thoughts-mcp`** and speaks MCP over stdin/stdout (uses **`CURATED_EMBED_STUB`**):

```bash
cd src-tauri
cargo test -p curated-thoughts --features mcp-server --test mcp_integration
```

```bash
# from repository root
cargo test --manifest-path src-tauri/Cargo.toml -p curated-thoughts --features mcp-server --test mcp_integration
```

Cargo sets **`CARGO_BIN_EXE_*`** when building that test target; build the MCP binary once first if you see a missing-binary error message from the harness.

### Bulk re-index (`bulk_reindex` CLI)

When chunking logic (`ast_*` tags, prose heuristics) or embedding settings change, the pipeline normally **skips** files whose bytes are unchanged (`hash` matches). Re-run chunking and embeddings for every indexed doc without touching files:

```bash
cargo run --manifest-path src-tauri/Cargo.toml --bin bulk_reindex -- --dry-run
cargo run --manifest-path src-tauri/Cargo.toml --bin bulk_reindex --
```

Uses the same **`CURATED_BRAIN_*`** env vars as MCP. Flags: **`--dry-run`**, **`--limit N`**, optional path substring filter. The desktop app can also call the **`queue_full_reindex`** command with **`force_rechunk: true`** to enqueue the same work on the running pipeline.

### Semantic search profiling

**`semantic_search`** does a full scan over all indexed embeddings. To measure mean query latency vs. chunk count (e.g. before adopting sqlite-vec / ANN):

```bash
CURATED_EMBED_STUB=constant8 cargo run --manifest-path src-tauri/Cargo.toml --release --bin semantic_search_profile -- 5000
```

### Security

This is a **local stdio** server: any client you attach can invoke tools that return **indexed chunk text and metadata** from your brain database. Treat the MCP process and its environment as part of your **trust boundary**; do not point it at sensitive data you would not show to the agent.

### Embeddings stub

**`CURATED_EMBED_STUB`** is for **tests and local harnesses only** (deterministic fake vectors). **Do not** enable it for production agent workloads where retrieval quality matters.

### Cursor / VS Code `mcpServers` snippet

Adjust the `command` path to your clone and build output:

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

## Recommended IDE setup

- [VS Code](https://code.visualstudio.com/)
- [Tauri VS Code extension](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode)
- [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)

## Project structure

- `src/` — React frontend and app UI
- `src-tauri/` — Rust backend, file watcher, SQLite, Ollama integration
- `public/` — static assets
- `package.json` — frontend dependencies and scripts

## Design inspiration

Curated Thoughts is built around the idea of a long-term AI memory store that can be queried semantically and augmented safely by local models. It follows the same spirit as the Equational Applications LLM Wiki packages and uses their React adapter to provide local-first wiki behavior in a desktop environment.

---

Made with ❤️ by Equational Applications LLC. [https://equationalapplications.com/](https://equationalapplications.com/)
