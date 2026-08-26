# Spec: `ct watch` — headless vault watcher (phase 2)

**Repo:** curated-thoughts · **Type:** feature · **Priority:** P2
**Status:** awaiting Kurt's spec approval
**Supersedes / builds on:** `2026-08-24-ct-headless-cli-phase1.md` (merged as PR #83 / PR #84)
**Context:** the Phase 1 CLI ships read-side commands + `ct ingest` / `ct librarian run` / `ct approve`. Phase 2 adds a foreground watcher daemon so files modified while no desktop app is running still get indexed, and splits the 1394-line `tools/src/cli_common.rs` into focused modules. Three files in the live vault (`people/tessera/INDEX.md`, `memories/memory-architecture.md`, `memories/farmhouse-arts.md`) are currently unindexed for exactly this reason — they are the smoke test for this PR.

## Problem / opportunity

CT's brain is only reachable via the GUI app or the MCP sidecar. The desktop app starts an in-process `notify::RecommendedWatcher` on `switch_vault` (`src-tauri/src/lib.rs:798`) that pushes events into a `mpsc::sync_channel(256)` pipeline (`src-tauri/src/pipeline/mod.rs:579`). When the desktop app is closed, **no watcher is running** and any file the user adds/edits/deletes is silently ignored until the next manual `ct ingest` or app launch.

This is the root cause of the three unindexed memory files. Their mtimes (Aug 25 14:30–18:09 UTC) fall in the gap between desktop sessions.

A separate housekeeping need: `tools/src/cli_common.rs` is 1394 lines mixing four roles (path resolution, read queries, write commands, low-level mutation helpers). CodeRabbit and aws-cloud-agent flagged this on PR #83 as a follow-up before adding more surface to the file.

## Proposed change

### 1. `ct watch` — new foreground daemon subcommand

```
ct watch [--foreground] [--once] [--once-timeout <DURATION>] [--json]
  --foreground                  Run in current terminal until SIGINT/SIGTERM (default behavior).
  --once                        Watch for a fixed window then exit (cron smoke test).
                                Defaults to 60s when --once-timeout is not set.
  --once-timeout <DURATION>     Override the --once timeout (e.g. "5s", "500ms", "2m").
                                Accepts the same format as `humantime`/clap's value parser.
                                Configurable per session-18 ruling (was fixed 60s in earlier
                                drafts; relaxed because smoke-test workflows want shorter
                                windows and real vaults want longer ones).
  --json                        Emit structured event lines to stdout.
```

Process model: **foreground daemon.** Matches `ct` philosophy (small, scriptable, explicit). Users wrap with `while true; do ct watch; done` or a systemd user unit if they want auto-restart. Single instance per vault — enforced by an exclusive advisory lock on `{brain_dir}/.curated_thoughts.lock` (chosen over `.ct-watch.lock` for filesystem-clarity; both crates use the same path so desktop and headless see each other).

### 2. Single-instance lock via `fs4::FileExt::lock_exclusive`

- Acquired at `ct watch` startup and held for the lifetime of the watcher.
- Cross-platform: Linux (flock), macOS (flock), Windows (LockFileEx). No platform-specific code.
- Auto-released on process exit or crash — OS reclaims the lock when the file handle closes.
- If another holder exists, `ct watch` prints `another watcher is already running on this vault (pid N, started ISO)` and exits 2.
- The desktop's `switch_vault` acquires the same lock before spawning its watcher, so the two coordinate: second one to start fails fast with exit 2.

### 3. Producer/consumer decoupling via the existing `documents.status='pending'` queue

**No new table. No schema migration.** Reuses the V1 `documents.status` field and the V11 `idx_documents_dirty` partial index.

- `ct watch` callback → `tauri_app_lib::db::queue::enqueue_vault_event(&mut conn, event_kind, path)` (called via a thin wrapper at `tools/src/cmds.rs::enqueue_vault_event` for ergonomics — see §11 amendment).
- For Add/Modify: `INSERT ... ON CONFLICT(path) DO UPDATE` upserts a `pending` row with fresh content hash. Idempotent: re-firing the same event for a doc that's already `indexed` with the same hash is a no-op.
- For Delete: `DELETE FROM documents WHERE path = ?1`. `chunks` cascade-delete via the FK in `schema.rs:18`.
- Consumer side is unchanged: `ct ingest`, `ct librarian run`, and the desktop's pipeline worker all already select `WHERE status='pending'`. `ct watch` is just another writer.

This means `ct watch` running alone with no consumer is safe — events accumulate harmlessly as `pending` rows and get drained the next time any consumer runs.

### 4. Path hardening (4-stage normalization in `enqueue_vault_event`)

```
raw_path → absolute() → canonicalize() → starts_with(vault_root) guard → DB write
```

1. `std::path::absolute(raw_path)` — defensive against (theoretical) relative-path delivery from a future notify version. No-op when already absolute.
2. `std::fs::canonicalize(absolute)` — resolves symlinks (e.g. macOS `/var` → `/private/var`, matching the desktop's `lib.rs:294-296` precedent). Falls back to the absolute path on failure — typical for Delete events where the file no longer exists, but the path string still matches the stored row.
3. `canonical.starts_with(vault_root)` guard — mirrors `lib.rs:805-807`; rejects events outside the watched vault. No DB write for rejected paths.
4. For Add/Modify only: read file bytes, `sha256()`, store as `documents.hash`. Idempotency guard on the upsert's `WHERE documents.hash != excluded.hash OR status IN ('pending','error','orphaned')`.

Delete events skip step 4 (file doesn't exist on disk). They use the path string from step 2 to find and `DELETE` the matching row.

### 5. `cli_common.rs` module split

| New file | Contents | Approx LOC |
|---|---|---|
| `tools/src/paths.rs` | `pub use tauri_app_lib::retrieval::BrainPaths;` + `resolve_brain_paths()` (re-export), `print_json<T>()`, `vault_contains(&Path, &Path) -> bool` | ~90 |
| `tools/src/queries.rs` | `status_cmd`, `search_cmd`, `recall_cmd`, `code_cmd`, `graph_cmd`, `wiki_list_cmd`, `wiki_get_cmd` | ~700 |
| `tools/src/cmds.rs` | `ingest_run`, `librarian_run`, `librarian_run_on`, `approve_one`, `approve_all`, `watch_run`; **new thin wrapper:** `enqueue_vault_event` (delegates to `tauri_app_lib::db::queue::enqueue_vault_event`) | ~600 |
| `tools/src/write.rs` | `open_ro(&Brain) -> Result<Connection>`, `open_rw(&Brain) -> Result<Connection>` (delegate to `cli_common::open_ro/open_rw`; `&Brain` signature preserved to match existing call sites — `cli_common::resolve()` returns `Brain`, integration tests read `brain.paths.db_path`) | ~100 |

`tools/src/cli_common.rs` becomes a thin re-export shim (`pub use {paths, queries, cmds, write};`) so the existing `use curated_thoughts_tools::cli_common::X` paths in `tools/src/bin/*` keep compiling. The re-exports can be removed in a follow-up PR once all consumers migrate.

### 6. Watcher code location

**Direction (corrected from original draft):** the watcher code stays in `src-tauri/src/watcher/fs_watcher.rs`. `tools/src/watcher.rs` becomes a thin re-export of `tauri_app_lib::watcher::*` so the `ct` binary can call the watcher without duplication.

**Why not the original "move it to tools" plan:** `tools/Cargo.toml` already declares `tauri_app_lib = { path = "../src-tauri", package = "curated-thoughts" }` (tools depends on src-tauri). If we also moved the watcher to `tools/` and tried to make src-tauri re-export `curated_thoughts_tools::watcher::*`, we'd need a path dep in the opposite direction, which Cargo rejects as a cyclic package dependency. The implementer who first hit this BLOCKED correctly (the cycle is real and unrecoverable without a workspace migration, which is out of scope for this PR). Therefore:

- `src-tauri/src/watcher/fs_watcher.rs` **keeps the canonical watcher** (`VaultEvent`, `WatcherHandle`, `spawn_vault_watcher`). The two existing `#[test]`s stay. The file gets extended in-place with the new `VaultLock` (since src-tauri is where desktop-mode locking lives).
- `src-tauri/src/watcher/mod.rs` keeps its current shape (`pub mod fs_watcher; pub use fs_watcher::*;`). **No change.**
- `tools/src/watcher.rs` becomes `pub use tauri_app_lib::watcher::*;` — a thin re-export. No logic lives here.
- `tools/src/lock.rs` becomes a small standalone `VaultLock` (uses `fs4::FileExt::lock_exclusive`) used by `ct watch` when no desktop is running. ~30 LOC + 2 unit tests. This duplicates the equivalent ~30 LOC in `src-tauri/src/watcher/fs_watcher.rs`; the duplication is forced by the cargo dep direction. **Phase-3 workspace migration can collapse this**; explicitly out of scope here.
- `src-tauri/src/lib.rs:798` swaps its `pipeline_tx.try_send(...)` body for `tauri_app_lib::db::queue::enqueue_vault_event(&mut conn, event.kind(), &path)` (no wrapper — direct call from src-tauri, same package, no cycle). The heal tick for Delete events stays because the desktop's heal scheduler is app-state cleanup that doesn't apply to the headless case. **Connection lifecycle (mutex trap, §11):** the desktop's watcher callback must NOT hold the `DbState` mutex (`Mutex<AppDb>` at `src-tauri/src/lib.rs:66`) during the `enqueue_vault_event` call — sha256 hashing on a large PDF can take ~150ms, freezing the React UI. Instead, open a fresh ephemeral `rusqlite::Connection::open(&brain_paths.db_path)` per event and pass that to `enqueue_vault_event`. WAL mode (`src-tauri/src/db/connection.rs:130`) permits concurrent writer + reader.

### 7. Contract for `spawn_vault_watcher`

```rust
pub fn spawn_vault_watcher<F>(
    vault_path: PathBuf,
    mut on_event: F,
) -> Result<WatcherHandle>
where
    F: FnMut(VaultEvent) + Send + 'static,
```

**Documented contract:** `on_event` callbacks receive `VaultEvent::Added/Modified/Deleted` whose `path` field is **always absolute** (per `notify` v6's guarantee on Linux inotify + macOS FSEvents + Windows ReadDirectoryChangesW). Consumers may rely on this without `std::path::absolute()` defensiveness, but `enqueue_vault_event` does the full 4-stage normalization anyway because the contract is one-way (we own the caller, but Rust's type system can't enforce it).

The two call sites inject different dispatch:

```rust
// src-tauri/src/lib.rs:798 — desktop mode (heal + DB enqueue)
let handle = spawn_vault_watcher(vault_root, move |event| {
    let _ = app.emit("vault-event", &event);
    if matches!(event, VaultEvent::Deleted(_)) { let _ = heal_tx.send(()); }
    let mut conn = open_brain_rw(&brain_paths).ok()?; // see §11: ephemeral conn, not db_state mutex
    let path = match &event {
        VaultEvent::Added(p) | VaultEvent::Modified(p) | VaultEvent::Deleted(p) => p,
    };
    let _ = tauri_app_lib::db::queue::enqueue_vault_event(&mut conn, event.kind(), Path::new(path));
});

// tools/src/bin/ct.rs — headless mode (DB enqueue only)
let handle = spawn_vault_watcher(vault_root, move |event| {
    let mut conn = open_brain_rw(&brain_paths).ok()?; // see §11: ephemeral conn, not db_state mutex
    let path = match &event {
        VaultEvent::Added(p) | VaultEvent::Modified(p) | VaultEvent::Deleted(p) => p,
    };
    let _ = tauri_app_lib::db::queue::enqueue_vault_event(&mut conn, event.kind(), Path::new(path));
});
```

**`tools::watcher` has zero coupling to `src-tauri`.** Both call sites inject their own dispatch; the watcher itself doesn't know about heal, pipeline, or Tauri.

## Files touched

| File | Action | Details |
|---|---|---|
| `tools/src/watcher.rs` | NEW (thin re-export) | `pub use tauri_app_lib::watcher::*;` — no logic, no tests. |
| `tools/src/lock.rs` | NEW | `VaultLock` (uses `fs4::FileExt::lock_exclusive`) — thin standalone wrapper, ~30 LOC + 2 unit tests. Duplicates ~30 LOC in `src-tauri/src/watcher/fs_watcher.rs`; forced by cargo dep direction. |
| `tools/src/paths.rs` | NEW (split) |
| `tools/src/queries.rs` | NEW (split) |
| `tools/src/cmds.rs` | NEW (split) — contains `watch_run` and a thin `enqueue_vault_event` wrapper delegating to `tauri_app_lib::db::queue::enqueue_vault_event`. Full logic lives in src-tauri (see §11). |
| `tools/src/write.rs` | NEW (split) |
| `tools/src/cli_common.rs` | Becomes thin re-export shim |
| `tools/src/bin/ct.rs` | Add `Watch` subcommand; import path update |
| `tools/Cargo.toml` | Add `fs4 = "0.7"` (verify latest stable at impl time) |
| `src-tauri/src/watcher/mod.rs` | **No change** — keeps `pub mod fs_watcher; pub use fs_watcher::*;` |
| `src-tauri/src/watcher/fs_watcher.rs` | Extends in place with `VaultLock` + 2 unit tests (`vault_lock_blocks_second_acquire`, `vault_lock_released_on_drop`). Existing watcher code unchanged. |
| `src-tauri/src/db/queue.rs` | NEW (amended §11) — `enqueue_vault_event` + 4-stage path hardening + sha256 + 6 unit tests. Shared queue logic lives here because both crates need to call it and the cargo dep direction (`tools → src-tauri`) prevents tools from being the canonical home. |
| `src-tauri/src/db/mod.rs` | Adds `pub mod queue;` re-export |
| `src-tauri/src/lib.rs:741-824` | `reconcile_vault` (line 786) + watcher callback (line 798) both swap to `enqueue_vault_event`; lock acquisition at `switch_vault`; `WatcherHandle::stop()` releases lock first |
| `src-tauri/Cargo.toml` | Add `fs4 = "0.7"` |
| `docs/superpowers/specs/2026-08-25-ct-headless-cli-phase2-watch.md` | THIS SPEC |
| `docs/superpowers/plans/2026-08-25-ct-headless-cli-phase2-watch.md` | NEW (implementation plan, written next session) |

## Cross-cutting requirements

- **Path resolution identical to sidecar.** `paths::resolve_brain_paths()` re-exports the canonical `tauri_app_lib::retrieval::resolve_brain_paths` — same env var precedence (`CURATED_BRAIN_DIR`, `CURATED_BRAIN_DB`, `CURATED_BRAIN_CONFIG`). **`BrainPaths` itself is re-exported, not redefined locally**, so its type identity (`Eq`/`PartialEq`/`Serialize`) is shared across crates and test fixtures.
- **Single source of truth for `enqueue_vault_event`.** Both the desktop's `lib.rs:798` callback and `ct watch`'s callback call the same `tauri_app_lib::db::queue::enqueue_vault_event` function (the desktop directly; `ct watch` via a thin wrapper at `tools/src/cmds.rs::enqueue_vault_event`). No copy-paste divergence.
- **Exit codes:** 0 ok / clean shutdown, 1 config error, 2 lock conflict, 3 DB/schema error, 4 notify init failure.
- **`--json` output on `ct watch`** — structured `{kind, path, ts_ms}` event lines on stdout. Matches the existing `--json` contract from phase 1 commands.
- **Log noise:** `ct watch` writes one stderr line per event in TTY mode; downgrades to startup-banner-only + final summary in non-TTY mode (cron-friendly).

## Non-goals (this phase)

- No coordination protocol beyond "single instance per vault, second one fails." Two simultaneously-running watchers are explicitly not supported in v1. The user can stop the desktop's watcher (UI surface TBD; not this PR) before running `ct watch`.
- No HTTP transport. No remote-watch API. The MCP sidecar remains the only network surface.
- No initial scan on `ct watch` startup. Files modified before the watcher started must be ingested via `ct ingest` separately. (This is what makes the 3-file backfill the natural smoke test.)
- No rate limiting / event coalescing on save-storms. `notify` already coalesces inotify events on Linux; macOS FSEvents and Windows ReadDirectoryChangesW have similar semantics. Out of scope to add additional logic.
- No `--once --until-empty` drain mode — `--once` is a fixed 60s window.
- The deferred-minors rollup from PR #83 (16 items: `--hops` unclamping, magic-5, `entity_of` dup, etc.) is NOT in this PR. Filed separately.

## Test plan

1. **Unit tests in `src-tauri/src/watcher/fs_watcher.rs`** (unchanged location): `test_watcher_detects_new_file`, `test_watcher_detects_deleted_file`, plus two new tests for `VaultLock`:
   - `vault_lock_blocks_second_acquire` — acquire lock in main test thread, attempt second acquire, assert conflict error.
   - `vault_lock_released_on_drop` — acquire, drop, second acquire succeeds.
2. **Unit tests in `tools/src/lock.rs`**: same two as above (`vault_lock_blocks_second_acquire`, `vault_lock_released_on_drop`) — `ct watch` exercises the duplicate type, and coverage parity ensures both paths work.

3. **Unit tests in `src-tauri/src/db/queue.rs`** (relocated from `tools/src/cmds.rs` per §11 amendment): `enqueue_vault_event` covered for:
   - Add on new path → row created with `status='pending'`, hash matches.
   - Modify on existing indexed row with different hash → status flips to `'pending'`.
   - Modify on existing indexed row with same hash → no-op.
   - Delete on existing row → row gone, chunks cascaded.
   - Delete on missing row → no error.
   - Path outside vault root → no DB write (logged warning).

4. **Integration test:** temp-dir vault fixture → `ct ingest` (seeded) → `ct watch --once --json` (60s) → modify a file → assert event delivered to stdout JSON; assert DB row has `status='pending'` and matching hash.

5. **Manual smoke against live vault:**
   - Stop desktop app.
   - Run `ct ingest` (initial drain — should pick up the 3 missing memory files).
   - Run `ct watch --foreground` in one terminal.
   - In another: `echo " " >> ~/Documents/equational-wiki/memories/farmhouse-arts.md`.
   - Watch the stderr line `[watch] ~ /home/kv/Documents/equational-wiki/memories/farmhouse-arts.md`.
   - `ct status --json` shows the row `status='pending'` (or `='indexed'` if a consumer also ran).
   - Ctrl-C: clean shutdown, lock released.

6. **Full suite green:** `cargo test --lib (tools)` ≥ 7 passing, `cargo build (tools)` clean, `cargo check --tests --features test-utils (src-tauri)` clean. Conventional commits `feat(tools): add ct watch + split cli_common` + `refactor(tools): split cli_common into paths/queries/cmds/write` + `fix(tauri): replace in-process pipeline with DB-backed enqueue` + `chore(deps): add fs4 cross-platform file lock`.

## Risks

- **Refactor scope creep in `cli_common.rs` split.** Mitigation: keep `cli_common.rs` as a re-export shim; only `cmds::enqueue_vault_event` is genuinely new code. Public API unchanged.
- **`fs4` API churn** (the crate is at 0.x). Mitigation: pin a specific version; review the API at impl time.
- **Watcher deliverer path contracts change in a future `notify` upgrade.** Mitigation: `test_watcher_delivers_absolute_paths` test catches the regression.
- **macOS FSEvents coalescing semantics differ from Linux inotify.** Mitigation: not a correctness issue (coalescing is desirable); just behavioral. Documented in non-goals.
- **Two simultaneous watchers** — explicitly unsupported, but the lock makes the failure mode clean. Documented in `ct watch` startup banner.
- **Cyclic-package dependency between `tools` and `src-tauri`** — disallows moving the watcher or VaultLock to `tools/` as a primary home. Mitigation: phase-2 keeps the watcher in `src-tauri/` and uses a thin re-export from `tools/`; `VaultLock` is duplicated ~30 LOC between the two crates (workspace migration in phase-3 can collapse this). The DB-write half of this dependency concern (`enqueue_vault_event` was originally planned for `tools/src/cmds.rs`) is **resolved by amendment (§11)**: the shared logic moved to `src-tauri/src/db/queue.rs` where both crates can reach it.
- **The desktop's `reconcile_vault` (`lib.rs:741-789`) still uses `pipeline_tx.try_send`.** Mitigation: also swap to `enqueue_vault_event` in this same PR (small additional change at `lib.rs:786`). This is intentional scope inclusion — leaving it on the in-memory channel while the watcher moves to DB-backed enqueue would create two divergent paths to the same DB. One source of truth for vault→DB writes.

## §11 Amendment — `enqueue_vault_event` canonical home (2026-08-25)

**Context.** Task 7 implementation BLOCKED on a cargo package cycle: `tools/Cargo.toml` depends on `src-tauri/Cargo.toml` (via `tauri_app_lib = { path = "../src-tauri", package = "curated-thoughts" }`), so `src-tauri/src/lib.rs` cannot `use curated_thoughts_tools::*`. Task 5 placed `enqueue_vault_event` in `tools/src/cmds.rs`; Task 7's brief assumed src-tauri could call it directly, which is impossible without creating a cycle.

**Resolution.** Move `enqueue_vault_event` (and its 4-stage path hardening + sha256 helper) to `src-tauri/src/db/queue.rs`. `tools/src/cmds.rs` keeps a one-line delegating wrapper so call sites in tools don't change. Both crates reach the same code:
- Desktop (`src-tauri/src/lib.rs`) → direct: `tauri_app_lib::db::queue::enqueue_vault_event(...)`
- Headless (`tools/src/cmds.rs::watch_run`) → via wrapper: `tools::cmds::enqueue_vault_event(...)` → `tauri_app_lib::db::queue::enqueue_vault_event(...)`

**Why not move the whole DB helpers (Brain, open_rw, BrainPaths) into src-tauri instead?** Brain and BrainPaths are CLI lifecycle concepts (ephemeral connections per `ct` invocation). Forcing them into src-tauri would couple the desktop crate to CLI plumbing. The split — queue logic in src-tauri, lifecycle helpers in tools — mirrors the natural boundary between "what to do" (queue logic, shared) and "how to connect" (per-crate lifecycle, different).

**Connection lifecycle (mutex trap).** The desktop's `DbState(Mutex<AppDb>)` is a long-lived lock shared with the React UI (DB state). If the watcher callback locks it and calls `enqueue_vault_event`, which reads file bytes and computes `sha256` (~150ms for a large PDF), the UI freezes for 150ms. Mitigation: **do not use `db_state.0.lock()` in the watcher callback**. Open a fresh `rusqlite::Connection::open(&brain_paths.db_path)` per event and pass that to `enqueue_vault_event`. SQLite WAL mode (`src-tauri/src/db/connection.rs:130`) permits concurrent readers + writers, so the UI's connection through `DbState` keeps reading while the watcher writes via the ephemeral connection. `ct watch` already uses ephemeral connections (`tools::write::open_rw`), so it needs no change.

**Tests.** The 6 `enqueue_vault_event` unit tests move from `tools/src/cmds.rs::tests` to `src-tauri/src/db/queue.rs::tests`. Test fixture: `tauri_app_lib::db::connection::open_in_memory()` (already exists at `src-tauri/src/db/connection.rs:163`), which avoids duplicating the inline CREATE TABLE DDL from the tools tests. WAL mode is not relevant to in-memory DBs; tests still cover the 4-stage normalization + sha256 + upsert contract.

## §11.1 Amendment — Post-fixup state (2026-08-25 follow-up PR, drift-fixups)

**Context.** PR #96 shipped the §11 amendment (single canonical home for `enqueue_vault_event`) but accumulated 7 spec/plan drift items + 1 missing e2e test during the 8-review cycle. This amendment records the post-fixup state so the spec reflects what the code actually does. No behavioral change beyond §5 exit-code mapping + `--once-timeout` configurability (both spec-required by §5/§6; documented here for traceability).

**Resolved drift items:**

- **`--once-timeout <DURATION>` is configurable** (per session-18 ruling, surfaced during PR #96 review of Task 1 implementation; commit `5ce3268`). Earlier drafts of this spec said "fixed 60s window, not configurable in v1". Rationale: smoke-test workflows want shorter windows (`--once-timeout 1s`), real-vault cron jobs want longer ones (`--once-timeout 5m`). The 60s default is preserved when `--once-timeout` is not provided.
- **Exit codes 3/4 are now distinct** (per spec §5). Previously all non-lock-conflict failures bubbled to `main()` and became exit 1. After this amendment: DB/schema errors at startup → exit 3, notify-deck init failures → exit 4. Implemented via a `WatchError` enum (`tools/src/cmds.rs`) with explicit `Display` impl so each bucket renders a distinct stderr message. Lock-conflict stays exit 2 (already correct in PR #96), config-error stays exit 1 (unchanged). Coverage: 4 integration tests in `tools/tests/ct_watch_exit_codes.rs` (process-level exit-code assertions) + 3 unit tests in `tools/src/cmds.rs::tests` (return-code-from-`watch_run` shape).
- **`--foreground` flag is plumbed end-to-end** (per spec §1 CLI usage table, drift item 4). Flag is a no-op in v1 — the daemon is always foreground by design — but the clap variant field + dispatch-arm `foreground: _` are wired so a future phase-3 PR can implement `--background` (systemd-style detached mode) without an interface break. Test: `tools/tests/ct_watch_help.rs::ct_watch_help_lists_all_flags` snapshots `--help` output and asserts `--foreground` is present (regression guard).
- **`tools/tests/ct_watch_e2e.rs` integration test added** (per spec §11 test plan item 4, drift item 7). Temp-dir vault + `ct ingest --yes` + `ct watch --once --json --once-timeout 10s` + file modification + assert (a) JSON event on stdout, (b) DB row flipped to `status='pending'`, (c) recomputed SHA-256 matches the stored hash. Uses the `env!("CARGO_BIN_EXE_ct")` + `temp_env::with_var` + `tempfile::TempDir` pattern matching the existing `tools/tests/` integration suite. 500ms watcher-startup delay tuned for inotify setup; comment in the test documents the upgrade path if CI becomes flaky.

**Items NOT in this amendment (deliberately):**

- Cosmetic spec line-refs (`lib.rs:798`, `lib.rs:805-807`, `lib.rs:741-789`) — these drifted 60–100 LOC during PR #96 review. Bumping them is a separate "spec cleanup" pass once the line numbers stabilize.
- `tools/src/queries.rs` header docstring (still describes pre-split state) and `tools/src/cmds.rs:1041` unused `tempfile::TempDir` import — tracked but cosmetic.
- Removal of `pipeline_tx`/`PipelineJob`/`PipelineHolder` dead code from src-tauri — `#[cfg(feature = "test-utils")]` gate keeps it contained. Separate cleanup PR.
- The `queries.rs` doc update + the dead-import removal were **not** rolled into this amendment because they don't affect runtime behavior; touching them here would expand scope beyond "fix drift items 1–7".

**Plan/operational handoff:**
- Drift-fixups plan: `docs/superpowers/plans/2026-08-25-ct-watch-drift-fixups.md`
- Operational handoff: `~/Documents/equational-wiki/operations/ct-watch-phase2-followup-handoff.md`
- Lessons extracted: `~/Documents/equational-wiki/operations/ct-phase-2-lessons.md`

## Sequencing

After this PR lands:
- The 3 missing memory files get ingested as part of the manual smoke test.
- A follow-up PR can add `ct watch --once` to a cron job if Kurt wants auto-drain (not this PR).
- The deferred-minors rollup becomes its own PR.
- `vault_write_note` MCP tool stays in the P1 backlog — not addressed here.

## Verification (pre-merge checklist)

- [ ] Spec doc committed to `docs/superpowers/specs/`.
- [ ] Implementation plan written to `docs/superpowers/plans/`.
- [ ] All `cli_common.rs` callers migrated to new module paths (or relying on re-exports).
- [ ] All `cargo test` green in `src-tauri` and `tools`.
- [ ] CodeRabbit round 1: addressed.
- [ ] aws-cloud-agent round 1: addressed.
- [ ] Live smoke on real vault: 3-file backfill + `ct watch --foreground` works.
- [ ] Lock conflict tested manually (two `ct watch` instances).
- [ ] Reviewed against PR reviewer's standing policy (`procedures/no-coderabbit-re-request.md` — never `@coderabbitai` to re-request; fix commits are the signal).