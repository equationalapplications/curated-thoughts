# AstSymbol Chunking (Tree-sitter) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement symbol-aware chunking for Rust, TypeScript, JavaScript, Python, and Go using tree-sitter, replacing the Scanner (`code_like`) chunker for those extensions.

**Architecture:** Add `AstSymbol(AstLang)` to `ChunkStrategy`, wire it through `chunk_autodetect`, and implement `chunker/ast_symbol.rs` using tree-sitter `Query` strings (inline in Rust). On parse failure or zero captures, `chunk_autodetect` falls back to Scanner. Method chunks carry a parent-scope prefix line; bare `impl` / `class` containers are generally not emitted as standalone chunks when methods are chunked separately (see implementation).

**Tech Stack:** Pinned crates (see Task 1): `tree-sitter` 0.26.x and grammar crates (`tree-sitter-rust` 0.24.x, `tree-sitter-typescript` 0.23.x, `tree-sitter-javascript` / `tree-sitter-python` / `tree-sitter-go` 0.25.x), all with `default-features = false` where set; existing `chunker/*` infrastructure. **API note:** `tree-sitter` 0.26 uses `QueryMatches` with the `StreamingIterator` trait (`use tree_sitter::StreamingIterator`) — not a plain `for m in cursor.matches(...)`.

**Implementation deltas vs an earlier draft of this plan:** `classify::path_uses_tsx`, `ast_symbol::chunk(lang, text, use_tsx)`, `code_like::statement_boundary_offsets` exported for AST post-processing, Go method names from the **receiver `type` AST node** (spec `(*T).M` / `T.M`), **min-merge only when `symbol_name` matches** adjacent chunks, and oversized split uses **inner spans → statement boundaries → line-based word fallback** (not paragraph `\n\n` only).

---

## File Map

| Action | Path | Responsibility |
|--------|------|---------------|
| Modify | `src-tauri/Cargo.toml` | Add tree-sitter crate deps (pinned versions) |
| Modify | `src-tauri/src/chunker/classify.rs` | Add `AstLang`, `path_uses_tsx`, `ChunkStrategy`, extension table |
| Modify | `src-tauri/src/chunker/code_like.rs` | Expose `pub(super) fn statement_boundary_offsets` for `ast_symbol` |
| Modify | `src-tauri/src/chunker/mod.rs` | `ChunkStrategyTag` variants, `mod ast_symbol`, `path_uses_tsx` in `pub use`, dispatcher passes `use_tsx` |
| Create | `src-tauri/src/chunker/ast_symbol.rs` | Parse, queries, symbol collection, oversized + merge passes |
| Create | `src-tauri/tests/fixtures/ast/sample.rs` | Rust fixture (incl. `impl` preamble: const before first method) |
| Create | `src-tauri/tests/fixtures/ast/sample.py` | Python fixture |
| Create | `src-tauri/tests/fixtures/ast/sample.go` | Go fixture (may include unused import to satisfy `go fmt` / compiler) |
| Create | `src-tauri/tests/fixtures/ast/sample.ts` | TS fixture (`export function`, `export const` arrow, class methods) |
| Create | `src-tauri/tests/fixtures/ast/sample.js` | JS fixture (top-level `function`, `export const` arrow, class) |
| Create | `src-tauri/tests/ast_symbol.rs` | Single integration test module (all languages + post-passes) |

---

## Task 1: Add tree-sitter crate dependencies

**Files:**
- Modify: `src-tauri/Cargo.toml`

- [ ] **Step 1: Add dependencies**

In `src-tauri/Cargo.toml`, append to `[dependencies]`:

```toml
tree-sitter = { version = "0.26.8", default-features = false }
tree-sitter-rust = { version = "0.24.2", default-features = false }
tree-sitter-typescript = { version = "0.23.2", default-features = false }
tree-sitter-javascript = { version = "0.25.0", default-features = false }
tree-sitter-python = { version = "0.25.0", default-features = false }
tree-sitter-go = { version = "0.25.0", default-features = false }
```

