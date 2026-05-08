# Design: V2 Code-First RAG Chunking

**Original date:** 2026-05-07  
**Revised:** 2026-05-07 — **Aligned with the shipped hybrid chunk autodetect layer** (`chunk_autodetect`, extension classifier, prose / code-like / declarative / fallback). Supersedes narrative written against the pre-hybrid codebase.

**Status:** Implemented

**Scope (target):** Evolve ingestion from **`Vec<String>` chunks** to **`Chunk { text, line range, optional symbol, strategy tag}`**, introduce **schema migration adding chunk metadata columns**, and make **embedding** selectable (**`nomic-embed-code` via Ollama** locally by default) while keeping **FastEmbed frozen-vector benchmarks** intact. Expose chunk metadata through **Tauri search APIs** (and TypeScript types); treat **MCP** as a **follow-on transport**, not a dependency for shipping retrieval upgrades.

**Pre-release posture:** The app has **no active production users** yet. **Breaking changes are acceptable** — schema reshaping, embedding dimension switches, wiping dev databases (`brain.db`), and non-additive Tauri/TS API changes do **not** require backward compatibility with prior milestones or lazy migration of legacy chunk rows.

**Out of scope:** Settings UI for chunk strategy, vault-level strategy overrides via `folder_rules`, wiki/fact extraction (separate spec), bulk re-index-all command.

---

## 0. Baseline — What Exists Today (Hybrid Autodetect)

This section is the **source of truth** for implementation reviewers; later sections describe the **delta** to V2.

### Ingestion and chunking

- **`chunk_autodetect(path, text) -> Vec<String>`** in `src-tauri/src/chunker/mod.rs`.
- **`classify(path) -> ChunkStrategy`** (`src-tauri/src/chunker/classify.rs`): **`Prose` | `CodeLike` | `Declarative` | `Fallback`** — pure extension table, deterministic.
- **`should_ingest_extension`** gates watcher / listing / pipeline (`pdf`, `docx`, prose/code/config extensions).
- Implementations:
  - **Prose** — sentence-aware + neighbor padding, **`TARGET_WORDS = 100`** (`chunker/prose.rs`).
  - **Code-like** — brace / statement heuristic scanner with comment/string/template awareness (`chunker/code_like.rs`) — serves the role of the design doc’s **Scanner** until Tree-sitter lands per language.
  - **Declarative** — YAML / JSON / TOML / XML logical splits (`chunker/declarative.rs`); YAML supports indented-root keys via minimum-indent map detection.
  - **Fallback** — blank-line-ish merges + char cap + overlap (`chunker/fallback.rs`).
- **Non-prose sizing today:** shared **`target_chars()` ≈ 1600**, **`overlap_chars()`**, **`code_overlap_lines()`** (`chunker/limits.rs`) — not the §5 word-budget table yet.

### Persistence and search

- **SQLite:** `chunks(id, doc_id, chunk_text, position)` only — **no** `start_line`, `end_line`, `symbol_name`, `strategy` yet (`schema.rs` through **`MIGRATION_V3`**; **`schema_version` up to 3** on full `AppDb::open`).
- **`SearchResult`** (Rust `search/mod.rs`, TS `src/lib/tauri.ts`): **`doc_path`, `chunk_text`, `chunk_position`, `score`** only.

### Embeddings

- **Production pipeline + wiki embed path:** **`Embedder`** = FastEmbed **`AllMiniLML6V2`**, **384 dimensions** (`embedder/mod.rs`).
- **SciFact / YAML / code recall benches:** frozen **`*.json.gz`** embeddings tied to that model width — kept stable intentionally (`tests/fixtures/`, `*_fixture.rs`).

### Debug logging

- **`[ingest-chunk]`** strategy trace only under **`cfg!(debug_assertions)`** (`chunker/mod.rs`).

### Not implemented yet

- Tree-sitter **`AstSymbol`** paths.
- **`Chunk` struct** and **`Vec<Chunk>`** through pipeline / **`insert_chunk`**.
- **`EmbedProfile`** / vault-config embedding selection / **`OllamaEmbedder`** in the hot path.
- **MCP server** — aspirational in `docs/superpowers/specs/2026-05-05-second-brain-app-design.md`; **no MCP bundle ships with the app today**. V2 delivers metadata through **`search_vault` / `get_related_chunks`** first.

