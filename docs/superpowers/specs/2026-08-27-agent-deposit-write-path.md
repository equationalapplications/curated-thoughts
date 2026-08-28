# Agent Deposit Write Path

**Date:** 2026-08-27
**Status:** SPEC (decisions locked with Kurt, Aug 27 2026 — awaiting review)
**Author:** Kurt VanDusen / Hermes
**Related Specs:**
- `2026-08-27-vault-folder-structure-refactoring.md` — v2 layout this builds on (implemented, PR #115/#116)

---

## Problem

After the v2 refactoring, `vault_write_note` accepts writes **only** under `wiki/`. Agents
(Hermes, Tessera, future ones) have no sanctioned way to deposit durable memories into the
vault. On wiki-shaped dogfood vaults (equational-wiki: top-level `people/`, `memories/`, …)
there is no `wiki/` folder at all, so **every agent write is currently rejected** (verified
live against bundled v1.33.0: `Path is outside vault root` for all paths).

We want agent memories to flow through the same pipeline as user sources: auto-watched,
chunked, embedded, and reconciled by the librarian — one ingestion path, clean provenance
(`deposit → ingest → curate`), no special-casing.

## Solution

Designate a **deposit subfolder** inside the immutable tier:

```
<vault>/
├── immutable-source-files/
│   └── agents/          ← NEW: the only agent-writable path in the source tier
│       └── <agent>/…    ←   per-agent namespaces (hermes/, tessera/, …)
├── wiki/                ← app-managed wiki (unchanged)
└── .brain/              ← app state (unchanged)
```

The v2 security model is preserved: agents still cannot touch user documents — only their
own namespaced deposit folder. "Immutable" retains its operational meaning: **the librarian
never rewrites sources**; deposits are append-only records the curator reads.

### Folder contract (amends v2 table)

| Folder | Contract | Reads | Agent writes |
|--------|----------|-------|--------------|
| `immutable-source-files/` (outside `agents/`) | User documents | ✓ | ✗ (unchanged) |
| `immutable-source-files/agents/**` | Agent memory deposits | ✓ | ✓ (append-only, OKF frontmatter) |
| `wiki/` | App-managed | ✓ | ✓ (unchanged) |

---

## Implementation

### Phase 1: `safe_path.rs` — deposit constant + writable list

```rust
/// Nested agent-deposit prefix inside the immutable tier (like ".brain/proposed").
pub const AGENTS_DEPOSIT_DIR: &str = "immutable-source-files/agents";

/// Writable targets for OKF note writes (vault_write_note both MCP + Tauri surfaces):
/// wiki pages + agent deposits.
pub const NOTE_WRITABLE_SUBDIRS: &[&str] = &[WIKI_DIR, AGENTS_DEPOSIT_DIR];
```

Mechanism is proven: nested prefixes already work via
`root_canonical.join(sub).canonicalize()` + component-based `starts_with`
(the `.brain/proposed` entry in `PROPOSED_SUBDIRS` is the precedent).

**Known wrinkle (verified live):** if an allowed subdir does not exist on disk, its
`canonicalize()` fails and it is silently dropped from the allowed list → writes fail with
`Outside`. This is exactly why `wiki/` writes fail on the dogfood vault today. Mitigations,
both required:

1. Vault-setup sites create the deposit dir (below).
2. `write_note`'s existing parent-bootstrap (`NotFound "parent directory not found"` →
   `create_dir_all` → retry) covers the lazy path — it already creates intermediate dirs,
   and the retry re-canonicalizes the allowed list, so the first deposit into a missing
   `agents/` succeeds as long as the vault root + `immutable-source-files/` exist.

**Caller change (one site):** `okf/write.rs::write_note` (both call sites, ~118/137)
switches `WRITABLE_SUBDIRS` → `NOTE_WRITABLE_SUBDIRS`. `WRITABLE_SUBDIRS` itself is
unchanged (still `[WIKI_DIR]`) for app-internal writers (`run_wiki_forget`, etc.).
`upsert_index_entry` already uses `READABLE_SUBDIRS`; since `agents/` nests under
`immutable-source-files/`, index upserts targeting deposit indexes remain allowed — no
change. `tool_dispatch.rs` adapters stay thin (no change).

### Phase 2: Vault layout creation

