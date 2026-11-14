# working-memory-age Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A chunks-only folder whose chunks drop out of chunk search after a configurable per-folder age — filter-only (reversible), driven by `ingest.folder_ages` config and each document's first successful index time from `ingest_runs`, implemented as one shared SQL predicate backed by a registered scalar function.

**Architecture:** New config map `ingest.folder_ages` (days, resolved like `folder_tiers`, deepest prefix wins, ties → larger wins). One new scalar SQLite function `ct_folder_age_days(path)` (returns the configured cutoff or NULL), registered by an idempotent `register_ct_functions(&Connection)` that EVERY filtered consumer calls right before preparing its query. The shared WHERE fragment (two mandatory NULL arms + the age comparison) lands in the verbatim SQL twins (`tool_dispatch.rs:371` and `tools/src/queries.rs:76`), `semantic_search` (`search/mod.rs:150-157`), and `related_chunks` (`search/mod.rs:209-214`, `:238-245`). No surfacing field ships.

**Tech Stack:** Rust (src-tauri + tools crate), SQLite via rusqlite (`functions` feature, newly enabled), `ct` CLI, MCP sidecar.

**Spec:** `specs/2026-11-13-working-memory-age/spec.md` (rev 4 + OQ rulings — the binding authority; on conflict the spec wins).

## Global Constraints

- Branch: `feat/working-memory-age`; worktree `/mnt/kv-work/build/worktrees/curated-thoughts/working-memory-age`. The controller pushes.
- Cargo is SERIAL machine-wide. `export CARGO_TARGET_DIR=<worktree>/.cargo-shared-target` (ABSOLUTE); pre-flight `df -h /mnt/kv-work` ≥ 30G; `CARGO_BUILD_JOBS=2`; full output to logs.
- Empty config ⇒ byte-identical behavior to today (the OR short-circuits on the first NULL arm). This property is tested explicitly.
- Aging is filter-only: NO deletion, NO reaper, NO vault-file edits (INTENT rule 9).
- The verbatim-SQL-twin discipline holds: any fragment change lands in BOTH copies plus `tools/src/queries.rs:1269-1272` and `tools/tests/shared_queries.rs:14` together.
- `ct wisdom match` and all `llm_wiki_entries` paths are untouched.
- Spec-and-code in SEPARATE commits; plan/LEDGER are transient (deleted in the cleanup commit before merge).

---

### Task 1: `ingest.folder_ages` config key

**Files:**
- Modify: `src-tauri/src/config/mod.rs:400-420` (field), `:1320` + `:1764-1796` (known keys/salvage), `:343-372`-shaped tie diagnostic, resolver wrapper near `:724-727`/`:1163-1166`.
- Test: `src-tauri/src/config/mod.rs` tests (near `:2191-2298`).

**Interfaces:** Produces: `IngestConfig.folder_ages: HashMap<String, u64>`; `IngestPolicy::age_days_for(&self, path: &str, vault_root: Option<&Path>) -> Option<u64>`; `IngestPolicy.vault_root` already exists (`:706`).

- [ ] **Step 1: Write the failing tests** — absent key → `age_days_for` returns `None` everywhere; `{"records": 90}` → 90 for `records/foo.md` and `records`, `None` for `records-archive/x.md`; sibling conflict `{"records/": 30, "records": 90}` → 90 (larger wins) + the load-time diagnostic fires (mirror `scan_ingest_ties`, `:343-372`); salvage preserves `folder_ages` on a degraded load.
- [ ] **Step 2: Run** `cargo test -p curated-thoughts config:: 2>&1 | tee /tmp/w1-tests.log` — FAIL.
- [ ] **Step 3: Implement** — serde field with doc comment; add `"folder_ages"` to BOTH `known_ingest_keys` lists (`:1320`, `:1764`); extend the tie scan to report folder_ages conflicts; implement `age_days_for` on `IngestPolicy`: the caller passes an ALREADY-RELATIVE path (see Task 2 — the scalar fn strips the root first), so `age_days_for` itself calls the `match_prefixes` core on a relative path only and returns `None` on `NoMatch`/`Unplaceable`/tie→larger-wins resolution.
- [ ] **Step 4: Run** — PASS. **Step 5: Commit** — `git commit -am "feat(config): ingest.folder_ages — per-folder chunk age-out (issue #347)"`.

### Task 2: `ct_folder_age_days` scalar function + registration helper

**Files:**
- Modify: root `Cargo.toml` (workspace rusqlite features add `"functions"`), `src-tauri/Cargo.toml:29` + `tools/Cargo.toml:16` (inherit), new `src-tauri/src/ct_sql_functions.rs`, `src-tauri/src/lib.rs` (module decl + `pub fn register_ct_functions`).