---

## 1. Goals

- Coding agents get **symbol-aware retrieval** where AST is available — queries mentioning **`ingest_file`** resolve to chunks carrying **file + line span + optional symbol name**.
- **Concept queries** still work via overlapping chunks and neighbor context (prose padding today; symbol parent prefixes after AST).
- **Mixed repos** (Rust, TS, YAML, Markdown, etc.) continue to work **without** chunk-strategy settings — classification stays extension-driven.
- **Privacy-first:** offline **local** embedding default (**`nomic-embed-code`** via Ollama); optional **cloud** profile later with user-supplied keys.
- **Breaking changes OK:** ship the simplest correct schema and APIs for V2; developers reset or recreate local DBs as needed. Optional **`NULL`** metadata columns remain useful only where a field is semantically absent (e.g. **`symbol_name`** on prose), not as a permanent compat layer for old chunk rows.

---

## 2. Non-Goals

- No settings surface for **chunk strategy** in this phase (unchanged from hybrid plan).
- No **`folder_rules`** coupling for chunk presets.
- No wiki/fact extraction here.
- No guarantee of perfect cuts on **polyglot single-file** formats (`.vue`, `.svelte`) — **Scanner / code-like** remains acceptable until dedicated grammars exist.
- No vault-level “prose-only embedder” mode — V2 stays **code-first**; prose vaults remain supported via **`Prose`** strategy and sentence chunking.

---

## 3. Architecture

### Today (shipped)

```
ingest(path, bytes)
      │
      ▼
classify(path) ──► ChunkStrategy { Prose | CodeLike | Declarative | Fallback }
      │
      ▼
chunk_autodetect ──► Vec<String>
      │
      ▼
embed via FastEmbed AllMiniLM (384-d)
      │
      ▼
store: chunks(chunk_text, position) + embeddings(blob)
```

### Target (V2)

```
ingest(path, bytes)
      │
      ▼
classify(path) ──► Strategy { AstSymbol(lang) | Scanner | Declarative | Prose | Fallback }
      │               ▲
      │               └── evolves from today's ChunkStrategy:
      │                     AstSymbol replaces CodeLike for grammars we ship;
      │                     remaining CodeLike extensions become Scanner-only.
      ▼
chunk_for_strategy ──► Vec<Chunk { text, start_line, end_line, symbol_name?, strategy }>
      │
      ▼
embed(chunks.text[]) via EmbedProfile { Local("nomic-embed-code") | Cloud(...) }
      │
      ▼
store: chunks (+ start_line, end_line, symbol_name, strategy) + embeddings
```

**Pipeline contract change:** `chunk_autodetect` (or successor name) returns **`Vec<Chunk>`**; **`insert_chunk`** / queries / **`SearchResult`** carry metadata; **`search_vault`** JSON gains matching fields.

---

## 4. Classification Rules (Target)

Order: **first match wins**; else **`Fallback`**.

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
| `Fallback` | everything else (incl. unknown / no ext, **`pdf`**, **`docx`** after text extraction) |

**Classifier:** pure function `classify(path: &Path) -> Strategy`, no I/O.

**Rollout note:** Until **`AstSymbol`** ships for a bucket, implementations **must** fall back to today’s **`code_like`** scanner for those extensions so ingest never regresses.

---

## 5. Word-Count Budgets (Design Targets)

Chunks are sized by **whitespace-split word count** as a cheap proxy for token limits. **Today’s code uses character caps for non-prose** (`limits.rs`); V2 may **converge** strategies toward this table or keep chars internally — either way, **document the chosen mapping in PRs**.

| Strategy | Word budget (target) | When oversized |
|----------|----------------------|----------------|
| `AstSymbol` | 400 | Split inner boundaries inside the symbol; never mid-statement |
| `Scanner` | 200 | Split at safe scanner boundaries + indent / brace cues |
| `Declarative` | 150 | Next top-level key / table / logical block |
| `Prose` | 100 | Existing sentence-group + neighbor padding |
| `Fallback` | 100 | Blank-line / paragraph merge pattern |

