# Design: V2 Code-First RAG Chunking

**Date:** 2026-05-07
**Scope:** Replace the universal prose chunker with a hybrid strategy that uses Tree-sitter AST symbols for supported languages, a brace-depth scanner for unsupported code, declarative chunking for config/data files, and the existing sentence chunker for prose. Add line-range metadata to every chunk. Make the embedding model configurable with `nomic-embed-code` as the local default. Expose full chunk metadata through the MCP server for coding agent use.

**Out of scope:** Settings UI, vault-level strategy overrides, wiki/fact extraction (separate spec), re-index-all workflow.

---

## 1. Goals

- Coding agents get symbol-level retrieval — queries for a function name or concept return the full symbol body with exact file + line range.
- Mixed codebases (Rust, TS, YAML, Markdown) work without configuration.
- Privacy-first: fully local by default (`nomic-embed-code` via Ollama); cloud providers optional per vault.
- Clean schema break (no existing users); old chunks re-index naturally on next file change.

## 2. Non-Goals

- No settings surface for chunk strategy in this phase.
- No folder_rules coupling for chunk presets.
- No wiki/fact extraction (scheduled for immediately after this ships).
- No perfect classification for polyglot files (`.vue`, `.svelte`) — Scanner fallback is acceptable.

---

## 3. Architecture

```
ingest(path, bytes)
      │
      ▼
classify(path) ──► Strategy { AstSymbol(lang) | Scanner | Declarative | Prose | Fallback }
      │
      ▼
chunk_for_strategy(strategy, text) ──► Vec<Chunk { text, start_line, end_line, symbol_name? }>
      │
      ▼
embed(chunks) via EmbedProfile { Local("nomic-embed-code") | Cloud(provider, model, key) }
      │
      ▼
store: chunks (text, start_line, end_line, symbol_name, strategy) + embeddings
```

The pipeline call site changes from `chunk_text(&text)` to `chunk_autodetect(path, &text) -> Vec<Chunk>`. Everything downstream (embed, insert) is unchanged in structure.

---

## 4. Classification Rules

Order: first match wins; else Fallback.

| Strategy | Extensions |
|----------|-----------|
| `AstSymbol(Rust)` | `rs` |
| `AstSymbol(TypeScript)` | `ts`, `tsx` |
| `AstSymbol(JavaScript)` | `js`, `jsx`, `mjs`, `cjs` |
| `AstSymbol(Python)` | `py` |
| `AstSymbol(Go)` | `go` |
| `Scanner` | `java`, `kt`, `swift`, `c`, `h`, `cpp`, `hpp`, `cs`, `rb`, `php`, `vue`, `svelte` |
| `Declarative` | `yaml`, `yml`, `json`, `jsonc`, `toml`, `xml` |
| `Prose` | `md`, `markdown`, `txt`, `rst`, `org` |
| `Fallback` | everything else, unknown extension, no extension |

Classifier is a pure function: `classify(path: &Path) -> Strategy`. No I/O, no content sniffing in v2.

---

## 5. Chunk Struct

```rust
pub struct Chunk {
    pub text: String,
    pub start_line: u32,   // 1-indexed, inclusive
    pub end_line: u32,     // 1-indexed, inclusive
    pub symbol_name: Option<String>,
    pub strategy: ChunkStrategy,
}
```

All chunkers return `Vec<Chunk>`. The pipeline stores `start_line`, `end_line`, `symbol_name`, and `strategy` alongside `chunk_text`.

---

## 6. Strategy Behavior

### 6.1 AstSymbol

Tree-sitter parses the file for the target language. Top-level symbols become chunks:

- **Rust:** `fn`, `impl` blocks, `struct`, `enum`, `trait`, `const`, `type`
- **TypeScript/JavaScript:** `function`, `class`, `const`/`let`/`var` with arrow or function RHS, exported declarations
- **Python:** `def`, `class` at module level
- **Go:** `func`, `type`, `const` block, `var` block

**Method/associated fn chunks** prepend the parent scope signature as a single line prefix (e.g. `impl PipelineWorker {`) so the embedding captures the type relationship without duplicating the full parent body.

**Small symbol merging:** symbols under 20 tokens merge with the next sibling to avoid micro-chunks (e.g. a one-line `const` pairs with the following `fn`).

**Line ranges** come directly from Tree-sitter node `start_position().row` / `end_position().row` — no heuristics.

**Unsupported language fallback:** if Tree-sitter parse fails or returns zero top-level nodes, fall through to Scanner for that file.

### 6.2 Scanner

Brace-depth + indentation scanner. Attempts cuts at statement/declaration boundaries. No parser dependency. Line ranges derived by counting newlines in the accumulated text buffer as chunks are emitted.

Known limitation: cannot correctly handle template literals, JSX, or heredocs — these are acceptable false cuts for unsupported languages.

### 6.3 Declarative

- **YAML:** split on `---` document boundaries, then on top-level keys (column-0 or consistent indent root); group short list items. Symbol name = top-level key.
- **JSON/JSONC:** split on top-level array elements or object keys by tracking character depth. Symbol name = key string.
- **TOML:** split on `[table]` / `[[table]]` boundaries. Symbol name = table header.
- **XML:** split on top-level element boundaries.