> **Note:** Exact versions may drift; keep grammar crates on a `tree-sitter` ABI line they support. Pin what `cargo` resolves after `cargo update -p tree-sitter`.

- [ ] **Step 2: Verify the crate compiles**

```bash
cargo build -p curated-thoughts 2>&1 | head -40
```

Expected: compile succeeds (grammar crates pull in C source; the first build will be slow).

If a grammar crate version is unavailable, try `"*"` for that crate and pin after resolution:
```bash
cargo update && cargo build -p curated-thoughts 2>&1 | head -40
```

- [ ] **Step 3: Commit**

```bash
git add src-tauri/Cargo.toml src-tauri/Cargo.lock
git commit -m "chore(deps): add tree-sitter grammar crate dependencies"
```

---

## Task 2: Add `AstLang` enum and update `classify.rs`

**Files:**
- Modify: `src-tauri/src/chunker/classify.rs`

**Context:** `ChunkStrategy` gains `AstSymbol(AstLang)`. Any older test named `code_extensions()` that asserted `.rs` / `.ts` returned `CodeLike` must be updated (this repo uses `ast_symbol_extensions` + `scanner_extensions`). **`path_uses_tsx`** is separate from classification: `.tsx` stays `AstSymbol(TypeScript)`, and the dispatcher passes `path_uses_tsx(path)` into `ast_symbol::chunk` so the TSX grammar is selected only when needed.

- [ ] **Step 1: Write failing classifier tests**

In `src-tauri/src/chunker/classify.rs`, ensure coverage with **`ast_symbol_extensions`** and **`scanner_extensions`** (snippet below). If a legacy `code_extensions` test still exists, replace it with:

```rust
#[test]
fn ast_symbol_extensions() {
    assert_eq!(classify(&p("main.rs")), ChunkStrategy::AstSymbol(AstLang::Rust));
    assert_eq!(classify(&p("app.ts")), ChunkStrategy::AstSymbol(AstLang::TypeScript));
    assert_eq!(classify(&p("ui.tsx")), ChunkStrategy::AstSymbol(AstLang::TypeScript));
    assert_eq!(classify(&p("index.js")), ChunkStrategy::AstSymbol(AstLang::JavaScript));
    assert_eq!(classify(&p("comp.jsx")), ChunkStrategy::AstSymbol(AstLang::JavaScript));
    assert_eq!(classify(&p("util.mjs")), ChunkStrategy::AstSymbol(AstLang::JavaScript));
    assert_eq!(classify(&p("mod.cjs")), ChunkStrategy::AstSymbol(AstLang::JavaScript));
    assert_eq!(classify(&p("script.py")), ChunkStrategy::AstSymbol(AstLang::Python));
    assert_eq!(classify(&p("service.go")), ChunkStrategy::AstSymbol(AstLang::Go));
}

#[test]
fn scanner_extensions() {
    // Extensions that still use the Scanner (CodeLike)
    assert_eq!(classify(&p("Main.java")), ChunkStrategy::CodeLike);
    assert_eq!(classify(&p("App.kt")), ChunkStrategy::CodeLike);
    assert_eq!(classify(&p("View.swift")), ChunkStrategy::CodeLike);
    assert_eq!(classify(&p("main.c")), ChunkStrategy::CodeLike);
    assert_eq!(classify(&p("main.cpp")), ChunkStrategy::CodeLike);
    assert_eq!(classify(&p("page.vue")), ChunkStrategy::CodeLike);
    assert_eq!(classify(&p("comp.svelte")), ChunkStrategy::CodeLike);
}
```

- [ ] **Step 2: Run tests to confirm they fail**

```bash
cargo test -p curated-thoughts classify 2>&1 | tail -20
```

Expected: compile error — `AstLang` and `AstSymbol` don't exist yet.

- [ ] **Step 3: Add `AstLang` and update `ChunkStrategy`**

Replace the entire `classify.rs` content:

