# P0 Security Debt Cleanup

**Date:** 2026-08-31
**Status:** proposed
**Baseline:** main @ v1.38.1

## Summary

PR #124 landed the trusted-symlink ledger and its hardening pass, but left
two P0 items on main:

1. `tools/src/lock.rs` opens the `ct watch` lockfile with `truncate(true)`,
   reintroducing the symlink-truncation hazard that `6c1113e` fixed in the
   duplicate `fs_watcher.rs` implementation.
2. CodeQL alert #2 (HIGH, `rust/cleartext-logging`, `tools/src/bin/ct.rs:575`)
   is still open. The inline `// codeql[...]` suppression written to silence
   it has never worked — Rust has no inline suppression support.

Both are small, independent, and touch code already reviewed under #124. One
PR clears both.

## Non-goals

Deliberately out of scope, to keep this reviewable in minutes:

- Auditing or changing the other path-printing sites in `ct.rs`.
- `--json` output redaction carve-outs.
- Lock-file byte-parity tests across the `tools` and `src-tauri` crates.
- Collapsing the `lock.rs` / `fs_watcher.rs` duplication (still the planned
  phase-3 workspace migration).
- Any P1/P2 item from the 2026-08-31 priority list (issue #119, the symlink
  health-check spec deviation, the friendly error screen, the ingest
  drain-stall watchdog). Each gets its own spec.

## Item 1 — `lock.rs` truncate hazard

### Problem

`VaultLock::acquire` (`tools/src/lock.rs:46`) opens
`<vault>/.curated_thoughts.lock` with:

```rust
fs::OpenOptions::new()
    .create(true)
    .truncate(true)   // <- hazard
    .write(true)
    .read(true)
```

If `.curated_thoughts.lock` is a symlink, opening it for write follows the
link and truncates its target. An attacker (or an accident) that plants a
symlink at that path destroys the pointed-at file's contents the next time
`ct watch` starts.

This is the same defect fixed in `src-tauri/src/watcher/fs_watcher.rs` by
`6c1113e`. `tools/src/lock.rs` is a deliberate duplicate of that type — the
cargo dependency direction is `tools -> src-tauri`, so the desktop crate
cannot re-export a type living in `tools` without a cyclic package dep. The
module header documents the duplication; the fix simply never crossed over.

### Fix

Mirror `fs_watcher.rs:131-140` exactly: `.truncate(false)`, carrying the same
rationale comment. The lock file's contents are never read — the lock is held
via `fs4::FileExt::lock_exclusive` on the open handle — so truncation was
always unnecessary. The fix is narrow: it prevents truncation of a symlinked
target. A planted symlink can still redirect the lock and cause contention or
denial of service; no-follow / rejection behavior is a separate concern,
explicitly out of scope here.

### Test

New test in `tools/src/lock.rs`'s `mod tests`, alongside
`vault_lock_blocks_second_acquire`:

`vault_lock_does_not_truncate_symlink_target` — create a temp vault, write a
canary file with known contents, symlink `.curated_thoughts.lock` to the
canary, call `VaultLock::acquire`, then assert the canary's contents are
unchanged.

This exercises the real exploitation path against the real filesystem with no
mocking. It fails on `truncate(true)` and passes after the fix.

### Known follow-up

`fs_watcher.rs` carries the correct `truncate(false)` but has no regression
test of its own. Porting this canary test to the desktop crate is tracked as
a follow-up, not part of this PR — the active hazard is in `lock.rs`, and the
scoping keeps this change reviewable.

## Item 2 — CodeQL alert #2 (`rust/cleartext-logging`)

### Problem

`tools/src/bin/ct.rs:575` prints the trusted-link ledger:

```rust
println!("{} -> {}", entry.link, redact_home(&entry.target));
```

The value is already sanitised: `redact_home` collapses a `$HOME` prefix to
`~`, so a target like `~/.ssh` is never printed as an absolute path into CI
logs or system journals.

CodeQL's `rust/cleartext-logging` query does not model `redact_home` as a
sanitiser, so it flags the call anyway. The `// codeql[rust/cleartext-logging]`
comment added above it is inert — inline suppression comments work for some
CodeQL languages but **not** for Rust, so the alert has stayed open since #124
while appearing, to a reader, to be handled.

### Decision: false positive, dismissed upstream

`ct trust --list` exists to show the user what they trusted. Printing the
link without its target would remove most of the listing's value, so the
`println!` stays as-is. The alert is a genuine false positive on a
user-facing CLI listing of a user-authored, sanitised value.

### Fix

Delete the inert suppression comment and replace it with a plain comment that
records the sanitiser, the verdict, and a warning against re-adding the
suppression:

```rust
// `entry.target` is sanitised by `redact_home` above: the
// `$HOME` prefix is collapsed to `~` before printing.
// CodeQL rust/cleartext-logging flags this anyway (it does
// not model the sanitiser); alert #2 dismissed as a false
// positive. Inline `// codeql[...]` suppression does NOT
// work for Rust — do not re-add it.
println!("{} -> {}", entry.link, redact_home(&entry.target));
```

Alert #2 is then dismissed in the GitHub Security UI as a false positive,
citing that justification. **This is a manual maintainer step** — it cannot
be done from the PR, and the CodeQL run on this PR will still report alert #2
until it is performed. The PR body will call this out.

### Why no CodeQL config file

Adding `.github/codeql/config.yml` with a `query-filters` exclusion for
`rust/cleartext-logging` would suppress the query more broadly than one line
and could mask a real future leak. With dismissal-only, a future print site
correctly earns its own alert to triage.

## Verification

- `cargo test -p tools` — new symlink canary test plus existing lock tests.
- `cargo clippy` — clean.
- CI green on the PR.
- CodeQL green **except** alert #2, which persists until manually dismissed.

## Risks

- **Dismissal is manual and off-repo.** If the maintainer does not dismiss
  alert #2, the HIGH alert stays open and this PR only removes a misleading
  comment. Mitigated by calling the step out explicitly in the PR body.
- **The duplication remains.** `lock.rs` and `fs_watcher.rs` must stay in
  sync by hand until the phase-3 workspace migration collapses them. Both
  module headers already document this; the divergence this PR fixes was
  caught only by manual review, not by any test or lint.
