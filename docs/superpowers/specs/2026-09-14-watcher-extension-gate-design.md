# Watcher: gate `enqueue_vault_event` staging by ingestable extension

**Date:** 2026-09-14
**Status:** Implemented (rev 1)
**Branch:** spec/watcher-extension-gate
**Issue:** #203
**Priority:** Low (sweep churn and log noise; no data corruption)

## Problem

`enqueue_vault_event` (`src-tauri/src/db/queue.rs:36`) hashes and upserts a
`documents` row with `status='pending'` for **any** in-vault Add/Modify
event, regardless of file extension. The ingest pipeline then refuses the
file: `ingest_file_virtual` early-returns `Ok(())` for non-ingestable
extensions (`src-tauri/src/pipeline/mod.rs:676-685`) without touching the
row's status. The row stays `pending` forever, and the supervisor sweep
(`pipeline/watchdog/sweep.rs:82`, `list_sweepable_pending`) re-enqueues it
on every pass.

The `.brain` exclusion spec
(`2026-09-10-watcher-walker-brain-dir-exclusion-design.md`) fixed only the
`.brain/*.log` instance and explicitly declared the general defect class
out of scope. Any other non-ingestable file in the vault — `.log`, images,
archives, binaries, extensionless files — still produces an immortal row.

The other two staging paths already gate correctly:

| Path | Gate |
|---|---|
| Walker, `collect_files` (`walk_vault.rs:268-271`) | `should_ingest_extension`; extensionless → rejected |
| Reconcile Create (`lib.rs:1263`) | `should_ingest_extension` before calling `enqueue_vault_event` |
| Desktop watcher (`lib.rs:1396`) | **none** |
| `ct watch` (`tools/src/cmds.rs:1196`, `vault_root = None`) | **none** |

## Design

### D1 — Gate location: inside `enqueue_vault_event`, after the Remove branch

Add an extension gate in `enqueue_vault_event` immediately after the
`EventKind::Remove` branch and before `is_excluded_file`, the
excluded-directory check, and the `std::fs::read` + sha256.

```rust
// Issue #203: gate STAGING by extension, mirroring the walker
// (`collect_files`) and the pipeline's own early-return. A row for a
// non-ingestable file could never leave `pending`, so the supervisor
// sweep would re-enqueue it forever. Remove events above deliberately
// bypass this gate: a row staged before the gate existed must still be
// deletable when its file goes away.
let ext = abs.extension().and_then(|e| e.to_str()).unwrap_or("");
if !crate::chunker::should_ingest_extension(ext) {
    return Ok(());
}
```

Rationale:

- **Inside the function, not at call sites.** Both ungated callers (desktop
  watcher, `ct watch`) are covered by one change, and any future caller
  inherits the gate. The existing reconcile-Create call-site check becomes
  redundant but harmless; it is left in place (removing it is not needed
  for the fix).
- **After Remove.** Exclusion gates staging, never healing — the same
  principle the `.brain` spec applied to `is_excluded_file`
  (`queue.rs:93-96`). Gating Remove would strand pre-existing immortal rows.
- **Before the vault-root-dependent checks.** The gate needs no vault
  root, so it is active in `ct watch` (which passes `None`) and in the
  desktop configuration without `CURATED_VAULT_ROOT`.
- **Before I/O.** Rejected events cost no read and no hash.

### D2 — Single source of truth: `should_ingest_extension`

Reuse `crate::chunker::should_ingest_extension`
(`chunker/classify.rs:24-62`) directly. No duplicated extension list.
Case-insensitivity comes from that function (it lowercases).

### D3 — Extension taken from `abs` (the virtual path)

The extension is read from `abs`, the virtual pre-canonicalize path — the
value stored in `documents.path` (issue #204) and the path
`ingest_file_virtual` and `collect_files` read the extension from. For a
trusted-link symlink whose virtual name and target name differ in
extension, the watcher, walker, and pipeline therefore agree.

### D4 — Extensionless and non-UTF-8 extensions are rejected

`abs.extension()` returning `None`, or an extension that is not valid
UTF-8, maps to `""`, which `should_ingest_extension` rejects. This matches
`collect_files` (`.unwrap_or(false)`) and the pipeline (`.unwrap_or("")`).

### D5 — No migration for existing immortal rows

Existing rows heal through paths that already exist:

1. **Reconcile** — a non-ingestable file never appears in `collect_files`
   output, so its row is in reconcile's vanished set on every successful
   (non-empty) walk. It is never a rename candidate (candidates come only
   from walked paths), so it falls to the no-candidate delete arm
   (`reconcile.rs:~218`). Chunks cascade via `ON DELETE CASCADE`.
2. **Remove events** — still delete the row, because the gate sits after
   the Remove branch.

An empty walk preserves non-`.brain` rows by design (mount-failure safety
net, `.brain` spec item 4); those rows heal on the next non-empty walk.
A dedicated purge pass would duplicate reconcile and is out of scope.

## Testing

Add to the `queue.rs` test module, reusing its existing schema fixture:

1. `.log` Modify inside the vault → no `documents` row.
2. Seed a `.log` row, send Remove → row deleted (gate does not block heal).
3. Uppercase `.LOG` Modify → no row (case handling via shared predicate).
4. Extensionless file (e.g. `Makefile`) Modify → no row.
5. `.md` Modify → row staged `pending` (regression guard).
6. `.log` Modify with `vault_root = None` and `CURATED_VAULT_ROOT` unset
   → no row (gate is independent of vault-root resolution).
7. Gate precedes I/O: seed a row for a `.log` path that does not exist on
   disk, send Modify → `Ok(())` and the row is **still present**. Without
   the gate, the read's NotFound arm would delete it, so this observably
   pins the gate ahead of `std::fs::read`. (Healing that row is
   reconcile's and Remove's job, D5.)

Existing walker, queue, reconcile, and sweep suites stay green, with one
required fixture change: four `.brain` exclusion tests in `queue.rs`
(`gate_rejects_brain_paths_and_stages_lookalikes`,
`gate_rejects_symlinked_out_brain_dir`,
`gate_skipped_when_vault_root_unresolvable`,
`gate_fails_open_for_unrelativizable_path`) use `.log` files. Under this
gate those files are rejected by extension before the directory gate runs —
one test would fail outright and three would pass without exercising the
gate they were written for. They move to `.md` so each stays pinned to the
directory gate. `remove_event_still_deletes_pre_staged_brain_row` keeps
`.log` (Remove bypasses both gates).

## Out of scope

- Pipeline-side defense-in-depth (making `ingest_file_virtual` mark or
  delete rows it refuses). The staging gate removes the source; revisit
  only if another staging path appears.
- Removing the now-redundant call-site check at `lib.rs:1263`.
- Proactive purge of existing immortal rows beyond what reconcile does.
- Changing the set of ingestable extensions.