```rust
//! Extension-based chunk strategy classification (deterministic, no I/O).

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AstLang {
    Rust,
    TypeScript,
    JavaScript,
    Python,
    Go,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkStrategy {
    AstSymbol(AstLang),
    Prose,
    CodeLike,
    Declarative,
    Fallback,
}

/// Extensions the pipeline indexes (binary formats use extractors; others UTF-8 / lossy).
pub fn should_ingest_extension(raw_ext: &str) -> bool {
    let ext = raw_ext.to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "md" | "markdown" | "txt" | "rst" | "org" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs"
            | "rs" | "py" | "go" | "java" | "kt" | "swift" | "c" | "h" | "cpp" | "hpp" | "cs"
            | "rb" | "php" | "vue" | "svelte" | "yaml" | "yml" | "json" | "jsonc" | "toml"
            | "xml" | "pdf" | "docx"
    )
}

pub fn classify(path: &Path) -> ChunkStrategy {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return ChunkStrategy::Fallback;
    };
    let ext = ext.to_ascii_lowercase();
    match ext.as_str() {
        "md" | "markdown" | "txt" | "rst" | "org" => ChunkStrategy::Prose,
        "rs" => ChunkStrategy::AstSymbol(AstLang::Rust),
        "ts" | "tsx" => ChunkStrategy::AstSymbol(AstLang::TypeScript),
        "js" | "jsx" | "mjs" | "cjs" => ChunkStrategy::AstSymbol(AstLang::JavaScript),
        "py" => ChunkStrategy::AstSymbol(AstLang::Python),
        "go" => ChunkStrategy::AstSymbol(AstLang::Go),
        "java" | "kt" | "swift" | "c" | "h" | "cpp" | "hpp" | "cs" | "rb" | "php" | "vue"
        | "svelte" => ChunkStrategy::CodeLike,
        "yaml" | "yml" | "json" | "jsonc" | "toml" | "xml" => ChunkStrategy::Declarative,
        _ => ChunkStrategy::Fallback,
    }
}

/// True when we must parse with the TSX grammar (`.tsx` sources).
#[inline]
pub fn path_uses_tsx(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("tsx"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn p(name: &str) -> PathBuf {
        PathBuf::from("/vault/documents").join(name)
    }

    #[test]
    fn prose_extensions() {
        assert_eq!(classify(&p("note.md")), ChunkStrategy::Prose);
        assert_eq!(classify(&p("README.markdown")), ChunkStrategy::Prose);
        assert_eq!(classify(&p("LICENSE.txt")), ChunkStrategy::Prose);
        assert_eq!(classify(&p("guide.rst")), ChunkStrategy::Prose);
        assert_eq!(classify(&p("tasks.org")), ChunkStrategy::Prose);
    }

    #[test]
    fn ast_symbol_extensions() {
        assert_eq!(classify(&p("main.rs")), ChunkStrategy::AstSymbol(AstLang::Rust));
        assert_eq!(classify(&p("app.ts")), ChunkStrategy::AstSymbol(AstLang::TypeScript));
        assert_eq!(classify(&p("ui.tsx")), ChunkStrategy::AstSymbol(AstLang::TypeScript));
        assert_eq!(classify(&p("index.js")), ChunkStrategy::AstSymbol(AstLang::JavaScript));
        assert_eq!(classify(&p("comp.jsx")), ChunkStrategy::AstSymbol(AstLang::JavaScript));
        assert_eq!(classify(&p("util.mjs")), ChunkStrategy::AstSymbol(AstLang::JavaScript));
        assert_eq!(classify(&p("mod.cjs")), ChunkStrategy::AstSymbol(AstLang::JavaScript));
        assert_eq!(classify(&p("script.py")), ChunkStrategy::AstSymbol(AstLang::Python));
        assert_eq!(classify(&p("service.go")), ChunkStrategy::AstSymbol(AstLang::Go));
    }

    #[test]
    fn scanner_extensions() {
        assert_eq!(classify(&p("Main.java")), ChunkStrategy::CodeLike);
        assert_eq!(classify(&p("App.kt")), ChunkStrategy::CodeLike);
        assert_eq!(classify(&p("View.swift")), ChunkStrategy::CodeLike);
        assert_eq!(classify(&p("main.c")), ChunkStrategy::CodeLike);
        assert_eq!(classify(&p("main.cpp")), ChunkStrategy::CodeLike);
        assert_eq!(classify(&p("page.vue")), ChunkStrategy::CodeLike);
        assert_eq!(classify(&p("comp.svelte")), ChunkStrategy::CodeLike);
    }

    #[test]
    fn declarative_extensions() {
        assert_eq!(classify(&p("cfg.yaml")), ChunkStrategy::Declarative);
        assert_eq!(classify(&p("cfg.yml")), ChunkStrategy::Declarative);
        assert_eq!(classify(&p("data.json")), ChunkStrategy::Declarative);
        assert_eq!(classify(&p("tsconf.jsonc")), ChunkStrategy::Declarative);
        assert_eq!(classify(&p("Cargo.toml")), ChunkStrategy::Declarative);
        assert_eq!(classify(&p("layout.xml")), ChunkStrategy::Declarative);
    }

    #[test]
    fn fallback_unknown_or_missing_ext() {
        assert_eq!(classify(&p("Makefile")), ChunkStrategy::Fallback);
        assert_eq!(classify(&p("Dockerfile")), ChunkStrategy::Fallback);
        assert_eq!(classify(&p("bin/tool")), ChunkStrategy::Fallback);
        assert_eq!(classify(Path::new("/no/extension")), ChunkStrategy::Fallback);
        assert_eq!(classify(&p("doc.pdf")), ChunkStrategy::Fallback);
        assert_eq!(classify(&p("paper.docx")), ChunkStrategy::Fallback);
    }
}
```