**Minimum chunk size (target):** ~**20 words** — merge undersized siblings before persist (not enforced in today’s hybrid paths).

---

## 6. Chunk Struct

```rust
pub struct Chunk {
    pub text: String,
    pub start_line: u32,   // 1-indexed, inclusive
    pub end_line: u32,     // 1-indexed, inclusive
    pub symbol_name: Option<String>,
    pub strategy: ChunkStrategyTag, // serializable tag for DB + API (string enum)
}
```

All chunkers return **`Vec<Chunk>`**. The DB stores **`chunk_text`** plus **`start_line`**, **`end_line`**, **`symbol_name`**, **`strategy`** (see §9).

---

## 7. Strategy Behavior

### 7.1 AstSymbol (new)

Tree-sitter parses the file for `lang`. Top-level symbols become chunks (Rust `fn`/`impl`/…; TS/JS exports; Python `def`/`class`; Go `func`/`type`/blocks). **Method chunks** prepend a one-line parent scope header for context. **Line ranges** come from Tree-sitter node rows.

**Fallback:** parse failure or empty capture → **Scanner** (`code_like` logic) for that file.

### 7.2 Scanner

Brace-depth + indentation heuristics — **`chunker/code_like.rs`** is the reference implementation today (string/template/comment-aware). V2 adds **accurate line ranges** on emitted segments and aligns sizing with §5 over time.

Known limitation: imperfect on JSX-heavy files without TS grammar — acceptable until **`AstSymbol`** covers them.

### 7.3 Declarative

As **`chunker/declarative.rs`** today, plus **1-indexed line spans** per emitted block and **`symbol_name`** where meaningful (YAML key, JSON key, TOML header, XML element).

### 7.4 Prose

**`chunker/prose.rs`** unchanged logically; wrap outputs as **`Chunk`** with line ranges derived from substring → newline counting; **`symbol_name = None`**.

### 7.5 Fallback

**`chunker/fallback.rs`** unchanged logically; attach line ranges; **`symbol_name = None`**.

---

## 8. Embedding Model

### EmbedProfile (new)

```rust
pub enum EmbedProfile {
    Local { model: String },                          // default: "nomic-embed-code"
    Cloud { provider: CloudProvider, model: String, api_key: String },
}

pub enum CloudProvider { OpenAi, Voyage, Cohere }
```

Stored in **vault config JSON** (extends `ConfigFile` in `vault/config.rs`). Pipeline + wiki embedding construct **`OllamaEmbedder`** (or cloud client) from profile.

**Benchmark exception:** keep **`Embedder` (FastEmbed)** crate surface for **SciFact / frozen gzip benchmarks** — do **not** retune those fixtures when flipping production defaults.

**Dimension change:** switching embedders changes vector width — **query vectors and stored blobs must match**; expect **full re-embed** of touched docs after profile change (document UX separately).

---

## 9. Schema Migration (V4)

```sql
ALTER TABLE chunks ADD COLUMN start_line   INTEGER;
ALTER TABLE chunks ADD COLUMN end_line     INTEGER;
ALTER TABLE chunks ADD COLUMN symbol_name  TEXT;
ALTER TABLE chunks ADD COLUMN strategy     TEXT NOT NULL DEFAULT 'prose';

INSERT OR IGNORE INTO schema_version (version) VALUES (4);
```

Implementations may use **`ALTER TABLE … ADD COLUMN`** as above or replace **`chunks`/`embeddings`** wholesale during development — **no obligation** to preserve rows across milestones. If **`ALTER`** is used, **`DEFAULT`** values for **`strategy`** are acceptable for one-off dev DBs; fresh installs should always persist full metadata from ingest.

For fields that are **optional by semantics** (`symbol_name`), **`NULL`** is fine. **`start_line` / `end_line`** should be **required for every newly indexed chunk** once chunkers emit ranges; APIs may still expose them as **`Option`** only if a legacy code path remains temporarily.

---

## 10. Search — Rust, TypeScript, Future MCP

### Ship-of-record for V2

Extend **`SearchResult`** (`search/mod.rs`):

