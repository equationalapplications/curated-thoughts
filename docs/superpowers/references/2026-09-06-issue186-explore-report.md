# Issue #186 — Evidence `source_ref` writer mangling: exploration report

Repo: `/home/kv-thinkpad-t420-ubuntu/code/github/equationalapplications/curated-thoughts`
(HEAD of working tree, no branch created; read-only exploration).
Frontend pins `@equationalapplications/core-llm-wiki` `7.1.0` in `package.json:47`, but the
**installed** copy in `node_modules` is **6.0.1** (`node_modules/@equationalapplications/core-llm-wiki/package.json:3`,
`node_modules/.pnpm/@equationalapplications+core-llm-wiki@6.0.1`) — all engine line numbers below are against 6.0.1 dist.

## (A) Root-cause verdict: **CONFIRMED** (with one correction to the issue's framing)

The mangler is `normalizeSourceRef` in core-llm-wiki, and the write path is the engine's
**`setup()`-time back-rewrite**, not `ingestDocument` itself.

The normalizer (exact quote):

```js
// node_modules/@equationalapplications/core-llm-wiki/dist/index.js:3905 (v6.0.1)
function normalizeSourceRef(value) {
  if (typeof value !== "string") return null;
  const cleaned = value.replace(/[^A-Za-z0-9._\- ]/g, "").trim().slice(0, 255);
  return cleaned.length > 0 ? cleaned : null;
}
```

This reproduces the DB damage byte-for-byte, including two details no other candidate explains:

1. **The 255-char cap.** Every mangled `evidencechunk…` row in the live DB is exactly **255 chars**
   (`~/.brain/brain.db`: 260 librarian_inferred rows with `length(source_ref)=255`). The Rust writer
   never truncates; `.slice(0, 255)` does. (I did not inspect all 260 rows, so state this as
   "all sampled rows are 255", but the cap is in the code.)
2. **Newlines inside quotes survive as `n`-looking artifacts** — actually the regex *deletes* `\n`
   entirely, which glues `end_line` and `quote` together (`…end_line33quote Procedures…` where `33`
   is really `:3,` mangled). Only `[A-Za-z0-9._- ]` survive; `{}" :,[]` and `\n` are deleted, so the
   `n` in the sample is from prose text, not a preserved newline.

### Exact call chain (producer → mangler)

| # | Step | Location |
|---|------|----------|
| 1 | Librarian synthesis resolves chunk evidence | `src-tauri/src/librarian/synthesis.rs:725-744` (`resolve_evidence` builds `Vec<StoredEvidenceChunk>`) |
| 2 | Proposal item persisted with evidence | `synthesis.rs` (~line 831 `resolve_evidence(...)` → `NewProposalItem.evidence`) |
| 3 | Commit path builds JSON evidence blob | `src-tauri/src/db/commit.rs:601-642` (`evidence_json_with_hashes` → `serde_json::json!({"proposal_id":…, "evidence":[…]}).to_string()`) |
| 4 | **INSERT with JSON source_ref** | `commit.rs:900` (`let source_ref = evidence_json_with_hashes(…)`) → `commit.rs:917-936` (`INSERT INTO llm_wiki_entries (… source_ref …)` — written **clean and valid JSON**) |
| 5 | Same JSON goes to outbox payload | `commit.rs:938-960` (`push_entries_outbox` + `wiki_fact_outbox_payload` at commit.rs:645) |
| 6 | Engine frontend instance created / re-created | `src/lib/wiki.ts:319,334,345,357` (`createWiki(tauriWikiAdapter, …)`) |
| 7 | `setupWiki()` → `wiki.setup()` | `src/lib/wiki.ts:335/346/358`, called from `src/main.tsx:32` (also on every outbox worker start/stop event, wiki.ts:332-352) |
| 8 | `setup()` runs legacy-ref back-rewrite **unconditionally** | `node_modules/.../core-llm-wiki/dist/index.js:7353-7361`: `findRowsForSourceRefMigration()` → for each row `normalizeSourceRef(row.source_ref)` → `updateSourceRefByRowid(rowid, normalized, tx)` |
| 9 | Selector matches JSON blobs | dist/index.js:1363-1376 `findRowsForSourceRefMigration`: `source_ref GLOB '*[^-A-Za-z0-9._ ]*'` — every JSON blob fails this GLOB (it contains `{`, `"`, `:`) and is selected |
| 10 | **Mangled value written back** | dist/index.js:1377-1381 `updateSourceRefByRowid` → `UPDATE ${prefix}entries SET source_ref = ?` on the **shared** brain.db via the JS adapter |
| 11 | Shared-DB link | `src/lib/wikiAdapter.ts` (tauriWikiAdapter → `invoke("wiki_exec"/"wiki_run"/"wiki_get_all"…)`) → `src-tauri/src/lib.rs:2190` (`wiki_exec`, `execute_batch` on the one `DbState` connection) / `lib.rs:2207` (`wiki_run`) |

