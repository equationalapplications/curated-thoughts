# Design: AstSymbol Chunking (Tree-sitter)

**Date:** 2026-05-07

**Status:** Implemented

**Scope:** Implement the `AstSymbol` chunking strategy for Rust, TypeScript, JavaScript, Python, and Go using the `tree-sitter` crate with per-language `.scm` query strings. This covers M5 and M6 from the V2 Code-First RAG Chunking spec (`2026-05-07-v2-code-rag-chunking-design.md`).

**Pre-release posture:** No active production users. Breaking changes to chunk shape, strategy tags, and DB rows are acceptable.

**Out of scope:** MCP server, additional grammars beyond the five above, cloud embed profiles, Settings UI, folder_rules.

---

## 0. Baseline

From the V2 spec (now implemented):

- `Chunk { text, start_line, end_line, symbol_name, strategy }` — shipped.
- `ChunkStrategy::CodeLike` handles Rust/TS/JS/Python/Go via `code_like.rs` Scanner today.
- `ChunkStrategyTag` serializes as `"scanner"`, `"prose"`, `"declarative"`, `"fallback"`.
- No `tree-sitter` dependency in `Cargo.toml` yet.

---

## 1. Goals

- Symbol-aware chunks for Rust, TS, JS, Python, Go: each top-level function / class / impl / type becomes its own chunk with accurate `start_line`, `end_line`, `symbol_name`.
- Method chunks carry a one-line parent-scope prefix for embedding context.
- Parse failures degrade gracefully to the Scanner (`code_like`) — no panics, no empty results.
- Benchmark parity: SciFact / YAML / code `slow-tests` are unaffected.

---

## 2. Non-Goals

- No grammars beyond the five listed above in this phase.
- No polyglot single-file formats (`.vue`, `.svelte`) — Scanner remains.
- No vault-level grammar overrides.

---

## 3. Architecture

### Classifier delta

`classify.rs` gains `AstSymbol(AstLang)` as a new `ChunkStrategy` variant:

```rust
pub enum AstLang { Rust, TypeScript, JavaScript, Python, Go }

pub enum ChunkStrategy {
    AstSymbol(AstLang),   // new
    CodeLike,
    Declarative,
    Prose,
    Fallback,
}
```

Extension table update (first-match wins):

| Extensions | Strategy |
|-----------|---------|
| `rs` | `AstSymbol(Rust)` |
| `ts`, `tsx` | `AstSymbol(TypeScript)` |
| `js`, `jsx`, `mjs`, `cjs` | `AstSymbol(JavaScript)` |
| `py` | `AstSymbol(Python)` |
| `go` | `AstSymbol(Go)` |
| `java`, `kt`, `swift`, `c`, `h`, `cpp`, `hpp`, `cs`, `rb`, `php`, `vue`, `svelte` | `CodeLike` (unchanged) |

### `ChunkStrategyTag` additions

```rust
pub enum ChunkStrategyTag {
    AstSymbolRust,
    AstSymbolTypeScript,
    AstSymbolJavaScript,
    AstSymbolPython,
    AstSymbolGo,
    Scanner,       // existing
    Declarative,   // existing
    Prose,         // existing
    Fallback,      // existing
}
```

Serialization: `"ast_symbol_rust"`, `"ast_symbol_typescript"`, `"ast_symbol_javascript"`, `"ast_symbol_python"`, `"ast_symbol_go"`.

### Dispatch

`chunk_autodetect` adds the `AstSymbol(lang)` arm:

```
ingest(path, text)
  → classify(path) → AstSymbol(lang) → ast_symbol::chunk(lang, text)
                                        ↳ parse fail / 0 captures → code_like::chunk_code_like_chunks(text)
  → CodeLike                          → code_like::chunk_code_like_chunks(text)   (unchanged)
  → Declarative / Prose / Fallback    (unchanged)
```

---

## 4. Module: `chunker/ast_symbol.rs`

Single public entry point:

```rust
pub fn chunk(lang: AstLang, text: &str) -> Vec<Chunk>
```

Internal steps:

