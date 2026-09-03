# Spec: open-issue remediation sweep (#158, #159, #163, #162, #125)

**Repo:** curated-thoughts · **Type:** remediation sweep · **Priority:** P1 (PR 1) → P4 (PR 5)
**Status:** Draft
**Builds on:** `0adbcff` (ontology activation / strict edge gate, v1.42.0), PR #132 (outbox `Delete` on forget), PR #99 (`source_ref` consumer contract)
**Context:** Five issues are open. Four were filed from the 2026-09-03 production-brain session; #125 predates it. This spec sequences all five into five independent PRs, ordered by *live blast radius* rather than filing order.

Two investigation findings reorder the naive priority:

1. **#162 is closed by fact, not by fix.** The corrupt brain has been purged, and the current writer is proven sound (see §5). What remains is a cheap regression guard, not a migration.
2. **#158's bad data survived that purge.** `SOURCE_ALIVE_SQL` (`src-tauri/src/db/edge_purge.rs:50-52`) counts an edge as alive when its endpoint exists in `llm_wiki_entries` **or `curated_entities` or `llm_wiki_tasks`**. The fabricated edges anchor `curated_entities`, so purging entries left them untouched. #158 is therefore the only issue with confirmed live corruption, and leads.

## Ordering

| PR | Issue | Scope | Effort | Why here |
|----|-------|-------|--------|----------|
| 1 | #158 | Read-side manifest filter, prompt vocabulary, off-manifest sweep | M | Only confirmed live corruption; poisons every `wiki_context` call |
| 2 | #159 | Rename detection on reconcile + `ct vault heal-orphans` | M | Ongoing; fires on every vault reorganisation |
| 3 | #163 | `ct wiki forget --ref/--like` | S | Ops gap; pure delegation, no new risk |
| 4 | #162 | Regression guards only (OKF round-trip test + startup canary) | S | Data gone, writer sound — guard against recurrence only |
| 5 | #125 | Doc close-out | XS | Theoretical under threat model |

PRs are independent and may be parallelised, except that PR 4's canary is most useful after PR 1 lands (both answer "is stored data well-formed?").

---

## PR 1 — #158: fabricated graph edges

### Problem

`wiki_context` / `wiki_traverse_graph` return edges whose `edge_type` is not in the active ontology manifest, including a future-dated `has_open_bug_in_ct_recall_path_reported_2026-09-09`. Three distinct gaps, only one of which the issue names:

**Gap A — the issue is mis-scoped.** It reads as "there is no ontology gate." There is one: `resolve_strict_edge_vocabulary` (`src-tauri/src/db/commit.rs:152`) plus the drop branch at `commit.rs:1307-1320`. It entered in `0adbcff`, first tagged **v1.42.0** — one day before the observation on v1.42.7. It is deliberately non-retroactive; `strict_mode_grandfathers_edges_written_before_the_manifest` (`commit.rs:2897`) pins that. The observed edges are grandfathered pre-v1.42.0 rows. **Update the issue body before assigning, or the implementer will rebuild what exists.**

**Gap B — reads are unfiltered.** `wiki_context` calls `walk_seed(..., &[])` (`src-tauri/src/wiki_graph.rs:1008-1014`); `fetch_neighbors` (`wiki_graph.rs:417-429`) builds an edge-type filter *only* from caller-supplied types. Every stored row is returned, manifest-valid or not. This is what makes stale rows user-visible.

**Gap C — generation is unconstrained.** `src-tauri/src/librarian/synthesis.rs` contains no reference to the ontology. The system prompt (`synthesis.rs:590-599`) offers a free-form `"edge_type"` string. The model is invited to invent. Separately, `LlmEdge` (`synthesis.rs:106-110`) has **no `evidence` field** — unlike `LlmFact` (`:102`) and `LlmTask` (`:116`) — and edge proposal items are built with hardcoded `evidence: Vec::new()` (`synthesis.rs:792`). An edge is *structurally ungroundable*: there is no cited chunk to verify a span or date against.

### Change

**1. Read-side manifest filter.** In `fetch_outbound_neighbors` / `fetch_inbound_neighbors` / `fetch_entity_neighbors` (`wiki_graph.rs:493` / `:531` / `:572`), when the entity's ontology is strict, intersect the stored `edge_type` against the manifest vocabulary via `declares_edge_type` (`wiki_graph.rs:72`). Resolve the vocabulary **once** per `wiki_context` / `wiki_traverse_graph` call, not per hop.

