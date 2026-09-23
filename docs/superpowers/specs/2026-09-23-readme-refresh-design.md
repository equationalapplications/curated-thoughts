# README Refresh — Design Spec

**Date:** 2026-09-23
**Status:** Approved — rev 2.3 (2026-09-23)
**Type:** Docs-only
**PR:** #224 (`docs/readme-refresh-2026-09`; carries spec + implementation)

## Problem

`README.md` was last substantively updated 2026-09-05 (commit 4d98774), before
v2.5.0 shipped on 2026-09-06. The app has since shipped v2.5.0 → v2.13.0,
including user-facing capabilities that a new user cannot discover from the
README:

| Capability | Shipped | Where |
|---|---|---|
| Agent-memory CRUD MCP tools (`curated_add_wisdom` / `_update_` / `_archive_`) | v2.5.0 | PR #185 |
| Cross-partition wiki-graph traversal (`wiki_traverse_graph` without `entityId`) | v2.7.0 | issue #190, PR #197 |
| Evidence regrade: MIGRATION_V20 gate + manual `ct evidence regrade` recovery | v2.8.0 | issue #186, PR #200 |
| Human-verification gate: `ct proposals review`, `curated_proposals_list` / `curated_proposal_decide` | v2.9.0 | PR #201 |
| `.brain/` excluded from the vault walk and watcher | v2.10.0 | PR #205 |
| Deleted-source provenance + Review-desk markers (V23) | v2.11.0 | issue #211, PR #214 |
| Vault switch clears the knowledge layer after a confirmation; restore syncs the replica | v2.12.0 | issue #213, PR #215 |
| Drafts panel, lint health report, "Type untyped facts" with an optional Jev classifier | v2.13.0 | PR #219 |

Several README claims have also drifted from the code (Phase 1 inventory
below). The worst: every `-p tools` command names a package that does not
exist, the documented MCP build command fails, and the README describes one
MCP server when the repo builds two with different tool sets. The README is
the front door of an open-source repo; stale claims misrepresent the project
to newcomers and contributors.

## Goals

1. Every factual claim in the README verified against the code at the spec's
   base commit (05c0b2d) — no claim survives on memory alone.
2. Reader outcomes. After reading the README alone, a new user can get the
   app (Releases) and follow the first-run loop — drop files in the vault,
   watcher ingests, librarian proposes, Review desk approves — and a
   developer can build from a fresh clone, run both MCP servers, and run
   the test suites.
3. Architecture section reflects the current workspace layout and the current
   memory-model vocabulary the app itself uses in its UI.

## Non-goals

- No code changes, no CI changes, no docs/ file moves or edits outside this
  spec (stale docs found during Phase 1 are listed as follow-ups).
- No marketing rewrite of the project's tone; keep the existing voice.
- Not a CONTRIBUTING.md authoring effort (possible follow-up only).

## Phase 1 — Inventory (verification gate) — DONE

Rows I1–I23 were verified at base commit `05c0b2d`; rows I24–I31 were added
and verified during PR review. "Ran" means the command or binary was
executed; "Read" means the claim was checked in source.

