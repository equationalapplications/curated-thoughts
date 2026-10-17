# Task 3 Handoff — `two_stage` module (issue #271 two-stage retrieval)

**Date:** 2026-10-15 (session interrupted mid-verify; resume from here)
**Plan:** docs/superpowers/plans/2026-10-15-issue271-two-stage-retrieval.md §Task 3
**Base commit:** df8d60b (Task 2: shared eligibility predicate)
**Branch state:** uncommitted — see "Working tree" below.

## What's DONE (all on disk, no commit yet)

1. **`src-tauri/src/two_stage.rs`** — full Task 3 implementation + tests:
   - `two_stage_schema_ready` — sqlite_master table probes (librarian_evidence,
     curated_proposal_sources, chunks) + documents columns (embed_key AND
     last_indexed_hash) via `existing_columns`. Any miss ⇒ false ⇒ v1 fallback.
   - `candidate_set` — evidence-hash hop ∪ proposal chain (r28-M1). Both legs
     go through the shared Task-2 eligibility fragment (`entry_eligibility_where`,
     alias `e`) so only ELIGIBLE entries anchor edges. Raw UNION, no doc dedupe
     (r29-M2). `:now` bound iff the fragment references it.
   - `complement_entry_ids` — STRUCTURAL: eligible set (via `eligible_entry_ids`)
     anti-join reachable-set, never by provenance (r27-M1/r29-M1). Rules:
     `"restricted-cosine"` (rule i: candidate doc with ≥1 scorable chunk) and
     `"chunk-hop"` (rule ii: member chunk only; proposal-chain-only docs are
     rule-(ii)-dark; chunk-id-only evidence = complement, r30-m4).
   - `stage1_search` — chunk⋈embeddings over candidates, cosine from
     `crate::search`, sort by score desc/lowest chunk_id, RESULT-level dedupe of
     byte-identical vectors (`same_vector`), truncate to k AFTER dedupe (r29-M2).
   - `member_chunk_ids` — candidate-doc chunks whose content_hash appears in a
     live entry's evidence blob AND have an embeddings row.
   - `coverage` — (n, covered) over candidate docs with ≥1 scorable chunk;
     embed_key match = covered; None on probe failure. Vacuously (0,0) on empty
     candidate set. Now_ms is the REAL time, never 0 (plan NOTE).
2. **6 tests, all PASSING** (`two_stage::tests::*`): the plan's 5
   (candidate_set union / stage1 result-dedupe / complement structural per rule /
   coverage call-independent / probes-None on schema skew) + the sor-mandated
   pinning test `future_supersession_stays_eligible` (SOR Finding 1: pins the
   flipped `superseded_at > :now` comparison in BOTH consumers —
   `eligible_entry_ids` and `wisdom_match_with_floor`).
3. **`src-tauri/src/lib.rs`** — `pub mod two_stage;` added.
4. **`src-tauri/src/wisdom_match.rs`** — exactly the two permitted touches:
   `temporal_columns` now `pub(crate)`; `#[allow(dead_code)]` dropped from
   `eligible_entry_ids` (real consumer landed). No other changes.

## Working tree

```
M  src-tauri/src/lib.rs
M  src-tauri/src/wisdom_match.rs
?? src-tauri/src/two_stage.rs
?? .cargo-shared-target/   (build artifact dir — do NOT commit)
```

## What REMAINS (in order)

1. **Full test suite** — verified: targeted run `cargo test --manifest-path
   src-tauri/Cargo.toml --features test-utils,mcp-server --lib --
   --test-threads=1 two_stage` → 6/6 ok. NOT yet verified: whole-suite run
   (interrupted). Watch for:
   - `embed_scheme` reader-inventory test: two_stage's SELECTs read
     embedding_blob only via the SHARED FRAGMENT (the interpolating statements
     don't name embedding_blob in their own literal), so no `{scheme_filter}`
     token was added — if the inventory complains, that's the place to look.
   - Compile is clean (verified); watch for warnings on the test features.
   Run: `cargo test --manifest-path src-tauri/Cargo.toml --features
   test-utils,mcp-server -- --test-threads=1` (background; ~15-20 min).
2. **Commit** (do NOT push):
   ```
   git add src-tauri/src/two_stage.rs src-tauri/src/lib.rs src-tauri/src/wisdom_match.rs
   git commit -m "feat(wisdom): two_stage module — candidate set, structural complement, stage-1 search (Task 3)"
   ```
3. Tick the Task 3 checkboxes in the plan file (separate docs commit, matching
   the Task-1 convention: "docs(plan): tick Task 3 checkboxes").

## Gotchas learned this session

- `open_in_memory()` logs a benign `[ct::repair FATAL] V22 DEFERRED` warning in
  tests (no vault root) — expected noise, not a failure.
- `llm_wiki_entries.created_at/updated_at` are NOT NULL — fixtures set them to 0.
- `Temporal` fields are private: detect `:now`-binding from the fragment string
  (`elig.contains(":now")`), don't reach into the struct.
- No FK enforcement in CT connections; dangling `librarian_evidence.proposal_id`
  rows join to nothing (by design) and land in the complement.
