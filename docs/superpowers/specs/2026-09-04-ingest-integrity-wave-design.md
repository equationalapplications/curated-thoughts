# Spec: Ingest Integrity Wave (PRs 1–4)

**Status:** Approved design, not yet implemented.
**Date:** 2026-09-04
**Baseline:** `main` @ `4a5f17b` (v2.2.0)
**Scope:** Curated Thoughts codebase only. No upstream (`core-llm-wiki` /
`expo-llm-wiki`) changes.

Four independent changes to the ingest spine and CI, sequenced so three can be
implemented in parallel worktrees and the fourth lands after they merge.

| PR | Title | Worktree | Size | Depends on |
| --- | --- | --- | --- | --- |
| 1 | Embed-profile fallback guard | `wt-embed-guard` | ~40 lines | none |
| 2 | Ghost tmp filter in `enqueue_vault_event` | `wt-ghost-tmp` | ~120 lines | none |
| 3 | Rename-aware vault reconciliation (issue #159) | `wt-rename-reconcile` | ~400 lines | none |
| 4 | Clippy in CI (warn-only) | `wt-clippy` | config only | 1, 2, 3 merged |

PRs 1–3 branch from the same `main` commit and may be implemented
concurrently. PR 4 branches from `main` **after** 1–3 have merged, because a
lint pass touches files all three edit.

---

## §1 — Shared context

Every worktree reads this section before touching code. It records the facts
that are easy to get wrong and expensive to get wrong.

### 1.1 The documents/chunks data model

```
documents(id, path, hash, tier, folder_rules_id, last_indexed, status)
chunks(id, doc_id → documents(id) ON DELETE CASCADE, chunk_text, position, ...)
embeddings(chunk_id → chunks(id) ON DELETE CASCADE, ...)
```

Canonical definition: `src-tauri/src/db/schema.rs:13-29`.

**There is no `doc_path` column on `chunks`.** The `doc_path` field that
appears in `vault_related_chunks` results, in `wiki_context` provenance, and in
`search::SearchResult` (`src-tauri/src/search/mod.rs:106`) is
`documents.path`, joined in at query time (`search/mod.rs:174`).

This matters because **issue #159's own text is wrong on this point.** It
proposes to "rewrite `doc_path` on affected chunks" and to "re-point chunk
provenance". No such column exists; there is nothing on a chunk to re-point.
Do not go looking for it. The correct lever is the `documents` row the chunks
already hang from — see §4.

Because of the FK cascade, deleting a `documents` row destroys its chunks and
their embeddings. Updating a `documents` row's `path` leaves every chunk and
embedding attached and untouched. That asymmetry is the whole basis of PR 3.

### 1.2 `documents.status`

Declared at `schema.rs:21` and `schema.rs:305`:

```
'pending' | 'pending_reindex' | 'indexed' | 'error' | 'orphaned'
```

- `pending` — staged, awaiting chunk+embed.
- `pending_reindex` — staged by `queue_full_reindex` / `run_wiki_reembed`;
  must be re-enqueued as a **forced** rechunk or `ingest_file`'s unchanged-hash
  check will silently drop the upgrade.
- `orphaned` — set on `wiki_pages`, not meaningfully on `documents`.

Note: `src-tauri/src/db/queue.rs:116` contains a *different, older* status
CHECK without `pending_reindex`. That is a **test-only** inline schema
(`enqueue_test_schema_sql`, documented at `queue.rs:100-105`), deliberately
decoupled from the canonical one. Do not "fix" it to match `schema.rs`; do not
treat it as the production schema.

### 1.3 The pending drainer already exists

A recurring misreport in the backlog claims PR #130's §5 pending-row drainer is
"still spec-only". It is implemented and wired:

- `src-tauri/src/pipeline/watchdog/sweep.rs:83` — `list_sweepable_pending`
  selects `status IN ('pending','pending_reindex') AND quarantined_at IS NULL`.
- `src-tauri/src/pipeline/watchdog/mod.rs:482` — `sweep()` runs on every
  normal supervisor pass; also at `:452` (post-respawn) and `:477` (drain
  stall).

Therefore recurring "ghost pending" rows are **not** an undrained queue. They
are rows the sweep faithfully re-enqueues forever because the file behind them
never existed or no longer exists. That is PR 2's subject, not a drainer bug.

### 1.4 The two vault-walk call sites

`walk_vault::walk_vault` (`src-tauri/src/walk_vault.rs:159`) is **pure
discovery**: it collects files from disk and returns them. It performs no
database access, no comparison against `documents`, and no orphan marking.

Production callers — there are exactly two, and **only one of them ingests**:

1. `src-tauri/src/lib.rs:3905` — inside a **read-only Tauri command that lists
   pending symlinks** for the approval UI. This is NOT an ingest path.
   **Never wire reconciliation into it.** Doing so would run a pass that
   `DELETE`s `documents` rows every time the UI asks which symlinks need
   approval.
2. `tools/src/cmds.rs:117` (with a re-walk at `:155`) — the `ct` CLI ingest
   path. This is the only full-vault walk that ingests.

**The desktop app never walks the vault for ingest.** Desktop ingest is
entirely event-driven: the filesystem watcher calls `enqueue_vault_event`
(§1.2) and the pipeline drains it. Verify with:

```bash
grep -rn --include='*.rs' "collect_files(\|walk_vault::" src-tauri/src tools/src
```

Consequence for PR 3: reconciliation is wired into the `ct` CLI only. The
watcher already handles renames correctly *while the app is running* — a
`Remove` event deletes the row and chunks cascade. The uncovered case is an
**offline** move (app closed, `git mv`, reopen), which for a desktop-only user
heals on their next `ct ingest`. Closing that gap for pure-GUI users would mean
adding a full vault walk to every app launch; that is deliberately deferred to
a tracking issue rather than rushed into the startup sequence without measuring
the cost on large vaults.

### 1.5 virtual_path vs read_path

`walk_vault` returns `WalkedFile { virtual_path, read_path }`
(`walk_vault.rs:77-80`). They differ only for content reached through a tracked
symlink under `<vault_root>/documents/`.

**`documents.path` stores `virtual_path`.** Confirmed at `tools/src/cmds.rs:217`,
where `ingest_document_virtual` is called with `virtual_str` as the stored path
and `read_str` only as the byte source.

Any code comparing database paths to walk results MUST compare against
`virtual_path`. Comparing against `read_path` will report every symlinked file
as vanished.

### 1.6 Content hashing

`documents.hash` is the lowercase hex sha256 of the file's bytes. The reference
implementation is `sha256_hex` at `src-tauri/src/db/queue.rs:84`. Reuse it
rather than writing a second hasher; if it needs to be visible to a new module,
make it `pub(crate)` rather than duplicating it.

### 1.7 Verification contract (binding on every PR here)

No subagent may report a task complete without pasting the actual output of the
commands it ran. "Tests pass" without the test output is not a completion
signal and must be rejected by the orchestrator.

Minimum gate for every PR in this wave:

```bash
# Matches what CI actually runs (.github/workflows/ci.yml). The feature flags
# and --test-threads=1 are not optional: the mcp-server feature is required for
# the crate to build the way CI builds it, and there is a known parallel-
# execution flake in paths::tests that single-threading avoids.
cargo test --manifest-path src-tauri/Cargo.toml \
  --features test-utils,mcp-server -- --test-threads=1

cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
```

No PR in this wave touches TypeScript, so `pnpm test` is not part of the gate.

If the `mcp-server` feature fails to build locally for want of sidecar
binaries, CI pre-creates placeholders (`ci.yml`, step "Pre-create placeholder
sidecar binaries") because `tauri-build` validates `externalBin` paths at
compile time. Reproduce that locally rather than dropping the feature flag.

PR-specific additions are listed in each section.

---

## §2 — PR 1: Embed-profile fallback guard

**Worktree:** `wt-embed-guard` · **Branch:** `fix/embed-profile-no-silent-fallback`

### Problem

A silent downgrade to a local embedding profile poisons the vector space: new
vectors are written by a different model than the ones already indexed, so
similarity scores across the corpus become meaningless. Nothing surfaces an
error; recall quality just quietly degrades. This is the oldest standing
directive in the backlog ("no path ever routes a failed external embed to
local") and it is not yet enforced.

There are **two** sites, and they need **different** treatment.

**Site A — `src-tauri/src/vault/config.rs:81-86`:**

```rust
pub fn get_embed_profile(&self) -> Result<EmbedProfile> {
    let paths = self.brain_paths();
    let report = BrainConfig::load_lenient(&paths)
        .map_err(|e| anyhow::anyhow!("config.json failed to load: {e}"))?;
    Ok(report.config.embed_profile.unwrap_or_default())
}
```

`EmbedProfile::default()` is `Local { model: "nomic-embed-code" }`
(`embedder/mod.rs:163-169`) — an Ollama profile. So a config that parses fine
but carries no `embed_profile` key yields a local profile. This affects every
caller, including the live search path at `lib.rs:2325`.

**Site B — `src-tauri/src/lib.rs:3247-3249`:**

```rust
let embed_profile = config
    .get_embed_profile()
    .unwrap_or_else(|_| crate::embedder::EmbedProfile::default());
```

Here a genuine load *failure* is swallowed and becomes a local profile.

### The line to draw

These are not the same defect and must not get the same fix.

- **Absent key is legitimate.** A fresh install has no `embed_profile`
  configured and must still work. Defaulting there is correct behavior, not a
  bug. It should, however, be **explicit and logged once**, not an invisible
  `unwrap_or_default()`.
- **A load failure is never legitimate.** An unreadable or malformed config
  must not be silently reinterpreted as "use the local model."

### Design

1. In `config.rs`, keep the absent-key default but make it deliberate: match on
   `report.config.embed_profile`, and on `None` emit a one-time
   `eprintln!("[embed] no embed_profile configured; defaulting to {…}")`
   before returning the default. The load-error arm already propagates via `?`
   — leave it.
2. In `lib.rs:3247`, delete the `unwrap_or_else`. A failure here occurs during
   Tauri setup, so surface it: log the error at `eprintln!` level and propagate
   it into the startup failure path rather than substituting a default profile.
   Do not `panic!` — a corrupt config should produce a legible startup error,
   not a crash dump.

### Do not touch

`embedder::embed_batch` (`embedder/mod.rs:196-204`) is **already correct**: it
dispatches `External → profile.embed()` and returns `Err` on failure with no
fallback branch. `EmbedProfile::Local` means Ollama, not fastembed; fastembed
survives only in `Embedder` for frozen bench fixtures and the `init_fastembed`
command. There is no bug in the dispatch layer. Leave it alone.

The many `EmbedProfile::default()` uses in `#[cfg(test)]` blocks and in
`lib.rs:3045` (deliberately a `Cloud` profile, with a long comment explaining
why) are intentional. Leave them alone.

### Acceptance criteria

- AC1: `get_embed_profile` on a config whose `config.json` fails to load
  returns `Err`, never a `Local` profile. (Regression test.)
- AC2: `get_embed_profile` on a valid config with no `embed_profile` key
  returns the default **and** the absent-key branch is explicit in source.
- AC3: No `unwrap_or_else(|_| EmbedProfile::default())` remains in
  non-test code. Verify with:
  `grep -rn "unwrap_or_else.*EmbedProfile::default" src-tauri/src`
- AC4: Existing tests in `vault/config.rs:217-265` still pass unchanged.

### Files

`src-tauri/src/vault/config.rs`, `src-tauri/src/lib.rs`.

---

## §3 — PR 2: Ghost tmp filter in `enqueue_vault_event`

**Worktree:** `wt-ghost-tmp` · **Branch:** `fix/ghost-tmp-enqueue-filter`

### Problem

Nightly runs accumulate `documents` rows in `status='pending'` whose backing
file does not exist — 15 ghosts on 2026-09-04 against 0 real pending rows. The
supervisor sweep (§1.3) re-enqueues them on every pass, forever.

Root cause is an asymmetry between the two ingest entry points:

- The **walker** filters. `walk_vault::is_excluded_file`
  (`src-tauri/src/walk_vault.rs:61`) screens lockfiles, generated changelogs,
  and generated-output path segments.
- The **watcher** does not. `enqueue_vault_event`
  (`src-tauri/src/db/queue.rs:26`) applies no exclusion whatsoever. It stages
  whatever the filesystem event names, including editor temp files that exist
  for milliseconds.

A secondary contributor: the Add/Modify arm reads bytes at `queue.rs:67-68`
with `.with_context(...)`. When a temp file vanishes between the event firing
and the read, this returns `Err` for a condition that is not an error — the
file is simply gone, which is a delete.

### Design

1. **Promote the exclusion predicate.** `is_excluded_file` and its two const
   tables are private to `walk_vault`. Make the predicate reachable from
   `db::queue` — either by making it `pub(crate)` in place or by moving it to a
   small shared module. Prefer the smaller change. `walk_vault`'s own call site
   must keep working unchanged.
2. **Extend it with editor temp patterns.** Add a rule covering at minimum:
   files whose name ends in `~`, `.tmp`, `.swp`, or `.swx`; names beginning
   `.#` (Emacs lock) or `#` (Emacs autosave); and vim's numeric probe file
   `4913`. Keep these as a named const table alongside the existing ones, in
   the same style.
3. **Call it from `enqueue_vault_event`.** Screen the canonicalized path
   *after* the vault-root containment check at `queue.rs:45-52` and *before*
   the Remove branch, so an excluded path is dropped on every event kind.
   Screening before the Remove branch means a Remove event for an excluded
   path will not clean up a row staged before this filter existed. That is
   intentional: those pre-existing ghost rows are cleaned up by PR 3's
   reconciliation pass (§4), which deletes rows whose files are gone. Do not
   add a second cleanup path here.
4. **Treat a vanished file as a delete.** In the Add/Modify arm, when
   `std::fs::read` fails with `ErrorKind::NotFound`, delete any `documents` row
   for that path and return `Ok(())` instead of propagating an error. Any other
   IO error keeps its current error behavior.

### Acceptance criteria

- AC1: `enqueue_vault_event` for an excluded path (each new pattern, table
  driven) inserts no `documents` row.
- AC2: `enqueue_vault_event` with a `Create` event for a path that does not
  exist on disk returns `Ok(())` and leaves no row.
- AC3: A pre-existing row whose file has vanished is removed when an
  Add/Modify event fires for it.
- AC4: A non-NotFound IO error still propagates as `Err`.
- AC5: `walk_vault`'s existing exclusion tests still pass unchanged.
- AC6: A normal `.md` file is still enqueued — the filter must not over-match.

### Files

`src-tauri/src/db/queue.rs`, `src-tauri/src/walk_vault.rs` (visibility only, or
a new small shared module).

### Note for the implementer

`queue.rs`'s test module uses `enqueue_test_schema_sql` (§1.2) — a deliberately
divergent inline schema. Add new tests in that module using the same fixture
style. Do not reconcile that schema with `schema.rs`.

---

## §4 — PR 3: Rename-aware vault reconciliation (issue #159)

**Worktree:** `wt-rename-reconcile` · **Branch:** `fix/159-rename-reconciliation`

Read §1.1 and §1.5 before starting. The issue text is wrong about the data
model; §1.1 explains how.

### Problem

Reproduced against the production brain on 2026-09-03: a file moved with
`git mv` (byte-identical, 100% rename) leaves `vault_related_chunks` returning
the **old** path as provenance. Citations point at files that no longer exist,
and they rot silently — nothing errors.

The live-watcher path handles moves correctly: a `Remove` event deletes the
`documents` row (`queue.rs:57-63`) and chunks cascade. The gap is the
**offline** move — app closed, `git mv`, restart. On restart `walk_vault`
discovers the new path and ingests it as a new row, while nothing ever compares
the database against the filesystem, so the old row and its chunks survive
indefinitely. The result is two rows with the same content hash, one of them
pointing at nothing.

The path-spelling fallback in `build_path_candidates`
(`src-tauri/src/tool_dispatch.rs:61`) is what made this look like a
partially-working feature in the repro: lookup by the *new* path succeeds
because the candidate list is generous, while the returned provenance still
carries the stale path.

### Design

Add a reconciliation pass that runs after the walk and before ingest, in a new
module `src-tauri/src/reconcile.rs`, exposed as one function called from the
`ct` CLI ingest path only (`tools/src/cmds.rs:117`). See §1.4 for why the
desktop call site is excluded — it is a read-only listing command, not an
ingest path.

```rust
pub struct ReconcileOutcome {
    pub repointed: Vec<(String, String)>,  // (old_path, new_path)
    pub deleted: Vec<String>,
    pub ambiguous: Vec<String>,
}

pub fn reconcile_vault(
    conn: &mut Connection,
    walked: &[WalkedFile],
) -> Result<ReconcileOutcome>;
```

Algorithm:

1. Load every `documents` row with `tier = 'user_doc'` as `(path, hash)`.
   **Only `user_doc`.** Wiki-tier rows are not all filesystem-backed and must
   never be reconciled against a vault walk.
2. Build the set of walked `virtual_path` values (§1.5 — `virtual_path`, not
   `read_path`).
3. `vanished` = database paths absent from the walked set.
   `unknown` = walked paths absent from the database.
4. Hash each `unknown` path's bytes with `sha256_hex` (§1.6). Hash only
   `unknown` paths — never re-hash the whole vault.
5. For each `vanished` row, look for an `unknown` path with an identical hash:
   - **Exactly one match, and that hash is claimed by exactly one vanished
     row** → `UPDATE documents SET path = ?new WHERE path = ?old`. Chunks and
     embeddings stay attached; nothing re-embeds. Record in `repointed`.
   - **No match** → `DELETE FROM documents WHERE path = ?old`. Chunks cascade.
     Record in `deleted`.
   - **Ambiguous** — the hash is claimed by more than one `unknown` path, or
     more than one vanished row shares it → **skip entirely.** Change nothing,
     record in `ambiguous`, log it. Never guess which of several
     identical-content files is "the" rename.
6. Run the whole pass in a single transaction.
7. Log a one-line summary at both call sites.

Order matters: reconcile **before** ingest, so a re-pointed row is already at
its new path when the ingest loop reaches that file and short-circuits on the
unchanged hash instead of duplicating work.

### Why re-point rather than delete-and-re-ingest

A re-point is a single `UPDATE` that preserves every chunk and embedding. The
alternative pays the full embedding cost of the moved content and leaves a
recall gap until the sweep catches up. Vault reorganizations move many files at
once, so that cost is not hypothetical.

### Edge cases to handle explicitly

- **Path collision.** Never `UPDATE` a row's path to a path that already has a
  row — `documents.path` is `NOT NULL UNIQUE` (`schema.rs:15`) and the write
  would fail. If the target path already exists in the database, treat the
  vanished row as unmatched and delete it.
- **Empty walk.** If the walk returned zero files (a misconfigured or
  unmounted vault root), **skip reconciliation entirely** and log a warning.
  Otherwise a transient mount failure would delete the entire index.
- **Non-UTF-8 paths.** Skip with a recorded warning, matching the existing
  handling at `tools/src/cmds.rs:200-212`.
- **Unreadable `unknown` file.** Skip it as a match candidate; it simply can't
  participate in rename detection.

### Acceptance criteria

- AC1: A 100% rename (identical bytes, new path) re-points the row. The chunk
  count for that document is unchanged and chunk IDs are stable.
- AC2: After AC1, a query returns the **new** path as `doc_path`.
- AC3: A deleted file's row is removed and its chunks are gone (cascade).
- AC4: Two vanished files with identical content, and two new paths with that
  same content, leave all rows untouched and land in `ambiguous`.
- AC5: A rename whose target path already has a `documents` row deletes the
  vanished row and does not violate the UNIQUE constraint.
- AC6: `tier='wiki'` rows are never modified, even when their path is absent
  from the walk.
- AC7: An empty walk result makes no database changes.
- AC8: `reconcile_vault` is called from the `ct` CLI ingest path in
  `tools/src/cmds.rs`, after the walk and before the ingest loop. It is NOT
  called from `src-tauri/src/lib.rs` — verify `lib.rs` is absent from the
  diff.
- AC9: A modified-in-place file (same path, different hash) is not treated as
  vanished and is not touched by reconciliation.

### Files

New: `src-tauri/src/reconcile.rs`, plus its `mod` declaration.
Modified: `tools/src/cmds.rs` (call site), `src-tauri/src/db/queue.rs`
(`sha256_hex` visibility), `src-tauri/src/lib.rs` (**module declaration only** —
adding `pub mod reconcile;` alongside the existing `pub mod walk_vault;` at
`lib.rs:34`; no logic changes).

### Out of scope

Do not add a `ct` maintenance subcommand. Automatic reconciliation on the
ingest path covers the reported workflow; a manual command is redundant surface
area. Do not add a `doc_path` column to `chunks` (§1.1). Do not add a vault
walk to desktop startup.

**Named follow-up (do not do it in this PR):** file a tracking issue,
"Desktop startup reconciliation for offline moves", covering the pure-GUI user
who never runs `ct ingest`. It needs a performance measurement of a full vault
walk at launch on a large vault before any implementation.

---

## §5 — PR 4: Clippy in CI (warn-only)

**Worktree:** `wt-clippy` · **Branch:** `chore/clippy-ci-warn-only`
**Blocked until PRs 1–3 have merged to `main`.**

### Problem

Verified absent on 2026-09-04: no clippy step in any of the four workflows
(`.github/workflows/{build,ci,codeql,release}.yml`), no `clippy.toml`, no
`rust-toolchain.toml`. Lint drift is unbounded and toolchain versions are
unpinned across local and CI.

### Design

1. Add `rust-toolchain.toml` declaring **`channel = "stable"`** plus
   `components = ["clippy", "rustfmt"]`.

   **Do not pin a fixed version number.** Rolling stable is a deliberate,
   documented decision in this repo, commented identically at `ci.yml:64`,
   `ci.yml:117`, and `build.yml:63`:
   `toolchain: stable # intentional rolling stable; see spec Post-execution outcomes`.
   A version pin would silently override that decision in all three places.
   The file's job here is local/CI **component** parity — guaranteeing
   `cargo clippy` and `cargo fmt` exist on a contributor's machine — not
   version pinning. Leave the three workflow `toolchain:` lines and their
   comments untouched.
2. Add a minimal `clippy.toml`. Start empty-but-present with a comment
   explaining its role; do not pre-tune thresholds nobody has hit yet.
3. Add a clippy step to `ci.yml`, **warn-only**: `cargo clippy` without
   `-D warnings`, and the step must not fail the job. Place it in the
   `rust-ubuntu` job after the `swatinem/rust-cache` step and before
   "Pre-create placeholder sidecar binaries".

### Why warn-only

Turning on `-D warnings` in the same change surfaces an unknown-size lint
backlog and blocks the pipeline on cleanup unrelated to any current defect.
Landing the infrastructure first makes the backlog *visible* and lets the
cleanup be scoped and reviewed on its own terms.

**Named follow-up (do not do it in this PR):** once the backlog is triaged,
flip the step to `-D warnings` and make it blocking. File this as a tracking
issue when PR 4 merges.

### Acceptance criteria

- AC1: `ci.yml` runs clippy on every PR.
- AC2: A crate with an intentional lint does not fail the build.
- AC3: `clippy.toml` and `rust-toolchain.toml` exist at the repo root.
- AC4: `rust-toolchain.toml` declares `channel = "stable"` and contains no
  fixed version number.
- AC5: The three `toolchain: stable` lines in `ci.yml` and `build.yml` are
  unchanged, comments included. Verify with `git diff` on those files.
- AC6: The follow-up tracking issue is filed and linked in the PR body.

### Files

`.github/workflows/ci.yml`, `clippy.toml`, `rust-toolchain.toml`.

---

## §6 — Orchestration contract

For the orchestrator driving subagents across worktrees.

### Worktrees

| PR | Worktree | Branch | Base |
| --- | --- | --- | --- |
| 1 | `wt-embed-guard` | `fix/embed-profile-no-silent-fallback` | `main` @ `4a5f17b` |
| 2 | `wt-ghost-tmp` | `fix/ghost-tmp-enqueue-filter` | `main` @ `4a5f17b` |
| 3 | `wt-rename-reconcile` | `fix/159-rename-reconciliation` | `main` @ `4a5f17b` |
| 4 | `wt-clippy` | `chore/clippy-ci-warn-only` | `main` after 1–3 merge |

This spec file must be present in every worktree. PRs 1–3 may run
concurrently; they touch disjoint code with one exception noted below.

### The one overlap

PR 2 and PR 3 both touch `src-tauri/src/db/queue.rs` — PR 2 edits
`enqueue_vault_event`'s body, PR 3 only changes `sha256_hex`'s visibility.
These do not collide textually, but whichever merges second should rebase and
re-run its tests rather than assuming a clean merge.

### Merge rules

- **Regular merges only.** `gh pr merge --merge`. Never `--squash`, never
  `--rebase`. Full branch history is preserved deliberately.
- Before declaring any PR ready, check both mergeability and CI on the tip SHA:
  `gh pr view <n> --json mergeable,mergeStateStatus` — a `CONFLICTING` PR runs
  **zero** CI checks silently, reporting `total_count: 0` with no error. A PR
  with no check runs is not a passing PR.
- Do not trust a PR body's claim that threads are resolved. Query review
  threads directly.

### Subagent instructions

Each subagent gets: this spec, its own §, and §1 + §6. It must not edit files
outside its § "Files" list. If it believes it needs to, it stops and reports
rather than expanding scope.

Every completion report must include pasted command output per §1.7. A report
without output is rejected and the task is re-run.

### Known-wrong claims to ignore

The improvement backlog contains two assertions that live verification on
2026-09-04 disproved. A subagent that encounters them should disregard them:

1. "PR #130 §5 pending-row drainer is still spec-only." It is implemented and
   wired — see §1.3.
2. "Spec #136 is merged but implementation is pending." PR #136 shipped ~2,200
   lines across 21 files; the ontology seed, tier column and filter, and
   `wiki_context` are live on `main`. The only genuine remainder is
   `src/lib/folderTypeMap.ts`, whose `resolveFolderType` has no production call
   site. That is a separate, later task and is **not** part of this wave.