The JS engine and the Rust writer share the **same SQLite file**: the frontend adapter issues raw SQL
over Tauri IPC (`wikiAdapter.ts:21-79`) into the same `DbState` connection (lib.rs:2190) that
`commit.rs` writes through. There is one `llm_wiki_entries` table (DDL in Rust `db/okf_ddl.rs:12-43`,
mirrored in engine dist/index.js:30+ with `tablePrefix = llm_wiki_`).

Note the issue's original hypothesis ("passes through the sourceRef normalizer used by
ingestDocument") is directionally right about the function but wrong about the trigger: the producer
normalizer call site (`ingestDocument`, dist/index.js:4171) never sees these blobs because **CT never
calls `ingestDocument`** (documented in
`docs/superpowers/specs/2026-09-01-memory-architecture-intent-implementation-design.md:40-42`: the
guard "sits on `wiki.ingestDocument`, and **CT never calls it**"). The destruction happens in the
unconditional post-setup migration loop (dist/index.js:7353).

**Trigger cadence (severity):** `setup()` runs on every app launch (main.tsx:32) and again on every
`outbox-worker-started`/`-stopped` event (wiki.ts:332-352). Any JSON `source_ref` written since the
last setup is mangled at the next setup.

### Damage model

- `proposal_id` key is destroyed → `wiki_forget::forget_entries_by_source_refs`
  (`src-tauri/src/db/wiki_forget.rs:25`, exact-match `WHERE source_ref IN (…)`), proposal-based
  retraction by ref, and `source_ref`-keyed dedupe/supersede semantics can never match.
- `evidence` array structure is destroyed → `source_docs_from_ref` (`db/entities.rs:201-244`)
  `serde_json::from_str` fails → returns `[]` → **provenance display silently empty** for 260+ live rows.
- `source_ref_is_still_grounded` (`db/commit.rs:294`) treats unparseable JSON-looking refs as
  *still-grounded* (commit.rs:368-380 "defensive"), so heal passes over them — the damage is
  **permanent** and invisible.