- Keep **`doc_path`, `chunk_text`, `chunk_position`, `score`**
- Add **`start_line`**, **`end_line`** (present for all chunks produced under V2 ingest), **`symbol_name: Option<…>`**, **`strategy`**

Breaking changes to the **`search_vault`** JSON shape are **acceptable** — bump any brittle frontend helpers alongside Rust.

Mirror fields in **`src/lib/tauri.ts`** **`SearchResult`** for **`search_vault`** / **`get_related_chunks`**.

UI components (**`SearchResults.tsx`**, **`RelatedNotes.tsx`**) display snippet + optional **`path:line`** affordance.

### MCP (later)

Per **`2026-05-05-second-brain-app-design.md`**, an MCP server would wrap the **same query layer** as Tauri — **no separate retrieval semantics**. V2 **does not require MCP** to be valuable; milestones below gate on **Tauri JSON shape**, not MCP packaging.

Example payload shape (informative):

```json
{
  "file": "src-tauri/src/pipeline/mod.rs",
  "start_line": 210,
  "end_line": 254,
  "symbol_name": "ingest_file",
  "strategy": "ast_symbol_rust",
  "score": 0.91,
  "text": "fn ingest_file(...) { ... }"
}
```

---

## 11. Testing

| Layer | Required |
|-------|-----------|
| **Classifier** | Extension → strategy; unknown → Fallback; **`pdf`/`docx` → Fallback** |
| **Hybrid parity** | Existing **`md`/`txt`** tests remain (`chunk_text` vs prose branch) |
| **AstSymbol** | Per-language symbol splits + parent prefix + parse-fail → Scanner |
| **Scanner** | Strings with misleading `};`, JSX smoke, **line ranges** |
| **Declarative** | Multi-doc YAML, nested JSON, TOML tables, XML smoke |
| **Prose / Fallback** | Line ranges on every chunk |
| **DB** | Migration V4 columns exist; insert/select round-trip |
| **Search API** | New fields present (serde + TS types) |
| **EmbedProfile** | Config round-trip; local embed calls Ollama fixture / mocked HTTP |
| **Benchmarks** | **`slow-tests`** SciFact + YAML/code benches still pass **without** switching them to Ollama |

---

## 12. Milestones (Rebased on Current Tree)

| # | Scope |
|---|-------|
| **✓ shipped** | Hybrid **`chunk_autodetect`**, **`ChunkStrategy`**, **`code_like` / declarative / fallback / prose**, ingest extension list, debug-only strategy logging |
| **M1** | **`MIGRATION_V4`** + **`Chunk`** type + thread through **`pipeline`** / **`insert_chunk`** / queries — embed path **unchanged** (still FastEmbed) optional first PR |
| **M2** | **`EmbedProfile`** + **`OllamaEmbedder`** + vault config; pipeline switches default local model to **`nomic-embed-code`**; dimension / migration UX documented |
| **M3** | **`SearchResult` + TS** extended metadata; UI shows line hints |
| **M4** | Declarative + Scanner chunks emit **non-placeholder line ranges** everywhere |
| **M5** | **AstSymbol:** Rust + Python + Go |
| **M6** | **AstSymbol:** TypeScript + TSX + JavaScript |
| **Later** | MCP wrapper; extra grammars; cloud provider UI; bulk re-index |

---

## 13. Risks / Mitigations

| Risk | Mitigation |
|------|------------|
| Tree-sitter crate size | Ship only launch grammars; lazy-add others |
| Parse failures | Scanner fallback + `(path, reason)` logging |
| Model not pulled (`nomic-embed-code`) | Embedder init surfaces actionable **pull** hint |
| `.vue` / `.svelte` cuts | Document Scanner limits; grammars later |
| **`symbol_name` absent** | Expected for prose/fallback; **`Option`** in API |
| **384 → N dim** embedder switch | Breaking for stored vectors — acceptable; devs re-index or wipe DB |

---

## 14. Future Work

- **MCP server** package exposing **`search_vault`-equivalent** tools.
- **Wiki fact extraction** pipeline.
- **`folder_rules`** overrides for chunk / embed profile.
- **Re-index-all** command.
- Additional Tree-sitter grammars for **Scanner-only** extensions today.