**Interfaces:** Produces: `pub fn register_ct_functions(conn: &Connection)` (idempotent-by-replacement, cheap); the function reads the policy snapshot ONCE per registration (captured in the closure), never per row, never `fs::canonicalize`.

- [ ] **Step 1: Enable the feature** — workspace-level rusqlite features become `["bundled", "functions"]`; remove the `["backup"]` override shape in `src-tauri/Cargo.toml` to `features = ["backup", "functions"]` (or inherit workspace + add). `cargo check -p curated-thoughts 2>&1 | tee /tmp/w2-check.log` — compiles.
- [ ] **Step 2: Implement** `ct_sql_functions.rs`:

```rust
use rusqlite::functions::FunctionFlags;
use std::sync::Arc;

/// Register `ct_folder_age_days(absolute_path) -> INTEGER cutoff_days or NULL`.
/// The policy snapshot is captured ONCE here (per registration); per-row
/// evaluation touches only the in-memory snapshot. Pure string strip_prefix
/// against the captured root; a placement miss => NULL ("never ages");
/// NEVER fs::canonicalize, NEVER fs::read per row.
pub fn register_ct_functions(conn: &rusqlite::Connection) {
    let policy: Arc<crate::config::IngestPolicy> =
        Arc::new(crate::config::ingest_policy_snapshot());
    conn.create_scalar_function(
        "ct_folder_age_days",
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        move |ctx| {
            let raw: String = ctx.get(0)?;
            let root = policy.vault_root.as_deref();
            let Some(root) = root else { return Ok(None) };
            let rel = match std::path::Path::new(&raw).strip_prefix(root) {
                Ok(r) => r.to_string_lossy().into_owned(),
                Err(_) => return Ok(None), // placement miss: never ages
            };
            Ok(policy.folder_ages_lookup(&rel)) // deepest-prefix, larger-wins tie
        },
    )?;
}
```

