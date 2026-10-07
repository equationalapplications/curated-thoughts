# `ct wisdom match` (issue #265) — Step-0 investigation

**Date:** 2026-10-06 · **Status:** investigation (pre-spec) · **Base:** `origin/main` @ `9c2281b` (3.2.0)
**Requester:** curated-thoughts-integrations (CTI) live wisdom delivery —
CTI spec `docs/superpowers/specs/2026-10-06-intuitive-wisdom-live-delivery-design.md`
§"CT prerequisite" (branch `feat/intuitive-wisdom-live-delivery`).

Tags: **[V]** = verified this session by reading the cited source; **[A]** =
assumption the spec or plan must close.

## Q1 — Can today's `ct recall` serve a per-turn relevance gate?

No.

- **[V]** `recall_cmd` (`tools/src/queries.rs:431-459`) returns exit 2
  (`EXIT_NO_RESULTS`) when the chunk leg is empty, before the wiki leg runs.
- **[V]** Its wiki leg `rank_wiki_entries` (`tools/src/queries.rs:166-243`) is
  lexical: every whitespace token of length ≥ 2 is matched with `title LIKE
  '%t%' OR body LIKE '%t%'`; rank = matching-term count, then confidence, then
  `updated_at`. No stopwords, no score, no threshold, fixed limit 5. It filters
  `deleted_at IS NULL` only — superseded rows are returned.
- `--k` sizes the chunk leg only.

## Q2 — Is there semantic search over wiki entries already?

Yes.

- **[V]** `wiki_graph::wiki_search` (`src-tauri/src/wiki_graph.rs:278-344`):
  cosine over `llm_wiki_entries.embedding_blob` (little-endian f32), skipping
  rows whose blob length ≠ `query_dim * 4`; `raw <= 0.0` dropped; score =
  `raw * tier_weight(entity_id)` (`:216-223`: `tier_fact` 1.5, `tier_wisdom`
  1.0, `tier_working::*` 0.6, else 1.0); sorted desc; limit clamped 1..25.
  Filters `deleted_at IS NULL AND embedding_blob IS NOT NULL`, optional
  entity-id and tier filters. **No supersession filter, no floor.**
- **[V]** Served to agents as MCP `wiki_search` / `wiki_context`
  (`src-tauri/src/mcp_server.rs:55-74`, `tool_dispatch.rs:198-232`).
- **[V]** Blobs are filled at write time and by `embed_sweep`
  (`src-tauri/src/embed_sweep.rs:1-60`); embed text =
  `"{title}\n\n{body}"` (`embed_text_for_entry`).
- **[V]** `wiki_graph`, `embedder`, `retrieval` are `pub mod` in
  `src-tauri/src/lib.rs:9,28,39`, so the `tools` crate (which already uses
  `tauri_app_lib::…`, `tools/src/cmds.rs:38-42`) can call them.
- **[V]** Query embedding in the CLI: `recall_chunks` → `embed_one(profile, query)`
  (`tools/src/queries.rs:307-330`, `src-tauri/src/embedder/mod.rs:207`), profile
  from `retrieval::load_embed_profile(config_path)` (`queries.rs:404-406`).

## Q3 — Embed profile identity (for a per-model floor)

- **[V]** `EmbedProfile` (`src-tauri/src/embedder/mod.rs:147-170`) is
  `Local{model} | Cloud{provider, model, api_key} | External{base_url, model,
  api_key}`; default `Local{"nomic-embed-code"}`.
- **[V]** `CURATED_EMBED_STUB=constant8` makes embedding return small
  deterministic vectors (`embedder/mod.rs:172-…`) — used by pipeline and CLI
  tests; benches use frozen FastEmbed 384-d vectors instead
  (`docs/benchmarks/README.md`).
- **[V]** Nothing records which model produced a given `embedding_blob`; only
  the dimension check protects against a profile change. **[A]** A model swap
  that keeps the dimension yields meaningless scores until re-embed — a
  pre-existing limitation of `wiki_search` too; carried to the spec as a known
  limitation, not fixed here.

## Q4 — Wiki-level supersession

- **[V]** V24 adds `valid_from`, `valid_to`, `superseded_by`, `superseded_at`
  to `llm_wiki_entries` (`src-tauri/src/db/connection.rs:721-830`, mirroring
  core-llm-wiki 7.9.0 engine migration 13), plus partial index
  `llm_wiki_entries_superseded_idx (entity_id, superseded_by) WHERE
  superseded_by IS NOT NULL` (`src-tauri/src/db/okf_ddl.rs:250-270`).
- **[V]** Writer: core-llm-wiki 7.9.0 `supersedeInTx`
  (`dist/chunk-J2TWKOEE.mjs:3787-3803`): old row gets `valid_to = t`,
  `superseded_by = newId`, `superseded_at = now` (epoch **ms**, `Date.now()`);
  rejects cross-entity, already-superseded, `immutable_document` targets and
  cycles; chain depth bound `HISTORY_MAX_DEPTH = 100` (`:3773`).
- **[V]** No Rust code reads or writes `superseded_by` (git grep: only DDL,
  schema guard, drop paths). CT INTENT lists "supersession/current-only
  filtering in Rust recall (migration-13 columns)" as decided, not built.
- **[V]** Deposit-level supersession is a separate concept:
  `okf::Frontmatter.supersedes` = vault-relative path of the superseded
  *deposit* (`src-tauri/src/okf/mod.rs:44-46`). **[A]** Whether the Active
  Librarian turns a supersession deposit into an engine `supersede` call is
  not established here; CT INTENT rule 4 says it applies them (decided). The
  spec therefore only *reads* `superseded_by`; populating it is a dependency.

## Q5 — Provenance data available today

- **[V]** `llm_wiki_entries.source_type` values written in Rust:
  `librarian_inferred` (default in DDL), `user_stated`, `user_confirmed`; the
  engine also knows `immutable_document` (supersede guard above).
- **[V]** `tier` column `fact | wisdom | NULL` (V16, `db/schema.rs:355-367`,
  `VALID_TIERS` `:486-499`).
- **[V]** `human_attestations` (INTENT rule 2) does not exist in code.

## Q6 — Fact ids

- **[V]** Production ids: `generate_llm_id("fact_")` = `fact_` + 24 hex
  (`src-tauri/src/db/commit.rs:448-452`). Matches CTI's
  `^[A-Za-z0-9._:-]{1,128}$`. **[A]** INTENT rule 5 plans deterministic hashed
  ids — still within the charset if hex.

## Q7 — Governing rules (CT INTENT)

- Rule 6: System One is optional; recall works without it → the gate must not
  need it.
- Rule 7: "A weak top hit is treated as 'no answer' … the abstention floor is a
  fixed, benchmark-calibrated threshold."
- Non-goal: "No provenance used as a hidden ranking penalty" → the floor must
  compare raw cosine, not the tier-weighted score.
- Workflow 4: recall changes gate on benchmarks (abstention precision,
  cross-model paraphrase probes n ≥ 200) run on scratch brains with real
  embeddings.
- Rule 8 (work scope) has no implementation anywhere in recall (git grep).

## Decisions taken (owner, 2026-10-06)

1. Per-model calibrated floor (constant table keyed by profile kind + model;
   unknown model abstains).
2. `provenance` = stored `source_type` verbatim when in the known set, else
   `null`.
3. Library function in `wiki_graph` + thin `ct wisdom match` CLI; no MCP tool;
   `ct recall` unchanged.
