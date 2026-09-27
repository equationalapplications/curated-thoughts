# Spec: Vault ingestion policy & write-path hardening (records root, allow-list, frontmatter strip, per-folder tiers)

- **Date:** 2026-09-27
- **Status:** Proposed
- **Branch:** `vault-ingest-policy`
- **Priority:** High
- **Requested by:** maintainer (approved independently-reviewed plan, 2026-09-27)
- **Related issues:** #240 (size-drop guard, separate), #241 (delete hygiene — amended)

## Problem (verified evidence, all from live system 2026-09-27)

1. **Every wisdom-tier fact inherits noise from raw YAML.** 378 of the indexed
   chunks in the production vault contain raw frontmatter tokens (`okf_version:`,
   `updated_at:`), because the chunker slices files verbatim including the
   `---` frontmatter block. Both the librarian (fact extraction) and the
   embedder (semantic search) see it. Frontmatter pollution was confirmed in
   the round-2 Opus review (finding m2) as a chunker-level problem, not just
   a librarian-level one.

2. **No structural separation between durable memory and working records.**
   Session records (`people/agent/sessions/`, 69+ files) and operations
   notes (`operations/`, 80 files) are ingested exactly like curated facts.
   In the 2026-09-27 architecture review (briefs in operations/, verdict
   APPROVE WITH NITS → REQUEST CHANGES), the agreed policy is: sessions and
   operations stay *searchable* but must not feed fact extraction; review
   briefs/logs should not be indexed at all. the approved plan moves them
   to a vault-root `records/` tree that is **never ingested**.

3. **The write path accepts any folder.** `vault_write_note` has no
   top-level constraint beyond being inside the vault. A deposit to
   `agents/agent/…` (a retired layout) silently recreated the flat folder
   (2026-09-27 incident, Opus round-1 M1). The choke point needs an
   allow-list so structural mistakes fail loudly instead of silently
   forking the ontology.

4. **No per-folder or per-note escape hatch.** Ingestion policy is
   all-or-nothing per file extension. There is no way to say "index this
   folder's chunks but never extract facts" (`chunks-only`) or "skip this
   note's fact extraction" (`wisdom: false`) without moving the file.

## Approach

### Feature 1 — writable never-ingested `records/` root

- `<vault>/records/` is a **writable** location for `vault_write_note`
  (new allowed root alongside `immutable-source-files/agents/` and `wiki/`).
- The watcher/ingest pipeline **excludes** `records/` entirely: no
  document rows, no chunks, no embeddings, no fact extraction.
- Intended subfolders (documented; not enforced): `records/sessions/`,
  `records/operations/`, `records/archive/`.
- `wiki_context` / search tools simply never return `records/` content
  (it is absent from the DB, not filtered at query time).

### Feature 2 — write-path top-level allow-list

- `vault_write_note` rejects any target whose first path segment is not in:
  `immutable-source-files`, `records`, `wiki` (plus existing internal
  exclusions like `.brain`).
- Error message names the allowed roots (machine-parseable, per the
  existing MCP error conventions used by `PathOutsideVault`).
- `immutable-source-files` writes remain constrained to
  `immutable-source-files/agents/**` (current behavior preserved).

### Feature 3 — frontmatter stripped at the chunker

- During chunking, parse leading YAML frontmatter (`^---\n…\n---`) and
  exclude it from all emitted chunks. Metadata still flows to the indexer
  through the existing structured path (it already reads frontmatter
  separately for OKF fields).
  - Fence parsing MUST accept LF and CRLF line endings (normalize before
    matching) so CRLF files cannot silently leak YAML into chunks (dedicated
    CRLF test required).
  - Emitted chunk spans stay SOURCE-file lines (the librarian cites them):
    the stripped fence's line count is added back to every chunk, and the
    pass-2 reference extractor scans the same stripped body at the same
    offset, so frontmatter can never yield call-site chunks.
- One-time reindex note: existing chunks keep their old text until their
  document is re-indexed (hash-gated, standard behavior). No forced
  migration in this PR; a maintenance reindex is a follow-up ops task.

### Feature 4 — per-folder ingest tiers + `wisdom: false` note override

- New config block in `~/.brain/config.json`:
  `"ingest": { "folder_tiers": { "<vault-relative-prefix>": "full" |
  "chunks-only" | "none" } }` (absent = `full`). Resolution: longest
  matching prefix wins, matched at PATH-COMPONENT boundaries only — a tier
  for `ops` never matches `ops-archive` (sibling-prefix test required).
  Prefixes are anchored at the VAULT ROOT: absolute paths are relativized
  against the vault root first, so neither the vault's own ancestor folders
  nor a same-named folder nested deeper in the vault can match; a path that
  cannot be placed in the vault resolves to `full`. `\` and `/` separators
  are normalized on both the path and the configured keys.
  - `full` — current behavior (chunks + embeddings + librarian facts).
  - `chunks-only` — chunk + embed; librarian skips fact extraction.
  - `none` — do not index at all (equivalent to watcher exclusion). Enforced
    in the ingest pipeline before any document row, chunk, or embedding is
    written, so every ingest entry point (watcher worker, Tauri command,
    `ct ingest`, bulk reindex) honors it — not only the librarian.
- Frontmatter override on any note: `wisdom: false` → librarian skips
  fact extraction for that document regardless of folder tier.
- Tiers apply to newly indexed/re-indexed documents; changing a tier does
  not trigger a sweep. A document in a folder newly tiered `none` has its
  row removed (with deletion provenance) the next time it is ingested;
  operators wanting it gone immediately reindex.
- The policy is read from the config of the brain that owns the database
  and cached per config file, re-parsed only when its mtime/length changes,
  so hand-edits take effect on the next document without a per-file parse.
- `wisdom: false` detection accepts the same opener as the chunker's fence
  strip (an optional UTF-8 BOM before `---`).

## Rejected alternatives

- **Keep sessions/operations where they are and filter at query time** —
  rejected: fact extraction would still burn tokens on raw material, and
  the pollution would remain one config regression away from resurfacing.
- **Strip frontmatter only in the librarian** — rejected by evidence:
  378 polluted chunks are *embedded*; semantic search itself returns
  frontmatter noise today, so the fix must live at the chunker.
- **Globally disable operation-folder indexing via `.gitignore`** —
  rejected: `.gitignore` is not an ingest contract, the vault backup cron
  relies on these files being tracked, and per-folder tiers make the
  policy explicit and machine-readable.

## Open questions

1. Should `records/` exclusion also apply to `vault-related-files` style
   diagnostics (e.g. `vault_stats`), or only to ingestion? (Lean: exclude
   everywhere for consistency.)
2. For `chunks-only`, should the librarian skip at *proposal* creation or
   at *commit*? (Lean: skip at proposal creation — cheaper.)

## Implementation plan (summary; full plan in PR)

1. Ingest exclusion: extend `walk_vault` / watcher exclude set with
   `records/` (test: file created under `records/` never appears in
   `documents`).
2. Write allow-list: `tool_dispatch` write-path validation
   (tests: allowed roots succeed; `agents/agent/…` → error naming roots).
3. Chunker: frontmatter parse-and-skip (tests: file with frontmatter
   yields zero chunks containing `okf_version:`; metadata still parsed).
4. Tiers: config plumbing + librarian skip (tests: folder tier matrix ×
   `wisdom: false` override).
5. Migration safety: none required for existing rows (hash-gated reindex);
   document the one-time reindex recommendation in the PR body.