**2. Prompt vocabulary injection.** Pass the manifest's `edge_type_names()` (`wiki_graph.rs:81`) into the synthesis system prompt at `synthesis.rs:590-599`, so the closed vocabulary constrains generation and not just persistence.

**3. Fail loud, not open.** `resolve_strict_edge_vocabulary` returns `None` — disabling the gate entirely — when the ontology is unreadable (`commit.rs:180-191`) or when strict mode declares zero edge types (`commit.rs:170-176`). Both paths currently only `eprintln!`. Surface them as real warnings through the app's tracing layer.

**4. Off-manifest sweep.** Add `purge_off_manifest_edges` alongside `purge_dead_edges` (`src-tauri/src/db/edge_purge.rs:183`), emitting proper outbox `Delete` rows. Do **not** route this through `wiki_forget` — that path is entry-keyed and would not match edges anchored on `curated_entities`.

### Explicitly out of scope

Full post-extraction span/date verification (the issue's mitigation 2) is **L** and deferred. Adding `evidence: Vec<String>` to `LlmEdge` is the enabling step and touches the golden JSON fixtures at `synthesis.rs:1393/1474/1606/1721`; do it in a follow-up so this PR stays reviewable.

### Verification

Before implementing, confirm the bad rows are still present — the entry purge should not have touched them:

```sql
SELECT COUNT(*) FROM llm_wiki_edges
 WHERE edge_type LIKE '%2026-09-09%'
    OR edge_type LIKE 'draft_v1_43_0%';
```

Tests: a strict-ontology brain with one manifest edge and one off-manifest edge, asserting `wiki_context` returns only the former; a sweep test asserting outbox `Delete` rows are emitted; a regression test that a *non*-strict brain is unfiltered.

---

## PR 2 — #159: stale `doc_path` after vault file move

### Problem

`doc_path` is **not stored on chunks**. `documents.path` is the source of truth (`src-tauri/src/db/schema.rs`), chunks reference it by `doc_id` FK, and `related_chunks` (`src-tauri/src/search/mod.rs:205`) joins to select `d.path AS doc_path`.

On `git mv`, the startup reconcile (`src-tauri/src/lib.rs:1035-1065`) sees "old path gone → dispatch `Remove`" and "new path exists → dispatch `Create`". `Remove` hard-deletes the row and cascade-deletes its chunks; `Create` re-inserts and re-chunks from scratch. So the rename is never *recognised* as a rename: best case the file is needlessly re-chunked and re-embedded, worst case (events coalesced or delivered out of order, or reconcile interrupted mid-batch) the old row survives alongside the new one and serves stale provenance — the reported symptom.

The periodic sweep (`src-tauri/src/pipeline/watchdog/sweep.rs:118`) does not consult disk at all; it only drains rows already marked pending. It cannot detect renames.

### Change

**1. Rename probe at reconcile.** Before dispatching `Remove` for a missing path, look for another `tier='user_doc'` row whose `documents.hash` matches. `documents.hash` is a per-file SHA-256 written by `upsert_document` (`src-tauri/src/db/queries.rs:18`), so a byte-identical `git mv` matches exactly. On a unique match, `UPDATE documents SET path = ?new WHERE id = ?old` — the `doc_id` FK is preserved, so chunks, embeddings, and curated relationships all survive with no re-embedding. Ambiguous (multi-match) cases fall through to today's `Remove`.

**2. `ct vault heal-orphans`.** Same pass, runnable without a watcher restart, for vaults reorganised in bulk. Reuse `build_path_candidates` (`src-tauri/src/tool_dispatch.rs:61`) for input normalisation. Route via `run_cli_subcommand` (`src-tauri/src/main.rs:31`).

### Trap for the implementer

**`chunks.content_hash` is path-mixed and must not be used as a file identity.** `compute_chunk_hash(text, doc_path, position)` (`src-tauri/src/db/chunk_hash.rs:18`) deliberately bakes the path in, and `compute_chunk_hash_differs_on_path_change` pins that behaviour. Use `documents.hash`. If a future change needs chunk-level remapping, `run_chunk_hash_migration` (`src-tauri/src/db/migration.rs:44`) already implements the `chunk_text`-equality remap and is the template.

No schema change, no migration.

### Tests

Auto-heal happy path; hash-mismatch no-op; ambiguous multi-match falls through to `Remove`; reconcile interrupted mid-batch; CLI dry-run.

---

## PR 3 — #163: `ct wiki forget --ref / --like`

### Problem

Purging entries by `source_ref` requires compiling a throwaway bin against `tauri_app_lib` (~7 min on the target laptop). The GUI forget button is path-keyed and cannot match evidence refs; MCP deliberately excludes DB-entry purge (#137 Non-Goals).

### Change

All safety-critical work already exists. `forget_entries_by_source_refs` (`src-tauri/src/db/wiki_forget.rs:25-76`) owns a single transaction that: selects doomed `(id, entity_id)` **before** delete so each outbox row gets the right entity partition; pushes one `OutboxOperation::Delete` per entry; hard-deletes; calls `purge_edges_for_hard_deleted`; commits. Rollback is automatic and pinned by `forget_entries_by_source_refs_rolls_back_if_purge_fails` (`wiki_forget.rs:244-271`). **The caller adds nothing but selection and UX.**

New `WikiCmd::Forget` variant in `tools/src/bin/ct.rs` (alongside `List`/`Get` at `:236-247`, dispatch at `:314-317`), body in `tools/src/cmds.rs`:

```
ct wiki forget --ref <source_ref> [--ref ...] [--dry-run] [--yes]
ct wiki forget --like <prefix>                [--dry-run] [--yes]
```

- `--like` resolves client-side via `SELECT DISTINCT source_ref ... WHERE source_ref LIKE ?1` bound as `prefix%`, then passes **exact** refs to the production function. Never interpolate into the DELETE.
- Reject `%` and `_` inside `--like` so the selector stays a literal anchored prefix and cannot silently become a substring scan.
- Reject `--ref` and `--like` together as ambiguous in v1.
- Without `--yes` and not `--dry-run`, refuse and exit 1 — mirror `librarian_run_cmd` (`ct.rs:568-581`).
- Apply `redact_home` (`ct.rs:52-86`) to every echoed ref; they routinely contain `$HOME`-rooted paths.

**Document, do not change:** the function does not filter on `deleted_at`, so it hard-deletes live *and* soft-deleted matches. That is correct for incident cleanup. No `--include-soft-deleted` flag.

Integration test in `tools/tests/ct_wiki_forget.rs`, mirroring `tools/tests/ct_graph_wiki.rs` (spawn via `env!("CARGO_BIN_EXE_ct")`, seed with `AppDb::open_with_config`, `CURATED_BRAIN_DIR` via `temp_env::with_vars`). Assert the entry, both edge directions, and one outbox `Delete` per entry.

No new `[[bin]]` — incremental build cost only.

---

## PR 4 — #162: regression guards only

### What the investigation established

- **The current writer is sound.** `cargo test --lib db::commit::` → 42 passed, 0 failed, including `resolve_proposal_writes_content_hash_in_source_ref`.
- **The corrupt rows were mangled *genuine* writer output.** Two independent fingerprints match byte-for-byte. `src-tauri/Cargo.toml:28` pins `serde_json = "1"` with no `preserve_order` feature, so `serde_json::Map` is a `BTreeMap` and keys serialise **alphabetically** — `evidence` before `proposal_id`, even though the source lists `proposal_id` first. The corrupt rows begin `evidence…`, matching the serialised order. Separator style is compact (`.to_string()`, not pretty-printed); the apparent space in `quote CT` is the quote text's own leading space, confirmed by reconstruction. **This eliminates the "sanitising writer variant" hypothesis** — a different writer would not reproduce serde_json's exact ordering *and* compact separators.
- **The OKF round-trip is cleared.** `source_ref` round-trips through OKF frontmatter as `resource` (`okf/fact_file.rs:35` write, `:138` read) and serialises as an *unquoted flow mapping*, so it depends on `parse_flow_mapping_text`, which goes opaque when `max_brace_depth > 1`. Probed with 8 adversarial payloads (nested braces, `#`, `: `, `}`, backslashes, embedded quotes): all lossless and valid JSON. `okf/frontmatter.rs` and `okf/fact_file.rs` are **unchanged since v1.39.0**, so this held during the corruption window too.
- **The data is gone.** The brain has been purged.

Cause remains formally unidentified, consistent with an external writer (`generated_by IS NULL` on every affected row; every in-tree writer populates it).

### Change — and what is deliberately dropped

**Dropped: the `MIGRATION_V17` CHECK constraint.** SQLite cannot `ALTER TABLE ADD CHECK`, so it needs a full-table rebuild in the V15 style (`src-tauri/src/db/schema.rs:287-327`). That is M-effort structural risk to guard against a cause that is gone, in a writer proven sound, on a table with no remaining bad rows. **Not worth it.** Revisit only if the canary below ever fires.

**Ship 1 — close the OKF coverage gap.** Every existing OKF test sets `source_ref: None`, so the JSON round-trip has **zero** coverage on a path that `bundle_apply.rs:374` writes straight to the DB. Add a round-trip test with a realistic evidence blob plus the adversarial quote matrix above. This is the highest-value artifact from the investigation: it is cheap, and it pins a path that is one refactor away from actually causing this corruption.

**Ship 2 — startup canary.** One query after `migrate()` in `src-tauri/src/db/connection.rs`:

```sql
SELECT COUNT(*) FROM llm_wiki_entries
 WHERE source_ref IS NOT NULL
   AND substr(source_ref, 1, 1) = '{'
   AND NOT json_valid(source_ref);
```

Emit a single `tracing::warn!` with the count when non-zero. `json_valid` availability is already proven at runtime by `tools/src/tier_backfill.rs:62`. The leading-`{` test avoids flagging legitimate plain-path refs.

**Ship 3 — `tier_backfill` visibility.** `tier_backfill.rs:62` already excludes malformed rows from `plan_backfill`, which is correct, but silently. Log the excluded count once per `apply_backfill` run. One line.

### Deferred, with rationale

Flipping `source_ref_is_still_grounded` (`src-tauri/src/db/commit.rs:270`) from `return true` to `return false` on parse error. The asymmetry is real — the non-JSON branch correctly returns `false` on `QueryReturnedNoRows` (`commit.rs:252-262`), while the JSON branch returns `true` on a parse failure even though the leading-`{` test at `:251` has *already* classified the row as JSON-shaped. But with no malformed rows in existence the flip changes nothing observable, and it is a deliberate PR #99 contract change that would require re-pinning the D3 test. Do it only alongside PR 2's heal work, where the "is the source still real?" question is already open.

---

## PR 5 — #125: TOCTOU in `create_parents_no_symlink`

The issue body is the literal word `placeholder`; scope is reconstructed from the in-source comment.

The race is real. `create_parents_no_symlink` (`src-tauri/src/okf/write.rs:142-169`) stats each component with `symlink_metadata` then, on `NotFound`, calls `create_dir` — a window in which the entry can be replaced by a symlink that `create_dir` follows. The doc comment at `:137-141` already names the window, the fix shape (`mkdirat(_, O_NOFOLLOW)`), the comparison to `create_dir_all`, and the threat-model qualifier.

**Recommendation: close as accepted risk, doc-only.**

- One caller (`write_note`, `okf/write.rs:252`), and its `path` is shape-vetted (relative, no `..`, no NUL, no dot-enders) before reaching the helper.
- Round-two `safe_vault_path` (`:254-255`) refuses the write even if the race is won, so **no file is ever placed outside the vault**. The residue is an empty directory at the symlink target.
- Exploitation requires local write access inside the vault, which already affords simpler escapes. Single-user desktop app, not multi-tenant.

A truly race-free fix needs `openat(O_DIRECTORY|O_NOFOLLOW)` + `mkdirat` per component — a `libc`/`rustix` dependency plus `unsafe`, ~60-80 LOC, and a fork/thread race test to be verifiable. **M effort for no reachable exposure.**

If the team prefers the code fix, gate it `#[cfg(unix)]` and leave the non-Unix body as-is; the fix must ship with the race test or it is unverifiable.

**Before actioning:** the issue's author never recorded intent. Confirm the above matches what was meant before closing.

---

## Cross-cutting notes

- **Merge style:** regular merges only on this repo (`gh pr merge --merge`); never `--squash` or `--rebase`.
- **CI:** confirm `mergeStateStatus` and check-runs on the tip SHA before calling any PR ready — a `CONFLICTING` PR runs zero checks silently.
- **`wiki_forget` vs `edge_purge`:** PR 1's sweep must use `edge_purge` (emits outbox deletes for entity-anchored edges). PR 3's CLI uses `wiki_forget` (entry-keyed). They are not interchangeable — see `SOURCE_ALIVE_SQL`, `edge_purge.rs:50-56`.
- **Issue hygiene:** #158's body needs rewriting before assignment (see Gap A); #162 should be closed with the findings in PR 4 recorded on it; #125 needs author confirmation.