| # | Claim / fact | Evidence | How |
|---|---|---|---|
| I1 | Workspace members are `src-tauri` (package `curated-thoughts`) and `tools` (package **`curated-thoughts-tools`**) | `src-tauri/Cargo.toml:2`, `tools/Cargo.toml:2` | Read |
| I2 | `cargo build -p tools …` fails: no such package. Affects the `ct`, `bulk_reindex` and `semantic_search_profile` commands | `error: package ID specification 'tools' did not match` | Ran |
| I3 | `cargo build -p curated-thoughts --features mcp-server --bin curated-thoughts-mcp` fails: `curated-thoughts` has one bin (`autobins = false`) | `src-tauri/Cargo.toml:8,84`; cargo error | Ran |
| I4 | **Two MCP servers.** (a) The app binary run with `--mcp` (feature `mcp-server`): 16 tools, read + write. (b) The tools-crate `curated-thoughts-mcp` dev binary: 7 read-only tools | `src-tauri/src/main.rs:6-19,73-77`, `src-tauri/src/mcp_server.rs` `#[tool]` attrs; `tools/Cargo.toml:57`, `tools/src/bin/curated_thoughts_mcp.rs` `#[tool]` attrs; `tools/list` over stdio for both | Ran |
| I5 | (a) tools: `vault_semantic_search`, `vault_related_chunks`, `wiki_search`, `wiki_context`, `wiki_get_ontology`, `wiki_traverse_graph`, `vault_write_note`, `vault_upsert_index_entry`, `curated_recall_context`, `curated_get_wiki_entry`, `curated_search_code`, `curated_add_wisdom`, `curated_update_wisdom`, `curated_archive_wisdom`, `curated_proposals_list`, `curated_proposal_decide` | `tools/list` response | Ran |
| I6 | (b) tools: `vault_semantic_search`, `vault_related_chunks`, `curated_recall_context`, `curated_get_wiki_entry`, `curated_search_code`, `graph_neighbors`, `curated_superpowers_setup`; ignores `--mcp` | `tools/list` response with and without `--mcp` | Ran |
| I7 | The release sidecar `curated-thoughts-mcp` is server (a): the app binary built with `--features mcp-server --bin curated-thoughts`, copied under the sidecar name | `.github/workflows/build.yml:85-94,129-136` | Read |
| I8 | No MCP server exposes drafts or lint tools; drafts and lint health are UI-only (`DraftsPanel.tsx`, `HealthReportPanel.tsx`) | I5/I6 lists; `src/components/settings/` | Ran/Read |
| I9 | Fresh clone: every cargo build touching `curated-thoughts` fails until a placeholder `src-tauri/binaries/curated-thoughts-mcp-<host-triple>` exists (the directory is gitignored except `.gitignore`) | build error `resource path … doesn't exist`; `.github/workflows/ci.yml:78-86` | Ran |
| I10 | `ct` verbs: `status search recall code graph wiki{list,get,forget,sweep} evidence{regrade} proposals{list,show,review} approve ingest librarian{run} trust watch` | `target/debug/ct --help` + subcommand help | Ran |
| I11 | `ct graph <SYMBOL>` walks code-symbol callers/callees (`--dir` takes `callees`, `callers` or `both`; `--hops`), not the wiki graph | `ct graph --help` | Ran |
| I12 | `ct evidence regrade --yes` re-runs the V20 unanchored-evidence re-grade (export + purge), idempotent | `ct evidence regrade --help`; CHANGELOG 2.8.0 | Ran |
| I13 | `ct approve [PROPOSAL_ID] [--all] [--yes]` approves without the interactive loop | `ct approve --help` | Ran |
| I14 | `ct watch` requires `CURATED_VAULT_ROOT` | `tools/src/cmds.rs:1091-1095` | Read |
| I15 | Env vars `CURATED_BRAIN_DIR`, `CURATED_BRAIN_DB`, `CURATED_BRAIN_CONFIG` are honored by both servers; missing `brain.db` is a startup error | both binaries' stderr on an empty dir | Ran |
| I16 | Jev classifier: optional, used only by "Type untyped facts" in Maintenance; provider defaults to None; sends fact titles and bodies to the endpoint; blocked in Strict privacy; never proposes relationships | `src/components/settings/ClassifierPanel.tsx:10,96-127` | Read |
| I17 | `wiki_traverse_graph` walks across namespaces when `entityId` is omitted (cross-partition), keeping the top 8 ranked by matching-edge count and flagging the cut (`partitions_truncated`) | `src-tauri/src/mcp_server.rs:109-110`; `src-tauri/src/wiki_graph.rs:12-16,898-906` | Read |
| I18 | `pnpm typecheck`, `pnpm lint`, `pnpm test` exist and pass | `package.json:36-41`; run: 83 files / 524 tests pass | Ran |
| I19 | `@equationalapplications/{react,expo,core}-llm-wiki` exist on npm (7.7.4) | `npm view` | Ran |
| I20 | Workspace-layout drift after PR #165 was already fixed by 14a26eb (`--manifest-path` forms still work); the remaining drift is I2/I3 | `git show 05c0b2d:README.md` | Read |
| I21 | Neither `docs/superpowers/specs/curated-thoughts-mcp-coding-spec.md` (no HVG tools, no `graph_neighbors`) nor `docs/mcp-write-tools-okf-frontmatter.md` ("v0.1", 2 tools) is a current tool inventory | grep | Read |
| I22 | `mcp_integration` tests return early (pass, 0.00s) unless `CURATED_MCP_INTEGRATION_TESTS=1`; with it set, 4/4 pass in 4.7s | `src-tauri/tests/mcp_integration.rs:5,151-154` | Ran |
| I23 | `bulk_reindex -- --dry-run` parses and opens the brain; on a synthetic empty brain it stops at the V22 vault-root guard (expected), so a full run needs a real configured brain | run output | Ran |
| I24 | Release-sidecar notes carried over from the prior README: the sidecar name must differ from the package name (Tauri rule), tracing goes to stderr with protocol traffic on stdout, and on Windows the console stays attached in `--mcp` mode, so an agent-spawned sidecar can flash a console window | `5eab8fa` (PR #67: "tauri forbids sidecar = package name"); `src-tauri/src/mcp_server.rs:311-314`; `src-tauri/src/main.rs:1-3,23` (console hidden only in GUI mode) | Read |
| I25 | Draft promotion is a direct user action, not a proposal: it records reviewer `human:local` (trust tier `human-reviewed`) and pushes an outbox row in one transaction | `src-tauri/src/db/drafts.rs:1-15` | Read |
| I26 | The health report is read-only and counts dangling edges, manifest violations, untyped facts, drafts and unverified inferred facts | `src/components/settings/HealthReportPanel.tsx:5-16` | Read |
| I27 | Restore recovery: an install marker plus the staged file let the next launch tell a finished install from one that never happened; the outgoing WAL is kept for rollback | `src-tauri/src/db/restore_sync.rs:1-30` | Read |
| I28 | Deleted-source provenance is stored per proposal (`curated_proposal_deleted_sources`), and the Review desk marks deleted and stranded sources | `src-tauri/src/db/schema.rs:474-480`; CHANGELOG 2.11.0 (`ed35698`) | Read |
| I29 | `ct watch --json --once` prints `start` / `shutdown` JSON lines on stdout; `--once-timeout` defaults to 60s | run against a scratch brain + vault; `ct watch --help` | Ran |
| I30 | Rust tracks the rolling `stable` channel; `rust-toolchain.toml` only adds clippy and rustfmt | `rust-toolchain.toml` | Read |
| I31 | Release bundles for macOS (universal), Linux and Windows are published on the Releases page on `v*` tags (not drafts) | `.github/workflows/build.yml:19-26,142-152`; README badges | Read |

## Phase 2 — README revision

The table below maps every Problem-table row to the README section that
covers it.

| Section | Treatment | Covers |
|---|---|---|
| Badges / header | Keep as-is | — |
| Getting started (new block after the intro) | Download pointer to Releases + the first-run loop in two lines; the loop restates Architecture-section claims already in the README | I31 |
| Three-Tier Memory System | Keep structure; mention the human-verification gate as the only path into the Semantic tier (the wiki) | #201 |
| Architecture & Data Flow | Review-queue bullet: deleted-source provenance; `.brain/` bullet: excluded from the walk, crash-safe restore; workspace bullet names both packages (I1) | #205, #211, #213 |
| Key Features: BYOI | Keep; describe the optional classifier accurately (I16) | #219 |
| Key Features: Human-in-the-loop verification | Review desk, `ct proposals review`, `ct approve`, the `curated_proposals_list` / `curated_proposal_decide` MCP tools; mention evidence regrade as the provenance-recovery path | #201, #186 |
| Key Features: Wiki maintenance | Drafts panel, lint health report, "Type untyped facts" (UI only, I8) | #219 |
| Key Features: Backup, restore & vault switching | Restore syncs the replica; vault switch clears the knowledge layer after a confirmation | #213 |
| Key Features: MCP Agent Server | Full server (read + write, including wisdom CRUD and proposal decisions) vs read-only dev server (I4) | #185, #201 |
| Key Features: Offline-first | Keep as-is | — |
| Key Features: Cross-Partition Wiki Graph | New short subsection tied to `wiki_traverse_graph`, including the top-8 partition cap (I17) | #190/#197 |
| Local Development | Add the fresh-clone sidecar-placeholder step (I9) | — |
| MCP Agent Server (detailed) | Fix the dev build to the app binary + `--mcp` (I3, I7); list both servers' tools inline (I5, I6); env vars (I15); remove the stale inventory pointer here and in Project Structure (I21); sidecar notes (I24) | #185, #190, #201 |
| CLI Tools & Testing | Fix `-p` to `curated-thoughts-tools` (I2); correct the `ct` verb list (I10-I14); add the integration-test opt-in variable (I22) | #186, #201 |
| Related Packages | Keep (I19) | — |

## Acceptance criteria

1. Every factual claim in the new README maps to a Phase 1 inventory row; a
   claim with no row is either added to the inventory (with evidence) or cut.
2. Every command block is **executed** at the PR tip, except these, which are
   checked by reading only, with the reason given:
   - `pnpm tauri dev` / `pnpm tauri build` — GUI launch / full signed bundle;
     both depend on the same `cargo build` of `curated-thoughts` that is run.
   - The `mcpServers` JSON snippet and the release-bundle sidecar path —
     configuration, not commands; the binaries they name are run (I4-I7).
   - `ct` verbs that write to the brain (`ingest`, `librarian run`,
     `approve`, `evidence regrade`, `wiki forget|sweep`) and the `watch`
     daemon — each verb's syntax is executed via `--help` (I10-I14); read verbs
     run against a scratch brain.
   - `bulk_reindex` — executed against a scratch brain up to the vault-root
     guard (I23).
   The fresh-clone precondition is exercised by deleting the sidecar
   placeholder before the first cargo command (I9).
3. The diff touches `README.md` and `docs/superpowers/specs/2026-09-23-*.md`
   only.
4. CI green on the PR tip (`gh pr checks` + `mergeStateStatus` CLEAN). CI does
   not lint Markdown (no markdownlint config); it runs the full build/test
   suite regardless of path.
5. Spec Status is Approved before the README commit is (re)written.

## Phase 3 — Review & merge

Docs-only diff: one review pass plus the bot review. Before merging, a normal
commit on the branch sets Status to `Implemented (PR #224)`. A merge commit
cannot carry content changes, so the flip cannot happen in the merge itself.
Merge with a regular merge commit, not squash or rebase, so the spec and
review-fix commits stay in `main`'s history (this repo's standing rule).