1. Select grammar and query string for `lang`.
2. Parse `text` with `tree_sitter::Parser`.
3. Run the `.scm` query on the root node, collecting `@symbol` + `@name` captures.
4. If capture list is empty or parse returns `None`, fall back to `code_like::chunk_code_like_chunks(text)` with the appropriate `AstSymbol*` strategy tag replaced by `Scanner`.
5. For each captured symbol node:
   a. Extract `start_line` / `end_line` from node positions (0-indexed row → 1-indexed).
   b. Extract `symbol_name` from `@name` capture text; qualify with parent name if inside a method container (see §5).
   c. Prepend parent-scope header line if applicable (see §5).
   d. If chunk word count > 400, apply inner-boundary split (see §6).
   e. If chunk word count < 20, flag for merge with adjacent sibling (post-pass).
6. Return `Vec<Chunk>` with `strategy = ChunkStrategyTag::AstSymbol*`.

### Cargo.toml additions

```toml
tree-sitter = "0.22"
tree-sitter-rust = "0.21"
tree-sitter-typescript = "0.21"
tree-sitter-javascript = "0.21"
tree-sitter-python = "0.21"
tree-sitter-go = "0.21"
```

Exact versions: pin to whatever is latest-stable at implementation time; note in PR.

---

## 5. Per-Language Query Strings & Symbol Sets

Each query is an embedded `const &str` inside `ast_symbol.rs` (or a sibling `ast_queries.rs` if the file grows large).

Captures required: `@symbol` (whole node), `@name` (identifier child).

### 5.1 Rust

**Granularity rule:** `impl_item` blocks are NOT emitted as whole chunks. Each method (`function_item` inside an `impl_item`) becomes its own chunk. Non-method items (const, type aliases) inside an `impl_item` that appear before any method are included as a preamble on the first method chunk.

Target symbol types:

| Node | Treatment |
|------|-----------|
| Top-level `function_item` | Whole chunk |
| `struct_item`, `enum_item`, `trait_item`, `type_item` | Whole chunk |
| `function_item` inside `impl_item` | Method chunk with `// impl TypeName` prefix |

```scheme
; Top-level functions
(source_file (function_item name: (identifier) @name) @symbol)
; Named types — whole chunk
(struct_item name: (type_identifier) @name) @symbol
(enum_item name: (type_identifier) @name) @symbol
(trait_item name: (type_identifier) @name) @symbol
(type_item name: (type_identifier) @name) @symbol
; Methods inside impl blocks — each method is its own chunk
(impl_item body: (declaration_list
  (function_item name: (identifier) @name) @symbol))
```

`symbol_name` for methods: `"TypeName::method_name"`. Parent prefix line prepended to chunk text: `// impl TypeName`.

### 5.2 TypeScript

**Granularity rule:** `class_declaration` bodies are NOT emitted as whole chunks. Each `method_definition` inside a class body becomes its own chunk. `interface_declaration` and `type_alias_declaration` have no methods — emitted as whole chunks. Top-level `function_declaration` and exported arrow functions are whole chunks.

```scheme
; Top-level functions
(program (function_declaration name: (identifier) @name) @symbol)
; Interfaces and type aliases — whole chunk
(interface_declaration name: (type_identifier) @name) @symbol
(type_alias_declaration name: (type_identifier) @name) @symbol
; Exported arrow functions
(export_statement declaration: (lexical_declaration
  (variable_declarator name: (identifier) @name))) @symbol
; Methods inside class bodies — each method is its own chunk
(class_declaration name: (type_identifier) @name
  body: (class_body (method_definition name: (property_identifier) @name) @symbol))
```

`symbol_name` for methods: `"ClassName.methodName"`. Parent prefix: `// class ClassName`.

### 5.3 JavaScript

**Granularity rule:** Same as TypeScript minus `interface_declaration` and `type_alias_declaration`. Exported arrow functions apply.

```scheme
(program (function_declaration name: (identifier) @name) @symbol)
(export_statement declaration: (lexical_declaration
  (variable_declarator name: (identifier) @name))) @symbol
(class_declaration name: (identifier) @name
  body: (class_body (method_definition name: (property_identifier) @name) @symbol))
```

`symbol_name` for methods: `"ClassName.methodName"`. Parent prefix: `// class ClassName`.

### 5.4 Python

**Granularity rule:** `class_definition` bodies are NOT emitted as whole chunks. Each `function_definition` inside a class body becomes its own chunk. Top-level `function_definition` and `class_definition` are otherwise treated as whole chunks only if the class contains no methods (i.e. empty body or only attribute assignments). Do not recurse into nested inner functions — they are not symbols.

```scheme
; Top-level functions
(module (function_definition name: (identifier) @name) @symbol)
; Methods inside classes — each method is its own chunk
(class_definition name: (identifier) @name
  body: (block (function_definition name: (identifier) @name) @symbol))
; Classes with no method children (data-only or empty) — whole chunk
; (captured when the class_definition has no function_definition children)
```