with `ingest_policy_snapshot()` in config/mod.rs returning the current snapshot from the policy cache (one `fs::read` via the cache's refresh-on-change — this is what makes config edits take effect on the next query), and `folder_ages_lookup(&rel)` the relative-path resolver (reuses `match_prefixes`). `register_ct_functions` is `pub` and re-registration simply replaces — call it as often as needed.
- [ ] **Step 3: Rust-side unit test** — open an in-memory connection, register, `SELECT ct_folder_age_days('/vault/records/a.md')` with a config mapping `records` → 90 and vault root `/vault`: expect 90; `'/vault/other/a.md'` → NULL; no config → NULL.
- [ ] **Step 4: Commit** — `git commit -am "feat(config): ct_folder_age_days scalar function + register_ct_functions (issue #347)"`.

### Task 3: The shared WHERE fragment

**Files:**
- Modify: `src-tauri/src/tool_dispatch.rs:371-378` (`RECALL_CHUNKS_SQL_BASE`), `tools/src/queries.rs:76` (verbatim twin), `src-tauri/src/search/mod.rs:150-157` (`semantic_search`), `:209-214` + `:238-245` (`related_chunks`), `tools/src/queries.rs:1269-1272` (shape assertions), `tools/tests/shared_queries.rs:14` (parity).

**Interfaces:** Produces: the fragment (both NULL arms) appended to every chunk query; both SQL copies identical.

- [ ] **Step 1: Append the fragment to both `RECALL_CHUNKS_SQL_BASE` copies** (identical text in both crates):

```sql
AND (
      ct_folder_age_days(d.path) IS NULL
   OR (SELECT MIN(ir.run_at) FROM ingest_runs ir
        WHERE ir.doc_id = c.doc_id AND ir.outcome = 'indexed') IS NULL
   OR (unixepoch()
       - (SELECT MIN(ir.run_at) FROM ingest_runs ir
          WHERE ir.doc_id = c.doc_id AND ir.outcome = 'indexed')) / 86400
       <= ct_folder_age_days(d.path)
    )
```

Add the same fragment to the `semantic_search` and `related_chunks` SQL in `search/mod.rs` (both queries join `documents d` — the alias matches).
- [ ] **Step 2: Update the shape assertions** (`tools/src/queries.rs:1269-1272`) and extend `tools/tests/shared_queries.rs:14` to assert the fragment exists IDENTICALLY in both crates' copies (read both constants via `include_str!`-style or a duplicated assertion with a comment tying them).
- [ ] **Step 3: Empty-config byte-identity test** — with NO `folder_ages` config, `semantic_search` returns the same rows as before the fragment (fixture: seed two chunks, search, compare to a pre-fragment expectation). This is the spec's §2 promise.
- [ ] **Step 4: Run** — `cargo test -p curated-thoughts search 2>&1 | tee /tmp/w3.log && cargo test -p ct shared_queries 2>&1 | tee -a /tmp/w3.log` — PASS. **Step 5: Commit** — `git commit -am "feat(search): age-out predicate in the shared chunk-search SQL (issue #347)"`.

### Task 4: Registration at every filtered consumer + completeness test

**Files:**
- Modify: `search/mod.rs` (`semantic_search`, `related_chunks`), `tool_dispatch.rs:692`, `:810`, `tools/src/queries.rs:316` (`recall_chunks`), `tools/src/bin/curated_thoughts_mcp.rs:239`, `:363`, `src-tauri/src/lib.rs:3129` area (`search_vault` — inherits via `retrieval::semantic_search_chunks`), `:3146` (`get_related_chunks`).
- Test: new `src-tauri/tests/ct_sql_registration.rs`.

**Interfaces:** Consumes: Task 2's `register_ct_functions`. Produces: every recall entry point works on ANY connection.

- [ ] **Step 1: Call `register_ct_functions(conn)` immediately before each consumer's query prepare** — in `semantic_search` and both `related_chunks` fns (before `conn.prepare`), in `recall_chunks` (`tools/src/queries.rs:316` area), and at the four `format!` consumer sites where the connection is local to the call (`tool_dispatch.rs:692`, `:810`, `curated_thoughts_mcp.rs:239`, `:363`). Registration is unconditional and cheap (replacement + one policy-cache snapshot); duplicating it per query is the DESIGN (43 open sites make register-at-open incomplete — Opus delta MAJOR-1).
- [ ] **Step 2: The completeness test** — `src-tauri/tests/ct_sql_registration.rs`: build a brain with a `records/` folder aged 0 days AND an unaged folder; then run EVERY recall entry point — `search::semantic_search`, `related_chunks`, `recall_chunks` (tools), the two `tool_dispatch` legs, both `curated_thoughts_mcp` consumers' SQL, `search_vault`, `get_related_chunks` — each on a FRESHLY OPENED raw connection. Every entry point must succeed (no `no such function`) and must exclude the aged chunk while keeping the unaged one.
- [ ] **Step 3: Behavior tests** — with `records` aged 0: aged chunk hidden from `semantic_search` and `recall_chunks`; with the age removed from config (fresh registration): the SAME chunk reappears with NO re-ingest (reversibility, spec §7).
- [ ] **Step 4: Pre-V10 test** — a document with NO `ingest_runs` row stays visible even inside an aged folder (the second NULL arm).
- [ ] **Step 5: Run** — `cargo test --workspace 2>&1 | tee /tmp/w4.log` — PASS. **Step 6: Commit** — `git commit -am "feat(search): register the age predicate at every filtered consumer + completeness test (issue #347)"`.

### Task 5: Benchmark backdating fixture + hit-rate gate

**Files:**
- Test: `src-tauri/tests/` (new `working_memory_age_bench.rs` or extend Task 4's file).

**Interfaces:** Consumes: Tasks 1-4. Produces: the measurable before/after hit-rate record.

- [ ] **Step 1: Backdating fixture** — seed a brain where a known share of documents get `ingest_runs` rows backdated (`UPDATE ingest_runs SET run_at = unixepoch() - 400*86400`), others fresh. (Plain scratch brains are freshly indexed — the filter would never fire; this fixture is REQUIRED, Opus MINOR-1.)
- [ ] **Step 2: Hit-rate gate** — run the same query set with the filter active and inactive (age absent); record hit@k delta in a benchmark record `docs/benchmarks/2026-11-13-working-memory-age-hit-rate.md`. Uses STORED embeddings (no external model needed). Gate: aged-doc exclusion works; unaged-doc hit rate is unchanged.
- [ ] **Step 3: Commit** — `git commit -am "test(bench): backdated working-memory age-out hit-rate gate (issue #347)"`.
- [ ] **Step 4 (note):** the full LongMemEval/paraphrase step-4 battery runs controller-side if the session has model access; otherwise record `benchmark-pending` in the spec status line and PR body (merge-blocks, like #349's Task 8).

### Task 6: Example config + cleanup

**Files:**
- Modify: `specs/2026-11-13-working-memory-age/spec.md` (status flip), delete `plan.md` + `LEDGER.md`; README config docs if a config example section exists (grep first; do not invent one).

- [ ] **Step 1: Status flip + cleanup commit** — `git commit -m "chore(spec): working-memory-age implemented — drop transient scaffolding"`.
- [ ] **Step 2: Post-session sweep** — `du -sh .cargo-shared-target`; verify no stray `target/` dirs; `df -h /mnt/kv-work`.
