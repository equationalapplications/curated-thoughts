# Empty-Allowlist Bootstrap in `safe_vault_path` (`vault_write_note`) — Design

**Date:** 2026-09-03
**Status:** Proposed
**Branch:** `fix/vault-empty-allowlist-bootstrap`
**Priority:** P2 (correctness; unblocks first-write on fresh vaults; closes issue #119)

## 1. Problem (as filed, and as verified on main @ 446389b)

Issue #119: on a wiki-shaped vault with **neither** `wiki/` nor
`immutable-source-files/agents/` on disk, `vault_write_note` to
`immutable-source-files/agents/x.md` fails with `Path is outside vault
root`, even though `write_note`'s lazy-bootstrap is supposed to create the
parents and retry. Reproduced live on the dogfood vault (v1.34.0 sidecar,
2026-08-28); `mkdir -p <vault>/immutable-source-files/agents` makes the
identical call succeed.

**Verified mechanism today.** In `safe_path.rs:264-271` the allowlist is
built by canonicalizing each allowed subdir and silently dropping the ones
that fail, then bailing early when nothing survives:

```rust
let allowed_canonical: Vec<PathBuf> = allowed_subdirs
    .iter()
    .filter_map(|sub| root_canonical.join(sub).canonicalize().ok())   // :266  missing dir → dropped
    .filter(|canonical_sub| canonical_sub.starts_with(&root_canonical))
    .collect();
if allowed_canonical.is_empty() {
    return Err(SafePathError::Outside);                                // :270  fires for BOTH modes
}
```

That early return runs **before** the `MayCreate` branch can reach its
parent-canonicalize at `safe_path.rs:336-338`, which is the

```rust
SafePathError::NotFound(format!("parent directory not found: {}", user_path))
```

that `write.rs:231` matches on to trigger the bootstrap
(`create_parents_no_symlink` → re-resolve). So the retry path is
unreachable in exactly the state it exists to handle: the recovery is
gated behind a precondition that the thing being recovered from destroys.

With one of the two subdirs present the allowlist is non-empty, the early
return doesn't fire, and bootstrap works — which is why the existing AD
fixtures (always created `wiki/`) never caught it.

## 2. Approach

**Take option 2 from the issue** — stop the empty allowlist from
short-circuiting `MayCreate` — and reject options 1 and 3.

### The change

Gate the early return to `MustExist` only:

```rust
// An empty allowlist is only decisive in MustExist mode. In MayCreate the
// allowed subdir may legitimately not exist YET: the caller's bootstrap
// (write_note) creates it and retries. Returning Outside here pre-empts
// that retry (issue #119). Containment is still enforced below — both
// branches test `allowed_canonical.iter().any(..)`, which is vacuously
// false on an empty list, so nothing is admitted by falling through.
if allowed_canonical.is_empty() && matches!(mode, PathMode::MustExist) {
    return Err(SafePathError::Outside);
}
```

Nothing else in `safe_vault_path` changes. On a fresh vault the flow
becomes: allowlist empty → fall through → `:336` canonicalize of the
missing parent fails → `NotFound("parent directory not found")` →
`write.rs:231` bootstraps → `create_parents_no_symlink` creates
`immutable-source-files/agents` component-by-component → re-resolve, now
with a non-empty allowlist → write succeeds.

### Why this is safe without adding any guard

Three independent properties already hold, and the fix leans on all three
rather than introducing new trust:

1. **Containment is not delegated to the empty check.** Both branches
   gate on `allowed_canonical.iter().any(|sub| ..starts_with(sub))`
   (`:282`, `:339`). `Iterator::any` on an empty list is `false`, so an
   empty allowlist still denies every path — it just denies it a few lines
   later, after the `NotFound` that the caller needs. The early return is
   a fast path, not the security boundary.

2. **The bootstrap is lexically fenced before it touches the disk.**
   `write.rs:240` runs `under_any(path, NOTE_WRITABLE_SUBDIRS)` and
   returns `PathOutsideVault` *before* creating anything. `under_any` is
   component-based and requires `comps.len() > prefix.len()`
   (`write.rs:115-125`), so `immutable-source-files/agents-evil/x.md` and
   a bare `immutable-source-files/agents` both fail to match. A path that
   is not lexically inside an allowed subdir never reaches `create_dir`.

3. **Symlinked components are still refused during bootstrap.**
   `create_parents_no_symlink` (`write.rs:142-169`) stats each component
   with `symlink_metadata` and errors on any symlink before descending, so
   a pre-planted `immutable-source-files` → `/tmp/evil` symlink is
   rejected rather than followed. The post-bootstrap re-resolve
   (`write.rs:255`) then re-canonicalizes and re-checks containment.

**Symlink-escape case specifically.** If an allowed subdir exists but is a
symlink pointing outside the vault, it is dropped by the
`starts_with(&root_canonical)` filter at `:267` and the allowlist can be
empty *even though the directory exists*. Under this change `MayCreate`
falls through, `:336` canonicalizes the (existing) symlinked parent to a
path outside the root, and `:339`'s `any()` over the empty list returns
false → `Outside`. Same verdict as today, reached one step later. This is
asserted directly by a new test (§3) so the gating cannot silently regress
into a hole.

### Why not option 1 (retry inside `write_note`)

Option 1 — have `write_note` catch `Outside`, match the path's first
component against a `NOTE_WRITABLE` prefix, `create_dir_all`, retry —
would put a second, independent containment decision in the caller.
`write_note`'s own doc comment (`write.rs:180-182`) states the invariant
that the resolution is repeated "so every containment/symlink decision
stays inside `safe_vault_path`." Option 1 breaks that invariant to work
around a bug in `safe_vault_path`; two prefix-matchers that must agree
forever is precisely the shape that produced the `agents-evil` sibling-prefix
class of bug this module already guards against. Fix it where it is wrong.

### Why not option 3 (document the `mkdir -p`)

It leaves a first-run failure with an error message (`Path is outside vault
root`) that actively misdescribes the cause — the path is *inside* the
vault; the directory is merely absent. The issue itself rates this
weakest.

### Error-code change (intentional, narrow)

For a **non-existent** path under a **fully absent** allowlist, `MayCreate`
now reports `NotFound("parent directory not found: ..")` where it
previously reported `Outside`. That is the point: the `NotFound` is the
bootstrap trigger. `MustExist` is untouched in every case. No existing test
covers empty-allowlist `MayCreate` (the three `Outside` assertions at
`safe_path.rs:478`, `:513`, `:607` all run `MustExist` with populated
allowlists, and `rejects_allowed_subdir_that_is_symlink_escape` at `:612`
is `MustExist`), so this spec expects **zero churn to existing
assertions** — if any turn red, the change is wrong, not the test.

## 3. Testing

In `src-tauri/src/vault/safe_path.rs` (inline `mod tests`):

- `may_create_reports_not_found_when_no_allowed_subdir_exists` — vault root
  with **no** allowed subdirs on disk; `safe_vault_path(root,
  "wiki/new.md", allowed(), MayCreate)` is
  `NotFound(msg)` where `msg.contains("parent directory not found")`. This
  is the exact discriminator `write.rs:231` matches, so it pins the
  contract the bootstrap depends on, not just "an error".
- `must_exist_still_outside_when_no_allowed_subdir_exists` — same fixture,
  `MustExist` → still `Outside`. Pins the gate.
- `may_create_still_outside_when_sole_allowed_subdir_is_symlink_escape`
  (`#[cfg(unix)]`) — mirrors `rejects_allowed_subdir_that_is_symlink_escape`
  but in `MayCreate`: allowlist emptied by the `starts_with` filter, parent
  exists as an escaping symlink → `Outside`. Proves falling through did not
  open the escape hole.

In `src-tauri/tests/mcp_write_integration.rs` (the AD fixture variant the
issue asks for):

- `first_deposit_succeeds_on_vault_with_neither_wiki_nor_agents` — temp
  vault containing `immutable-source-files/` but neither `wiki/` nor
  `agents/`; `write_note` to `immutable-source-files/agents/x.md` succeeds,
  the file lands with correct OKF frontmatter, and
  `immutable-source-files/agents/` now exists. This is the live repro from
  the issue, in CI.
- `first_write_succeeds_on_vault_with_no_subdirs_at_all` — same but with
  the vault root empty, covering the `wiki/` side and confirming the
  bootstrap creates a two-level chain.
- `bootstrap_refuses_sibling_prefix_dir` — `write_note` to
  `immutable-source-files/agents-evil/x.md` on the bare vault errors and
  **creates no directory** (assert `!agents-evil.exists()`), proving guard
  (2) fires before any `create_dir` and that a rejected write leaves no
  residue.

**Sabotage check:** revert the `matches!(mode, PathMode::MustExist)` gate →
both `may_create_*` unit tests and both integration bootstrap tests fail.
Remove the `under_any` check at `write.rs:240` →
`bootstrap_refuses_sibling_prefix_dir` fails.

**Gate:**

```
cargo test --manifest-path src-tauri/Cargo.toml --lib vault::safe_path
cargo test --manifest-path src-tauri/Cargo.toml --test path_traversal
cargo test --manifest-path src-tauri/Cargo.toml --test mcp_write_integration
```

`path_traversal` is included because it exercises `safe_vault_path`
containment broadly and must stay green untouched.

## 4. Out of scope

- **#125** — TOCTOU in `create_parents_no_symlink`. Adjacent code, and this
  change routes *more* traffic through that function, but the race is
  unchanged by it: the same stepwise loop, same exposure, same requirement
  of local vault write access. Closing it needs fd-relative syscalls
  (`openat(O_NOFOLLOW|O_DIRECTORY)` + `mkdirat`) and a Windows story;
  tracked separately.
- Changing which directories are writable (`NOTE_WRITABLE_SUBDIRS`
  membership) — untouched.
- The `.canonicalize().ok()` silent-drop *itself*. This spec leaves the
  drop in place and stops it being load-bearing for `MayCreate`. Making the
  drop noisy (logging which subdirs vanished) is a plausible follow-up but
  changes observable behavior on every call and is not needed to close #119.