- [ ] **Step 4: Run classifier tests**

```bash
cargo test -p curated-thoughts classify 2>&1 | tail -20
```

Expected: all `classify::tests` pass. If `chunk_autodetect` in `mod.rs` has a non-exhaustive match, it will be a compile error — that is fixed in Task 3.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/chunker/classify.rs
git commit -m "feat(chunker): add AstLang enum and AstSymbol(AstLang) to ChunkStrategy"
```

---

## Task 3: Update `mod.rs` — ChunkStrategyTag, dispatcher, mod declaration

**Files:**
- Modify: `src-tauri/src/chunker/mod.rs`

- [ ] **Step 1: Write a failing tag test**

Add this test at the bottom of `src-tauri/src/chunker/mod.rs` inside the existing `#[cfg(test)]` block:

```rust
#[test]
fn ast_symbol_tags_serialize() {
    assert_eq!(ChunkStrategyTag::AstSymbolRust.as_db_str(), "ast_symbol_rust");
    assert_eq!(ChunkStrategyTag::AstSymbolTypeScript.as_db_str(), "ast_symbol_typescript");
    assert_eq!(ChunkStrategyTag::AstSymbolJavaScript.as_db_str(), "ast_symbol_javascript");
    assert_eq!(ChunkStrategyTag::AstSymbolPython.as_db_str(), "ast_symbol_python");
    assert_eq!(ChunkStrategyTag::AstSymbolGo.as_db_str(), "ast_symbol_go");
}
```

- [ ] **Step 2: Run to confirm it fails**

```bash
cargo test -p curated-thoughts "ast_symbol_tags" 2>&1 | tail -10
```

Expected: compile error — `AstSymbolRust` etc. don't exist yet.

- [ ] **Step 3: Apply all changes to `mod.rs`**

Merge into `src-tauri/src/chunker/mod.rs` (do not blindly overwrite the whole file if it has diverged): add `mod ast_symbol;`, extend `ChunkStrategyTag` / `as_db_str`, import `path_uses_tsx`, and wire the AST branch as below.

