# Plan: Hybrid autodetect chunking (no settings in v1)

**Date:** 2026-05-07  
**Scope:** Infer chunking strategy **per file at ingest** from path (and optional lightweight content hints). Implement **three internal strategies** — prose, code-like, declarative — with a safe universal fallback.  
**Out of scope for this phase:** Settings UI, persisted user overrides, folder-rule integration for chunk presets, re-index-all workflow.

---

## 1. Goals

- Mixed folders (README, YAML, TS in one tree) work **without** users changing repo layout or configuring chunking.
- **Transparent upgrade:** existing vaults re-ingest with new logic when files change; no mandatory migration step.
- **Quality:** beat a single universal window on notes and on structure-heavy files; treat YAML/JSON/TOML as **declarative**, not TS-style braces.

## 2. Non-goals (explicit)

- No **Settings** surface for chunk strategy (defer to a later milestone).
- No **folder_rules** coupling for chunking in this phase (can layer overrides later).
- No guarantee of perfect classification on extensionless or polyglot files; fallback must be conservative.

---

## 3. Architecture

```
ingest(path, bytes)
      │
      ▼
 classify(path) ──► ChunkStrategy { Prose | CodeLike | Declarative | Fallback }
      │
      ▼
 chunk_for_strategy(strategy, text)
      │
      ▼
 embed(chunks)  →  existing pipeline
```

- **Classifier:** pure function, fast, deterministic. Primary input: **extension** (and tier: user doc vs wiki if needed later).
- **Chunkers:** three implementations + one fallback, each returns `Vec<String>` (same contract as today’s `chunk_text`).
- **Pipeline:** replace unconditional `chunk_text(&text)` with `chunk_autodetect(path, &text)` (name TBD).

---

## 4. Classification rules (v1)

Order: match **first** rule; else **Fallback**.

| Bucket | Extensions (initial set) | Strategy |
|--------|--------------------------|----------|
| Prose | `md`, `markdown`, `txt`, `rst`, `org` | `Prose` |
| Code-like | `ts`, `tsx`, `js`, `jsx`, `mjs`, `cjs`, `rs`, `py`, `go`, `java`, `kt`, `swift`, `c`, `h`, `cpp`, `hpp`, `cs`, `rb`, `php`, `vue`, `svelte` | `CodeLike` |
| Declarative | `yaml`, `yml`, `json`, `jsonc`, `toml`, `xml` | `Declarative` |
| Fallback | everything else (incl. unknown, no ext) | `Fallback` |

**Optional v1.1:** if extension is `.txt` and first N bytes look like `{`/`[` heavy JSON or `---` YAML front matter, reclassify to `Declarative` — only if cheap and tested.

---

## 5. Strategy behavior (high level)

### 5.1 Prose

- Reuse or adapt **current sentence-aware + neighbor padding** path already in `chunker` (prose-tuned).
- Do **not** use “uppercase after period” logic for code files; here it is appropriate for notes/docs.

### 5.2 CodeLike

- **v1:** indentation / brace-depth **scanner** with string/template/comment awareness (no Tree-sitter dependency required to ship).
- Prefer cuts at **statement boundaries** and **top-level declarations**; never split inside a string literal when avoidable.
- Target size: token/word budget aligned with embedder limits; include **small overlap** repeating **function/component signature** lines for context (analogous to prose neighbor padding).
- **v2 optional:** swap core to Tree-sitter for TS/TSX when build/deps are acceptable.

### 5.3 Declarative

- **YAML:** split on `---` document boundaries; then on **top-level keys** (column-0 or consistent indent root); keep list items grouped when short.
- **JSON / JSONC:** split on top-level array elements or object keys (streaming character depth); respect strings.
- **TOML:** split on top-level `[[table]]` / `[table]` boundaries and large key blocks.
- Size cap + overlap between adjacent logical blocks if a block is huge.

### 5.4 Fallback

- **Conservative:** blank-line / paragraph-like splits + max segment size + small overlap; no sentence “uppercase” heuristic (avoids bad cuts on arbitrary text).
- Suitable for extensionless configs, LICENSE files, etc.

---

## 6. Pipeline integration

1. Extend `ingest_file` eligible extensions if needed (`ts`, `tsx`, `yaml`, …) alongside existing types.
2. After `extract_text` / UTF-8 string is available, call **`chunk_autodetect(path::Path, text: &str) -> Vec<String>`** instead of **`chunk_text(text)`**.
3. Keep chunk position ordering and embedding insert unchanged.
4. **Logging (dev/debug):** one `eprintln!`/trace line per ingest with resolved `(path, strategy)` — helps verify autodetect without settings UI.

---

## 7. Testing

| Layer | Cases |
|-------|-------|
| **Classifier** | sample paths for each ext + fallback |
| **Prose** | existing / adapted chunker unit tests |
| **CodeLike** | multiline functions, JSX snippet, nested braces, strings with `};` inside |
| **Declarative** | multi-doc YAML, nested JSON, TOML tables |
| **Fallback** | no extension, random mixed text |
| **Integration** | optional: one ingest test per strategy with small fixtures |

---

## 8. Milestones

1. **M1:** `classify(Path) -> ChunkStrategy` + wire `Fallback` only (swap pipeline call); parity check on `.md`/`.txt`.
2. **M2:** Prose branch = current sentence pipeline; CodeLike scanner v1; Declarative YAML + JSON.
3. **M3:** TOML + XML declarative; expand extension list; tune sizes/overlap.
4. **Later:** Settings + folder overrides + optional “force strategy” persisted in DB.

---

## 9. Risks / mitigations

| Risk | Mitigation |
|------|------------|
| Misclassified file | Fallback is safe; classifier table is conservative; logging |
| Large deps (Tree-sitter) | Deferred; scanner v1 for CodeLike |
| Token overflow | Shared max-chunk budget across strategies |

---

## 10. Doc / product note

When settings exist later, autodetect remains the **default**; folder or file overrides become **optional advanced** behavior — consistent with mixed-folder repos and zero-config v2 upgrades.
