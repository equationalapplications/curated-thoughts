# Curated Thoughts — Design Spec
**Date:** 2026-05-05
**Status:** Implemented  
**Stack:** Tauri + React + BlockNote + Rust + Ollama + FastEmbed + SQLite/sqlite-vec

---

## Overview

Privacy-first, local-first second brain desktop app for non-technical knowledge workers. Users drop documents into a watched folder; an Active Librarian (local LLM) indexes, summarizes, and synthesizes wiki pages from those documents. All processing happens on-device by default. Cloud LLM providers available as opt-in.

## Future Expansion (Out of Scope for v1)

**MCP Server for VS Code Copilot:** Expose the Curated Thoughts knowledge graph as an MCP server so VS Code Copilot (and other MCP-compatible agents) can query the local SQLite/sqlite-vec database as RAG context during coding sessions. Architecture already supports this — sqlite-vec queries are already the core retrieval primitive. MCP server would be a thin stdio wrapper around existing Rust query layer. Distribution via `.vsix` on Open VSX Registry.

---

## Target User

Non-technical knowledge workers. Zero-config onboarding. No terminal, no manual model setup.

---

## Core Concepts

**Two-tier document model:**
- **Tier 1 — User Documents:** Immutable source truth. PDFs, DOCX, MD files dropped into `documents/`. App never writes to this folder. Enforced at Rust layer.
- **Tier 2 — Wiki Pages:** Librarian-generated synthesis and summaries, stored in `wiki/`. User-editable in BlockNote.

**Active Librarian:** Background agent that watches the vault, processes new documents per folder rules, and queues proposed wiki pages for human review before writing.

---

## Architecture

```
┌─────────────────────────────────────────────────────┐
│  Tauri Shell                                        │
│  ┌─────────────────────┐  ┌───────────────────────┐ │
│  │  React Frontend     │  │  Rust Backend          │ │
│  │                     │  │                        │ │
│  │  BlockNote editor   │  │  File watcher          │ │
│  │  Folder tree        │◄─►  (chokidar-rs)         │ │
│  │  Search bar         │  │                        │ │
│  │  Related Notes      │  │  SQLite + sqlite-vec   │ │
│  │  sidebar            │  │  (docs + embeddings)   │ │
│  │                     │  │                        │ │
│  │  react-llm-wiki     │  │  Ollama sidecar        │ │
│  │  WikiProvider       │◄─►  (LLM inference)       │ │
│  │                     │  │                        │ │
│  │                     │  │  FastEmbed sidecar     │ │
│  │                     │◄─►  (local embeddings)    │ │
│  └─────────────────────┘  └───────────────────────┘ │
│                                   │                  │
│                          ┌────────▼────────┐         │
│                          │  OS Keychain    │         │
│                          │  (cloud API     │         │
│                          │   keys)         │         │
│                          └─────────────────┘         │
└─────────────────────────────────────────────────────┘
         │ optional cloud fallback
         ▼
┌─────────────────────────────────────────────────────┐
│  Provider Abstraction (Rust LLMClient trait)        │
│  ┌──────────┐ ┌──────────┐ ┌────────┐ ┌─────────┐  │
│  │ Anthropic│ │  OpenAI  │ │ Gemini │ │ Ollama  │  │
│  └──────────┘ └──────────┘ └────────┘ └─────────┘  │
└─────────────────────────────────────────────────────┘
```

**Key constraints:**
- Frontend never touches files directly — all file ops via Tauri `invoke()`
- Ollama + FastEmbed run as managed Tauri sidecars, bundled in installer
- Cloud API calls proxied through Rust — API key never exposed to JS layer
- No hardcoded model lists — models discovered dynamically from each provider's API

---

## Provider + Model Strategy

**Local (default):** Ollama sidecar. Models listed via `GET /api/tags`. User can pull any model by name. "Pull new model" UI in settings.

**Cloud (opt-in):** Anthropic, OpenAI, Gemini, and others. Models fetched dynamically from each provider's `/models` endpoint using user's API key. Free-text fallback field for unlisted model IDs.

**Model roles (user-assignable, per-folder overridable):**
- Synthesizer — complex cross-doc synthesis
- Utility/Librarian — fast summarization, link linting
- Embeddings — local FastEmbed by default, not overridable to cloud

**API keys:** stored in OS keychain per provider, never in plaintext.

---

## Data Model

**Vault structure on disk:**
```
~/SecondBrain/
├── documents/          ← immutable (watched, never written by app)
│   ├── research/
│   └── notes/
├── wiki/               ← librarian-generated + user-editable
│   ├── synthesis/
│   └── index/
└── .brain/
    ├── brain.db        ← SQLite database
    ├── converted/      ← shadow copies of PDFs/DOCX as markdown
    └── errors.log
```

**SQLite schema:**