```rust
mod ast_symbol;
mod classify;
mod code_like;
mod declarative;
mod fallback;
mod limits;
mod prose;

pub use classify::{classify, path_uses_tsx, should_ingest_extension, AstLang, ChunkStrategy};

use std::path::Path;

/// 1-indexed inclusive lines for `source[start_byte..end_byte]` (byte indices must be on char boundaries).
pub fn lines_for_byte_span(source: &str, start_byte: usize, end_byte: usize) -> (u32, u32) {
    let len = source.len();
    let start_byte = start_byte.min(len);
    let mut end_byte = end_byte.min(len);
    if end_byte < start_byte {
        end_byte = start_byte;
    }
    let start_line = 1 + source[..start_byte].bytes().filter(|&b| b == b'\n').count() as u32;
    let end_line = 1 + source[..end_byte].bytes().filter(|&b| b == b'\n').count() as u32;
    (start_line, end_line.max(start_line))
}

/// Split a block into capped pieces with spans as byte offsets in the vault file `source` string.
pub(super) fn split_oversized_block_spans(
    block: &str,
    block_base_abs: usize,
    max_c: usize,
    overlap: usize,
) -> Vec<(String, usize, usize)> {
    let block = block.trim();
    if block.is_empty() {
        return vec![];
    }
    if block.len() <= max_c {
        let trimmed = block.trim();
        let off = block.find(trimmed).expect("trim");
        let lo = block_base_abs + off;
        let hi = lo + trimmed.len();
        return vec![(trimmed.to_string(), lo, hi)];
    }

    let mut out = Vec::new();
    let mut start = 0usize;
    while start < block.len() {
        let mut end = (start + max_c).min(block.len());
        if end < block.len() {
            let slice = &block[start..end];
            if let Some(rel) = slice.rfind('\n') {
                end = start + rel + 1;
            } else if let Some(rel) = slice.rfind(' ') {
                end = start + rel + 1;
            }
        }
        let raw = &block[start..end];
        let piece = raw.trim();
        if !piece.is_empty() {
            let off = raw.find(piece).expect("trim") + start;
            let lo = block_base_abs + off;
            let hi = lo + piece.len();
            out.push((piece.to_string(), lo, hi));
        }
        if end >= block.len() {
            break;
        }
        start = end.saturating_sub(overlap);
        while start < block.len() && !block.is_char_boundary(start) {
            start += 1;
        }
        if start >= end {
            start = end;
        }
    }
    out
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ChunkStrategyTag {
    AstSymbolRust,
    AstSymbolTypeScript,
    AstSymbolJavaScript,
    AstSymbolPython,
    AstSymbolGo,
    Prose,
    Scanner,
    Declarative,
    Fallback,
}

impl ChunkStrategyTag {
    pub fn as_db_str(&self) -> &'static str {
        match self {
            ChunkStrategyTag::AstSymbolRust => "ast_symbol_rust",
            ChunkStrategyTag::AstSymbolTypeScript => "ast_symbol_typescript",
            ChunkStrategyTag::AstSymbolJavaScript => "ast_symbol_javascript",
            ChunkStrategyTag::AstSymbolPython => "ast_symbol_python",
            ChunkStrategyTag::AstSymbolGo => "ast_symbol_go",
            ChunkStrategyTag::Prose => "prose",
            ChunkStrategyTag::Scanner => "scanner",
            ChunkStrategyTag::Declarative => "declarative",
            ChunkStrategyTag::Fallback => "fallback",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Chunk {
    pub text: String,
    pub start_line: u32,
    pub end_line: u32,
    pub symbol_name: Option<String>,
    pub strategy: ChunkStrategyTag,
}

/// Legacy prose-only API (sentence-aware); retained for benchmarks and tests.
pub fn chunk_text(text: &str) -> Vec<String> {
    chunk_prose_chunks(text)
        .into_iter()
        .map(|c| c.text)
        .collect()
}

pub fn chunk_prose_chunks(text: &str) -> Vec<Chunk> {
    prose::chunk_prose_chunks(text)
}

/// Choose chunking strategy from path extension and dispatch.
pub fn chunk_autodetect(path: &Path, text: &str) -> Vec<Chunk> {
    let strategy = classify(path);
    if cfg!(debug_assertions) {
        eprintln!(
            "[ingest-chunk] {} strategy={:?}",
            path.display(),
            strategy
        );
    }

    match strategy {
        ChunkStrategy::AstSymbol(lang) => {
            let use_tsx = path_uses_tsx(path);
            let chunks = ast_symbol::chunk(lang, text, use_tsx);
            if chunks.is_empty() {
                // parse failure or zero captures: fall back to Scanner (code_like)
                code_like::chunk_code_like_chunks(text)
            } else {
                chunks
            }
        }
        ChunkStrategy::Prose => prose::chunk_prose_chunks(text),
        ChunkStrategy::CodeLike => code_like::chunk_code_like_chunks(text),
        ChunkStrategy::Declarative => declarative::chunk_declarative_chunks(path, text),
        ChunkStrategy::Fallback => fallback::chunk_fallback_chunks(text),
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn md_matches_legacy_chunk_text() {
        let p = PathBuf::from("/v/note.md");
        let text = "Aa bb cc. Dd ee ff.";
        let a: Vec<String> = chunk_autodetect(&p, text).into_iter().map(|c| c.text).collect();
        assert_eq!(a, chunk_text(text));
    }

    #[test]
    fn txt_matches_legacy_chunk_text() {
        let p = PathBuf::from("/v/readme.txt");
        let text = "One two. Three four.";
        let a: Vec<String> = chunk_autodetect(&p, text).into_iter().map(|c| c.text).collect();
        assert_eq!(a, chunk_text(text));
    }

    #[test]
    fn ast_symbol_tags_serialize() {
        assert_eq!(ChunkStrategyTag::AstSymbolRust.as_db_str(), "ast_symbol_rust");
        assert_eq!(ChunkStrategyTag::AstSymbolTypeScript.as_db_str(), "ast_symbol_typescript");
        assert_eq!(ChunkStrategyTag::AstSymbolJavaScript.as_db_str(), "ast_symbol_javascript");
        assert_eq!(ChunkStrategyTag::AstSymbolPython.as_db_str(), "ast_symbol_python");
        assert_eq!(ChunkStrategyTag::AstSymbolGo.as_db_str(), "ast_symbol_go");
    }
}
```

