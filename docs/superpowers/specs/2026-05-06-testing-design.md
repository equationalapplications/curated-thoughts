# Testing Design — Curated Thoughts

**Goal:** Catch regressions in the Tauri IPC contract and core business logic. Tests invoke commands exactly as the frontend does, using a real SQLite database and a headless Tauri app. A gated SciFact benchmark asserts semantic search quality using pre-computed AllMiniLML6V2 embeddings.

**Status:** Implemented

---

## Approach

Real IPC via `tauri::test::mock_builder()` for contract-critical commands. Direct Rust function calls for pure logic already covered by existing unit tests (chunker, embedder, hasher, DB queries — 41 tests). No mocking of the Tauri IPC layer.

Rejected alternatives:
- **Logic-layer bypass + specta types** — misses JSON↔Rust serialization bugs entirely
- **WebdriverIO E2E** — requires full `cargo tauri build` per run, brittle, needs display server

---

## File Map

```
src-tauri/
  tests/
    integration/
      helpers.rs          # make_app(tmp) + typed invoke() helper
      pipeline.rs         # ingest flow: index, dedup, re-index, unsupported ext
      review_queue.rs     # pending → approve/reject lifecycle
      folder_rules.rs     # CRUD round-trip, index mode skip, auto_approve write
      deletion.rs         # cascade: DB rows, shadow copy, wiki orphan
      vault.rs            # set_vault_path subdir bootstrap
    scifact/
      mod.rs              # Recall@10 benchmark (feature-gated)
      loader.rs           # corpus.jsonl + queries.json + qrels.json + embeddings parser
  tests/
    scripts/
      embed_scifact.rs    # one-time generation binary — NOT run in CI
  tests/fixtures/scifact/
    corpus.jsonl              # 5,183 abstracts (copied from expo-llm-wiki)
    queries.json              # 300 test queries (copied from expo-llm-wiki)
    qrels.json                # ground-truth relevance judgments (copied from expo-llm-wiki)
    scifact-embeddings.bin.gz # AllMiniLML6V2 pre-computed vectors — generated once, committed
```

---

## Cargo.toml Changes

```toml
[features]
slow-tests = []

[[bin]]
name = "embed_scifact"
path = "tests/scripts/embed_scifact.rs"
```

Run fast tests: `cargo test`
Run SciFact benchmark: `cargo test --features slow-tests`
Generate embeddings (once): `cargo run --bin embed_scifact`

---

## Helper Infrastructure (`tests/integration/helpers.rs`)

`make_app(tmp: &TempDir)` builds a headless Tauri app with:
- Real SQLite at `tmp/brain.db` (all migrations applied)
- Real `VaultConfig` at `tmp/config.json`
- `PipelineTx` channel with receiver dropped — pipeline worker not needed for IPC contract tests
- `WikiEmbedder` initialized to `None` (lazy-loaded on first embed call)
- Full `invoke_handler!` registration of all contract-critical commands

`invoke<T>(app, cmd, payload)` wraps `tauri::test::get_ipc_response` with typed deserialization. Payload is `serde_json::Value` — exactly what a frontend `invoke(cmd, params)` sends. Catches param name mismatches (e.g. `vaultPath` vs `vault_path`) and serialization bugs.

```rust
// Example usage
let queue: Vec<ReviewPage> = invoke(&app, "get_review_queue", json!({})).unwrap();
invoke::<()>(&app, "approve_wiki_page", json!({
    "id": 1, "content": "# Wiki", "vaultPath": tmp.path()
})).unwrap();
```

---

## Integration Test Modules

### `vault.rs`
- `set_vault_path` creates `documents/`, `wiki/`, `.brain/converted/` on disk

### `pipeline.rs`
- Write `.md` to `tmp/documents/` → direct `PipelineJob::Ingest` → assert `status='indexed'`, chunk count > 0, embedding count matches chunk count
- Same file unchanged → assert no re-index (hash match)
- Same file with changed content → assert old chunks gone, new chunks present
- Unsupported extension (`.png`) → assert not indexed, no rows in DB

Note: pipeline worker is driven directly via `PipelineWorker::run` in a test thread rather than through the watcher, to avoid filesystem event timing issues.

### `review_queue.rs`
- Seed `wiki_pages` row with `status='pending_review'`; write proposed content to `tmp/.brain/proposed/<filename>`
- `get_review_queue` → returns the seeded page
- `get_proposed_content` → returns file contents verbatim
- `approve_wiki_page` → file written to `tmp/wiki/<filename>`, status = `approved`, page absent from `get_review_queue`
- `reject_wiki_page` → no file written to disk, status = `rejected`, page absent from `get_review_queue`

