# Agent Deposit Write Path

**Date:** 2026-08-27
**Status:** ✅ IMPLEMENTED — rev 2 (nested deposits). Phases 1–4 shipped in PR #118
(`3e0eb2f`); nested-deposit amendment shipped in `a552753` + `4b6a123`.
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
│       ├── mem.md …     ←   deposits may nest at any depth (rev 2, Aug 29)
│       └── operations/…   ←   subfolders allowed under the deposit root
├── wiki/                ← app-managed wiki (unchanged)
└── .brain/              ← app state (unchanged)
```

**Ruling (Kurt, Aug 29 2026, rev 2):** deposits may nest at **any depth** under
`agents/`; agents may organize them into subfolders. Provenance is carried by
frontmatter (`title`, `tags`, `created_at`, `supersedes` chains) as well as by
folder structure.

> ~~**Superseded (Kurt, Aug 28 2026):** no per-agent namespaces (`hermes/`,
> `tessera/`, …) — all deposits go directly under `agents/`.~~ Replaced by the
> Aug 29 ruling above; see §Ruling history.

The v2 security model is preserved: agents still cannot touch user documents — only the
shared deposit folder. "Immutable" retains its operational meaning: **the librarian
never rewrites sources**; deposits are append-only records the curator reads.

### Folder contract (amends v2 table)

| Folder | Contract | Reads | Agent writes |
|--------|----------|-------|--------------|
| `immutable-source-files/` (outside `agents/`) | User documents | ✓ | ✗ (unchanged) |
| `immutable-source-files/agents/**` | Agent memory deposits, nested at any depth | ✓ | ✓ (append-only, OKF frontmatter) |
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

**Caller change (one site):** `okf/write.rs::write_note` switches `WRITABLE_SUBDIRS`
→ `NOTE_WRITABLE_SUBDIRS` at both `safe_vault_path` calls (the initial resolution
and the post-bootstrap retry). `WRITABLE_SUBDIRS` itself is
unchanged (still `[WIKI_DIR]`) for app-internal writers (`run_wiki_forget`, etc.).
`upsert_index_entry` already uses `READABLE_SUBDIRS`; since `agents/` nests under
`immutable-source-files/`, index upserts targeting deposit indexes remain allowed — no
change. `tool_dispatch.rs` adapters stay thin (no change).

### Phase 2: Vault layout creation

All three vault-setup sites additionally `create_dir_all` the deposit dir (idempotent):

| Site | Creates |
|------|---------|
| `set_vault_path` (`lib.rs:550`) | `IMMUTABLE_DIR`, `WIKI_DIR`, `AGENTS_DEPOSIT_DIR`, `.brain/converted` |
| `switch_vault` (`lib.rs:1180`) | same |
| startup default-vault bootstrap (`lib.rs:2745`) | same, plus the fallback-vault branch (`lib.rs:2780`) |
| `onboard::run_onboard` (`onboard/mod.rs:200`) | delegates to `vault::layout::create_vault_layout` (`layout.rs:11`), which creates the same set |

**Duplication note:** the first three sites inline the `create_dir_all` calls that
`create_vault_layout` already encapsulates. Folding them into that helper is a
worthwhile follow-up — it would have made the deposit dir a one-line change
instead of a four-site one — but is out of scope here.

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

- Watcher roots at the **vault root** with `RecursiveMode::Recursive`
  (`watcher/fs_watcher.rs:49-55`), so `immutable-source-files/agents/**` — at any
  depth — is already covered. None of the ingestion pipeline changes.
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

- [x] `ad1` — write `immutable-source-files/agents/mem.md` via `vault_write_note` succeeds
- [x] `ad3` — write `immutable-source-files/secrets.md` (deposit prefix absent) → `PathOutsideVault`
- [x] `d3` — traversal `immutable-source-files/agents/../../x.md` → `Traversal` (components-based)
- [x] `ad4` — first deposit into a missing `agents/` dir bootstraps parents and succeeds
- [x] `ad2` — write `immutable-source-files/agents/people/tessera/x.md` (subfolder) **succeeds** (rev 2; replaces the rev-1 flat-layout rejection test)
- [x] `e2` — deep deposit (4 levels under `agents/`) succeeds
- [x] `ad8` — sibling prefix `immutable-source-files/agents-evil/…` rejected
- [x] `ad3b` — a **rejected** write leaves no directories behind (no-side-effect)
- [x] `ad3c` — a symlinked component under `agents/` is never traversed when bootstrapping parents
- [x] Vault-setup sites create `immutable-source-files/agents/` (set/switch/default bootstrap/onboard)
- [x] `ad5`/`ad5b`/`ad6`/`ad7`/`ad9` — `supersedes` validates: deposit-to-deposit only, target must exist, nested targets allowed
- [x] `WRITABLE_SUBDIRS` unchanged (app-internal writers unaffected)
- [x] **FULL suite green** — `cargo test --features test-utils,mcp-server,tauri/test` incl. `tests/` integration, 0 failures (PR #115 lesson: `--lib` alone is not evidence). Note the `tauri/test` feature is required or the test target fails to compile (`cannot find test in tauri`).
- [ ] Deposit-index upsert (`upsert_index_entry` into `agents/`) — allowed by construction via `READABLE_SUBDIRS`, but **untested**

## Risks / Trade-offs

| Risk | Mitigation |
|------|------------|
| Allowed-dir-not-exists silently drops the prefix (live-verified) | Phase 2 creation + Phase 1 bootstrap; regression test for the lazy path |
| Deposit sprawl (unbounded append-only growth) | Supersession chains + heal flags; rev 2 removed the flat-folder constraint, so subfolders are now the organizing tool — pruning remains a future, explicit decision |
| `supersedes` typo'd path | Target-must-exist validation |
| Semantic drift: "immutable" now includes agent deposits | Accepted (Kurt): still immutable in the sense that the librarian never rewrites it; refine naming later |

## Open Questions

**None.** Decisions locked Aug 27 2026: deposit folder named `agents/` (nested under
`immutable-source-files/`); supersession = newest wins + heal flags conflicts; retrieval
contract as documented above.

---

## Ruling history

**Aug 27 2026** — deposit folder `immutable-source-files/agents/`; supersession =
newest wins + heal flags conflicts; retrieval contract as documented above.

**Aug 28 2026** — deposits flat only, no per-agent subfolders. *Superseded.*

**Aug 29 2026 (rev 2, current)** — deposits may **nest at any depth** under
`immutable-source-files/agents/`. Generic vault behavior for Curated Thoughts,
not specific to any one agent. Governing spec, Phase B — deposit layout:
`~/Documents/equational-wiki/immutable-source-files/agents/operations/tessera-memory-unification.md`
(vault-local path; equational-wiki dogfood vault).

**Unchanged across rev 2:**

- Supersession containment is exactly as before, widened only to depth:
  `supersedes` must still reference a **deposit under
  `immutable-source-files/agents/`** (deposit-to-deposit only) and the target
  **must exist** (`invalid_frontmatter:supersedes_not_found:{path}` otherwise).
- Component-based containment (`under_deposit`, now expressed via `under_any`)
  still rejects sibling-prefix paths such as
  `immutable-source-files/agents-evil/…`.
- `NOTE_WRITABLE_SUBDIRS` already allowed the whole `agents/` subtree, so
  `safe_path.rs` needed no change.

**What shipped for rev 2 (`a552753`):** the rev-1 `is_flat_deposit()` helper and
its rejection block were deleted; the supersedes check moved to `under_deposit()`;
nested and deep-nested deposit writes now succeed.

**Hardening added during rev-2 review:** the parent bootstrap in `write_note` no
longer calls `create_dir_all` directly. Two side effects were possible on a
*rejected* write, because parents were created before round-two containment ran:

1. `create_dir_all` follows symlinks on existing components, so a symlink planted
   under `agents/` let directories be created outside the vault root. Now created
   component-by-component via `create_parents_no_symlink`, which refuses to
   traverse a symlink (`ad3c`).
2. An out-of-tree path (`agents-evil/nested/…`) had its directories created before
   being rejected. The bootstrap is now gated on a lexical `under_any` containment
   check (`ad3b`).

Both predate rev 2 — the same retry was reachable via nested `wiki/` paths on
`main` — but rev 2 widened the surface, so they are fixed here.

**Self-review follow-ups (same rev):**

3. The lexical `under_any` gate initially rejected a leading `./`:
   `Path::components` normalizes interior `.` away but keeps a leading one, so
   `./wiki/deep/new/x.md` compared as `[".", "wiki", …]` and failed to match.
   `safe_vault_path` accepts `./` (it rejects only `..` and prefix components),
   so the gate was strictly stricter than the resolver it fronts — a regression
   against `main`. `under_any` now drops `Component::CurDir`
   (`dot_prefixed_path_with_missing_parents_still_writes`).
4. `create_parents_no_symlink` treated *any* `symlink_metadata` failure as
   "does not exist"; an `EACCES`/`ELOOP` from stat now surfaces as itself
   instead of as a confusing `create_dir` error.
5. `create_parents_no_symlink`'s doc comment now states that the check is
   **not atomic** — a writer racing the loop can swap a just-created directory
   for a symlink before the next `mkdir`. Closing that needs
   `mkdirat(_, O_NOFOLLOW)` per component; `create_dir_all` had the identical
   exposure, so this is a narrowing rather than a guarantee. Exploiting it
   requires local vault write access. Accepted, documented, not fixed here.
