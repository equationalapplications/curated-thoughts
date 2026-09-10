# Watcher + walker: exclude `.brain` working directories from ingestion

**Date:** 2026-09-10
**Status:** Implemented (rev 7 — second Opus 5 review of rev 6 + plan addressed:
desktop startup connection must set `PRAGMA foreign_keys=ON` explicitly
(it bypasses `migrate()`, so the chunks cascade never fired there),
relativization gained a canonicalize-the-input fallback (canonical root ×
non-canonical event path), and the ancestor-vault test wording corrected
to absolute paths. Rev 6 — Opus 5 review of rev 5 addressed: desktop purge
root corrected from `raw_docs` to the vault root, D2's "one path space"
claim narrowed to what `queue.rs` actually stores plus an explicit
relativization-failure rule, reconcile pre-pass placed inside the existing
transaction, item 5's rationale corrected, `DenyReason` variant pinned)
**Branch:** docs/spec-2026-09-10-vault-walk-brain-dir-exclusion
**Priority:** Low (log noise; no data corruption)

**D2a unified via issue #204 (2026-09-10).** The watcher's pre-fix canonical-
path write is replaced by a virtual-path write; a one-shot V22 migration
rewrites existing canonical-path rows in place and deletes the trusted-link
phantoms the divergence created. See D2a below for the full wiring.

## Problem

CT's vault accumulates immortal `pending` rows for `.brain` working files.
Verified on Kurt's ThinkPad (2026-09-10, re-verified by a GLM 5.3 review
pass against the live DB and source): documents ids **6185** and **6215** —
`immutable-source-files/agents/people/.brain/errors.log` and
`immutable-source-files/agents/people/tessera/.brain/errors.log` — are
stuck `pending`, never ingest, and are re-enqueued by the sweep every pass.

Root cause (rev-1 of this spec attributed this to the walker; the GLM
review of `pipeline/mod.rs` and `db/queue.rs` proved that wrong):

- The **walker** is not the vector: `collect_files` extension-gates its
  output via `should_ingest_extension` (`walk_vault.rs:169`), and `.log`
  is not an ingestable extension (`chunker/classify.rs:24-69`) — the
  pipeline also early-returns for non-ingestable extensions before
  `upsert_document` (`pipeline/mod.rs:676-685`). The walker never staged
  these rows.