- Startup canary `warn_on_malformed_source_refs` (`db/connection.rs:205`, wired at :185-187,
  references issue **#162**) logs the malformed count on every launch but does not repair.

## (B) Not applicable — verdict is CONFIRMED (no alternative hypothesis survives)

Discriminating evidence already gathered:

- Rust side has no character-stripping normalizer on this path; the only Rust sanitizer
  (`okf/sanitize.rs:27-76`) **replaces** disallowed chars with `_` (and appends a hash suffix), it
  never deletes them, and it is not called from the librarian/commit path.
- `normalize_path_argument_to_vault_relative` (lib.rs:353-410) returns non-absolute input unchanged;
  it cannot delete punctuation.
- The engine's own librarian (`doRunLibrarian`, dist/index.js:5091 `source_ref: null`) writes NULL,
  so the engine cannot be the *producer* — but its setup-rewrite (step 8-10) is the mangler, proven
  by the 255-cap fingerprint.

## (C) Generation-stub harness (for an end-to-end librarian test without a real LLM)

**There is no env-var stub for generation analogous to `CURATED_EMBED_STUB`.** The equivalent
capability exists two ways:

1. **mockito HTTP mock of an `External` generation provider** — the closest analog, and the pattern to copy:
   - Example test: `src-tauri/tests/folder_rules.rs:147-190` —
     - `mockito::Server::new()`; mock `POST /v1/chat/completions` returning
       `{"choices":[{"message":{"content":"<json-string>"}}]}` (lines 164-172);
     - write `LlmConfig { provider: GenerationProviderKind::External, external_url: Some(server.url()), … }`
       via `write_config` into `CURATED_BRAIN_DIR` (lines 174-190);
     - `LlmConfig`/`GenerationProviderKind` from `tauri_app_lib::inference::config`.
   - The Rust librarian consumes it: `generate_text` (`inference/mod.rs:87-130`) posts
     `{model, messages:[system,user]}` to `{external_url}/v1/chat/completions` and returns
     `body["choices"][0]["message"]["content"]`.
   - Note this stubs the **Rust** librarian path (`librarian::synthesis`), which is the path whose
     output gets mangled — exactly what an issue-#186 regression test needs.
2. **JS-side engine librarian** is driven through the Tauri command `generate_text`
   (inference/mod.rs:87; wired into the engine's `llmProvider` at `src/lib/wiki.ts:270-285`). There
   is no JS test that runs the real engine's `runLibrarian`; `src/__tests__/wiki.test.ts:4-11` mocks
   `createWiki` entirely. No vitest fixture exercises the real engine + SQLite adapter.

Embeddings, for contrast: `CURATED_EMBED_STUB=constant8` (and `constant8_short`) —
`embedder/mod.rs:171-179`, consumed by `embed_batch`; extensively used via `temp_env::with_vars`
(e.g. `src-tauri/src/db/commit.rs:3753`, `embed_sweep.rs:224+`, tool_dispatch.rs:1906).

A copyable end-to-end shape: `TestApp::new()` (`tests/helpers/mod.rs:22`) + mockito server +
`CURATED_EMBED_STUB` + ingest → run synthesis command → assert `llm_wiki_entries.source_ref`
parses as JSON → **simulate the engine setup pass** by running the selector+normalizer semantics →
assert idempotent.

## (D) `normalizeSourceRef` call-site inventory (core-llm-wiki 6.0.1 dist/index.js)

| Line | Caller | Receives structured blobs today? | Path-legit? |
|------|--------|----------------------------------|-------------|
| 3905 | definition | — | — |
| 4171 | `IngestionService.ingestDocument(params.sourceRef)` | No — caller passes path-like refs (`ingestDocumentByPath` at src/lib/wiki.ts:253-266); **CT never calls it in production** | ✅ yes |
| 4956 | `forget({sourceRef})` | Only string selectors; equality-based lookup needs canonical form | ✅ yes (but would break if refs become JSON — see below) |
| 5025 | `forgetDryRun({sourceRef})` | same | ✅ yes |
| 5798 | `importDump` fact import (throws if normalizes to null) | **Yes — could receive JSON blobs** if a dump carries CT facts | ⚠️ structured-capable callers exist upstream |
| 7356 | **`setup()` post-migration rewrite (THE BUG)** | **Yes — reads all rows from the shared table and destroys any JSON blob** | ❌ this is the mangler; selector assumes path-like refs |
| 7366 | `hasChanged(entityId, sourceRef: string, hash)` | String compare against stored refs | ✅ yes |
| 7382 | `hasChanged(entityId, entries[])` | same | ✅ yes |
| 7585 | `upsertGraph(params.sourceRef)` | Graph-writer refs | ✅ yes |

Also relevant: the *selector* `findRowsForSourceRefMigration` (dist/index.js:1363-1376) and the
migration-gap detector inside MIGRATION v9 (dist/index.js:1366-1373, same GLOB) both assume
`source_ref` values are path-like; both are engine-internal and cannot distinguish "legacy path that
needs normalization" from "structured JSON evidence".

**Fix-shape note for the spec (engine side):** the table ownership is split — Rust writes JSON
source_refs for `librarian_inferred` rows; the engine assumes every non-null source_ref is either
path-like or NULL. Options to evaluate in the fix spec: (a) engine: exempt rows whose source_ref
starts with `{` from `findRowsForSourceRefMigration` (JSON parse check instead of GLOB); (b) CT
side: stop storing JSON in the engine-owned column (move evidence to a side table keyed by entry id
or `source_hash`), or (c) encode evidence in `llm_wiki_source_ref_index` /
`curated_proposal_items.evidence` (already stores full JSON evidence per item —
`synthesis.rs` tests at 1594-1616) and keep `source_ref` path-shaped or NULL. Option (c) aligns with
engine ownership but loses the "same row carries its own provenance" property that
`source_docs_from_ref`, `source_ref_is_still_grounded`, and `get_chunk_ids_for_wiki_entry`
(lib.rs:2528, still calls `normalize_path_argument_to_vault_relative` on the JSON blob and returns
`Ok(vec![])` — dead legacy consumer) depend on; a fix spec must decide which consumers to migrate.

## (E) DDL facts

`src-tauri/src/db/okf_ddl.rs:12-48` (applied to the shared brain.db; the engine's own DDL at
dist/index.js:30-72 is kept in sync):

```sql
CREATE TABLE IF NOT EXISTS llm_wiki_entries (
  id TEXT PRIMARY KEY,
  entity_id TEXT NOT NULL,
  ...
  source_hash TEXT,
  source_ref TEXT,            -- line 21: plain TEXT, no CHECK, no length limit
  ...
);
CREATE INDEX llm_wiki_entries_source_ref_idx ON llm_wiki_entries(entity_id, source_ref);        -- :46
CREATE INDEX llm_wiki_entries_source_hash_idx ON llm_wiki_entries(entity_id, source_hash) WHERE source_hash IS NOT NULL;  -- :47
```

- `source_ref` is **nullable TEXT, no CHECK constraint, no length limit, no JSON validation** at the
  DDL level. The 255/64-char limits are applied only in JS (`normalizeSourceRef`/`normalizeSourceHash`).
- `llm_wiki_source_ref_index` (okf_ddl.rs:56-65): `(entity_id, source_hash)` UNIQUE partial index
  with `source_ref TEXT NOT NULL`; enforces one canonical ref per (entity, hash) among live rows.
  Currently **0 rows** in the live DB (`SELECT COUNT(*) FROM llm_wiki_source_ref_index` → 0), so no
  canonical-ref conflicts are being tracked at all.
- Rust migration machinery also rewrites source_ref in place: `db/migration.rs:194-246` (chunk-hash
  migration, `UPDATE llm_wiki_entries SET source_ref = ?1 WHERE id = ?2`), with tests pinning JSON
  shape (migration.rs:337-465) — precedent that rewriting this column in bulk is established practice.

## (F) Implementer landmines

1. **`resolve_brain_paths` panics in test builds** without `CT_ALLOW_LIVE_BRAIN=1` —
   `src-tauri/src/retrieval/mod.rs:71-72` gates `guard_against_live_brain` under
   `#[cfg(any(test, feature = "test-utils"))]`; the guard (mod.rs:85-118) panics if the resolved
   config/db path **is** the live `~/.brain` path unless `CT_ALLOW_LIVE_BRAIN` is truthy
   (`"0"`/`""`/`"false"` do NOT disable). Tests must redirect via
   `CURATED_BRAIN_DIR`/`CURATED_BRAIN_CONFIG`/`CURATED_BRAIN_DB` under `temp_env::with_vars`
   (commit.rs:3753 pattern) — introduced by issue **#178** (docs reference v2.4.3 era; CHANGELOG
   confirms #178 guard). Any new test that runs librarian synthesis **must** set `CURATED_BRAIN_DIR`
   (the `TestApp` helper at tests/helpers/mod.rs:22-33 does this via `make_test_app(tmp.path())`).
2. **`CURATED_EMBED_STUB` is not a generation stub** — do not assume it unblocks librarian runs;
   generation needs the mockito `External` provider (section C).
3. **Engine version skew**: package.json says 7.1.0, node_modules has 6.0.1. If the fix is made
   engine-side (option (a) above), the 6.0.1 line numbers here will not match 7.1.0 dist. Re-verify
   against the installed version actually pinned at fix time, and note the fix must ship in the npm
   package (a CT-side workaround alone leaves old setups mangled at every setup).
4. **`source_ref_is_still_grounded` is defensively permissive** (commit.rs:368-380): unparseable
   JSON-looking refs are treated as grounded, so no heal pass will clean these rows. A data-repair
   migration is needed for the ~260 existing damaged rows; they cannot be regenerated from the rows
   themselves, but *can* be re-derived: `curated_proposal_items.evidence` still holds the full JSON
   evidence (synthesis.rs:1594-1616 test), and `curated_proposals.id` + chunk `content_hash` allow
   rebuilding `{"proposal_id":…,"evidence":[…]}`.
5. **Setup rewrite runs on every `createWiki`**, including outbox-worker transitions (wiki.ts:332-352)
   — a repair migration must also be idempotent against re-mangling, i.e. the engine-side guard (a)
   must land before or with any CT-side repair, else repaired rows are re-mangled at next launch.
6. **The Rust `curated_proposal_items` schema keeps `item.id` = `generate_id("item_")`**
   (synthesis.rs:821/838) and `curated_proposals.id` = `generate_id("prop_")`; the mangled DB rows
   show `evidenceproposal_idprop_<24hex>` — confirming the mangler also deleted the `{` that
   separated keys, gluing `evidence` + `proposal_id` together. Any grep-based damage census should
   match both `evidenceproposal_id%` and `evidencechunk_id%` shapes (both present in live DB).
7. **`get_chunk_ids_for_wiki_entry` (lib.rs:2528-2574)** still routes source_ref through
   `normalize_path_argument_to_vault_relative` → `safe_vault_path` and silently returns `[]` for
   JSON refs — a legacy consumer already dead for JSON rows; don't treat its silence as health.
8. **Warning-only detection today**: `warn_on_malformed_source_refs` (db/connection.rs:205) counts
   but does not fail or repair; its count query is a ready-made damage census for a repair migration
   (`substr(source_ref,1,1)='{' AND NOT json_valid(source_ref)` — though note the mangled rows no
   longer start with `{`; the census needs the `evidence%` prefix patterns too).
