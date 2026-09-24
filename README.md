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

**Get it:** download the latest installers for macOS, Linux, or Windows from the [Releases page](https://github.com/equationalapplications/curated-thoughts/releases). Then drop PDFs, DOCX, or MD files into your vault — the watcher indexes them and the librarian proposes wiki pages that wait for your approval in the Review desk.

---

## 🧠 The Three-Tier Memory System

Curated Thoughts models AI memory biologically, moving information from raw input to crystallized knowledge:

1. **Working Memory (The Context):** The active UI state, conversation history, and current focus window. Fast, highly relevant, but volatile.
2. **Episodic Memory (The RAG Layer):** Raw recall. When you drop supported files into the vault, the watcher chunks them and embeds them with your configured embedding profile — a local Ollama model by default — into `brain.db` (SQLite) in your brain directory. This allows the LLM to semantically search exact quotes and track raw facts before deep synthesis occurs.
3. **Semantic Memory (The LLM Wiki):** The long-term truth. The system actively condenses raw facts into a curated, interlinked web of concepts and entities. This acts as a semantic wiki stored natively in SQLite, with wiki notes readable and writable as real `.md` files in your vault's `wiki/` folder, allowing the LLM to naturally read, link, and traverse relationships.

The librarian's wiki proposals reach the long-term wiki only through the **human-verification gate**: they land in a review queue, and only your explicit approval commits them (see [Human-in-the-Loop Verification](#human-in-the-loop-verification)). The exception is direct agent wisdom: the MCP wisdom write tools record your agent's entries straight into the wiki as user-stated, confirmed facts, without the proposal gate (see [MCP Agent Server](#-mcp-agent-server)).

---

## 🏗️ Architecture & Data Flow

The app strictly separates your source material from the generated AI memory, managed entirely by a background Rust engine called the **Active Librarian**.

- **`immutable-source-files/` (The Immutable Vault):** Your source of truth. The local file watcher monitors this folder for PDFs, DOCX, and MD files. The app's write tools can write only the `wiki/` folder and the agent deposit (`immutable-source-files/agents/`) — everything else here is read-only to the app. Wiki notes live as `.md` files under `wiki/`. (A vault from the older layout still has `documents/`; it is migrated to `immutable-source-files/` on first run.)
- **The Review Queue (Human-in-the-Loop):** The Active Librarian synthesizes new episodic data and proposes interconnected wiki pages. Nothing is committed to long-term memory until you approve, edit, or reject it — in the Review desk, headlessly with `ct proposals review` / `ct approve`, or through the MCP `curated_proposal_decide` tool. When a source document is deleted, each proposal built from it records which sources were deleted, and the Review desk marks those deleted and stranded sources.
- **`.brain/` (The Mutable State):** The knowledge base itself lives in `brain.db` (SQLite) in your brain home — `~/.brain` unless `CURATED_BRAIN_DIR` is set — alongside your `config.json`. The vault's own `.brain/` folder holds runtime state: the ingest error log, pending-proposal staging, and `brain.db` backups. The vault walker and file watcher never ingest a `.brain/` directory. Backup and restore are crash-safe: a restore captures the knowledge replica's obligations before overwriting `brain.db` and re-syncs the replica afterward.

The repo is a single Cargo workspace with two packages: `curated-thoughts` in `src-tauri/` (the desktop app, whose binary doubles as the full MCP server) and `curated-thoughts-tools` in `tools/` (the `ct` headless CLI and helper binaries), sharing the same database layer.

---

## ⚡ Key Features

### Bring Your Own Inference (BYOI)
The memory system seamlessly routes generation to your preferred engine. Spin up a local sidecar (like Ollama/Llama) for full offline privacy, or connect to external OpenAI-compatible APIs for heavy lifting. The frontend handles the wiki logic while the app supplies the `generateText` function. 

An optional **Jev classifier** — a dedicated fact-typing model, served by Cloudflare Workers AI (`typesafe/jev`) or any Jev-compatible endpoint — can type facts faster and more cheaply than your generation model when you click **Type untyped facts** in Settings → Maintenance:

- Off until you pick a provider in Settings → Models.
- Blocked in Strict privacy mode.
- Sends fact titles and bodies to the endpoint you configure.

### Human-in-the-Loop Verification
Proposals are gated, not automatic. Approve or reject wiki changes from the UI Review desk, interactively with `ct proposals review`, or in bulk with `ct approve`, or programmatically through the MCP `curated_proposal_decide` tool — every decision is stamped with who reviewed it. Librarian evidence that no longer anchors to a source chunk is re-graded (exported, then purged) by a one-time migration; `ct evidence regrade --yes` re-runs that pass on demand.

### Wiki Maintenance
**Settings → Maintenance** lets you:

- **Promote** draft facts into the wiki. Your click is recorded as a human review.
- **Run health report** — a read-only count of dangling edges, manifest violations, untyped facts, drafts, and unverified inferred facts.
- **Type untyped facts** — assign ontology types to facts that have none, using the Jev classifier if one is configured (see [BYOI](#bring-your-own-inference-byoi)), otherwise your generation model.

### Backup, Restore & Vault Switching
- **Restore is crash-safe.** The knowledge replica's pending changes are captured before `brain.db` is replaced and re-synced afterward; if the app is interrupted mid-restore, it finishes or rolls back the install on the next launch.
- **Switching vaults discards the knowledge layer built from the current vault** — the wiki, agent memories, and manual edits are per-vault. The app offers to back the current brain up first, and asks for explicit confirmation before a switch that would destroy it.

### Unified MCP Agent Server
Curated Thoughts isn't just a standalone desktop app; it acts as a system-wide brain. The app binary doubles as a standard **stdio Model Context Protocol (MCP) server** (`--mcp`). Hook it into any MCP-compliant client — Claude Desktop, Cursor, or your IDE's agent — and the agent sees your vault as native tools: read tools for recall, search and graph traversal, plus write tools for vault notes, agent wisdom entries and proposal decisions. A separate read-only server can be built for development (see [MCP Agent Server](#-mcp-agent-server)).

### Offline-First & Privacy Native
Parsing, chunking, and SQLite storage run entirely on your machine, and so do embeddings by default: the vault pipeline embeds with a local Ollama model, and the UI initializes a local Fastembed model for wiki-side embeddings. External endpoints are used only where you configure them (see [BYOI](#bring-your-own-inference-byoi)).

### Cross-Partition Wiki Graph
An agent walking the wiki graph isn't confined to one namespace. `wiki_traverse_graph` with an `entityId` keeps the walk in that namespace; without one, it ranks namespaces by matching-edge count, walks the top eight, and tells you when it cut more.

---

## 🚀 Local Development

### Install & Run

**Prerequisites:** [Rust via rustup](https://rustup.rs) (stable channel; `rust-toolchain.toml` adds clippy and rustfmt) and [pnpm](https://pnpm.io).

On a fresh clone, Tauri's build script requires the MCP sidecar path to exist before any Rust build (including `pnpm tauri dev`). If you skip this, the build fails with ``resource path `binaries/curated-thoughts-mcp-<host-triple>` doesn't exist``. Create an empty placeholder once:

```bash
mkdir -p src-tauri/binaries
# rustc -vV prints "host: <triple>"; sed keeps just the triple.
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

**Direct wisdom writes:** the wisdom write tools skip the proposal gate — `curated_add_wisdom` records your agent's entry in the wiki immediately as user-stated, confirmed.

The source of truth for both lists is the `#[tool(name = …)]` attributes in `src-tauri/src/mcp_server.rs` and `tools/src/bin/curated_thoughts_mcp.rs`.

### Build from Source

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

> **Note:**
> - **Naming:** the sidecar is the app binary, copied under the name `curated-thoughts-mcp` because Tauri requires a sidecar's name to differ from the Cargo package name. Don't confuse it with the read-only dev server of the same name.
> - **Streams:** stdio only — protocol traffic on stdout, tracing on stderr.
> - **Windows:** an agent-spawned sidecar may briefly flash a console window unless the client passes `CREATE_NO_WINDOW`.

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
ct graph <symbol> --dir callers --hops 2   # code call-graph lookups (--dir: callers, callees, both)
ct wiki list                           # inspect and curate the wiki layer;
                                       #   also: wiki get, wiki forget, wiki sweep
ct ingest --yes                        # (re-)ingest the vault
ct librarian run --yes                 # run a synthesis pass on demand
ct proposals review                    # the human-verification gate, headless;
                                       #   also: proposals list, proposals show
ct approve <proposal-id>               # approve without the interactive loop
ct approve --all --yes                 #   ...or every pending proposal at once
ct evidence regrade --yes              # re-run the V20 evidence re-grade (idempotent)
ct trust [--list] [--revoke <path>]    # manage symlinks the ingest walker may follow
```

`ct watch` is the one long-running command: a foreground daemon that watches the vault and requires `CURATED_VAULT_ROOT`. Add `--json` for one JSON event per line on stdout, or `--once` to exit after a bounded window (default 60s, set with `--once-timeout`):

```bash
CURATED_VAULT_ROOT=/path/to/vault ct watch --json
```

### Bulk Re-index (`bulk_reindex` CLI)

When chunking logic (`ast_*` tags, prose heuristics) or embedding settings change, the pipeline normally **skips** files whose bytes are unchanged. Re-run chunking and embeddings for every indexed doc without touching files:

```bash
cargo run -p curated-thoughts-tools --bin bulk_reindex -- --dry-run
cargo run -p curated-thoughts-tools --bin bulk_reindex --
```

### OKF Bundle Import/Export

The brain round-trips through OKF bundles (OKF v0.2, `llm-wiki/2` profile). In the GUI, the OKF bar in **Brain** mode has **Export brain as OKF bundle** / **Import bundle** buttons; they write and read zip bundles (`.zip` or `.okf`), and import shows a preview with merge/replace/clone modes before anything is applied.

For scripting and backups, the `tools` package builds a headless exporter that runs the same load + write code as the GUI (read-only against the database, so it is safe to run while the app is open — all queries share one consistent snapshot, and no `exported` event rows are recorded):

```bash
cargo build --release -p curated-thoughts-tools --bin export_okf_bundle
target/release/export_okf_bundle ~/backups/brain-okf.zip
# exported entities=452 files=3032 sha256=… path=/home/…/brain-okf.zip
```

`export_okf_bundle [dest]` defaults to `~/brain-okf.zip` and honors the same `CURATED_BRAIN_*` variables as the rest of the `tools` package. It writes to a temp file, parse-checks the result against the same import limits the reader enforces, and only then atomically replaces the destination — so a failed run never destroys the previous backup. It is what powers the nightly `backups/okf/brain-okf.zip` commit in the equational-wiki backup cron. Note that the bundle is a **full, unredacted copy** of the brain — including soft-deleted facts — so only commit it to a private repo.

### Semantic Search Profiling

To measure mean query latency vs. chunk count (e.g., before adopting sqlite-vec / ANN):

```bash
CURATED_EMBED_STUB=constant8 cargo run --release -p curated-thoughts-tools --bin semantic_search_profile -- 5000
```

### Integration Tests

End-to-end tests spawn the app binary with **`--mcp`** and speak MCP over stdin/stdout. They are **opt-in**: without **`CURATED_MCP_INTEGRATION_TESTS=1`** they return early and report as passed (in 0.00s) without testing anything. They set **`CURATED_EMBED_STUB`**, so no real embedding model is loaded:

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
