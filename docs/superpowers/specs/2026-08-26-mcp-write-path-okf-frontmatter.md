# Spec: MCP Write Path + OKF Frontmatter — Revision 2

**Date:** 2026-08-26 (v1) / **Revised:** 2026-08-27 (v2)
**Author:** Hermes Agent (Tessera)
**Status:** Revised — supersedes v1 solution architecture and test strategy
**Related:** `procedures/curated-thoughts-improvement-backlog.md`, PR #101 (closed; branch history rewritten) → **PR #102**, branch `feat/mcp-write-path-okf-v2`

---

## Why This Revision Exists (Post-Mortem Summary)

PR #101 went through ~10 fix rounds and still cannot merge. Root causes found on
2026-08-27 (all verified against HEAD `facfdcc`, CI run `33047267015`, and a local
build — see `.superpowers/sdd/2026-08-27-pr-101-stabilization/HANDOFF.md`):

| # | Failure mode | v1 spec gap that allowed it |
|---|---|---|
| 1 | **Three parallel implementations** of the write path with divergent semantics: (a) Tauri commands in `src-tauri/src/lib.rs:573-860`, (b) core fns in `src-tauri/src/okf/mod.rs`, (c) a third inline copy in `tool_dispatch.rs::dispatch_vault_upsert_index_entry`. Fixes landed in one copy and not the others. | v1 never designated a **single source of truth**. It sketched a Tauri command and separately an MCP tool, with no rule that they must delegate to one core. |
| 2 | **The tested code path is not the shipping code path.** The integration tests (`mcp_write_integration.rs`) drive Tauri commands registered ONLY in `make_test_app` (lib.rs:2822). The production app handler (lib.rs:3067) does NOT register them, and the real MCP server routes `mcp_server.rs → tool_dispatch.rs` (implementation (c), which has zero test coverage). | v1's test strategy tested "the command" without pinning **which surface must be covered** (MCP dispatch is the actual consumer). |
| 3 | **Stale-update semantics were unworkable.** v1 said reject when `updated_at ≤ file mtime`. But every write sets mtime = now, so the next legitimate edit is always rejected. Tests escaped by faking **future timestamps** (commit `facfdcc` admits this) — tests lying to make code pass. | v1 chose mtime as the concurrency token. mtime is metadata, not content; it cannot serve as a read-modify-write token. |
| 4 | **Upsert regex bugs.** `Regex::new(r"^##\s*{name}")` used with `.is_match(&content)`: without `(?m)`, `^` only matches the haystack start, so mid-file entries NEVER match → always appends → duplicate entries (2 known failing tests). Also prefix matching collides (`multi-update` matches `## multi-update-2`). | v1 specified "entry lookup by regex `^## {entry_name}`" — an underspecified matcher. |
| 5 | **Path safety re-implemented three times** (ancestor walks, string-prefix `starts_with(&format!("{}/", vault))` — which doesn't even compile, E0277) instead of using the existing hardened `crate::vault::safe_vault_path` (null-byte, absolute, `..`, drive-prefix, symlink checks; 36 existing call sites; `tests/path_traversal.rs`). | v1 said "reject `../`, absolute paths" without requiring the existing helper. Each re-implementation reopened the same security review rounds. |
| 6 | **87 MB of model-cache blobs committed** (`tools/.fastembed_cache/`, 16 files — fastembed ONNX weights). `.gitignore` only covers `src-tauri/.fastembed_cache/`. Plus scaffolding junk at repo root (`MCP_TOOL_REGISTRATION_SUMMARY.md`, `MCP_WRITE_INTEGRATION_TESTS.md`, `test_mcp_registration.sh`). | v1 had no repo-hygiene/PR-content requirements. |
| 7 | **Pushed without compiling.** HEAD `facfdcc` does not compile (`E0277` ×2 at lib.rs:607/615, unused import at 580, unused `mut` at 759), yet its commit message claims "8/11 passing". | v1 had no local CI-parity verification gate before push. |

**Verdict: the spec needed structural revision**, not another patch round. v2 below
preserves v1's problem statement, vision, and migration stance, and replaces the
solution architecture, behavioral contracts, and test strategy.

---

## Problem Statement (unchanged from v1)

1. **No MCP write capabilities** — the MCP server exposes read-only tools; agents
   with tool-only access cannot persist knowledge to the vault.
2. **No standardized frontmatter** — the chunker/embedder cannot uniformly extract
   metadata (entity_type, tags, created_at) from agent-written notes.
3. **Wiki layer stays empty** — agent notes never contribute to the wiki graph.

See v1 (git history) for the full narrative; vision alignment and goals are unchanged:

- `vault_write_note` MCP tool: write markdown with OKF v0.1 frontmatter.
- `vault_upsert_index_entry` MCP tool: auditable INDEX.md updates.
- OKF frontmatter on all NEW vault files going forward.

Non-goals (unchanged): no back-migration, no librarian synthesis, no ct-memory-eval
removal, no chunker changes (Phase 4, separate PR).

---

## Revised Solution Architecture (v2 — NORMATIVE)

**One core implementation. Every surface is a thin adapter. No logic in adapters.**

```
src-tauri/src/okf/mod.rs        (types, validate, render, parse, sha256 — keep as-is)
src-tauri/src/okf/write.rs      (NEW — the single write-path core)
  pub fn write_note(vault_root, path, fm, body) -> Result<WriteNoteResult, WriteNoteError>
  pub fn upsert_index_entry(vault_root, index_path, entry_name, entry_path,
                            entry_type, metadata) -> Result<UpsertResult, UpsertError>

Adapters (each ≤ ~15 lines of glue, zero decision logic):
  src-tauri/src/tool_dispatch.rs   dispatch_vault_write_note / dispatch_vault_upsert_index_entry
                                   → call okf::write core. DELETE the current 100-line
                                     inline upsert copy in dispatch_vault_upsert_index_entry.
  src-tauri/src/lib.rs             #[tauri::command] vault_write_note / vault_upsert_index_entry
                                   → resolve vault root from VaultConfigState, call okf::write core.
                                   DELETE the current ~290 lines of inline logic (573-860).
                                   Register BOTH commands in the PRODUCTION handler
                                   (lib.rs:3067 block) as well as make_test_app.
```

**Deletions required by this spec** (this is what "clean" means for merge):
- `lib.rs` inline write/upsert bodies (replaced by wrappers over `okf::write`).
- The third inline upsert implementation inside `tool_dispatch.rs`.
- The v1 signatures in `okf/mod.rs` (`vault_upsert_index_entry(vault_root, index_path,
  entry_id, metadata)` — wrong shape: no entry_path/entry_type, JSON-comment format,
  auto-creates missing index files, non-atomic `fs::write`). Move corrected logic to
  `okf/write.rs`; old fns removed.

---

## Behavioral Contracts (v2 — NORMATIVE)

### A. Path safety (both tools)

MUST use the existing hardened helper — no re-implementations, ever:

```rust
crate::vault::safe_vault_path(vault_root, user_path, &["."], crate::vault::PathMode::MayCreate)
```

- Rejects: absolute paths, `..` components, Windows drive prefixes, NUL bytes,
  symlink escapes. Errors map to `path_outside_vault`.
- Writes are allowed anywhere under the vault root (`&["."]`), matching existing
  read-tool behavior.
- For `upsert`: `index_path` additionally MUST resolve to an existing file, else
  `index_not_found`. `entry_path` is syntax-validated only (may not exist yet).

### B. `vault_write_note`

**Parameters:** `path` (vault-relative), `frontmatter` (OKF v0.1 object), `body`.

**Order of operations:**
1. `safe_vault_path` → `path_outside_vault`.
2. `validate_frontmatter` (okf/mod.rs, unchanged) → `invalid_frontmatter:{detail}`.
   Includes: `updated_at >= created_at` when both present.
3. **Stale-update contract v2 (replaces mtime check — mtime is NEVER consulted):**
   - If target file exists AND its frontmatter contains `updated_at = X`:
     the caller MUST supply `frontmatter.updated_at == X` (exact string match).
     Any mismatch or absence → `stale_update:{X}` (error carries the current token
     so the caller can re-read and retry).
   - If target file exists but has no `updated_at` (legacy/bootstrap): accept the
     write (no token to compare).
   - New file: accept; `updated_at` optional on create.
   
   Rationale: this is a read-modify-write optimistic-lock token (If-Match/ETag
   semantics). It is deterministic, portable, and testable without clock games.
4. Render document: `---\n{frontmatter yaml}\n---\n{body}`.
5. **Atomic write:** temp file in the SAME directory (unique suffix), then
   `fs::rename`. No partial writes visible; no leftover `.tmp` on success or failure.
6. Return `WriteNoteResult { success: true, path: <vault-relative>, sha256 }`
   — `path` is vault-RELATIVE (portable; do not leak absolute layout), `sha256`
   is over the full document bytes written.

**Error strings (stable machine-readable prefix before `:`):**
`path_outside_vault`, `invalid_frontmatter:{detail}`, `stale_update:{current}`,
`write_error:{io detail}`.

### C. `vault_upsert_index_entry`

**Parameters:** `index_path`, `entry_name`, `entry_path`, `entry_type`, `metadata`
(object, optional). Serde keeps both camelCase (primary) and snake_case (alias).

1. `entry_name` MUST match `^[A-Za-z0-9_-]+$` → else `invalid_entry_name`.
2. `safe_vault_path(index_path)` → `path_outside_vault`; file must exist →
   `index_not_found:{path}`. (Ruling Q2: NEVER auto-create.)
3. `safe_vault_path(entry_path)` syntax check only → `path_outside_vault`.
4. **Entry block format (pinned; the JSON-comment format from v1 okf/mod.rs is dead):**
   ```
   ## {entry_name}
   [[{entry_path}]]
   - Type: {entry_type}
   - {key}: {value}        ← each metadata key, insertion order
   ```
5. **Entry matching — line scan, whole-line equality, NO regex:**
   an entry exists iff some line satisfies `line.trim() == format!("## {}", entry_name)`.
   (Rationale: `^` without `(?m)` never matches mid-file — the shipped duplicate bug;
   and prefix regex collides with `entry-name-2`.)
6. **Replace:** overwrite from the header line through the line preceding the next
   `## ` header (or EOF). All other content preserved byte-for-byte.
   **Append:** exactly one blank line of separation at EOF.
7. Atomic write (temp + rename), same rules as B.5.
8. Return `UpsertResult { success, index_path: <vault-relative>, entry_id: entry_name,
   appended, line_number }` — `appended: true` iff no prior entry existed;
   `line_number` = 1-based line of the entry header in the NEW content (the old code
   returned `unwrap_or(0)` post-loop — always wrong; this pins it).

---

## Resolved Open Questions (v1 → v2 rulings)

| Q | v1 open question | v2 ruling |
|---|---|---|
| Q1 | `updated_at` required on new files? | **Optional on create; required-on-edit only as the match token** (must equal the file's current frontmatter `updated_at` when the file has one). |
| Q2 | Auto-create missing INDEX.md? | **No — reject with `index_not_found`.** |
| Q3 | Chunker adoption timing? | Separate PR (unchanged). |
| Q4 | Back-migration? | Defer; new files only (unchanged). |

---

## Repository Hygiene Requirements (NEW in v2 — blocking for merge)

1. `git rm -r --cached tools/.fastembed_cache` and add `tools/.fastembed_cache/`
   to `.gitignore`. 87 MB of ONNX weights must not land on `main`.
   - Preferred: purge from branch history before merge (`git filter-repo
     --path tools/.fastembed_cache --invert-paths` + force-push; reviews re-run
     automatically). Requires Kurt's sign-off because it rewrites SHAs.
   - Fallback: remove from tip only; accept history bloat.
2. Delete root scaffolding: `MCP_TOOL_REGISTRATION_SUMMARY.md`,
   `MCP_WRITE_INTEGRATION_TESTS.md`, `test_mcp_registration.sh`.
3. Single doc: `docs/mcp-write-tools-okf-frontmatter.md` is the only write-tools
   doc (v1's referenced `docs/mcp-tools.md` never existed — do not create it).
4. Update `procedures/curated-thoughts-improvement-backlog.md` (mark P1 write-path
   items done) at merge time, not before.

---

## Test Strategy v2 (NORMATIVE — three tiers)

### Tier 1 — Core unit tests (`okf/write.rs`, inline `#[cfg(test)]`)
- D1 frontmatter validation (existing 17 okf tests stay green).
- D2 path safety: traversal (`../`), absolute, embedded `..`, symlink escape →
  all `path_outside_vault`.
- D3 stale contract: token mismatch → `stale_update`; token match → ok;
  bootstrap (no token in file) → ok; new file without `updated_at` → ok.
  **No `thread::sleep`, no future timestamps — the contract is content-based.**
- D4 append: format exactness, blank-line separation, `appended: true`.
- D5 replace: no duplicates after repeated upserts; prefix-collision case
  (`multi-update` vs `multi-update-2` are distinct entries); unrelated entries
  and prose preserved byte-for-byte; `appended: false`; `line_number` correct.
- D6 atomicity: no `.tmp` remnants after success and after induced failure.
- D7 `index_not_found` + `invalid_entry_name` errors.

### Tier 2 — Adapter tests (existing `mcp_write_integration.rs`, updated)
The 11 Tauri-invoke tests remain, with assertion updates for v2 semantics:
- Remove ALL future-timestamp hacks; drive the token contract instead.
- `e2_*` now assert single-instance + `appended` flags (they will genuinely pass
  once the core is fixed — currently they fail for the regex reason in §4).
- `e3_*` traversal tests unchanged in spirit (now trivially satisfied by
  `safe_vault_path` rejecting `..`).

### Tier 3 — MCP-surface tests (NEW — the shipping path, currently zero coverage)
At least one roundtrip THROUGH `tool_dispatch::dispatch_tool_call` with a
`ToolDispatchContext` whose `vault_dir` points at a temp vault (write note →
read back → verify frontmatter + sha; upsert → verify index). If cheap, extend
`tests/mcp_integration.rs` (spawned-server harness, `CURATED_MCP_INTEGRATION_TESTS=1`)
with a write-tool call instead — preferred, it is the true E2E.

### Verification gate (MUST be green locally before ANY push)
Exact CI parity — from repo root:
```bash
mkdir -p src-tauri/binaries && touch src-tauri/binaries/curated-thoughts-mcp-x86_64-unknown-linux-gnu
cargo check --manifest-path src-tauri/Cargo.toml --features test-utils,mcp-server --all-targets
CURATED_MCP_INTEGRATION_TESTS=1 cargo test --manifest-path src-tauri/Cargo.toml \
  --features test-utils,mcp-server -- --test-threads=1
cargo clippy --manifest-path src-tauri/Cargo.toml --features test-utils,mcp-server --all-targets
git status --short   # must be clean after committing
```
**A commit whose message claims test status MUST show that status from a local run
of the above, pasted into the PR thread. Subagent claims are verified, not trusted.**

---

## Success Criteria (v2)

1. Exactly ONE implementation of each write operation (`okf/write.rs`); adapters
   contain no logic. Verified by `grep`: no `entry_regex`, no ancestor-walk, no
   `canonicalize` loops outside `okf/write.rs` + `vault/safe_path.rs`.
2. Production Tauri handler (lib.rs:3067 block) registers both commands.
3. All three test tiers green locally via the verification gate; CI green on
   rust-ubuntu AND rust-macos.
4. `tools/.fastembed_cache` absent from the merge tip; junk root files gone.
5. No `thread::sleep`/future-timestamp workarounds in any write-path test.
6. Wiki layer untouched; read tools unchanged (unchanged from v1).

---

## Merge Checklist

- [ ] T0 compile fixes land (E0277 ×2, unused import, unused mut) — see HANDOFF
- [ ] Core consolidation to `okf/write.rs` per architecture section
- [ ] Adapters thinned; production handler registers commands
- [ ] Tier 1/2/3 tests updated & green (verification gate)
- [ ] Hygiene: cache blobs, junk files, docs consolidation
- [ ] Kurt rules on history-rewrite vs tip-only for the 87 MB blobs
- [ ] CI green both platforms; CodeRabbit + aws-cloud-agent reviews addressed
      (standing rules: never `@coderabbitai` re-request; evaluate both bots;
      log to reviewer scorecard)
- [ ] Backlog P1 items marked done AT MERGE

## References

- HANDOFF (next-session plan): `.superpowers/sdd/2026-08-27-pr-101-stabilization/HANDOFF.md`
- v1 spec: git history of this file (commit `86c2b24`)
- `procedures/software-development/pr-spec-driven/references/coderabbit-review-handling.md`
