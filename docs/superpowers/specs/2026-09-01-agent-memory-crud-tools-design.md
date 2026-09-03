# Spec: Agent Memory CRUD Tools (scoped to ISF `immutable-source-files/agents/**`)

Baseline: v1.40.1 (`1e13ae7`, main). Scope: **Curated Thoughts codebase only** —
general-purpose MCP tools letting an agent perform full CRUD on its own memory
namespace inside an immutable source archive, without filesystem-level access.
Motivating deployment (Equational Applications' agent memory) is configured in
that project's private vault and referenced here only as context. Docs ride the
implementing PR (Kurt directive, Aug 31 2026).

## 1. Executive Summary & Problem Context

CT's MCP write path (PR #102) gave agents Create/Update over vault notes. The
read side is search-only, and Delete does not exist. An agent whose memory
lives under the existing source-archive deposit root
`immutable-source-files/agents` (the `AGENTS_DEPOSIT_DIR` constant in
`src-tauri/src/vault/safe_path.rs`; hereafter the **agent tier**, written
`immutable-source-files/agents/**`) therefore cannot fully manage its own
memory through the MCP contract:

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
paths under `immutable-source-files/agents/**` ONLY — the exact string bound
to the `AGENTS_DEPOSIT_DIR` constant in `safe_path.rs` (implementers must
reference the constant, not re-type the literal). All other paths —
`documents/`, `wiki/`, any location outside the agent tier — are hard-denied
at the tool boundary with a structured error (`path_outside_agent_tier`),
before any filesystem or DB work.

The deny rule is a **full-prefix match on the canonical path**, not a
first-component check: the path is rejected unless the canonical path equals
the agent tier root or begins with `AGENTS_DEPOSIT_DIR` followed by a path
separator. This is what prevents a sibling like
`immutable-source-files/agents-extra/x.md` (or any other
`immutable-source-files/*` location) from slipping through a prefix-string
comparison. Matching happens on the canonicalized path AFTER `safe_vault_path`
(no traversal, symlink, or prefix-confusion bypass), and the same
full-prefix rule is re-checked at every layer that guards a write.

### A.2 Config surface

The agent tier root is a config constant (default: the `AGENTS_DEPOSIT_DIR`
value, `immutable-source-files/agents/`), single place, so deployments naming
their tier differently configure once. Read-only tools
(`vault_semantic_search`, `vault_related_chunks`, wiki_*) are unchanged —
they already read the whole indexed corpus.

### A.3 Migration flag semantics

`enforce_agent_tier_scope` is a persisted config setting with the following
defined behavior:

- **Missing setting → upgraded vault → defaults to OFF.** A vault whose
  config predates the flag is treated as upgraded/legacy: the flag resolves
  to `false`, so pre-existing write behavior (notably writes under `wiki/`)
  keeps working, and a one-time load diagnostic is logged noting the default
  was applied because the setting was absent.
- **Fresh installs → written as `true`.** New config creation persists
  `enforce_agent_tier_scope: true` explicitly; the guard is on from the
  first write.
- **Scope of the bypass.** When the flag is OFF, the bypass applies ONLY to
  `vault_write_note` and `vault_upsert_index_entry`. `vault_delete_note` and
  `vault_read_note` are ALWAYS tier-scoped to `immutable-source-files/agents/**`
  regardless of the flag — legacy flag-off vaults gain read/delete over the
  agent tier only, never over `wiki/` or other locations.

### A.4 Migration note

The primary legacy prefix the flag protects is `wiki/`: `vault_write_note`'s
allowlist (`NOTE_WRITABLE_SUBDIRS` in `safe_path.rs`) is currently
`[wiki, immutable-source-files/agents]`, so existing agents may have written
notes under `wiki/` and those flows must keep working when the guard ships
behind the flag (per A.3). No legitimate historical path silently breaks.

## 3. Part B — `vault_read_note`

Params: `path` (vault-relative, must satisfy A.1).

**Normative response shape (single contract):**

```json
{
  "path": "immutable-source-files/agents/<note>.md",
  "frontmatter": { "...parsed OKF v0.1 frontmatter object..." },
  "body": "<markdown body, verbatim>",
  "ingestion": {
    "chunk_count": 12,
    "last_indexed_at": "2026-09-01T12:00:00Z"
  }
}
```

- The response is always **file content + ingestion state** (this resolves the
  former open question; file-only responses are not offered).
- `chunk_count` and `last_indexed_at` are JSON `null` when unavailable — never
  `0`, never omitted. A missing/unparseable field must not masquerade as an
  empty index.
- **Aggregation when multiple DB rows map to one note** (a note chunked into
  several `documents`/`chunks` rows): `chunk_count` = `COUNT` of chunk rows
  whose source path equals the note path; `last_indexed_at` = `MAX(indexed
  timestamp)` over those rows. **No matching rows → both fields `null`.**
- Missing file → structured `not_found`, never an error-class failure.

Read-by-path complements search-based recall; it is the primitive CRUD expects.

## 4. Part C — `vault_delete_note`

### C.1 Guards

Reuse the write path's protections, adapted:

- `safe_vault_path` canonical check; tier guard from Part A applies.
- **Provenance marker (fail-closed delete).** Existing `OkfFrontmatter` has no
  provenance field and any note can carry valid OKF fields, so "is
  agent-written" is otherwise unprovable. `vault_write_note` therefore writes
  an **immutable, schema-versioned provenance marker** into every tool-created
  note:

  ```yaml
  tool_provenance:
    writer: vault_write_note
    schema: 1
  ```

  **Schema preservation is a prerequisite, not an afterthought.** The current
  bound `crate::okf::OkfFrontmatter` (src-tauri/src/okf/mod.rs) has NO
  `tool_provenance` field, and `render_frontmatter` emits only its declared
  fields while `parse_frontmatter` (serde_yaml) silently discards unknown
  keys — so a parse→render round-trip on any update would erase the marker
  today. The implementation MUST add `tool_provenance: Option<ToolProvenance>`
  to `OkfFrontmatter` and teach `render_frontmatter` to emit it when present
  (preserved-extension mechanism: either a typed field or an explicit
  preserved-keys bag; a typed field is preferred for validation). Update paths
  (`vault_write_note` on an existing note) parse the existing marker, carry it
  through unchanged, and re-render it — an update that would drop or alter the
  marker is an implementation bug, not an acceptable outcome.

  **Forgeability — stated plainly.** Frontmatter values are client-supplied.
  The marker is NOT cryptographically unforgeable: a hand-authored note can
  copy `writer: vault_write_note, schema: 1` verbatim, and nothing in the
  current design distinguishes that from a tool-written note. The marker is an
  **integrity/origin contract for tool flows** — it makes accidental marker
  loss and tool-flow mistakes detectable and gives `vault_delete_note` a
  deterministic fail-closed gate — **not an authentication boundary** and not
  a defense against a malicious author who deliberately forges it. Authenticated
  provenance (HMAC over the marker keyed outside the note, OS-level
  attestation, or a DB-backed authorship record joined at delete time) is
  explicitly OUT OF SCOPE for this spec and tracked as open question Q3;
  implementers must not present the marker as tamper-proof in docs or error
  copy.

  `vault_delete_note` requires an exact, valid marker — a missing or altered
  marker (wrong `writer`, unknown `schema`, or any mutation) → structured
  refusal. The `safe_vault_path` canonical check and OKF validation still
  apply on top; the marker is an additional gate, not a replacement.
- Authorization token = If-Match on `updated_at` OR explicit `confirm: true`
  parameter (racing-update protection).

### C.2 Deletion protocol (live-safe, no cross-store transaction)

A single transaction across both stores is impossible: DB cleanup is
transactional, filesystem removal is not and cannot join the DB transaction.
The protocol is therefore rename-based and safe against a live indexer:

1. **Rename before commit.** Atomically RENAME the note to a pending-deletion
   marker name in the SAME directory (e.g. `<name>.md →
   <name>.md.pending-delete`), or to an excluded trash location — BEFORE the
   DB transaction commits. The rename is the crash-safe point: after it, the
   note is already invisible to writers.
2. **Indexer exclusion.** `PipelineJob::Ingest` and the startup
   reconciliation walk MUST skip/ignore pending-deletion markers, so a deleted
   note cannot be re-indexed between rename and commit (or after a crash
   mid-protocol).
3. **DB transaction.** The DB transaction then removes the `documents`/
   `chunks` rows plus `llm_wiki_source_ref_index` rows, applies
   soft-delete/`wiki_forget` semantics to any `llm_wiki_entries` rows sourced
   from the note, and enqueues an outbox `Delete` op per the PR #132 pattern,
   then commits. Search and graph stop surfacing ghost content without
   waiting on librarian regeneration.
4. **Filesystem unlink after commit — sweeper recovery rules.** Unlink of the
   marker happens after the DB commit. If unlink fails, retry is idempotent:
   the marker stays on disk, reconciliation continues to ignore it, and a
   later sweep retries the unlink. Crash at any point leaves either a pending
   marker (harmless, swept later) or a completed deletion — never a ghost
   index entry pointing at live content.
5. **Crash-before-commit recovery (sweeper precondition).** The sweeper MUST
   NOT unlink a pending-deletion marker unless DB cleanup is verified
   complete. Unconditionally unlinking after a crash that rolled back (or
   never reached) the DB commit would destroy the only remaining copy of the
   note's content while stale `documents`/`chunks` rows survive forever.
   Before unlinking a marker, the sweeper MUST:
   1. Query for `documents`/`chunks` rows (and `llm_wiki_source_ref_index`
      rows) whose source path equals the note path.
   2. If none exist → DB cleanup is confirmed complete; unlink the marker.
   3. If rows remain → re-run the C.2 step-3 DB cleanup first, then unlink.
   4. If the DB cleanup cannot be completed (transaction fails again, DB
      unavailable) → RESTORE the marker to its original filename (rename
      `<name>.md.pending-delete → <name>.md`, undoing the C.2 step-1 rename)
      instead of unlinking. Restoring the live filename lets indexing and
      writers resume normally; the alternative — unlinking with rows still
      present — permanently orphans the index. The restore is attempted
      idempotently on later sweeps if the live path was recreated
      concurrently (in that case the stale marker copy is discarded, since
      the live file governs).

### C.2a Deletion serialization with writers and ingestion (pending-deletion guard)

Rename-before-commit alone does not serialize deletion against in-flight
writers and ingest jobs. Two real races exist in the current code:

- `vault_write_note` reads the existing file and checks `updated_at`
  (If-Match staleness) BEFORE `safe_write_bytes` publishes the new content
  via rename. If `vault_delete_note` renames the file to its pending-deletion
  marker between those steps, the write either clobbers the marker (deleting
  newer content) or recreates the live path after deletion has begun.
- `PipelineJob::Ingest` reads and hashes the file, then commits `documents`/
  `chunks` rows. If the read happens before the deletion rename and the row
  commit after the deletion DB transaction, the job resurrects index rows for
  a deleted note. Skipping pending-deletion markers during the walk does not
  stop a job that is already running.

Both are closed by ONE mechanism: a **per-note pending-deletion lock record**
— a `pending_deletions` row in a small dedicated table, keyed by note path,
committed in its own DB transaction BEFORE the C.2 step-1 rename and deleted
only when the deletion fully completes (DB cleanup committed AND marker
unlinked/swept). The atomic rename to `<name>.md.pending-delete` remains the
filesystem half of the protocol; the row is the serialization point every
writer and ingest job checks.

Required behavior:

- `vault_write_note` MUST re-check the pending-deletion record for its target
  path **immediately before publishing** (after the staleness check, as the
  last step before `safe_write_bytes`/rename). Record present → the write is
  refused with a structured staleness/conflict error (`note_pending_deletion`),
  never a silent clobber and never a recreation of the live path.
- `PipelineJob::Ingest` MUST re-check the record for its note path
  **before committing rows** (after read+hash, before the `documents`/`chunks`
  insert transaction). Record present → the job DISCARDS its rows and logs the
  skip; it must not insert, not upsert, and not error the worker.
- `vault_delete_note` creates the record (committed) before starting C.2 and
  it persists through the protocol until completion, so any writer or job
  that races the deletion loses deterministically (deletion wins).
- A writer that passed its pre-publish check cannot race the record's
  creation afterward: the record is committed before the deletion rename, so
  a `vault_write_note` whose staleness pre-check passed on the live file
  re-checks the record as the last step before publish — and if the record
  appeared between its staleness read and that re-check, the re-check sees it
  (same DB, read committed after the record's commit). If an implementation
  cannot sequence the re-check this way on its platform, it must hold the
  record's row lock (`SELECT … FOR UPDATE`-equivalent, or a SQLite immediate
  transaction) across its publish step. The same serialization applies to
  the ingest commit.

### C.3 Conventions kept out of the tool

Index/README line removal (the deposit convention's two-step) remains the
calling agent's responsibility — the tool deletes the note, not its index
mentions.

## 5. Validation / Acceptance Criteria

1. **AC1** `vault_read_note` on a tool-written note returns the normative
   response shape (frontmatter + body matching disk, plus ingestion state);
   missing path → `not_found`; a note with no DB rows returns
   `chunk_count: null` and `last_indexed_at: null` (not 0, not omitted).
2. **AC2** `vault_delete_note` with If-Match removes the file and, in the
   same operation, the DB rows; subsequent `vault_semantic_search` for the
   note's content returns nothing (verified live, not by code reading).
3. **AC3** Two distinct cases, both verified:
   - **Case A (stale token):** If-Match present but stale AND `confirm` not
     true → structured staleness error (resource_version conflict; write-path
     If-Match symmetry).
   - **Case B (no token):** If-Match absent AND `confirm` not true →
     confirmation-required error (a different structured error than Case A).
4. **AC4** Every Part-A-guarded tool rejects `documents/x.md` and
   `../documents/x.md` and a symlinked escape with `path_outside_agent_tier`;
   the guard is a full-prefix match on the canonical path (a sibling prefix
   such as `immutable-source-files/agents-extra/x.md` is also rejected).
   **Race test:** a parent directory swapped to an outside symlink BETWEEN
   validation and use does not escape the vault — the use-time re-check (see
   5a) fails closed.
5. **AC5** Non-OKF file under `immutable-source-files/agents/` → delete
   refused; a note with a missing or altered provenance marker → delete
   refused (fail-closed).
6. **AC6** `enforce_agent_tier_scope=false` restores pre-guard write behavior
   for `vault_write_note`/`vault_upsert_index_entry` (legacy `wiki/` brains)
   while `vault_delete_note`/`vault_read_note` remain tier-scoped. A config
   missing the setting resolves to OFF with a one-time load diagnostic; fresh
   config creation writes it as `true`.
7. **AC7** Full CT test suite green (cargo test --features test-utils, full
   incl tests/); PR #102's write-path tests pass unchanged or explicitly
   migrated.
8. **AC8** Deletion serialization (C.2a): a `vault_write_note` that races
   `vault_delete_note` and reaches its publish step after the
   pending-deletion record exists is refused with `note_pending_deletion`
   (no clobber, no recreation of the live path); a `PipelineJob::Ingest`
   that read the file before the deletion rename but commits rows after the
   record exists discards its rows and logs the skip; both verified with an
   injected delay/fault between the pre-check and the publish/commit step.
9. **AC9** Sweeper recovery (C.2 step 5): with a pending-deletion marker on
   disk and the deletion DB transaction rolled back (simulated crash), the
   sweeper does NOT unlink; it re-runs DB cleanup and then unlinks — and if
   the DB transaction still cannot complete, it restores the marker to the
   original filename instead. The never-allowed outcome (marker unlinked
   while `documents`/`chunks` rows for the note path remain) is asserted.

## 5a. Use-time containment re-verification (all guarded operations)

`safe_vault_path` validates up-front, but `safe_write_bytes`,
`safe_copy_file`, `read_to_string`, and `remove_file` are path-based and can
be redirected if a parent directory is swapped to an outside symlink AFTER
validation (TOCTOU). `O_NOFOLLOW` alone is insufficient: it protects only the
FINAL path component — any intermediate (parent) component can still be a
symlink that redirects the whole path. Every guarded operation MUST therefore
re-verify containment at use time using **descriptor-relative (openat-style)
semantics**, concretely:

1. **Root anchor.** Open the vault root ONCE as a directory file descriptor
   (`open(root, O_RDONLY | O_DIRECTORY)`), resolve it to its canonical form,
   and `fstat` it, recording `(st_dev, st_ino)`. This fd — not the path
   string — is the trust anchor for the operation.
2. **Component-wise descent.** Resolve the vault-relative target by walking
   components with `openat(dirfd, component, O_RDONLY | O_DIRECTORY |
   O_NOFOLLOW)` for directories and `openat(dirfd, final_component,
   O_NOFOLLOW | <mode flags>)` for the final entry. `O_NOFOLLOW` on EVERY
   component means a swapped-in symlink fails the open (`ELOOP`) instead of
   silently redirecting the path outside the vault. `O_CREAT` is permitted
   only on the final component and only when the caller's intent is create.
3. **Identity confirmation.** After the open, `fstat` the resulting fd and
   confirm its `(st_dev, st_ino)` matches the pre-validation `stat` of the
   intended target (captured when `safe_vault_path` ran). A mismatch means
   the path was swapped between validation and use → fail closed. All
   subsequent read/write/unlink/rename of the target MUST go through this
   descriptor or `*at()` calls relative to its directory fd (`fstatat`,
   `unlinkat`, `renameat`) — never by re-deriving a path string and calling
   the path-based API.
4. **Fail closed.** If `openat`/`O_NOFOLLOW`/`*at()` primitives are
   unavailable on the platform (non-Unix fallback), the required equivalent
   is: open the target through the vault root by normalized relative path
   WITHOUT following any final-component symlink, then `fstat` the fd and
   compare `(st_dev, st_ino)` against the pre-validation stat as in step 3;
   if even that cannot be guaranteed (no fd-capable filesystem APIs), the
   operation MUST be refused — the platform fallback is refusal, not
   path-based best effort.

The up-front `safe_vault_path` check remains, but it is no longer the only
line of defense.

## 6. Open Questions (for Kurt — answer before or at plan time)

- **Q1:** Soft-delete vs hard-delete for the vault file: move to a
  `.trash/`-style location inside the vault (recommended — recoverable,
  consistent with brain.db backup posture) vs unlink?
- **Q2:** Name check: `vault_read_note` / `vault_delete_note` (recommended,
  family-consistent).
- **Q3 (added round 2):** Authenticated provenance for the
  `tool_provenance` marker — the marker as specified is client-supplied and
  forgeable by design (see C.1). If delete authorization must resist a
  deliberately forging author, pick a mechanism: HMAC over the marker keyed
  outside the note, OS-level attestation, or a DB-backed authorship record
  joined at delete time. Until answered, the marker is an integrity/origin
  contract for tool flows only, NOT an authentication boundary.

## 7. Non-Goals

- Bulk/batch operations; recursive folder deletes (one note per call).
- Editing `documents/` or other immutable tiers through any tool.
- MCP exposure of `wiki_forget` (DB-entry tool remains internal/CLI).
- Search tool changes (their corpus behavior is out of scope).