- The **filesystem watcher** is the vector: `enqueue_vault_event`
  (`db/queue.rs:26-108`) hashes and upserts **any** in-vault Add/Modify
  event. Its own comment concedes the gap ("The walker has always
  filtered these; the watcher never did", `queue.rs:70-73`). Its
  exclusion check does not cover `.brain` directories, so every write to
  a `.brain/errors.log` (CT writes its own at `<vault>/.brain/errors.log`,
  `pipeline/mod.rs:484`) stages a `documents` row that ingestion then
  silently skips forever.

`errors.log` is actively appended (the top-level one was modified today),
so rows deleted without a watcher gate would be re-staged immediately —
the watcher path must be fixed or the bug recurs.

## Path space and matching semantics (normative — read before the items)

Every exclusion decision in this spec operates on **one** path space: the
**vault-root-relative path**, matched **component-exact**. This section is
normative because three of the round-4 review findings were caused by
under-specifying it.

**D1 — Relativize before matching. Never match an absolute path.**
`EXCLUDED_DIRS` contains ordinary names (`target`, `build`, `out`, `dist`,
`node_modules`, `.cache`, `venv`, …). A vault legitimately living at
`/Users/kurt/Code/project/build/wiki` or `/var/build/wiki` has `build` as
an *ancestor* component of every absolute path inside it. Matching an
absolute path would reject **every event in that vault**, silently killing
live-sync with no log line. The helper therefore takes an already-relative
path:

```rust
/// `rel` MUST be vault-root-relative. Passing an absolute path is a bug:
/// EXCLUDED_DIRS names occur in ordinary ancestor directories.
pub fn rel_path_has_excluded_component(rel: &Path) -> bool
```

Callers relativize first. There is no absolute-path convenience overload —
the absence of one is the guard.

**D2 — Match the *virtual* (pre-canonicalize) path, never the canonical
read path.** `enqueue_vault_event` canonicalizes via
`std::fs::canonicalize` (`queue.rs:43-44`), which resolves symlinks. That
rewrites the path in two directions, both wrong for exclusion:

- *Strips* `.brain`: a user who redirects
  `<vault>/immutable-source-files/agents/.brain` at an external location
  canonicalizes to `/var/folders/…/tmp.brain/errors.log`. A canonical-path
  gate sees `tmp.brain`, not `.brain`, and the bug recurs for that subtree.
- *Injects* `.brain`: an approved trusted symlink
  `documents/specs` → `…/.brain/specs-target/` canonicalizes into a path
  containing `.brain`. A canonical-path gate would reject events for a
  directory the walker happily ingests, leaving those files stale and
  silent between ingest runs.

The virtual path is the space the **walker** uses, and therefore the space
`reconcile_vault` compares against (`reconcile.rs:53-59`). Concretely, the
gate uses `abs` (the output of `std::path::absolute`, `queue.rs:43`)
relativized against the **as-configured** vault root, falling back to the
canonical root if `abs` does not start with it (macOS `/var` →
`/private/var` and similar), and finally to a canonicalized `abs`
stripped against the canonical root — for a canonical root paired with a
non-canonical event path (symlinked ancestor). Canonicalizing the input
cannot mask a genuinely symlinked-out `.brain`: the strip still misses
once the link resolves, so D2's rejection behavior is unchanged.

**D2a — `documents.path` holds the virtual (configured-root-relative) path
on every write boundary; unification landed in issue #204.** `enqueue_vault_event`
now writes `path_str = abs.to_string_lossy()` (the VIRTUAL path, matching
the walker's `ingest_document_virtual` at `tools/src/cmds.rs:217` and the
documented contract at `reconcile.rs:74-77`). Pre-#204 the watcher wrote the
canonical path (`canonical.to_string_lossy()`); the divergence was latent for
non-symlinked content (where canonical and configured differ only when the
vault root itself canonicalizes — macOS `/var` → `/private/var` is the only
real-world case) but real for any vault with an approved `documents/specs` →
`<external>/.brain/specs-target/` trusted link: a Modify under the symlink
staged a row keyed by the external canonical path, which every subsequent
`reconcile_vault` pass then saw as vanished.

#204 ships two pieces, applied together:

* **Watcher writes `abs`.** The containment check still uses both forms
  (canonical-only would drop symlinked events before the gate runs; abs-only
  would miss the `tmp.brain` case D2 calls out), but the staged row keys on
  the virtual path.
* **Migration V22** (`src-tauri/src/db/schema.rs::MIGRATION_V22`, called from
  `db/connection.rs::v22_unify_documents_path`) rewrites existing
  canonical-path rows to configured-root form in place and deletes trusted-link
  phantoms (rows whose canonical path was outside the vault root — the
  watcher's pre-fix bug shape). V22 is wrapped in `BEGIN IMMEDIATE`/`COMMIT`
  and refuses to run without a resolved `VaultRoots`, in which case it logs
  a loud FATAL and does NOT stamp `schema_version` — leaving the schema below
  22 as a durable "recovery pending" marker that re-fires on every open
  until the user resolves the root.

The D2 test below lands in the right path space after #204: it asserts both
that the event is not gated AND that the staged row's `path` is the virtual
form (`<configured>/documents/specs/x.md`), not the canonical external target.

**D2b — Relativization failure is fail-open.** Both the watcher gate and
the row-oriented predicates of items 3a/3b must handle a path that does not
start with the vault root at all — possible for any row written by an older
code path (pre-#204 canonical-path rows that V22 had not yet rewritten, or
rows pre-dating a vault-root relocation), and structurally for symlinked
content where the canonical form points outside the vault.
`rel_path_has_excluded_component` operates only on a successfully
relativized path; when `strip_prefix` fails against **all** the attempts
(as-configured root, canonical root, canonicalized input against the
canonical root), the path is treated as **not excluded** (the event
stages; the row is left alone). Deleting rows we cannot place inside the
vault would be the same class of unrecoverable mistake the empty-walk
guard exists to prevent.

**D3 — The vault root must be known. Fail-closed on the gate, not on the
bug.** `enqueue_vault_event` reads `CURATED_VAULT_ROOT` from the process
environment (`queue.rs:36`), and **the desktop app never sets it** — which
is precisely the configuration where the reported bug was observed. A gate
that quietly no-ops without the env var would not fix Kurt's ThinkPad.
Therefore `enqueue_vault_event` gains an explicit parameter:

```rust
pub fn enqueue_vault_event(
    conn: &mut Connection,
    event_kind: notify::EventKind,
    raw_path: &Path,
    vault_root: Option<&Path>,   // NEW — explicit; env var is the fallback
) -> Result<()>
```

Resolution order: explicit argument → `CURATED_VAULT_ROOT` → `None`. All
three production call sites know their root and must pass it:
`lib.rs:1072`, `lib.rs:1102`, `lib.rs:1224`, plus the thin re-export
wrapper `tools/src/cmds.rs:671` (which forwards `None` from `ct watch`,
where the env var is the established mechanism). When the root resolves to
`None` the gate is skipped — matching today's behavior for the containment
check on the same line — and a one-time `eprintln!` records it so the
condition is diagnosable rather than invisible.

**D4 — Component-exact, never substring.** `my.brain.notes/`, `.brainish/`,
and `brain/` must all still ingest. Matching is `component == name` over
`Path::components`, reusing `EXCLUDED_DIRS` via `is_excluded_dir`.

## Approach

1. **Watcher gate (the actual fix).** In `enqueue_vault_event`
   (`db/queue.rs`), before staging a new row, compute the vault-relative
   virtual path per D1–D3 and reject when
   `rel_path_has_excluded_component` is true. Placement must respect the
   existing ordering: **after** the `EventKind::Remove` handling
   (`queue.rs:62-68` — deletes must stay ungated so pre-existing rows can
   still heal), alongside the existing `is_excluded_file` call.

2. **Walker exclusion (defense in depth).** Add `".brain"` to
   `EXCLUDED_DIRS` in `src-tauri/src/walk_vault.rs` (line 20). The
   `filter_entry` prune at every depth then guarantees `.brain` content
   can never enter `collect_files` output even if extensions change.
   The vault root itself is exempt from this prune: a vault rooted at
   `<tmp>/.brain/` must still have its files visited, because
   `filter_entry` returning `false` on the root would short-circuit
   the entire `WalkDir` before any descent. The
   `is_excluded_dir` check is therefore applied only to entries whose
   depth is greater than the root — the root itself is returned as-is
   before the prune. A naïve "add `.brain` to `EXCLUDED_DIRS` and let
   `filter_entry` do the rest" reading would silently empty a vault
   whose top-level directory happens to be named `.brain`.

   **Symlink-classification carve-out (round-4 finding 8).** Adding to
   `EXCLUDED_DIRS` is *not* inert: `walk_vault.rs:224` also calls
   `is_excluded_dir(&name)` on each direct-child symlink under
   `documents/` and `continue`s, which would make an approved
   `documents/.brain` trusted link **vanish from classification entirely**
   — not Trusted, not Pending, not Denied, absent from the approvals UI,
   with its previously ingested rows becoming "vanished" and deleted by
   reconcile on the next `ct ingest`. That is silent data loss.
   **Contract:** the symlink-classification call site must report such a
   link as `Denied` with reason "excluded directory name" rather than
   skipping it silently, so it stays visible in the approvals UI and the
   user can rename the link. The `continue` at `walk_vault.rs:224` is
   replaced by a `denied.push(…)` arm; `collect_files`/`filter_entry`
   behavior is unchanged. `DeniedLink.reason` is a plain `String`, but
   every other producer sources it from `DenyReason::message()`
   (`walk_vault.rs:245-249`), and the approvals UI may key off that text.
   This arm therefore adds a **new `DenyReason` variant** (e.g.
   `ExcludedDirName`) whose `message()` returns the reason string, rather
   than pushing a bare literal that no `DenyReason` can produce.

3. **Cleanup of existing rows — reconcile *and* the desktop startup pass.**
   Rows 6185/6215 are already absent from every walker output today (the
   extension gate predates this spec), so once the watcher gate stops
   re-staging them they are deletable as "vanished". Two corrections to
   rev 4:

   **3a — Excluded rows must be deleted *before* rename detection
   (round-4 finding 2).** Rev 4 claimed reconcile's "absence-driven delete
   arm heals them". It does not, reliably: the delete arm is the **last**
   branch of the match (`reconcile.rs:139-145`); a vanished row is first
   offered to hash-based rename detection (`reconcile.rs:125-138`).
   `.brain/errors.log` is routinely rotated or truncated to 0 bytes, and
   **every empty file shares one sha256**. With one empty `.brain/errors.log`
   row and one newly added empty user file (`inbox/todo.md`), the
   unique-hash branch fires and runs
   `UPDATE documents SET path = '…/inbox/todo.md' WHERE path = '…/.brain/errors.log'`
   — a bogus row pointing at the user's file, the real file's own row
   blocked by the UNIQUE path, and chunks left attached to the wrong
   content. With two empty candidates the row lands in `ambiguous` and
   survives forever.
   **Contract:** `reconcile_vault` partitions `vanished` first. Rows whose
   vault-relative path has an excluded component are deleted
   unconditionally in a pre-pass and are excluded from the
   `vanished_per_hash` accounting, so they can neither be repointed nor
   perturb another row's uniqueness verdict. Only the remaining rows enter
   the existing rename/delete match. (They need no exclusion from
   `unknown_by_hash`: that map is built solely from *walked* paths absent
   from `documents` (`reconcile.rs:85-110`), and item 2 guarantees an
   excluded path never appears in walker output.)

   **Transaction placement.** The pre-pass deletes run inside the **same**
   `conn.unchecked_transaction()` as the existing rename/delete match
   (`reconcile.rs:122,148`), not in a separate earlier one, so one rusqlite
   failure rolls the whole pass back. This requires moving the transaction
   open above the pre-pass and, critically, hoisting it above the
   `if vanished.is_empty() { return Ok(outcome) }` early return
   (`reconcile.rs:76`): when the pre-pass consumes every vanished row, that
   return must not skip the commit. The early return becomes a check on the
   *remaining* rows, taken only after the pre-pass has committed.
   `reconcile_vault` accepts `vault_root: &Path` as an explicit parameter
   (parallel to D3 for `enqueue_vault_event`) so the predicate can
   relativize the absolute paths emitted by `collect_files` against the
   as-configured vault root before applying `is_excluded_dir`. A raw
   absolute-path component check would falsely treat an excluded-name
   *ancestor* of the vault root (e.g. a vault at `<tmp>/target/wiki/`)
   as part of the vault-relative path, deleting every row. The existing
   CLI caller (`tools/src/cmds.rs:193`) forwards its known root.

   **3b — Desktop-only users must self-heal (round-4 finding 3).** Rev 4
   made clearing the stuck rows require one `ct ingest` run, because
   `reconcile_vault`'s only production caller is the CLI path
   (`tools/src/cmds.rs:193`). But the bug was reported on the **desktop**,
   and desktop-only users never run `ct ingest`; the startup pass
   (`lib.rs` ~995–1110) only purges rows whose file no longer exists, and
   both `errors.log` files exist. Shipping rev 4 as written would close
   the ingress while leaving the reported symptom live for the primary
   user population.
   **Contract:** the desktop startup pass gains the same targeted purge —
   delete `tier = 'user_doc'` rows whose vault-relative path has an
   excluded component, using `rel_path_has_excluded_component` — so the
   app self-heals on next launch with no CLI step. This is the same
   predicate as 3a and item 4's narrower sibling; no third notion of
   "excluded" is introduced.

   **The root for that relativization is the vault root — `target_canonical`
   (`lib.rs:995`) — not `raw_docs`.** `raw_docs` is
   `target_canonical.join(IMMUTABLE_DIR)`, i.e.
   `<vault>/immutable-source-files`, one level *inside* the vault.
   Relativizing against it fails `strip_prefix` for any row above that
   directory, and by D2b a failed relativization is fail-open — so a purge
   scoped to `raw_docs` would silently leave `<vault>/.brain/errors.log`
   staged forever. That is the file CT writes itself (`pipeline/mod.rs:484`)
   and the one the Problem section records as actively appended today, so
   getting this root wrong reproduces the reported bug for the top-level
   case while appearing to fix it for the nested ones. Item 5's walk filter
   takes the same root.

4. **Empty-walk sub-case — narrowed to `.brain` (round-4 finding 5).**
   `reconcile_vault` short-circuits when `walked.is_empty()`
   (`reconcile.rs:48-51`) so a transient mount failure cannot delete the
   entire index. That protection stays. Rev 4 punched a hole in it for all
   of `EXCLUDED_DIRS`, which is over-scoped: rows under `.git/`,
   `node_modules/`, `target/`, `__pycache__/` etc. may exist from older
   code paths or external tools, and deleting them on an unmounted vault
   is exactly the disaster the guard exists to prevent — for those names
   we have no proof the row *should* be absent, only that today's walker
   would not emit it.
   **Contract:** on `walked.is_empty()`, `reconcile_vault` performs a
   targeted delete of `user_doc` rows whose vault-relative path contains a
   **`.brain`** component — and nothing else. Every other row, excluded
   name or not, is preserved untouched. Rationale for the `.brain` carve-out
   specifically: CT owns that directory, writes into it itself
   (`pipeline/mod.rs:484`), and no CT code path has ever legitimately
   ingested from it. The narrow predicate is a distinct helper
   (`rel_path_has_brain_component`) so the scope difference is visible at
   the call site rather than implied by a shared constant.

   **Transaction and cascade contract (round-4 finding 9).** The new
   empty-walk branch wraps its deletes in `conn.unchecked_transaction()?`
   and commits at the end, matching the existing path
   (`reconcile.rs:122,148`), so a mid-loop rusqlite error rolls back
   rather than leaving a half-deleted index. Chunk cleanup relies on
   `chunks.doc_id` `ON DELETE CASCADE`, which fires **only** with
   `PRAGMA foreign_keys=ON`; that pragma is per-connection and set in
   `db/connection.rs:35` for every connection opened through the standard
   path — **but the desktop startup pass opens its connection raw**
   (`lib.rs:1021-1035`, `rusqlite::Connection::open` + `busy_timeout`
   only, never routed through `migrate()`), so item 3b must set
   `PRAGMA foreign_keys=ON` on that connection explicitly, after the
   `busy_timeout` block. Without it, both the new purge and the
   pre-existing Remove purge (`lib.rs:1072`) orphan the `chunks` rows
   behind every deleted `documents` row. The empty-walk regression test
   and the 3b purge test assert the chunk count reaches zero rather than
   assuming the cascade.

5. **Desktop startup walk also honors the exclusion (round-4 finding 10).**
   The startup pass builds its own `walkdir::WalkDir::new(&raw_docs)`
   inline (`lib.rs:1083`) with **no** `filter_entry` and no consultation of
   `EXCLUDED_DIRS`. Rev 4's claim that "the `filter_entry` prune at every
   depth guarantees `.brain` content can never enter walker output" is
   true for `walk_vault::collect_files` and false here: every nested
   `.brain/` subtree is descended into and `file_type()`/`metadata()`-read
   on every app launch. **This is a consistency and cost fix, not a
   correctness one** — rev 5 claimed those entries were "rejected at enqueue
   time", which is wrong: the loop already extension-gates via
   `should_ingest_extension(ext)` (`lib.rs:1092-1094`), so a `.brain/*.log`
   never reaches `enqueue_vault_event` from this path at all. The walk gains
   the same vault-relative exclusion filter so that one notion of "excluded"
   governs every traversal, and so the descent cost disappears.

6. **`.brain/proposed` is intentionally excluded too.** `vault/safe_path.rs`
   sanctions `.brain/proposed` as a write location for proposed content
   operations — but proposed documents reach the wiki through the
   proposals pipeline and OKF export, never through vault ingestion.
   Excluding the whole `.brain` tree from ingestion is therefore intended
   for `proposed` as well; stated explicitly so reviewers don't flag it as
   a regression.

### Rejected alternatives

- **Exclude only `errors.log` by suffix.** Rejected: `.brain/` holds the
  brain DB, embeddings, and conversion shadow copies
  (`pipeline/mod.rs:274-286`) — none are vault content.
- **Extend `folder_rules` with an `exclude` mode.** Rejected: per-folder
  rules are a user-facing librarian-policy feature; CT-owned working dirs
  should be excluded unconditionally, like `.git`. (The schema CHECK at
  `db/schema.rs:62-63` confirms there is no exclude mode today.)
- **Substring path-segment matching** (`EXCLUDED_PATH_SEGMENTS`-style
  `contains`). Rejected in rev 1 and re-rejected: over-matches lookalikes
  (`my.brain.notes/`); component-exact matching is the correct semantics.
- **Gating Remove events.** Rejected: would strand pre-existing rows
  (see `queue.rs:62-68` comment).
- **Matching against the canonical absolute path** (rev 4's implied
  reading). Rejected per D1/D2: breaks every vault under an
  `EXCLUDED_DIRS`-named ancestor, and both misses and over-rejects
  symlinked subtrees.
- **Leaving cleanup to `ct ingest` alone** (rev 4). Rejected per 3b: the
  reported bug is on the desktop, where that command is never run.

## Error handling

- Watcher gate: pure rejection before any hashing/DB work. The one new
  failure mode is an unresolvable vault root (D3), which skips the gate,
  preserves today's behavior, and logs once.
- Walker exclusion: `filter_entry` removes entries before any file I/O.
  The symlink-classification path changes a silent `continue` into a
  visible `Denied` entry (item 2) — strictly more information, no new
  failure mode.
- Reconcile: the excluded pre-pass (item 3a) and the empty-walk branch
  (item 4) both run inside the existing transaction discipline; a rusqlite
  failure rolls back the whole pass. A row whose path cannot be relativized
  against either the as-configured or the canonical vault root is left
  untouched (D2b), never deleted.
- Desktop purge: a delete of rows the pipeline can never ingest; failure
  is logged and non-fatal, and the next launch retries.

## Testing

- **Watcher unit test:** emit Add/Modify events for
  `<vault>/.brain/errors.log`, `<vault>/nested/.brain/x.log`, and control
  paths (`<vault>/notes.md`, `<vault>/brain/x.md`,
  `<vault>/my.brain.notes/x.md`, `<vault>/.brainish/x.md` — the
  substring-lookalike controls: a faulty `contains(".brain")` check would
  wrongly exclude them, so asserting they stage pins D4). Assert staged
  rows exist only for control paths.
- **Watcher regression — excluded-name vault root (D1, finding 1):** vault
  root at `<tmp>/build/wiki` (and a second case at `<tmp>/target/wiki`);
  assert `<root>/notes.md` still stages. Without relativization this test
  fails, which is the point.
- **Watcher regression — symlinked `.brain` (D2, finding 7):**
  `<vault>/nested/.brain` is a symlink to an external directory whose real
  name has no `.brain` component; assert an event for a file inside it is
  still rejected.
- **Watcher regression — trusted link into a `.brain` target (D2,
  finding 4):** `documents/specs` → `<external>/.brain/specs-target/`;
  assert a Modify event for `documents/specs/x.md` **stages** — i.e. that
  the gate does not reject it. Per D2a (post-#204) the staged row's
  `documents.path` is the *virtual* path (`documents/specs/x.md`); the
  external canonical form MUST NOT appear in `documents`, because that
  was the pre-fix divergent phantom row. Pin the stored path explicitly
  so the test asserts the row lands in the walker's path space and
  catches any future regression that restores the canonical-path write.
  than being papered over by a green test.
- **Watcher + purge regression — unrelativizable path (D2b):** an event
  whose `abs` starts with neither the as-configured nor the canonical vault
  root; assert it stages (fail-open). Companion row-side case: a
  `user_doc` row whose stored path lies outside the vault root; assert
  both the 3a pre-pass and the 3b desktop purge leave it untouched.
- **Watcher regression — no vault root (D3):** with `CURATED_VAULT_ROOT`
  unset and `vault_root: None`, assert behavior is unchanged from today
  (gate skipped, event stages).
- **Remove-ordering regression:** a pre-staged `.brain` row is still
  deleted when its Remove event arrives.
- **Walker unit test:** fixture vault containing `notes.md`,
  `.brain/errors.log`, `nested/.brain/errors.log`, `brain/` lookalike,
  `my.brain.notes/x.md`, `.brainish/x.md`. Assert only intended files are
  walked.
- **Walker regression — vault root named `.brain`:** fixture vault root
  at `<tmp>/.brain/` containing `notes.md`, `nested/.brain/errors.log`,
  and `my.brain.notes/x.md`. Assert `notes.md` and
  `my.brain.notes/x.md` are walked, while the nested
  `.brain/errors.log` is pruned (root exempt, descendants matched).
  Without the root-exemption carve-out the entire walk returns empty.
- **Walker symlink-classification test (finding 8):** a `documents/.brain`
  symlink is reported as `Denied` with the new `DenyReason` variant's
  message — asserting it is neither silently dropped nor classified
  `Trusted`.
- **Reconcile test (non-empty walk):** a pre-staged `.brain/errors.log`
  row is deleted and chunks cascade.
- **Reconcile test — empty-file hash collision (finding 2):** DB holds an
  empty `.brain/errors.log` row and the walk contains an empty
  `inbox/todo.md` not yet in the DB, both hashing to the
  empty-file sha256. Assert the `.brain` row is **deleted**, not repointed,
  and that `inbox/todo.md` is untouched by reconcile. A second case with
  *two* empty candidates asserts the `.brain` row is still deleted rather
  than landing in `ambiguous`.
- **Reconcile test (empty walk, finding 5 scope):** vault whose only
  content is `.brain/errors.log`; DB has a `.brain/errors.log` row, an
  unrelated `notes.md` row, **and** a `node_modules/x.md` row. Assert:
  `.brain/errors.log` deleted with chunk count zero, `notes.md` preserved,
  and `node_modules/x.md` **preserved** (the hole in the mount-failure
  safety net is `.brain`-only).
- **Reconcile regression — vault under excluded ancestor:** vault root
  at `<tmp>/target/wiki/` (an absolute ancestor path whose
  component `target` matches `EXCLUDED_DIRS`); DB has a `notes.md`
  row whose `documents.path` is the absolute vault-internal path
  `<tmp>/target/wiki/notes.md` (both `documents.path` values and
  `collect_files` output are absolute — the walker joins onto the
  canonicalized root), plus a phantom `.brain/notes.md` row whose
  absolute path is
  `<tmp>/target/wiki/.brain/notes.md`. Walk yields both absolute
  paths. Assert the `notes.md` row is **not** deleted (relativized
  against the `<tmp>/target/wiki/` root its path has no excluded
  component — `target` is the
  ancestor of the vault, not a vault-internal directory) and the
  `.brain/notes.md` row **is** deleted. Without `vault_root`
  parameterisation, a raw absolute-path component check would
  falsely match `target` as an ancestor of every row and delete
  the entire index.
- **Desktop self-heal test (finding 3b):** the startup purge deletes a
  pre-existing `immutable-source-files/agents/.brain/errors.log` row
  **and** a top-level `<vault>/.brain/errors.log` row, without any
  `ct ingest` run. The second row is the one a `raw_docs`-scoped purge
  would miss, so it is the assertion that pins the corrected root.
- Existing walker/queue/reconcile test suites stay green; the
  `enqueue_vault_event` signature change (D3) touches `lib.rs:1072`,
  `lib.rs:1102`, `lib.rs:1224` and `tools/src/cmds.rs:671`.

## Out of scope

- **The generalized defect class:** the watcher stages rows for *any*
  non-ingestable extension (`.log` today; images, binaries, etc.
  tomorrow), all of which become immortal pending rows re-enqueued by the
  sweep. This spec fixes only the `.brain` instance; a general
  extension-gate in `enqueue_vault_event` (mirroring
  `should_ingest_extension`) is a sensible follow-up and should be filed
  as an issue rather than folded in here.
- **Purging pre-existing rows under the other `EXCLUDED_DIRS` names**
  (`node_modules/`, `target/`, …). Two clauses, often conflated:
  - During **non-empty** reconciliation (item 3a), vanished rows under
    any excluded directory — `node_modules/`, `target/`, `.brain`, and
    every other name in `EXCLUDED_DIRS` — ARE deleted by the pre-pass.
    We *do* know such rows should be absent from a successful walk, the
    pre-pass exists precisely to prevent the empty-file hash collision
    from repointing them, and the empty-walk safety net does not apply
    here. Item 3a's wording ("Rows whose vault-relative path has an
    excluded component") is intentional and broad.
  - During **empty** walks (item 4), the narrow scope is `.brain`-only;
    `node_modules/`, `target/`, … rows are preserved because the empty
    walk may signal a transient mount failure rather than real absence,
    and the mount-failure safety net must hold for them.
  - What is genuinely **out of scope** in this spec is any code that
    proactively discovers and deletes *non-vanished* rows under
    `node_modules/`, `target/`, …. We have no proof such rows are
    illegitimate, no walker output today suggests them, and adding
    such a discovery pass is a separate change.
- folder_rules exclude mode.
- ct_doctor import-preflight live-row scoping
  (`curated-thoughts-integrations:2026-09-10-doctor-preflight-live-scope-design.md`).
  Note that spec concerns the `ct_doctor.py check` `source_ref` census
  **only** — it contains no statement about reconcile or `ct ingest`, and
  rev 1's claim that it is "the authoritative statement of where reconcile
  runs from" was factually wrong (round-4 finding 6). Reconcile's call
  sites are pinned here, in items 3–5.

## Open questions

None. Rev-1's misattribution (walker vs watcher), the recurrence gap, the
`.brain/proposed` interaction, the cleanup-mechanism ambiguity, the
empty-walk contract and substring-lookalike fixtures (rounds 1–3), the
path-space / relativization / ordering / scope defects of round 4, and
rev 5's wrong desktop-purge root, overstated path-space claim, unplaced
pre-pass transaction and incorrect item-5 rationale are all resolved above.

One thing is no longer unresolved: the watcher-stores-canonical-vs-walker-
stores-virtual divergence (D2a) was unified in issue #204 — the watcher
now writes the virtual path and migration V22 rewrites the legacy rows.
See D2a below for the wiring; pre-fix divergence stays visible only in
test history (the test that asserted canonical, pre-#204, was rewritten to
assert virtual).
