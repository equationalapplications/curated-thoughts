# PR #101 → #102 — MCP Write Path + OKF Frontmatter — Implementation Checklist (v2)

**Branch:** `feat/mcp-write-path-okf-v2` (renamed from `feature/mcp-write-path-okf-frontmatter` Aug 27 after history rewrite; old branch deleted, PR #101 closed, superseded by **PR #102**)
**Spec:** `docs/superpowers/specs/2026-08-26-mcp-write-path-okf-frontmatter.md` (v2 — REVISED 2026-08-27)
**Status:** In progress — 17 commits on branch, HEAD `3ccdd24` (docs+hygiene; still does not compile — T0 pending)
**Handoff:** `.superpowers/sdd/2026-08-27-pr-101-stabilization/HANDOFF.md`

> v1 checklist items are superseded. The v1 file-list and verification commands
> were wrong (`docs/mcp-tools.md` never existed; MCP handlers live in
> `src-tauri/src/mcp_server.rs`, NOT `tools/src/bin/curated_thoughts_mcp.rs`).

---

## T0 — Unbreak the build (FIRST, mechanical)

- [ ] Fix E0277 ×2: `src-tauri/src/lib.rs:607` and `:615` — `format!("{}/", canonical_vault)` on a `PathBuf`. (These lines are DELETED wholesale by T1; if T0 is committed separately, use `canonical_vault.display()` or drop the string-prefix hack for `starts_with(&canonical_vault)`.)
- [ ] Remove unused import `WriteNoteError` (lib.rs:580)
- [ ] Remove unused `mut` (lib.rs:759)
- [ ] `cargo check --features test-utils,mcp-server --all-targets` clean

## T1 — Consolidate to ONE core (the architectural fix)

- [ ] Create `src-tauri/src/okf/write.rs` with:
  - `write_note(vault_root, path, fm, body)` — uses `crate::vault::safe_vault_path(&["."], PathMode::MayCreate)`; stale contract = exact-match `updated_at` token from EXISTING file frontmatter (no mtime); atomic temp+rename in same dir
  - `upsert_index_entry(vault_root, index_path, entry_name, entry_path, entry_type, metadata)` — line-scan whole-line header match (NO regex, NO `(?m)`), pinned block format, replace-through-next-`## `-or-EOF, atomic write
- [ ] `tool_dispatch.rs::dispatch_vault_write_note` / `dispatch_vault_upsert_index_entry` → thin calls into `okf::write` (DELETE ~100-line inline upsert copy)
- [ ] `lib.rs` Tauri commands → thin wrappers (vault root from `VaultConfigState`) — DELETE inline logic (~lib.rs:573-860)
- [ ] DELETE old-shape fns from `okf/mod.rs` (`vault_write_note`, `vault_upsert_index_entry(vault_root, index_path, entry_id, metadata)`)
- [ ] Register both commands in PRODUCTION handler (lib.rs:~3067 block) — currently only in `make_test_app` (lib.rs:2822)
- [ ] grep gate: no `entry_regex`, no ancestor-walk loops, no `canonicalize` outside `okf/write.rs` + `vault/safe_path.rs` in write-path code

## T2 — Tests (three tiers)

- [ ] Tier 1: inline unit tests in `okf/write.rs` (D1-D7 per spec v2 §Test Strategy)
- [ ] Tier 2: update `tests/mcp_write_integration.rs` — remove ALL future-timestamp hacks + `thread::sleep`; token-based stale tests; e2 duplicate/prefix-collision assertions
- [ ] Tier 3: dispatch-level roundtrip via `ToolDispatchContext { vault_dir: tmp }` (or extend `tests/mcp_integration.rs` spawned-server harness — preferred)
- [ ] Verification gate green (spec v2 command block) BEFORE push; paste output in PR thread

## T3 — Hygiene (blocking for merge)

- [x] Drop 87 MB `tools/.fastembed_cache` blobs — DONE Aug 27 2026 (Kurt approved): history rewritten via `git filter-repo --invert-paths --path tools/.fastembed_cache --refs feature/mcp-write-path-okf-frontmatter` (backup bundle: `~/hermes-backups/pr-101-pre-rewrite-20260827-081046.bundle`); `tools/.fastembed_cache/` added to `.gitignore`. NOTE: `pr-99` branch still carries its own copy (incl. a 90 MB `model.onnx`) — separate cleanup, separate decision.
- [ ] Delete `MCP_TOOL_REGISTRATION_SUMMARY.md`, `MCP_WRITE_INTEGRATION_TESTS.md`, `test_mcp_registration.sh`
- [x] ~~Ask Kurt: history purge (`git filter-repo`) vs tip-only removal~~ — RULED Aug 27: full filter-repo, approved
- [ ] `docs/mcp-write-tools-okf-frontmatter.md` reviewed against v2 contracts (token staleness, error strings, block format)

## T4 — Merge

- [ ] CI green: rust-ubuntu AND rust-macos
- [ ] CodeRabbit + aws-cloud-agent reviews addressed (never `@coderabbitai`; evaluate both; log to scorecard)
- [ ] Mark backlog P1 write-path items done AT MERGE
- [ ] Post-merge: update `curated-thoughts-operations` skill with tool usage patterns

---

## Resolved rulings (from spec v2 — do not re-litigate)

| Q | Ruling |
|---|---|
| updated_at on new files | Optional on create; on edit must EXACTLY MATCH file's current token |
| Auto-create INDEX.md | NO — `index_not_found` |
| Staleness mechanism | Content token (If-Match style); mtime NEVER consulted |
| Entry matching | Whole-line `## {name}` scan; regex forbidden |
| Result `path` field | Vault-relative, not absolute |
| Path safety | `crate::vault::safe_vault_path` only — no re-implementations |
