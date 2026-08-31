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
was fake — it is that the fix was applied to exactly one of the four sites that
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

1. Apply path redaction consistently at every path-printing site in `ct.rs`.
2. Remove the inert `codeql[...]` comment and replace it with an honest,
   auditable disposition for alert #2.
3. Restore `tools/src/lock.rs` to lock semantics matching `fs_watcher.rs`.
4. Pin both twins with a behavioral test so the next divergence fails CI.

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

`redact_home` itself is unchanged — it is already correct and tested.

### D2 — Disposition for alert #2

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

### D3 — `tools/src/lock.rs` truncate parity

Change `.truncate(true)` to `.truncate(false)` and carry over the rationale
comment from `fs_watcher.rs:131-137`, adapted to name the CLI context. The
comment must state both hazards (symlink target destruction, and truncation of
a contended lock file before the lock is held), because only the first is
recorded in the desktop twin.

No other change to `VaultLock`. The lint stays satisfied by the explicit
`false`.

### D4 — Parity regression test in both crates

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
7. Post-merge: CodeQL run on `main` — alert #2 either closes or is dismissed
   per D2, and this spec's Status line records which.

## Risks

- **Redaction is not a CodeQL sanitiser.** Addressed by D2's explicit
  either/or disposition; the fix is justified on its own merits regardless of
  what the query decides.
- **`redact_home` depends on `dirs::home_dir()`**, which returns `None` in some
  sandboxed environments; the helper then returns the path unchanged. This is
  the existing documented behavior and is out of scope, but it means redaction
  is best-effort, not a guarantee. The spec does not claim otherwise.
- **The duplication remains.** D4 makes divergence *detectable*, not
  impossible. Collapsing the twins stays queued for the phase-3 workspace
  migration.