### `folder_rules.rs`
- `set_folder_rule` + `get_folder_rules` → round-trips `folder_path`, `librarian_mode`, `auto_approve` correctly
- `delete_folder_rule` → row removed, no longer in `get_folder_rules`
- Seed `mode='index'` rule for document's folder → call `librarian::generate_summary` directly → `wiki_pages` table remains empty
- Seed `auto_approve=true` rule → call `librarian::generate_summary` with pre-seeded chunks → wiki page written directly to `wiki/`, status = `approved`, page absent from `get_review_queue`

### `deletion.rs`
- Seed: document row + chunks + embeddings in DB; shadow `.md` in `tmp/.brain/converted/`; `wiki_pages` row with `source_doc_ids` referencing the document
- Send `PipelineJob::Delete` path through a test `PipelineWorker`
- Assert: document row absent from DB
- Assert: chunks cascade-deleted (count = 0)
- Assert: embeddings cascade-deleted (count = 0)
- Assert: shadow copy absent from `.brain/converted/`
- Assert: `wiki_pages` row status = `orphaned`

---

## SciFact Benchmark (`tests/scifact/mod.rs`)

### Fixtures

Copied from `expo-llm-wiki/packages/integration/fixtures/`:
- `corpus.jsonl` — 5,183 scientific abstracts (`_id`, `title`, `text`)
- `queries.json` — 300 test queries keyed by claim ID
- `qrels.json` — ground-truth relevant doc IDs per claim

Generated once via `embed_scifact` binary, committed:
- `scifact-embeddings.bin.gz` — AllMiniLML6V2 384-dim vectors, one per corpus doc, gzip-compressed little-endian f32 blobs

### Generation Script (`tests/scripts/embed_scifact.rs`)

Run once by developer on a capable machine. Not part of CI.

1. Parse `corpus.jsonl`
2. Embed each abstract's `text` field using `Embedder::new()` in batches
3. Write doc_id → f32 blob map as gzip-compressed binary to `scifact-embeddings.bin.gz`
4. Print progress and final doc count

### Benchmark Test Flow

`#[cfg(feature = "slow-tests")]`

1. `make_app(tmp)` — headless Tauri app with real SQLite
2. Seed corpus: for each of 5,183 abstracts, `INSERT INTO documents` (path = corpus `_id`) + `INSERT INTO chunks` + `INSERT INTO embeddings` (vectors loaded from side-car, no model inference). Path = `_id` so search results can be matched against qrels.
3. For each of 300 queries:
   - Embed query text via `Embedder` (300 short strings, ~2s total)
   - `invoke(&app, "search_vault", json!({ "query": text, "limit": 10 }))`
   - Collect `doc_path` values from results
4. Recall@10 = fraction of queries where ≥ 1 ground-truth doc appears in top 10
5. Assert `recall >= 0.60`
6. Print per-query misses and final score

**Expected runtime:** ~5–10s (no corpus embedding; only 300 query embeds at test time)

### Loader (`tests/scifact/loader.rs`)

```rust
pub struct ScifactFixtures {
    pub corpus: Vec<CorpusDoc>,      // { id, title, text }
    pub queries: HashMap<String, String>,  // claim_id → query text
    pub qrels: HashMap<String, Vec<String>>, // claim_id → [doc_id]
    pub embeddings: HashMap<String, Vec<f32>>, // doc_id → 384-dim vector
}

impl ScifactFixtures {
    pub fn load() -> Self { ... } // reads fixtures dir, decompresses .bin.gz
}
```

---

## Pass/Fail Criteria

| Test suite | Trigger | Pass criterion |
|---|---|---|
| Integration tests | `cargo test` | All assertions pass |
| SciFact benchmark | `cargo test --features slow-tests` | Recall@10 ≥ 0.60 |

### SciFact Benchmark Baseline History

| Chunker | Recall@10 | Notes |
|---------|-----------|-------|
| Fixed-word sliding window (180 words, 20-word overlap) | 0.823 | Pre-sentence-chunker baseline; 5,183 single-chunk docs |
| Sentence-aware + neighbor padding (`TARGET_WORDS=100`) | ≥ 0.85 (expected) | Implemented 2026-05-06; assertion threshold kept at 0.60 pending stable results |

Raise assertion threshold to 0.80 once sentence-chunker results are confirmed stable across runs.

---

## Out of Scope

- Frontend React component tests (existing 6 tests remain as-is)
- WebdriverIO E2E
- FTS5 hybrid search scoring
- Ollama/LLM response quality (librarian output not asserted — network dependency)
- Windows/Linux CI (macOS only for now)