The "class with no methods" case is handled in Rust code: if a `class_definition` node has no `function_definition` children in its body, emit it as a whole chunk with `symbol_name = "ClassName"`.

`symbol_name` for methods: `"ClassName.method_name"`. Parent prefix: `# class ClassName`.

### 5.5 Go

**Granularity rule:** Go has no class bodies — `function_declaration` and `method_declaration` are always top-level. Each is a whole chunk. `type_declaration` (struct, interface, type alias) is a whole chunk.

```scheme
(function_declaration name: (identifier) @name) @symbol
(method_declaration name: (field_identifier) @name) @symbol
(type_declaration (type_spec name: (type_identifier) @name)) @symbol
```

Receiver type for methods extracted from `receiver` child → `symbol_name` = `"(*ReceiverType).MethodName"` (pointer receiver) or `"ReceiverType.MethodName"`. Parent prefix: `// type ReceiverType`.

---

## 6. Oversized Symbol Splitting (>400 words)

Applied as a post-processing pass on each chunk that exceeds the budget:

1. Collect inner `function_item` / `function_definition` / `function_declaration` node ranges within the symbol (re-using the parse tree already in memory).
2. If inner functions exist, split at their boundaries, preserving the outer signature as a header stub on the first sub-chunk.
3. If no inner functions, split at blank-line paragraph boundaries within the symbol text.
4. Never split mid-statement: each sub-chunk must end at a complete statement or closing brace.
5. Each sub-chunk inherits `symbol_name` from the parent; `start_line` / `end_line` updated to reflect the sub-range.

---

## 7. Minimum Chunk Merge (<20 words)

After splitting, run a single-pass merge: if a chunk is <20 words and has an adjacent sibling from the same file, merge it into the following sibling (or the preceding one if it is the last chunk). This mirrors the behavior documented in V2 §5.

---

## 8. Testing

Fixture files live in `tests/fixtures/ast/`: one small source file per language containing at least 2 top-level symbols and 1 method inside a class/impl/type.

| Test | Assertion |
|------|----------|
| Classifier: `.rs` → `AstSymbol(Rust)`, `.ts` → `AstSymbol(TypeScript)`, `.java` still → `CodeLike` | `classify(path)` return value |
| Per-language symbol split | Chunk count matches expected symbol count; `symbol_name` correct; `start_line`/`end_line` non-zero and ordered |
| Method parent prefix | Rust impl method chunk text starts with `// impl TypeName`; TS/JS class method with `// class ClassName`; Python with `# class ClassName` |
| Strategy tag | `chunk.strategy` == correct `ChunkStrategyTag::AstSymbol*`; `.as_str()` == `"ast_symbol_rust"` etc. |
| Parse-fail fallback | Passing syntactically broken source returns Scanner chunks (non-empty, no panic) |
| Oversized split | Source with a single function >400 words produces 2+ chunks, no chunk ends mid-statement |
| Min-size merge | Tiny 1-liner function merges into adjacent chunk |
| Benchmark parity | `cargo test --features slow-tests` passes without touching SciFact/YAML/code fixtures |

---

## 9. Milestones

| # | Scope |
|---|-------|
| **M5** | Cargo deps + `AstLang` / `AstSymbol` variants in `classify.rs` + `ChunkStrategyTag` additions + `ast_symbol.rs` module with Rust, Python, Go queries + tests |
| **M6** | TypeScript + JavaScript queries + TSX extension + tests |

M5 and M6 can ship as separate PRs or together. Parse-fail Scanner fallback must be present in M5.

---

## 10. Risks & Mitigations

| Risk | Mitigation |
|------|-----------|
| C compilation of grammar crates slows CI | Expected; document build time in PR. Grammar crates are static deps, not rebuilt often. |
| `.scm` query correctness per language | Reference `nvim-treesitter` and `helix` query files as prior art; validate against fixture files in tests |
| `tree-sitter-typescript` covers TSX | `tree-sitter-typescript` exposes both a TypeScript and TSX grammar — use the TSX variant for `.tsx` |
| Oversized `impl` blocks (e.g. large generated code) | Inner-function split handles most cases; document Scanner fallback for pathological files |
| Symbol name absent (e.g. anonymous `impl` for foreign type) | `symbol_name = None` is valid; emit chunk without qualification |
