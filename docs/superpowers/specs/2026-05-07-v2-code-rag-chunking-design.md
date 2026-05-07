# Design: V2 Code-First RAG Chunking

**Date:** 2026-05-07
**Scope:** Replace the universal prose chunker with a hybrid strategy that uses Tree-sitter AST symbols for supported languages, a brace-depth scanner for unsupported code, declarative chunking for config/data files, and the existing sentence chunker for prose. Add line-range metadata to every chunk. Make the embedding model configurable with `nomic-embed-code` as the local default. Expose full chunk metadata through the MCP server for coding agent use.

**Out of scope:** Settings UI, vault-level strategy overrides, wiki/fact extraction (separate spec), re-index-all workflow.

---

## 1. Goals

- Coding agents get symbol-level retrieval — queries for a **known symbol name** ("what does `ingest_file` do?") and **concept queries** ("how does authentication work?") both return focused, relevant chunks with exact file + line range.
- Works across repo scales from small (< 50k lines) to large (50k–500k lines) without configuration changes.
- Mixed codebases (Rust, TS, YAML, Markdown) work without configuration.
- Privacy-first: fully offline by default (`nomic-embed-code` via Ollama, user controls all data); cloud providers optional per vault at user's discretion.
- Clean schema break (no existing users); old chunks re-index naturally on next file change.

## 2. Non-Goals

- No settings surface for chunk strategy in this phase.
- No folder_rules coupling for chunk presets.
- No wiki/fact extraction (scheduled for immediately after this ships).
- No perfect classification for polyglot files (`.vue`, `.svelte`) — Scanner fallback is acceptable.
- No prose-vault optimization mode. V2 is explicitly code-first. Pure prose vaults (personal notes, essays) are v1's domain. `nomic-embed-code` performs well on technical prose (READMEs, docstrings, ADRs) — the degradation vs. a general embedder is small for content that lives alongside code. A vault-level mode can be added in v3 if needed.

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

## 5. Word-Count Budgets

Chunks are sized in words (whitespace-split). No tokenizer dependency in v2 — word count is a fast, dependency-free proxy (~0.75 words per token for code; close enough for local embedding models). Precise tokenization is deferred to when cloud providers ship, as they have tighter hard limits.

| Strategy | Word budget | Split behavior when exceeded |
|----------|-------------|------------------------------|
| `AstSymbol` | 400 | Split at inner method/function boundaries within the symbol; never mid-statement |
| `Scanner` | 200 | Split at nearest indent-0 boundary |
| `Declarative` | 150 | Split at next top-level key or table boundary |
| `Prose` | 100 | Current sentence-group behavior |
| `Fallback` | 100 | Split at nearest blank line |

Minimum chunk size: 20 words. Chunks below this merge with the next sibling rather than being stored as micro-chunks.

---

## 6. Chunk Struct

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

## 7. Strategy Behavior

### 7.1 AstSymbol

Tree-sitter parses the file for the target language. Top-level symbols become chunks:

- **Rust:** `fn`, `impl` blocks, `struct`, `enum`, `trait`, `const`, `type`
- **TypeScript/JavaScript:** `function`, `class`, `const`/`let`/`var` with arrow or function RHS, exported declarations
- **Python:** `def`, `class` at module level
- **Go:** `func`, `type`, `const` block, `var` block

**Method/associated fn chunks** prepend the parent scope signature as a single line prefix (e.g. `impl PipelineWorker {`) so the embedding captures the type relationship without duplicating the full parent body.

**Line ranges** come directly from Tree-sitter node `start_position().row` / `end_position().row` — no heuristics.

**Unsupported language fallback:** if Tree-sitter parse fails or returns zero top-level nodes, fall through to Scanner for that file.

### 7.2 Scanner

Brace-depth + indentation scanner. Attempts cuts at statement/declaration boundaries. No parser dependency. Line ranges derived by counting newlines in the accumulated text buffer as chunks are emitted.

Known limitation: cannot correctly handle template literals, JSX, or heredocs — these are acceptable false cuts for unsupported languages.

### 7.3 Declarative

- **YAML:** split on `---` document boundaries, then on top-level keys (column-0 or consistent indent root); group short list items. Symbol name = top-level key.
- **JSON/JSONC:** split on top-level array elements or object keys by tracking character depth. Symbol name = key string.
- **TOML:** split on `[table]` / `[[table]]` boundaries. Symbol name = table header.
- **XML:** split on top-level element boundaries.

Size cap + small overlap between adjacent blocks when a block exceeds the word budget (§5). Line ranges tracked by scanning newlines as boundaries are found.

### 7.4 Prose

Existing sentence-aware chunker with neighbor-sentence padding. Line ranges added by scanning the input for each chunk's byte range and counting preceding newlines. Symbol name is always `None`.

### 7.5 Fallback

Blank-line / paragraph-like splits + max segment cap + small overlap. No sentence heuristics. Line ranges by newline scan. Suitable for LICENSE, extensionless configs, unknown formats.

---

## 8. Embedding Model

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

## 9. Schema Migration (V4)

```sql
ALTER TABLE chunks ADD COLUMN start_line   INTEGER;
ALTER TABLE chunks ADD COLUMN end_line     INTEGER;
ALTER TABLE chunks ADD COLUMN symbol_name  TEXT;
ALTER TABLE chunks ADD COLUMN strategy     TEXT NOT NULL DEFAULT 'prose';

INSERT OR IGNORE INTO schema_version (version) VALUES (4);
```

Old chunks get `start_line = NULL` / `end_line = NULL`. They re-index on next file change and pick up line ranges naturally. No forced migration pass required.

---

## 10. MCP Server Response

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

## 11. Testing

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

## 12. Milestones

| # | Scope |
|---|-------|
| **M1** | Schema MIGRATION_V4. `EmbedProfile` config + `Embedder::new(profile)`. Wire `nomic-embed-code` as default. Existing tests pass. |
| **M2** | Classifier + Fallback + Prose with line ranges. Parity: existing `.md`/`.txt` vaults re-index cleanly. |
| **M3** | Declarative (YAML + JSON + TOML + XML). Scanner with line ranges. |
| **M4** | AstSymbol: Rust + Python + Go. Integration tests per strategy. |
| **M5** | AstSymbol: TypeScript + TSX + JavaScript. MCP server returns full result shape. |
| **Later** | Additional Tree-sitter grammars, cloud provider config UI, vault-level embed profile picker. |

---

## 13. Risks / Mitigations

| Risk | Mitigation |
|------|------------|
| Tree-sitter binary size (~2 MB per grammar) | Ship only the 5 launch grammars; others added lazily |
| Tree-sitter parse failure on malformed file | Fallback to Scanner for that file; log `(path, reason)` |
| `nomic-embed-code` not pulled yet in Ollama | Embedder init checks model exists; surfaces error to user with pull command |
| `.vue`/`.svelte` Scanner produces bad cuts | Known limitation; documented; Tree-sitter grammars deferred to later milestone |
| Old chunks have NULL line ranges | Acceptable; re-index on file change; MCP handles NULL gracefully |

---

## 14. Future Work (Out of Scope)

- **Wiki fact extraction:** atomic claims from prose chunks, LLM-generated tags, librarian reconciliation — next spec after this ships.
- **Vault-level strategy override:** force a folder to use a specific strategy via folder_rules.
- **Re-index-all command:** bulk re-ingest existing vault with new chunking.
- **Additional grammars:** Java, Kotlin, Swift, C/C++, Ruby, PHP.