All three vault-setup sites additionally `create_dir_all` the deposit dir (idempotent):

| Site | Today creates |
|------|---------------|
| `set_vault_path` (lib.rs ~545) | `IMMUTABLE_DIR`, `WIKI_DIR`, `.brain/converted` |
| `switch_vault` (lib.rs ~1172) | same |
| startup default-vault bootstrap (~2624) | `IMMUTABLE_DIR`, `wiki`, `.brain/converted` |

Add `crate::vault::safe_path::AGENTS_DEPOSIT_DIR` to each loop (`create_dir_all` handles
the nesting). No config flag, no migration code — missing dir self-heals via Phase 1's
bootstrap on first deposit.

### Phase 3: Supersession semantics (frontmatter field)

Add optional field to `OkfFrontmatter` (okf/mod.rs):

```rust
/// Vault-relative path of the deposit this one supersedes. Deposit-to-deposit only.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub supersedes: Option<String>,
```

Rules (enforced in `validate_frontmatter`):
- May only reference a path under `AGENTS_DEPOSIT_DIR` (reject otherwise; a deposit may
  never claim to supersede a user document or wiki page).
- The referenced deposit need not exist yet? **No — must exist** (typo safety); error
  `invalid_frontmatter:supersedes_not_found:{path}`.

Reconciliation contract (librarian/heal — ruling accepted Aug 27):
- **Newest deposit wins** on declared chains (`supersedes`) and on `created_at` ordering.
- Heal **flags undeclared conflicts** (same-subject contradictory deposits with no chain)
  for review — it never silently merges, and never edits either deposit.

### Phase 4: Ingestion & retrieval (no code)

- Watcher already roots at `immutable-source-files/` → deposits auto-ingest. None of the
  ingestion pipeline changes.
- Retrieval contract is documentation, not code: `wiki_search` = default agent context
  (curated, reconciled); `vault_semantic_search` = deliberate deep search of the immutable
  record with verbatim provenance.

---

## Non-Goals

- **Local vault reorg** (moving wiki-shaped top-level dirs into `immutable-source-files/`)
  is LOCAL-ONLY data movement on the dogfood machine — explicitly no repo migration code
  (Kurt's ruling). The v2 `BothFoldersExist` machinery is NOT reused here.
- **Tessera's broader rights** (executive agent, person category, may touch the source
  tier) are an operational ruling, out of code scope for now.
- No frontend changes required (`agents/` nests under "Source Files" in the tree; labels
  unchanged).
- No v2 contract changes outside the deposit prefix.

## Acceptance Criteria

- [ ] Test: write `immutable-source-files/agents/hermes/mem.md` via `vault_write_note` succeeds (MCP + Tauri surfaces)
- [ ] Test: write `immutable-source-files/secrets.md` (deposit prefix absent) → `SafePathError::Outside`
- [ ] Test: traversal `immutable-source-files/agents/../../x.md` → `Traversal` (pinned, components-based)
- [ ] Test: first deposit into a missing `agents/` dir bootstraps parents and succeeds
- [ ] Vault-setup sites create `immutable-source-files/agents/` (set/switch/default bootstrap)
- [ ] `supersedes` validates: deposit-to-deposit only, target must exist
- [ ] `WRITABLE_SUBDIRS` unchanged (app-internal writers unaffected)
- [ ] **FULL suite green** — `cargo test --features test-utils,mcp-server` incl. `tests/` integration (PR #115 lesson: `--lib` alone is not evidence)

## Risks / Trade-offs

| Risk | Mitigation |
|------|------------|
| Allowed-dir-not-exists silently drops the prefix (live-verified) | Phase 2 creation + Phase 1 bootstrap; regression test for the lazy path |
| Deposit sprawl (unbounded append-only growth) | Per-agent namespaces; supersession chains; heal flags — pruning is a future, explicit decision |
| `supersedes` typo'd path | Target-must-exist validation |
| Semantic drift: "immutable" now includes agent deposits | Accepted (Kurt): still immutable in the sense that the librarian never rewrites it; refine naming later |

## Open Questions

**None.** Decisions locked Aug 27 2026: deposit folder named `agents/` (nested under
`immutable-source-files/`); supersession = newest wins + heal flags conflicts; retrieval
contract as documented above.