| Table | Columns |
|---|---|
| `documents` | id, path, hash, tier, folder_rules_id, last_indexed, status |
| `chunks` | id, doc_id, chunk_text, position |
| `embeddings` | id, chunk_id, vector (sqlite-vec) |
| `wiki_pages` | id, path, source_doc_ids[], generated_by, last_synced, status |
| `folder_rules` | id, folder_path, librarian_mode, provider_override, model_override, auto_approve |

**File deletion cascade:**
1. Remove chunks + embeddings (cascade delete)
2. Purge shadow copy from `.brain/converted/`
3. Flag sourced wiki pages as orphaned (not auto-deleted)
4. Notify user: "Source removed — review orphaned pages"
5. Broken `[[wikilinks]]` highlighted red in BlockNote

---

## Librarian Pipeline

Triggered by file watcher on new/changed file:

```
1. INGEST
   PDF/DOCX → markdown via pandoc sidecar
   Store shadow copy in .brain/converted/

2. CHUNK
   Split into ~512 token chunks
   Store in chunks table

3. EMBED
   FastEmbed generates vectors per chunk
   Store in sqlite-vec

4. CLASSIFY (per folder_rules)
   index only   → stop here
   summarize    → utility model generates wiki summary page
   synthesize   → synthesizer model finds related docs via vector
                  search, generates hub/cross-link pages

5. HUMAN-IN-THE-LOOP (mandatory for synthesize)
   Queue proposed pages for review
   User notification: "X pages ready to review"
   User approves / edits in BlockNote / rejects
   Only approved pages written to wiki/
   (summarize mode: auto_approve setting per folder rule)

6. LINK
   Scan approved pages for [[wikilink]] opportunities
   Suggest links to existing docs/pages inline in BlockNote
```

---

## Frontend Layout

**3-panel layout:**
```
┌──────────┬────────────────────────┬─────────────────┐
│  Sidebar │      BlockNote Editor  │  Related Notes  │
│          │                        │                 │
│  Folder  │  (active wiki page     │  Top 5 cosine   │
│  tree    │   or source doc view)  │  similar chunks │
│          │                        │                 │
│  Search  │                        │  Librarian      │
│  bar     │                        │  suggestions    │
│          │                        │                 │
│  Review  │                        │                 │
│  queue   │                        │                 │
│  badge   │                        │                 │
└──────────┴────────────────────────┴─────────────────┘
```

**First launch — setup wizard (4 steps):**
1. Welcome — "Your second brain, private by default"
2. Install Ollama — detect, download, install, pull default model (progress bar)
3. Choose vault folder — file picker sets `documents/` root
4. Done — librarian starts indexing, user enters app

**Core UX flows:**
- **Add docs:** drag into folder tree → file watcher → pipeline → review badge
- **Review queue:** side-by-side proposed page vs source — approve / edit / reject
- **Search:** hybrid full-text + semantic (sqlite-vec cosine), ranked by combined score
- **Source doc view:** read-only in BlockNote, "User Document — protected" badge, edit button disabled
- **Broken links:** deleted source → `[[doc]]` renders red, tooltip "Source removed"

**Settings panel:**
- Provider management (add API keys per provider → OS keychain)
- Per-folder rules (folder picker → mode → provider override → model override → auto-approve toggle)
- Model role assignment (synthesizer / utility / embeddings → dynamic model picker)

---

## Error Handling

**Ollama setup:**
- Download fails → retry button + manual install link
- Model pull fails → fallback to bundled Phi-4 Mini (~2GB)
- Ollama crashes → health check detects, banner shown, librarian pauses, no data loss

**Ingestion:**
- Corrupt PDF/DOCX → skip, log to `.brain/errors.log`, warning badge on file
- Chunk too large → auto-split at sentence boundary

**Embeddings:**
- FastEmbed OOM → reduce batch size, retry
- DB write fails → transaction rollback, file stays "pending", retry on next watch event

**LLM:**
- Timeout → retry once, then queue for manual review with error note
- Cloud rate limit → exponential backoff, fall back to local if available
- Hallucinated wikilinks → caught by human-in-the-loop before any write

**Vault edge cases:**
- File moved → treated as delete + new file (folder rules re-applied)
- Symlinks → follow one level, warn if circular
- Large vault (10k+ files) → initial index as background job, app fully usable during
- Duplicate files (same hash) → deduplicate chunks, single embedding entry

**Data safety:**
- `brain.db` is fully reconstructable from source files
- On detected corruption, app offers "Rebuild index" (non-destructive, wiki pages preserved)

---

## Key Design Principles

- **Privacy by default:** all processing local, cloud is explicit opt-in
- **Data portability:** vault = plain folder of `.md` files, no proprietary format
- **Human-in-the-loop:** librarian never auto-writes wiki pages without review (synthesize mode)
- **Immutability enforced at Rust layer:** `documents/` is write-protected by backend, not just UI convention
- **Reconstructable state:** DB is a cache, not source of truth — source of truth is the file system