Size cap + small overlap between adjacent blocks when a block exceeds the token budget. Line ranges tracked by scanning newlines as boundaries are found.

### 6.4 Prose

Existing sentence-aware chunker with neighbor-sentence padding. Line ranges added by scanning the input for each chunk's byte range and counting preceding newlines. Symbol name is always `None`.

### 6.5 Fallback

Blank-line / paragraph-like splits + max segment cap + small overlap. No sentence heuristics. Line ranges by newline scan. Suitable for LICENSE, extensionless configs, unknown formats.

---

## 7. Embedding Model

### EmbedProfile

```rust
pub enum EmbedProfile {
    Local { model: String },                          // default: "nomic-embed-code"
    Cloud { provider: CloudProvider, model: String, api_key: String },
}

pub enum CloudProvider { OpenAi, Voyage, Cohere }
```

Stored in vault config. `Embedder::new(profile)` selects the implementation. Local profile stays fully offline. Cloud profile is opt-in per vault — user supplies key.

Default for all new vaults: `Local { model: "nomic-embed-code" }`.

`nomic-embed-code` is pulled via Ollama like any other model. No additional runtime dependencies.

---

## 8. Schema Migration (V4)

```sql
ALTER TABLE chunks ADD COLUMN start_line   INTEGER;
ALTER TABLE chunks ADD COLUMN end_line     INTEGER;
ALTER TABLE chunks ADD COLUMN symbol_name  TEXT;
ALTER TABLE chunks ADD COLUMN strategy     TEXT NOT NULL DEFAULT 'prose';

INSERT OR IGNORE INTO schema_version (version) VALUES (4);
```

Old chunks get `start_line = NULL` / `end_line = NULL`. They re-index on next file change and pick up line ranges naturally. No forced migration pass required.

---

## 9. MCP Server Response

Each search result includes:

```json
{
  "file": "src-tauri/src/pipeline/mod.rs",
  "start_line": 210,
  "end_line": 254,
  "symbol_name": "ingest_file",
  "strategy": "ast_symbol",
  "score": 0.91,
  "text": "fn ingest_file(conn: &Connection, embedder: &Embedder, path: &str) -> Result<()> { ... }"
}
```

`start_line` and `end_line` are always present (nullable for legacy unindexed chunks). Coding agent can jump directly to source without reconstruction.

---

## 10. Testing

| Layer | Required Cases |
|-------|---------------|
| **Classifier** | Each extension → correct strategy; unknown ext → Fallback; no extension → Fallback |
| **AstSymbol/Rust** | fn, impl block, associated fn (parent prefix), const, small-symbol merge |
| **AstSymbol/TS** | function, class, arrow const export, nested fn not chunked separately |
| **AstSymbol/Python** | def, class, nested method has parent prefix |
| **AstSymbol/Go** | func, type, const block |
| **AstSymbol edge** | file with only imports → Scanner fallback; parse failure → Scanner fallback |
| **Scanner** | brace-depth cut, string with `};` inside not cut, line range from newline count |
| **Declarative** | multi-doc YAML, nested JSON top-level split, TOML tables, line ranges present |
| **Prose** | existing tests pass; line ranges present on all chunks |
| **Fallback** | no extension, line ranges present |
| **Integration** | one ingest-to-search round-trip per strategy with small fixture — required |
| **MCP shape** | response includes `start_line`, `end_line`, `symbol_name` for each strategy |
| **EmbedProfile** | local routes to Ollama; cloud routes to provider; vault config round-trips |

---

## 11. Milestones

| # | Scope |
|---|-------|
| **M1** | Schema MIGRATION_V4. `EmbedProfile` config + `Embedder::new(profile)`. Wire `nomic-embed-code` as default. Existing tests pass. |
| **M2** | Classifier + Fallback + Prose with line ranges. Parity: existing `.md`/`.txt` vaults re-index cleanly. |
| **M3** | Declarative (YAML + JSON + TOML + XML). Scanner with line ranges. |
| **M4** | AstSymbol: Rust + Python + Go. Integration tests per strategy. |
| **M5** | AstSymbol: TypeScript + TSX + JavaScript. MCP server returns full result shape. |
| **Later** | Additional Tree-sitter grammars, cloud provider config UI, vault-level embed profile picker. |

---

## 12. Risks / Mitigations

| Risk | Mitigation |
|------|------------|
| Tree-sitter binary size (~2 MB per grammar) | Ship only the 5 launch grammars; others added lazily |
| Tree-sitter parse failure on malformed file | Fallback to Scanner for that file; log `(path, reason)` |
| `nomic-embed-code` not pulled yet in Ollama | Embedder init checks model exists; surfaces error to user with pull command |
| `.vue`/`.svelte` Scanner produces bad cuts | Known limitation; documented; Tree-sitter grammars deferred to later milestone |
| Old chunks have NULL line ranges | Acceptable; re-index on file change; MCP handles NULL gracefully |

---

## 13. Future Work (Out of Scope)

- **Wiki fact extraction:** atomic claims from prose chunks, LLM-generated tags, librarian reconciliation — next spec after this ships.
- **Vault-level strategy override:** force a folder to use a specific strategy via folder_rules.
- **Re-index-all command:** bulk re-ingest existing vault with new chunking.
- **Additional grammars:** Java, Kotlin, Swift, C/C++, Ruby, PHP.