## Risks

- **The tool lists rot again.** No current in-repo tool inventory exists
  (I21), so the README lists both servers' tools inline and names the source
  of truth: the `#[tool(name = …)]` attributes in `src-tauri/src/mcp_server.rs`
  and `tools/src/bin/curated_thoughts_mcp.rs`. CI's tool-list gate (PR #201)
  covers the in-app server, not the README.
- **The README gets too long.** New features enter as short subsections, not
  essays.

## Follow-ups (out of scope)

- Refresh or retire `docs/superpowers/specs/curated-thoughts-mcp-coding-spec.md`
  and `docs/mcp-write-tools-okf-frontmatter.md` (I21).
- Two different binaries share the name `curated-thoughts-mcp` (I4/I7);
  consider renaming the read-only tools-crate binary.
- A setup script (or `build.rs` fallback) for the fresh-clone sidecar
  placeholder (I9).
- `ct watch`'s missing-root error says "(or pass --vault)", but `ct watch` has
  no `--vault` flag (`tools/src/cmds.rs:1092`; `ct watch --vault` → "unexpected
  argument"). Fix the message; the README correctly documents only
  `CURATED_VAULT_ROOT` (I14).
- `pipeline::watchdog::heartbeat::tests::seqlock_holds_under_concurrent_transitions`
  is timing-sensitive: it failed `rust-macos` on this docs-only PR (3904/5000
  even-seq reads against a 90% floor, CI run 35887223463). Make it
  deterministic or relax the threshold.

## Revision log

- **rev 1** (first push of this branch; that commit was rewritten into
  `5031197` when its authorship was fixed): problem, section treatment,
  acceptance criteria, status Draft.
- **rev 2** (`5031197`): folded in the spec review (wrong PR/version
  references, missing capabilities, the two-MCP-server split, the
  nonexistent tool-inventory doc, the merge-time status flip, references to
  rules that don't exist in the repo) and the PR-bot review (Phase 2 now covers
  every Problem row; criterion 2 now requires execution). Added the Phase 1
  inventory (I1-I23) and follow-ups. Status Approved.
- **rev 2.1** (`9fef52b`, `653970a`, `c1c408e`): I24 (sidecar notes),
  I11/I17 corrections from CodeRabbit, the `--vault` and flaky-test
  follow-ups, the Phase 2 row for the Cross-Partition Wiki Graph subsection,
  the reason for the merge style, and this log.
- **rev 2.2** (`3137a7b`): README wording pass from a readability review (bullets,
  concrete UI locations, the error you see if you skip the placeholder
  step). Several suggested phrasings added claims the code doesn't support
  (drafts going through the proposal gate, a "default" classifier, deleted
  documents' chunks staying traceable, an auto-installed pinned toolchain).
  Those were corrected against the code, and the claims that remain are
  backed by new rows I25-I30.
- **rev 2.3**: wording-only polish from a self-review of the spec — Status
  names the current revision, the Phase 1 intro splits base-verified rows
  (I1–I23) from PR-review rows (I24–I31), Goal 2 is rewritten as reader
  outcomes, PR #224 is named in the header and Phase 3, "tier 3" and
  "MCP decide" are spelled out in Phase 2, and the revision-log hashes are
  resolved. One README addition: a Getting-started pointer to Releases with
  the first-run loop, backed by I31 and a new Phase 2 row.
