# P0 Security Debt Cleanup

**Date:** 2026-08-31
**Status:** Implemented 2026-09-02 (PR #129)
**Baseline:** main @ v1.38.1

## Summary

PR #124 landed the trusted-symlink ledger and its hardening pass, but left
two P0 items on main:

1. `tools/src/lock.rs` opens the `ct watch` lockfile with `truncate(true)`,
   reintroducing the symlink-truncation hazard that `6c1113e` fixed in the
   duplicate `fs_watcher.rs` implementation.
2. CodeQL `rust/cleartext-logging` HIGH alert on `tools/src/bin/ct.rs:575`
   is still open. (CodeQL alert numbers rotate each time the flagged
   statement changes, so the durable identifier is the **rule** and the
   **location**, not the number — see Item 2 for the rotation behaviour.)
   The inline `// codeql[...]` suppression written to silence it has never
   worked — Rust has no inline suppression support.

Both are small, independent, and touch code already reviewed under #124. One
PR clears both.

## Non-goals

Deliberately out of scope, to keep this reviewable in minutes:

- Auditing the remaining path-printing sites in `ct.rs`. Specifically
  deferred: `ct.rs:469` (`db_path` in `ct status`). The two `ct trust`
  sites that print a canonicalized target (`ct.rs:634`, `ct.rs:646`) are
  **in** scope — see Item 2 — because they are the same command and the
  same leak class as the alert being dismissed.
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
rationale comment. Also correct the stale claim in the `lock.rs` module
header (`lock.rs:13`) that "Both implementations use
`fs4::FileExt::lock_exclusive`" — both actually use the **non-blocking**
`try_lock_exclusive`, which is the semantics this fix relies on. It is a
one-line doc fix, and it is the exact hand-sync drift listed under Risks;
leaving it costs nothing today and misleads the next reader of the
duplicate pair. The lock file's contents are never read — the lock is held
via the **non-blocking** `fs4::FileExt::try_lock_exclusive` on the open handle
(contention surfaces as `Err(AlreadyLocked)` / `Err(WouldBlock)`, not `Ok(false)`
— see the API note in `lock.rs:63-73` and `fs_watcher.rs:106-113`). Truncation
was therefore always unnecessary. The fix is narrow: it prevents truncation of a
symlinked target. A planted symlink can still redirect the lock and cause
contention or denial of service; no-follow / rejection behavior is a separate
concern, explicitly out of scope here.

### Test

New test in `tools/src/lock.rs`'s `mod tests`, alongside
`vault_lock_blocks_second_acquire`:

`vault_lock_does_not_truncate_symlink_target` — create a temp vault, write a
canary file with known contents, symlink `.curated_thoughts.lock` to the
canary, call `VaultLock::acquire`, drop the guard, then assert the canary's
contents are unchanged.

This exercises the real exploitation path against the real filesystem with no
mocking. It fails on `truncate(true)` and passes after the fix.

The assertion is **only** that the canary's contents are unchanged — the
test must not `expect()` a successful `acquire`. If a later hardening pass
makes `acquire` reject a symlinked lock path outright, the call returns
`Err`, the canary is still intact, and this test should keep passing
unmodified.

**Platform scope (Unix-only, this test only).** `#[cfg(unix)]` is applied to
`vault_lock_does_not_truncate_symlink_target` itself (or, equivalently, that
test lives in a Unix-only submodule) — **not** to the surrounding `mod tests`,
so `vault_lock_blocks_second_acquire` and `vault_lock_released_on_drop`
remain enabled on Windows. The skip exists because
`std::os::windows::fs::symlink_file` requires Developer Mode or
`SeCreateSymbolicLinkPrivilege`, and `LockFileEx` holds an exclusive lock on
the handle that would defeat a follow-up `fs::read` (or any second handle to
the locked range) with `ERROR_LOCK_VIOLATION` even on the same process.
Dropping the `VaultLock` before reading the canary sidesteps the latter, but
the former is a developer-environment property the test cannot rely on, so
we skip on Windows rather than carry a flaky gate. The `tools` crate has no
Windows CI surface today (`.github/workflows/ci.yml` runs `rust-ubuntu` and
`rust-macos` against `src-tauri` only; `build.yml` builds but does not test),
so the skip does not mask any CI signal — it only documents the platform
restriction for future maintainers.

### Known follow-up

`fs_watcher.rs` carries the correct `truncate(false)` but has no regression
test of its own. Porting this canary test to the desktop crate is tracked as
issue #141, not part of this PR — the active hazard is in `lock.rs`, and the
scoping keeps this change reviewable.

## Item 2 — CodeQL `rust/cleartext-logging` alert

> **Note on alert numbering.** CodeQL does not fingerprint the same query
> finding across a statement change — when this PR rewrites the flagged
> line, the prior alert closes itself and a new one opens at the new
> statement with a fresh number. Throughout this section "alert #2" means
> "the alert that the prior statement produced against
> `ct.rs:575`"; "the persisted alert" / "the post-merge alert" means
> whatever number lands on `main` after this PR merges. Both the spec
> and the in-source comment under the print site avoid pinning the new
> number, because that is exactly the bit that rotates.

### Problem

`tools/src/bin/ct.rs:575` prints the trusted-link ledger:

```rust
println!("{} -> {}", entry.link, redact_home(&entry.target));
```

`entry.target` is sanitised by `redact_home`: `$HOME` prefixes collapse to
`~`, so a target like `~/.ssh` is never printed as an absolute path into CI
logs or system journals.

`entry.link` is **not** sanitised by `redact_home`. The reviewer flagged
this, and the gap is real:

- `TrustedLink::link` (`src-tauri/src/trusted_links.rs:14-21`) is
  documented as "Vault-relative path of the symlink itself, e.g.
  `documents/specs`", and the in-tree writer `approve_into` does pass a
  vault-relative string — so a value routed through `ct trust` /
  `cmds.rs::approve_link` is structurally vault-relative.
- But `BrainConfig::load_lenient` (`src-tauri/src/config/mod.rs:531-541`)
  deserialises each ledger entry directly via
  `serde_json::from_value::<TrustedLink>`. There is no path-shape check.
  A hand-edited `~/.brain/config.json`, or any program that writes the
  config, can put an absolute path (or any other string) into `link`,
  and `ct trust --list` would print it verbatim into stdout.

The serde boundary — not the type docstring — is the surface that reaches
the `println!`, and it enforces nothing. The dismissal rationale therefore
has to rest on the print site, not on the load contract.

CodeQL's `rust/cleartext-logging` query does not model `redact_home` as a
sanitiser, so it flags the call anyway. The `// codeql[rust/cleartext-logging]`
comment added above it is inert — inline suppression comments work for some
CodeQL languages but **not** for Rust, so the alert against this prior
statement (numbered `#2` at the time of #124, and now historical — see the
rotation note at the top of Item 2) has stayed open since #124 while
appearing, to a reader, to be handled.

### Decision: false positive after defense-in-depth fix, dismissed upstream

`ct trust --list` exists to show the user what they trusted. Printing the
link without its target would remove most of the listing's value, so the
`println!` stays as-is — but the dismissal is now conditional on a
defense-in-depth fix in `ct.rs`: pass `entry.link` through `redact_home`
at the print site so the printable form is constrained regardless of what
the JSON on disk contains.

The load-boundary check (`BrainConfig::load` validating `entry.link` is
vault-relative, or rejecting it outright) is tracked as **issue #140** and
is **not** part of this PR's dismissal rationale. That follow-up is correct
hardening but needs a vault_root-at-load-time discussion and its own spec;
bundling it here would re-open PR #124's review surface.

### Fix

Four changes in `ct.rs`:

1. Pass `entry.link` through `redact_home` at the print site. `redact_home`
   collapses a `$HOME` prefix to `~` and leaves other strings untouched, so
   the typical `documents/specs` ledger entry prints unchanged. A
   misconfigured `~/.ssh/keys` entry would print as `~/.ssh/keys` (no
   leak). Note what this does **not** buy: `redact_home` is a prefix
   collapse, not validation. A non-home absolute path (`/var/tmp/x`, a
   network share) still prints in full, and `rust/cleartext-logging` models
   no sanitiser at all — it flags the sink regardless. So the dismissal
   rests on the narrow claim that **no `$HOME`-rooted path reaches stdout
   from this statement**, not on a claim that every sensitive value has
   been removed from the output. Constraining what `entry.link` can hold in
   the first place is the load-boundary fix tracked as issue #140; until
   that lands, this print site is defense in depth over an unvalidated
   field, which is exactly why the dismissal justification must cite the
   sanitiser rather than the field's documented shape.

   **The narrow claim is only as good as the prefix match, and that match
   was platform-dependent.** `redact_home` compares `Path::components()`
   against `dirs::home_dir()`, but on Windows — a platform `build.yml`
   ships (`windows-latest`) — `std::fs::canonicalize` returns
   extended-length paths (`\\?\C:\Users\me\.ssh`) whose first component is
   `Prefix::VerbatimDisk`, while `dirs::home_dir` yields `Prefix::Disk`.
   Those are distinct enum values, so the home prefix did not match and a
   `$HOME`-rooted absolute path printed **verbatim** — precisely the leak
   the claim denies. A `prefix_eq` helper now normalizes the verbatim/plain
   pair (and compares drive letters and UNC share names
   ASCII-case-insensitively), and `component_eq` compares the remaining
   components case-insensitively on Windows only, matching NTFS while
   leaving Unix's exact comparison — and its genuinely distinct
   `/Users/Me` vs `/users/me` — alone. `std::path::Prefix` is constructible
   on every platform, so `prefix_eq` is unit-tested on Unix hosts too;
   `redact_home_collapses_actual_canonicalize_output_for_home` additionally
   asserts the invariant against real `canonicalize` output rather than a
   hand-written string (it skips when `$HOME` itself traverses a symlink,
   where the two paths name the same directory by genuinely different
   components — the macOS `/var` → `/private/var` class, handled at the call
   site by canonicalizing `vault_root`, not here). CodeRabbit, PR #129.
2. Wrap **every** path the `ct trust` arms print in `redact_home` — not just
   the `--list` statement. Sites are named rather than numbered because
   line numbers drift (same reason the alert numbers were de-hardcoded
   above):
   - the `Denied` and `Pending` arms' `target_display`, each a raw
     `std::fs::canonicalize()` result and so an absolute path, commonly
     under `$HOME`;
   - the `{link}` echo in the `Denied`, `Trusted`, and `Pending` arms, plus
     the `no such link` / `not a symlink` guards;
   - the `--revoke` arm's `revoked <link>` and `is not in the ledger`
     messages. This is the one `link` print path with a *live* leak after
     change 4 below: `--revoke` matches against the on-disk ledger, so the
     unvalidated `TrustedLink::link` of issue #140 reaches it without
     passing through the CLI's own guard.

   CodeQL flagged none of these, but they are the same leak class as the
   alert on the `--list` statement, from the same command, and a dismissal
   that says "`ct trust` sanitises its printed paths" is not true while any
   of them stand. Naming all of them also makes the invariant
   grep-checkable: no `{link}`-style bare interpolation of a path survives
   in `trust_cmd`.
3. Delete the inert suppression comment and replace it with a plain comment
   that records the sanitiser, the verdict, and a warning against re-adding
   the suppression:

```rust
// Both fields on this line are sanitised by `redact_home` before
// printing: the `$HOME` prefix is collapsed to `~`, so the values
// this statement writes cannot contain an absolute path under the home
// (e.g. `~/.ssh/keys`). CodeQL rust/cleartext-logging flags this
// anyway (it does not model `redact_home` as a sanitiser); the
// persisted alert dismissed as a false positive citing this sanitiser.
// Inline `// codeql[...]` suppression does NOT work for Rust — do not
// re-add it.
println!("{} -> {}", redact_home(&entry.link), redact_home(&entry.target));
```

4. **Reject a `<link>` argument that is not vault-relative, before any
   join.** This one is not a logging fix; the review surfaced it while
   arguing about the print sites, and it is the more serious of the two.
   `trust_cmd` did `vault_root.join(&link)`, and `approve_into`
   (`src-tauri/src/trusted_links.rs`) independently does the same — but
   `Path::join` **replaces** its base when the argument is absolute, or on
   Windows merely carries a prefix (`C:foo`) or a root (`\foo`). So
   `ct trust /Users/me/.ssh` did not resolve inside the vault at all: it
   escaped to the absolute path, had `classify_link` judge a path the vault
   does not contain, and on a `Pending` verdict **persisted that absolute
   string into the ledger** as `TrustedLink::link`. That reaches the issue
   #140 gap through the CLI rather than only by hand-editing
   `config.json` — the CLI was the one writer the spec above credits with
   being "structurally vault-relative", and it was not.

   The guard rejects any `link` whose first component is
   `Component::Prefix(_)` or `Component::RootDir`, which covers Unix
   absolute (`/x`), Windows absolute (`C:\x`), Windows root-relative
   (`\x`), and Windows drive-relative (`C:x`) — the last of these is not
   `is_absolute()` yet still replaces the prefix on join, so
   `is_absolute()` alone would have been the wrong test. It runs before
   both joins, so it also makes every `{link}` echo in the arms below
   structurally incapable of carrying a `$HOME`-rooted absolute path;
   change 2's redaction of those echoes is then defense in depth rather
   than the only barrier. Covered by
   `trust_refuses_an_absolute_link_and_leaves_the_ledger_empty`, which
   asserts both the exit code and that the ledger stays empty.

   Traversal escapes (`../../etc`) are deliberately **not** rejected here:
   they stay vault-relative as strings, `classify_link`'s existing
   `vault_root` containment rules are what govern them, and widening this
   guard into a path-traversal policy is the load-boundary discussion
   tracked as #140. CodeRabbit, PR #129.

The post-merge alert is then dismissed in the GitHub Security UI as a false
positive, citing that justification. **This is a manual maintainer step** —
it cannot be done from the PR, and the CodeQL run on this PR will still
report a `rust/cleartext-logging` finding on the post-merge statement until
it is performed. The PR body will call this out.

### Why no CodeQL config file

Adding `.github/codeql/config.yml` with a `query-filters` exclusion for
`rust/cleartext-logging` would suppress the query more broadly than one line
and could mask a real future leak. With dismissal-only, a future print site
correctly earns its own alert to triage.

## Verification

- `cargo test --manifest-path tools/Cargo.toml` — new symlink canary test
  plus existing lock tests. (The package is `curated-thoughts-tools` and
  there is no root workspace, so `-p tools` does not resolve.)
- `cargo clippy --manifest-path tools/Cargo.toml --all-targets` — **run
  locally on 2026-09-02**; the only output is the pre-existing
  `variant \`Delete\` is never constructed` dead-code warning from the
  `curated-thoughts` lib (baseline, unrelated to this change). No new
  warnings attributable to `lock.rs` or `ct.rs`. This must be run locally:
  CI does not gate clippy today, so a warning introduced here would not
  fail the PR.
- CI green on the PR.
- CodeQL green **except** the persisted `rust/cleartext-logging` alert on
  the post-merge statement, which persists until manually dismissed (see
  Item 2's rotation note).

## Risks

- **Dismissal is manual and off-repo.** If the maintainer does not dismiss
  the post-merge alert, the HIGH alert stays open and this PR only removes
  a misleading comment. Mitigated by calling the step out explicitly in
  the PR body.
- **The duplication remains.** `lock.rs` and `fs_watcher.rs` must stay in
  sync by hand until the phase-3 workspace migration collapses them. Both
  module headers already document this; the divergence this PR fixes was
  caught only by manual review, not by any test or lint.
- **Load-boundary validation is deferred.** This PR closes the
  cleartext-logging print path via `redact_home` on both fields. The
  underlying gap — `BrainConfig::load_lenient` accepting any
  `TrustedLink` shape without validating `entry.link` as vault-relative
  — is a separate hardening item tracked as **issue #140** (could be a
  validation that rejects non-vault-relative entries, or a normalisation
  step that converts absolute paths to vault-relative). That work needs a
  vault_root-at-load discussion and its own spec; it is not a
  release-blocker for the `redact_home` print-site fix.