- [ ] **Step 4: Create the `ast_symbol.rs` stub** (so `mod ast_symbol` compiles)

Create `src-tauri/src/chunker/ast_symbol.rs`:

```rust
use super::classify::AstLang;
use super::Chunk;

/// Returns an empty vec on parse failure or zero captures.
/// Caller (`chunk_autodetect`) falls back to Scanner (`code_like`) when empty.
/// `use_tsx` selects `LANGUAGE_TSX` vs plain TypeScript grammar for `.tsx` paths.
pub(super) fn chunk(_lang: AstLang, _text: &str, _use_tsx: bool) -> Vec<Chunk> {
    vec![]
}
```

- [ ] **Step 5: Run all chunker tests**

```bash
cargo test -p curated-thoughts chunker 2>&1 | tail -30
```

Expected: all pass. The `ast_symbol` stub returns empty, so `.rs`/`.ts`/etc. files fall through to the Scanner — no regressions.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/chunker/mod.rs src-tauri/src/chunker/ast_symbol.rs
git commit -m "feat(chunker): add ChunkStrategyTag::AstSymbol* variants and dispatcher arm"
```

---

## Task 4: Implement `ast_symbol.rs` (all languages), `code_like` helper, fixtures, tests

**Canonical source of truth:** `src-tauri/src/chunker/ast_symbol.rs` — this plan does **not** embed the full implementation (the previous draft’s inline module was hundreds of lines and drifted). Open that file for queries, per-language filters, and post-passes.

**Supporting change:** `src-tauri/src/chunker/code_like.rs` exports `pub(super) fn statement_boundary_offsets` for splitting oversized symbol bodies along statement-like boundaries.

### As implemented (behavioral checklist)

- **Query iteration (tree-sitter 0.26):** use `QueryCursor` with `use tree_sitter::StreamingIterator` — `matches(...)` is **not** a Rust `Iterator`.
- **Dispatcher:** `chunk_autodetect` calls `ast_symbol::chunk(lang, text, path_uses_tsx(path))`; empty result → `code_like::chunk_code_like_chunks` (Scanner).
- **Rust:** captures top-level items via a flat `function_item` query and **`rust_function_item_keep`** so nested fns and impl-local fns are excluded correctly; **`impl` preamble** (leading comments + `const` in the impl, etc.) is prepended **only to the first method** chunk; qualified method names `Type::method`.
- **Python:** separate query from other languages; **`@dataclass`** / data-only classes can be a single named chunk; methods `Class.method`.
- **Go:** method display names from the **receiver parameter’s type node**: `(*Counter).Increment`, `Counter.Value` (not naive string parsing of `(c *Counter)`); fixture may use `var _ = fmt.Println` so `import "fmt"` survives `go fmt` / compiler.
- **TypeScript / JavaScript:** `export_statement` covers **`export function`** and **`export const` arrow** bindings; top-level lexical arrows are separate chunks; container filtering via **`tsjs_function_decl_top_level`**; class methods `Rectangle.area`; **TSX** selects `LANGUAGE_TSX` when `use_tsx` is true.
- **Oversized:** shrink inner span to word budget → `statement_boundary_offsets` → greedy newline split → **`split_fallback_newline_words_chunk`** for dense `let` bodies.
- **`merge_undersized`:** merges neighbors **only if `symbol_name` is equal** on both sides (prevents unrelated tiny symbols from being absorbed).

### Fixtures and single test module

| File | Role |
|------|------|
| `src-tauri/tests/fixtures/ast/sample.rs` | `impl Counter { const MAX: u32 = …` before first method (preamble test) |
| `src-tauri/tests/fixtures/ast/sample.py` | `standalone`, `Calculator` methods, data-only `Config` |
| `src-tauri/tests/fixtures/ast/sample.go` | `StandaloneFunc`, `Counter`, `(*Counter).Increment`, `Counter.Value`, `Adder`, `fmt` discard |
| `src-tauri/tests/fixtures/ast/sample.ts` | `export function topFn`, `export const arrowFn = …`, interface, type alias, class + methods |
| `src-tauri/tests/fixtures/ast/sample.js` | top-level `topFn`, `export const arrowFn`, `Vehicle` methods |
| `src-tauri/tests/ast_symbol.rs` | All language + TSX + fallback + post-pass tests in one file |

```bash
cargo test -p curated-thoughts --test ast_symbol 2>&1 | tail -30
```

Expected: **10** tests, all pass.

- [ ] **Step: Commit (when landing the ast chunker)**

Prefer explicit paths instead of `git add -A`:

```bash
git add src-tauri/src/chunker/ast_symbol.rs src-tauri/src/chunker/code_like.rs \
    src-tauri/tests/ast_symbol.rs src-tauri/tests/fixtures/ast/
git commit -m "feat(chunker/ast): tree-sitter AstSymbol chunking"
```

---

## Tasks 5–8: Language fixtures and tests (single module)

Earlier milestones described **incremental commits** (“append Python tests”). As implemented, **`src-tauri/src/chunker/ast_symbol.rs`** handles all languages and **`src-tauri/tests/ast_symbol.rs`** holds **all** assertions in one place.

| Area | Fixture | Tests (names as in repo) |
|------|---------|--------------------------|
| Python | `sample.py` | `python_standalone_methods_and_dataclass` — `standalone`, `Calculator.add` / `.sub`, `Config`, strategy `AstSymbolPython` |
| Go | `sample.go` | `go_method_names_use_receiver_form` — `(*Counter).Increment`, `Counter.Value` (+ other symbols from the fixture); strategy `AstSymbolGo` |
| TypeScript | `sample.ts` | `ts_export_fn_arrow_interface_type_class_methods` — `topFn`, **`arrowFn`**, `Shape`, `Color`, `Rectangle.area`; **`tsx_uses_tsx_grammar_strategy`** uses path `App.tsx` |
| JavaScript | `sample.js` | `js_top_export_arrow_and_class_methods` — `topFn`, **`arrowFn`**, `Vehicle.constructor`, `Vehicle.describe` |

Filter examples (optional):

```bash
cargo test -p curated-thoughts --test ast_symbol python_ -- --nocapture 2>&1 | tail -20
cargo test -p curated-thoughts --test ast_symbol go_ -- --nocapture 2>&1 | tail -20
cargo test -p curated-thoughts --test ast_symbol ts_ -- --nocapture 2>&1 | tail -20
cargo test -p curated-thoughts --test ast_symbol js_ -- --nocapture 2>&1 | tail -20
```

---

## Task 9: Oversized split and undersized merge

Post-passes are in **`ast_symbol.rs`**. Tests:

- **`oversized_rust_splits_with_shared_symbol_name`** — `fn big_fn()` with **300** `let` lines; `len > 1`; every chunk keeps `symbol_name == Some("big_fn")` and `AstSymbolRust`.
- **`tiny_fn_merges_undersized`** — expects **`chunks.len() == 1`** for a single oversized symbol body (coalesced); merge **requires matching `symbol_name`** so unrelated tiny nodes are not absorbed.

```bash
cargo test -p curated-thoughts --test ast_symbol oversized_rust -- --nocapture 2>&1 | tail -15
cargo test -p curated-thoughts --test ast_symbol tiny_fn_merges -- --nocapture 2>&1 | tail -15
```

---

## Task 10: Broader verification and benchmarks

- **AstSymbol integration tests** do not require extra features:

```bash
cargo test -p curated-thoughts --test ast_symbol 2>&1 | tail -20
```

- **Most other integration tests** need `test-utils` (see `src-tauri/tests/README.md`):

```bash
cargo test -p curated-thoughts --features test-utils 2>&1 | tail -40
```

- **SciFact / YAML / code recall benchmarks** need `slow-tests` as well (embedder + long runs):

```bash
cargo test -p curated-thoughts --features "test-utils,slow-tests" 2>&1 | tail -40
```

When committing, prefer **explicit `git add` paths** (e.g. `src-tauri/Cargo.toml`, `src-tauri/src/chunker/{classify,mod,code_like,ast_symbol}.rs`, `src-tauri/tests/ast_symbol.rs`, `src-tauri/tests/fixtures/ast/`) instead of `git add -A`.

---

## Implementation Notes

### tree-sitter 0.26 match iteration

`QueryCursor::matches` returns a **`StreamingIterator`**, not a Rust `Iterator`. Add `use tree_sitter::StreamingIterator` and use `while let Some(m) = cursor.next() { … }`.

### Grammar crate symbols

Pinned grammar crates compile with constants such as `tree_sitter_rust::LANGUAGE` / `LANGUAGE_TYPESCRIPT` / `LANGUAGE_TSX` and `.into()` for `Language`. If a future upgrade breaks `into()`, consult docs.rs for that crate revision.

### TSX vs TypeScript grammar

`tree-sitter-typescript` exposes **`LANGUAGE_TYPESCRIPT`** and **`LANGUAGE_TSX`**. As implemented: **`.ts`** uses the TypeScript grammar; **`.tsx`** uses TSX grammar via **`path_uses_tsx(path)` → `chunk(.., true)`**. The classifier keeps a single **`AstLang::TypeScript`** for both extensions.

### Query debugging

```bash
# Install: cargo install tree-sitter-cli
tree-sitter parse src-tauri/tests/fixtures/ast/sample.rs
```

### Python data-only classes / methods

If `python_standalone_methods_and_dataclass` fails on `Config` or method naming, inspect class-body children under **`block`** — grammar layout can vary by `tree-sitter-python` revision:

```rust
eprintln!("class children: {:?}", (0..class_node.child_count()).map(|i| class_node.child(i).unwrap().kind()).collect::<Vec<_>>());
```

### Oversized bodies and merges

Greedy newline splitting may leave **`let`-only** giants unsplit until **`split_fallback_newline_words_chunk`**. **`merge_undersized`** only merges chunks that share the **same `symbol_name`**.

