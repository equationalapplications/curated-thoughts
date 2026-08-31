# CodeQL Security Debt from PR #124 — Spec

**Date:** 2026-08-31
**Status:** Draft
**Packages:** `curated-thoughts` (`tools` crate, `src-tauri` crate)
**Branch:** `chore/clear-codeql-debt-pr124`
**Depends on:** PR #124 (merged 2026-08-31T06:25Z), commit `6c1113e`

---

## Executive Summary

PR #124's cleanup pass left two security defects behind. One is a standing
high-severity CodeQL alert whose attempted fix relies on a suppression
mechanism that does not exist for Rust. The other is a lock-file truncation
hazard that was fixed in one copy of a deliberately-duplicated module and
missed in the other, silently diverging two files whose header contract says
they must stay identical.

Neither is exploitable today. Both are traps armed for the next change: the
first hides a real inconsistency in how the CLI redacts filesystem paths, and
the second destroys data the moment anyone writes content into the vault lock
file — which the code's own doc comment already promises to do.

Fixing the first surfaces a third issue that only exists once the fix lands:
routing canonicalized paths through the redaction helper is a silent no-op on
Windows (D2). The spec also takes one piece of adjacent hardening — restricting
lock-file permissions (D5) — because the `OpenOptions` chain is already open in
front of us and the file is expected to hold holder identity later.

## Background

### Defect 1 — `rust/cleartext-logging`, alert #2

