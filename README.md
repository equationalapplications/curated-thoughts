# Curated Thoughts

Curated Thoughts is a privacy-first, local-first desktop second brain built with Tauri, React, and Rust.

Inspired by [Andrej Karpathy's LLM Wiki memory spec](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f). It uses `@equationalapplications/react-llm-wiki` to power a local LLM Wiki experience. The automatic librarian will correct the wiki entries based on your immutable sources of truth and the new information it ingests.

## Overview

Curated Thoughts lets users drop files into a watched vault and automatically indexes them into a searchable knowledge base. A local Active Librarian processes documents into wiki pages, while the frontend keeps the file system isolated from direct writes.

Key app concepts:
- **Immutable source documents** in a watched `documents/` vault
- **Generated, reviewable wiki pages** in `wiki/`
- **Background ingestion pipeline** for document conversion, chunking, embedding, and synthesis
- **Local LLM-powered memory** via `@equationalapplications/react-llm-wiki`

## Why this project exists

This app applies the LLM Wiki idea to a desktop second brain: persistent episodic memory, semantic retrieval, and human-in-the-loop synthesis. It connects local file content, embeddings, and long-term memory into a unified experience.

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

- `@equationalapplications/react-llm-wiki` — React web support for local LLM Wiki memory. https://www.npmjs.com/package/@equationalapplications/react-llm-wiki
- `@equationalapplications/expo-llm-wiki` — Expo / React Native version with `expo-sqlite` adapter. https://www.npmjs.com/package/@equationalapplications/expo-llm-wiki
- `@equationalapplications/core-llm-wiki` — framework-agnostic core logic for Node or browser environments. https://www.npmjs.com/package/@equationalapplications/core-llm-wiki
- `expo-llm-wiki` GitHub repo — https://github.com/equationalapplications/expo-llm-wiki

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
