# Curated Thoughts — agent notes

## Disk storage & build locations (read before running cargo)

This repo's Rust builds have filled the disk to 100% twice (Oct 2026), crashing
the machine. Root cause both times: multiple build-cache trees from an
inconsistently-set `CARGO_TARGET_DIR`. Follow these rules exactly.

- **ONE canonical build location**: `<repo-root>/.cargo-shared-target/`. Before
  any cargo command, export
  `CARGO_TARGET_DIR=<absolute repo root>/.cargo-shared-target`.
  A RELATIVE value resolves against the current directory — running from
  `tools/` or `src-tauri/` with a bare `.cargo-shared-target` silently creates a
  second multi-GB tree. Always use the absolute path.
- **Pre-flight**: `df -h /` must show ≥30G free before launching full test
  suites; fresh builds reach tens of GB.
- **On linker `Bus error` (signal 7) or `ENOSPC` / `os error 122`**: the disk
  hit 100% under the build — this is an environment failure. STOP and report.
  NEVER delete caches to unblock yourself; cache deletion is a controller
  decision made only after `lsof +D <dir>` confirms no open handles.
- **No stray target dirs**: only `.cargo-shared-target/` should exist in the
  repo. If you find `target/` elsewhere (e.g. `src-tauri/target/`), verify no
  open handles (`lsof +D`) and remove it.
- **Worktrees**: Rust/cargo work runs SERIAL — never inside a parallel wave
  (2 concurrent cargo worktrees max machine-wide). Wave done includes
  `git worktree remove` + build-dir cleanup; verify `df -h /` before the next
  stage.
- **Post-session sweep**: `du -sh .cargo-shared-target` and check for stray
  `target/` dirs before ending a session that ran Rust gates.