Code-scanning alert
[#2](https://github.com/equationalapplications/curated-thoughts/security/code-scanning/2)
is **open**, severity **high** (CWE-312 / CWE-359 / CWE-532), at
`tools/src/bin/ct.rs:575`:

```rust
println!("{} -> {}", entry.link, redact_home(&entry.target));
```

PR #124 addressed the review thread by adding an inline comment above it
(`ct.rs:571-574`):

```rust
// codeql[rust/cleartext-logging]: `entry.target` is sanitised by ...
```

GitHub's alert-suppression queries do not cover Rust, so this comment is
inert. It reads as a machine-honored suppression and is not one. The alert was
created 2026-08-31T02:23Z and survived the 06:25Z merge.

The `redact_home` helper it points at (`ct.rs:11-40`) is real, component-aware,
and covered by four tests (`ct.rs:658-702`). The problem is not that the fix
was fake — it is that the fix was applied to exactly one of the six sites that
print filesystem paths, and then annotated as if that settled the query.

Unredacted sites in the same file:

| Line | Command | Expression | Sensitivity |
|------|---------|-----------|-------------|
| 634 | `ct trust <link>` | `target_display` in the `refused:` message | **Canonicalized** symlink target — resolves to the real absolute path |
| 646 | `ct trust <link>` | `target_display` in the `trusted:` message | Same |
| 292 | `ct ingest` | `db_path.display()` in the refusal | Absolute path to the brain database |
| 469 | `ct status` | `brain.paths.db_path.display()` | Same |
| 530 | `ct librarian run` | `brain.paths.db_path.display()` in the refusal | Same |

Lines 634 and 646 are *more* sensitive than the flagged line 575: they print
the fully canonicalized target of a symlink the user is trusting, whereas 575
prints the stored ledger value. CodeQL flagged the one site that was already
sanitised and missed the five that were not.

`ct.rs:450` also emits `db_path`, but inside the `--json` branch. It is
excluded — see D1.

### Defect 2 — lock-file truncation, `tools/src/lock.rs:46`

```rust
let file = fs::OpenOptions::new()
    .create(true)
    .truncate(true)   // <-- hazard
    .write(true)
    .read(true)
    .open(&lock_path)?;
Self::try_lock_exclusive(&file)?;   // lock acquired AFTER the truncate
```

The file is truncated at open, and the lock is attempted afterward. Two
consequences:

1. **Contention destroys state.** A second `ct watch` arriving while the first
   holds the lock truncates the lock file *before* discovering it is locked.
   The file carries no content today, so the blast radius is currently zero —
   but `VaultLock::acquire`'s own doc comment (`lock.rs:38-39`) promises an
   error "identifying the existing holder", and the natural implementation of
   that promise is to write holder identity into the file. That change would
   turn this into live data loss with no test to catch it.
2. **Symlink target destruction.** If `.curated_thoughts.lock` is a symlink,
   opening it for write follows the link, and the truncate destroys the
   target's contents — content the application never intended to touch.

Consequence 2 is not hypothetical: it is the exact bug fixed in the twin
module by commit `6c1113e` (2026-08-30, PR #124's second review round), which
set `src-tauri/src/watcher/fs_watcher.rs` to `.truncate(false)` with a written
rationale (`fs_watcher.rs:131-137`).

`tools/src/lock.rs:1-16` declares itself a **deliberate duplicate** of that
type and states that "keeping the lock-file path and semantics identical
ensures the two lockers see each other across a desktop/CLI hand-off." PR #124
fixed one twin and left the other truncating. The duplicates have diverged in
precisely the way the header exists to prevent, and nothing in the test suite
notices.

Both `truncate` calls originated in the same clippy pass
(`suspicious_open_options`, which fires on `create(true)` when neither
`truncate` nor `append` is specified). The lint is satisfied by an explicit
`.truncate(false)` — the fix is to state the intent, not to invert it.

## Goals

1. Apply path redaction consistently at every human-readable path-printing
   site in `ct.rs`.
2. Make that redaction actually fire on Windows, where canonicalized paths
   would otherwise slip through unredacted.
3. Remove the inert `codeql[...]` comment and replace it with an honest,
   auditable disposition for alert #2.
4. Restore `tools/src/lock.rs` to lock semantics matching `fs_watcher.rs`.
5. Restrict lock-file permissions in both twins before holder identity is
   ever written to that file.
6. Pin both twins with a behavioral test so the next divergence fails CI.

## Non-Goals

- The phase-3 workspace migration that collapses `VaultLock` into a single
  shared definition. The duplication stays; this spec only makes the
  divergence detectable.
- A repo-wide code-scanning triage policy, SARIF gating, or review cadence.
- Auditing path-printing sites outside `tools/src/bin/ct.rs`.
- Changing what the CLI prints. Users must still be able to read which link
  they trusted; redaction collapses `$HOME`, it does not elide the path.

## Design

### D1 — Redact every path-printing site in `ct.rs`

Route every **human-readable** path-printing site through `redact_home`,
matching line 575: lines 292, 469, 530, 634, and 646.

`ct.rs:450` is deliberately excluded. It is inside the `--json` branch, whose
output is parsed by tooling; a `~`-collapsed path is not a valid path and would
break consumers that feed it back to the filesystem. `--json` output is
machine-facing and already at the same trust boundary as the brain directory
itself. This exclusion is a decision, not an oversight, and the code carries a
comment saying so.

For 634 and 646, the value is built by canonicalizing `link_path` and calling
`.display().to_string()` in two places with identical logic. Extract that into
a single helper that canonicalizes *and* redacts, so the two call sites cannot
drift apart the way the two `VaultLock`s did:

```rust
/// Canonicalize `link_path` for display, falling back to the uncanonicalized
/// path, with `$HOME` collapsed to `~`.
fn display_target(link_path: &Path) -> String {
    let resolved = std::fs::canonicalize(link_path)
        .map(|t| t.display().to_string())
        .unwrap_or_else(|_| link_path.display().to_string());
    redact_home(&resolved)
}
```

Lines 292, 469, and 530 wrap `db_path.display().to_string()` in `redact_home`
directly; they need no canonicalization.

`redact_home`'s matching logic is unchanged. Its input handling is not — see D2.

### D2 — UNC prefix normalization before redaction (Windows)

`display_target` is the only place where `std::fs::canonicalize` output reaches
`redact_home`, and on Windows that output is a verbatim UNC path
(`\\?\C:\Users\Name\Vault`) while `dirs::home_dir()` returns a normal path
(`C:\Users\Name`).

`redact_home` compares `Path::components()` slices rather than doing string
replacement, which is what makes it separator-agnostic today. That does **not**
save it here. The two paths differ at component *zero*:
`Component::Prefix(VerbatimDisk('C'))` versus `Component::Prefix(Disk('C'))`
are distinct `Prefix` variants and compare unequal, so the prefix match fails
immediately and `redact_home` returns the path untouched.

The result is a silent no-op: on Windows, `ct trust` prints the fully resolved
absolute target for both the `refused:` and `trusted:` messages — the two most
sensitive sites in the file — while every test passes on Unix CI. This is a
correctness bug in the fix D1 introduces, not a pre-existing one, because
`display_target` is what newly routes canonicalized output into `redact_home`.

Strip the verbatim prefix before redacting. In `display_target`, after
canonicalizing and before calling `redact_home`, drop a leading `\\?\` (and
`\\?\UNC\`, which canonicalize emits for network shares) so the components
line up with `home_dir()`'s. Implement it as a small named helper with its own
tests rather than inline, so the intent survives the next edit.

Tests must cover the UNC shapes explicitly. They are `#[cfg(windows)]`-gated
for the real `canonicalize` behavior, plus a platform-independent unit test of
the stripping helper against literal `\\?\C:\...` input so the logic is
exercised on Unix CI too — otherwise this regresses on a machine no one runs
tests on.

### D3 — Disposition for alert #2

Delete the `codeql[...]` comment block at `ct.rs:571-574`. Replace it with a
plain comment stating that `redact_home` is the sanitiser and pointing at its
tests, with no syntax implying machine enforcement.

Consistent redaction is the correct fix but is **not guaranteed** to clear the
alert: CodeQL's taint tracking carries no model marking `redact_home` as a
sanitiser, so the flow from `entry.target` to `println!` may still be reported.
The spec therefore commits to a disposition rather than assuming success.

After the change lands on `main` and CodeQL re-runs:

- **If the alert closes** — done. Record the outcome in this spec's Status
  line.
- **If the alert persists** — dismiss it via the code-scanning API with reason
  `false positive` and a comment naming `redact_home`, its four tests, and this
  spec. Record the dismissal, its date, and its reason in a "Disposition"
  section appended to this spec.

The dismissal is recorded in the spec, not in a source comment, because a
dismissal is a human judgment about an alert and belongs where it can be
audited and revisited — not in a comment that the next reader may again
mistake for machine-honored suppression.

### D4 — `tools/src/lock.rs` truncate parity

Change `.truncate(true)` to `.truncate(false)` and carry over the rationale
comment from `fs_watcher.rs:131-137`, adapted to name the CLI context. The
comment must state both hazards (symlink target destruction, and truncation of
a contended lock file before the lock is held), because only the first is
recorded in the desktop twin.

No other change to `VaultLock`. The lint stays satisfied by the explicit
`false`.

### D5 — Restrictive lock-file permissions in both twins

The lock file is opened for write in the same `OpenOptions` chain D4 already
edits, and `VaultLock::acquire`'s doc comment promises an error "identifying
the existing holder" — i.e. holder identity (PID, process name, user) is
expected to land in this file. Set the mode now, while the chain is already
being touched, so that data is not world-readable the day it arrives.

Add `.mode(0o600)` via `std::os::unix::fs::OpenOptionsExt`, `#[cfg(unix)]`-gated,
to **both** `tools/src/lock.rs` and `src-tauri/src/watcher/fs_watcher.rs`.
Applying it to only one twin re-opens the exact divergence this spec exists to
close.

Two limits, stated rather than papered over:

- **`mode()` applies only at creation.** An existing `.curated_thoughts.lock`
  keeps whatever mode it has. So this is a guarantee for new vaults and
  best-effort for existing ones. The spec does not chmod existing files: a
  silent permission change to a file the user may have created deliberately is
  a worse default than leaving it, and the file holds nothing sensitive today.
- **Unix only.** Windows has no `OpenOptions` equivalent; restricting the ACL
  there needs `windows-sys` security attributes, which is disproportionate for
  a currently-empty file. Out of scope, noted here so the gap is known.

This is defense in depth for data that does not exist yet. It is worth doing
because the cost is one gated line in a chain already being edited, not because
there is a live exposure.

### D6 — Parity regression test in both crates

Add the same behavioral test to `tools/src/lock.rs` and to the `src-tauri`
watcher tests:

```
given a vault dir containing `.curated_thoughts.lock` with known bytes
when VaultLock::acquire runs to completion
then the file's contents are byte-identical to what was written
```

This test fails today in `tools` and passes in `src-tauri`, so it encodes the
divergence directly rather than trusting the module header to prevent it. It is
a behavioral assertion about lock-file contents, not an assertion about
`OpenOptions`, so it keeps holding if either implementation is rewritten.

A second test covers the contention path in the `tools` crate: with known bytes
in the lock file and a lock already held, a second `acquire` must fail **and**
leave the bytes intact. This is the case that turns into data loss the moment
holder identity is written to the file.

## Verification

1. `cargo test --manifest-path tools/Cargo.toml` — new lock tests pass;
   existing `vault_lock_blocks_second_acquire` and `vault_lock_released_on_drop`
   still pass.
2. `cargo test --manifest-path src-tauri/Cargo.toml --features test-utils,mcp-server`
   — new parity test passes.
3. `cargo clippy -D warnings` clean across both crates (confirms
   `suspicious_open_options` stays satisfied by the explicit `truncate(false)`).
4. `cargo fmt --check` clean.
5. New `ct.rs` unit tests: `display_target` collapses `$HOME`; `db_path`
   output is redacted; `--json` output is **not** redacted (pins D1's
   exclusion so a later "consistency" pass does not silently break consumers).
6. Manual: `ct trust --list`, `ct trust <link>` on a home-directory symlink,
   `ct status`, and the `ct ingest` / `ct librarian run` refusals print
   `~`-collapsed paths; `ct status --json` still prints an absolute path.
7. UNC (D2): platform-independent unit test of the prefix-stripping helper
   against literal `\\?\C:\Users\...` and `\\?\UNC\server\share\...` inputs, so
   Unix CI exercises the logic; plus `#[cfg(windows)]` tests asserting
   `display_target` collapses `$HOME` on real `canonicalize` output.
8. Permissions (D5): `#[cfg(unix)]` test in **both** crates asserting a
   newly-created `.curated_thoughts.lock` has mode `0o600`.
9. Post-merge: CodeQL run on `main` — alert #2 either closes or is dismissed
   per D3, and this spec's Status line records which.

## Risks

- **Redaction is not a CodeQL sanitiser.** Addressed by D3's explicit
  either/or disposition; the fix is justified on its own merits regardless of
  what the query decides.
- **`redact_home` depends on `dirs::home_dir()`**, which returns `None` in some
  sandboxed environments; the helper then returns the path unchanged. This is
  the existing documented behavior and is out of scope, but it means redaction
  is best-effort, not a guarantee. The spec does not claim otherwise.
- **Windows redaction is untested in CI.** The release workflow runs
  `ubuntu-latest` only, so the `#[cfg(windows)]` half of D2's coverage never
  executes here. The platform-independent helper test is the real guard; the
  gated tests document intent for whoever adds a Windows runner.
- **Lock-file mode is create-only and Unix-only.** Per D5: existing lock files
  keep their current permissions, and Windows ACLs are untouched. This is
  hardening for data that does not exist yet, not a fix for a live exposure.
- **The duplication remains.** D6 makes divergence *detectable*, not
  impossible. Collapsing the twins stays queued for the phase-3 workspace
  migration.
