# Spec: Agent Memory CRUD Tools (scoped to ISF `agents/`)

Baseline: v1.40.1 (`1e13ae7`, main). Scope: **Curated Thoughts codebase only** —
general-purpose MCP tools letting an agent perform full CRUD on its own memory
namespace inside an immutable source archive, without filesystem-level access.
Motivating deployment (Equational Applications' agent memory) is configured in
that project's private vault and referenced here only as context. Docs ride the
implementing PR (Kurt directive, Aug 31 2026).

## 1. Executive Summary & Problem Context

CT's MCP write path (PR #102) gave agents Create/Update over vault notes. The
read side is search-only, and Delete does not exist. An agent whose memory
lives under a dedicated source-archive folder (hereafter the agent tier,
`agents/`) therefore cannot fully manage its own memory through the MCP
contract:

- **C/U — exists.** `vault_write_note` (create + If-Match update, OKF v0.1
  validation, `safe_vault_path`), `vault_upsert_index_entry`.
- **R — search-only.** `vault_semantic_search` / `vault_related_chunks` find
  content but cannot return a known note by path. No `vault_read_note`.
- **D — missing.** No MCP tool deletes a vault note. `wiki_forget` (PR #135)
  is DB-entry-level, not MCP-exposed, and never touches vault files. Confirmed
  absent from v1.40.1's 7 exposed tools.

The immutable tiers of a source archive (e.g. human-reconciled records,
third-party documents) must stay **structurally unreachable** by these tools —
the scope guard is the safety property, not a convention.

## 2. Part A — Tier scope guard (all memory-write tools)

### A.1 Allow/deny rule

MCP tools that mutate the vault (existing `vault_write_note`,
`vault_upsert_index_entry`; new tools in Parts B/C) accept vault-relative
paths under `agents/**` ONLY. All other prefixes — `documents/`, any
top-level tier outside `agents/` — are hard-denied at the tool boundary with
a structured error (`path_outside_agent_tier`), before any filesystem or DB
work. Deny-list is evaluated on the canonicalized path AFTER
`safe_vault_path` (no traversal, symlink, or prefix-confusion bypass).

### A.2 Config surface

The agent tier root is a config constant (default `agents/`), single place,
so deployments naming their tier differently configure once. Read-only tools
(`vault_semantic_search`, `vault_related_chunks`, wiki_*) are unchanged —
they already read the whole indexed corpus.

### A.3 Migration note

Existing deployments whose agents wrote under other prefixes via
`vault_write_note` (e.g. flat pre-nesting deposits) must be handled
explicitly: the guard ships behind a config flag (`enforce_agent_tier_scope`,
default ON for fresh installs; documented migration step for existing
brains) so no legitimate historical path silently breaks.

## 3. Part B — `vault_read_note`

Params: `path` (vault-relative, must satisfy A.1). Returns the note's OKF
frontmatter (parsed object) + body (markdown) + the DB-side echo where
available (chunk count, last indexed timestamp) so the caller can see both
the file and its ingestion state in one call. Missing file → structured
`not_found`, never an error-class failure. Read-by-path complements
search-based recall; it is the primitive CRUD expects.

## 4. Part C — `vault_delete_note`

### C.1 Guards

Reuse the write path's protections, adapted: `safe_vault_path` canonical
check; target must carry valid OKF frontmatter (tool-written notes only —
refuses to delete files the tool family didn't write, fail-closed);
authorization token = If-Match on `updated_at` OR explicit `confirm: true`
parameter (racing-update protection). Tier guard from Part A applies.

### C.2 Atomic dual-store cleanup

Deletion is one transaction across both stores: remove the vault file AND
purge the DB side — chunks, embeddings, `llm_wiki_source_ref_index` rows,
soft-delete/`wiki_forget` semantics for any `llm_wiki_entries` rows sourced
from the note, and an outbox `Delete` op per the PR #132 pattern — so search
and graph stop surfacing ghost content without waiting on librarian
regeneration. Filesystem removal happens only after the DB transaction
commits (crash between the two leaves an orphan file, never a ghost index
entry; startup reconciliation sweeps orphan files under the tier).

### C.3 Conventions kept out of the tool

Index/README line removal (the deposit convention's two-step) remains the
calling agent's responsibility — the tool deletes the note, not its index
mentions.

## 5. Validation / Acceptance Criteria

1. **AC1** `vault_read_note` on a tool-written note returns frontmatter +
   body matching disk; missing path → `not_found`.
2. **AC2** `vault_delete_note` with If-Match removes the file and, in the
   same operation, the DB rows; subsequent `vault_semantic_search` for the
   note's content returns nothing (verified live, not by code reading).
3. **AC3** Delete without If-Match/confirm on a stale token → structured
   staleness error (write-path If-Match symmetry).
4. **AC4** Every Part-A-guarded tool rejects `documents/x.md` and
   `../documents/x.md` and a symlinked escape with `path_outside_agent_tier`.
5. **AC5** Non-OKF file under `agents/` → delete refused (fail-closed).
6. **AC6** `enforce_agent_tier_scope=false` restores pre-guard write behavior
   (legacy brains) while Delete/Read remain tier-scoped.
7. **AC7** Full CT test suite green (cargo test --features test-utils, full
   incl tests/); PR #102's write-path tests pass unchanged or explicitly
   migrated.

## 6. Open Questions (for Kurt — answer before or at plan time)

- **Q1:** Soft-delete vs hard-delete for the vault file: move to a
  `.trash/`-style location inside the vault (recommended — recoverable,
  consistent with brain.db backup posture) vs unlink?
- **Q2:** Should `vault_read_note` echo DB ingestion state (recommended) or
  stay file-only?
- **Q3:** Name check: `vault_read_note` / `vault_delete_note` (recommended,
  family-consistent).

## 7. Non-Goals

- Bulk/batch operations; recursive folder deletes (one note per call).
- Editing `documents/` or other immutable tiers through any tool.
- MCP exposure of `wiki_forget` (DB-entry tool remains internal/CLI).
- Search tool changes (their corpus behavior is out of scope).
