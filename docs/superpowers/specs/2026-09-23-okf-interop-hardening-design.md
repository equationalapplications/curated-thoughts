---
okf_version: 0.1
profile: llm-wiki/1
title: Spec — OKF interop hardening (dialog filters + headless nightly export)
entity_type: doc
tags: [spec, okf, backup, exporter, pr-226]
created_at: 2026-09-23T21:30:00-04:00
updated_at: 2026-09-24T01:30:00Z
---

# Spec: OKF interop hardening

**Status:** approved (Kurt, 2026-09-24 — spec traveled in PR #226 description).
**Branch/PR:** `fix/okf-dialog-filter-and-exporter` / PR #226 — spec, plan and
implementation live on this same branch per the same-PR procedure
(Kurt directive 2026-09-24).

## Context

Round-trip testing of the v2.13.0 OKF interop found that a bundle named
`.okf` was invisible to the app's own Import dialog (filters were
`["zip"]`-only) although the Rust reader sniffs zip bytes. In parallel, the
nightly backup cron (Kurt directive 2026-09-23) needed a headless exporter.
PR #226 already ships: the filter fix, `tools/src/bin/export_okf_bundle.rs`,
README docs, and one Opus-review hardening pass (atomic publish, snapshot
transaction, restorability self-check). This spec pins the remaining
requirements, including the three open CodeRabbit findings on the
hardening commit.

## Requirements

### R1 — Dialog filters accept `.okf` (shipped)
Both OKF dialogs (`save` + `open` in `OkfInteropBar.tsx`) accept
`extensions: ["zip", "okf"]`.

### R2 — Headless exporter (shipped, must survive hardening)
`export_okf_bundle [dest]`: read-only DB open, same load+write code as the
GUI (OKF 0.2 / `llm-wiki/2`), no `exported` event rows, consistent-snapshot
read (`BEGIN DEFERRED`), restorability self-check (`read_bundle_source` +
`parse_bundle` + import caps), atomic publish, `--help`/arg validation,
streamed sha256, exit non-zero on any failure.

### R3 — Backup file permissions (CodeRabbit #4088815424, Major)
The exported bundle is an unredacted copy of the brain. On Unix the temp
file must be created `0o600` (not umask-dependent `0o666`) so the rename
never widens permissions on an existing `0o600` backup. Windows builds
must be unaffected.

### R4 — Durability of the atomic publish (CodeRabbit #4088815428, Minor)
The finished zip must be `sync_all`-ed before the rename, and the
destination's parent directory must be fsync-ed after the rename (Unix)
before success is reported, so a crash cannot leave a truncated or
unlinked backup behind the cron's commit.

### R5 — `$HOME` resolved only for the default (CodeRabbit #4088815418, Minor)
An explicit destination argument must work on accounts where
`dirs::home_dir()` is `None`. Resolve `$HOME` lazily, only when no dest
was supplied.

### R6 — Deterministic bundle bytes (deferred from Opus review, now in scope)
Zip entry timestamps must be pinned to a fixed value in `write_bundle_zip`
so that exporting an unchanged brain yields byte-identical files and
matching sha256 digests. This makes "nothing changed tonight" detectable
by the backup cron. Applies to the GUI path too (same writer); content is
otherwise unchanged.

### R7 — Operational follow-through (outside the repo)
After R6, `~/.hermes/scripts/okf-bundle-sync.sh` should skip commit+push
when the digest is unchanged from the committed bundle (nightly no-op
instead of a timestamp-only commit). Not part of this PR.

## Non-goals

- No GUI behavior changes beyond the already-shipped filter fix.
- No integration-test harness for the bin (deferred, own PR).
- No changes to bundle format or import semantics.

## Verification

- `cargo test -p curated-thoughts-tools` green; new tests cover permission
  preservation, determinism, and dest-arg precedence.
- `cargo clippy -p curated-thoughts-tools --bin export_okf_bundle` clean
  (clippy is enforced repo-wide).
- Frontend suites for `OkfInteropBar` + axe-core green.
- Live run against the real brain: restorability self-check passes,
  permissions of the published file are `0o600`, two consecutive runs with
  no data change produce identical digests.
- CodeRabbit threads resolved; PR CI green.
