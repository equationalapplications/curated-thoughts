# Load-Boundary Validation of `TrustedLink::link` — Design

**Date:** 2026-09-03
**Status:** Draft for implementation
**Branch:** `fix/trusted-link-load-validation`
**Priority:** P1 (completes the ledger-contract work started in #142/#144; closes issue #140)

## 1. Problem

`TrustedLink::link` is documented as the vault-relative path of the symlink
(`src-tauri/src/trusted_links.rs:15-16`), but the write-path guard added in
PR #144 (`is_vault_relative_link` in `approve_into`) only constrains **new**
entries. The load boundary — `BrainConfig::load_lenient`
(`src-tauri/src/config/mod.rs`, `trusted_links` block at ~607-621) —
deserialises each ledger entry with a bare
`serde_json::from_value::<TrustedLink>` and accepts any string for `link`.

A hand-edited `~/.brain/config.json` can therefore hold an absolute path, a
`..`-traversal string, or a Windows drive prefix as `link`, and it is loaded
verbatim into `report.config.trusted_links` — the list the walker consults
to decide which symlinks to follow.

## 2. Approach — reject at load, lexical predicate, no vault root needed

Issue #140 frames two options (reject vs normalise) and a
`vault_root`-at-load-time question. The question dissolves once you note
that the enforcement predicate does not need the vault root at all:
`is_vault_relative_link` (`src-tauri/src/trusted_links.rs:85`) is purely
component-lexical — it refuses `Prefix(_)`, `RootDir`, and `ParentDir`
components without touching the filesystem. So:

1. In `load_lenient`'s `trusted_links` loop, after a successful
   deserialisation, apply `is_vault_relative_link(&entry.link)`. On failure,
   push a diagnostic and **drop the entry** — exactly the existing lenient
   drop-one-keep-the-rest semantics:

   ```rust
   match serde_json::from_value::<TrustedLink>(entry.clone()) {
       Ok(e) => {
           if crate::trusted_links::is_vault_relative_link(&e.link) {
               kept.push(e);
           } else {
               report.diagnostics.push(format!(
                   "trusted_links entry rejected: link {:?} is not vault-relative (absolute, rooted, or contains `..`)",
                   truncate_for_diag(&e.link)
               ));
           }
       }
       Err(err) => report
           .diagnostics
           .push(format!("trusted_links entry unparseable: {}", err)),
   }
   ```

2. **One predicate, two boundaries.** The write path (#144) and the load
   path (#140) must share `is_vault_relative_link` — never a second copy of
   the component rules. If the predicate ever changes (see #143), both
   boundaries move together for free.

3. Update `TrustedLink::link`'s docstring to state the enforced reality:
   vault-relative is checked at BOTH the approval write path and the config
   load boundary; non-conforming entries are dropped with a diagnostic.

### Migration note (pre-#144 ledgers)

`approve_into` before PR #144 (merged 2026-09-03) accepted absolute links
and could persist them on a `Pending` verdict, so real user ledgers may
contain non-vault-relative `link` values. Dropping them at load is the safe
direction: the affected symlinks revert to `Pending`, the walker stops
following them (fail-closed — unapproved symlink targets are never read,
per the module's exfiltration boundary), and the user re-approves via
`ct trust`. No migration shim, no silent normalisation.

### Diagnostic echo

The diagnostic includes the offending `link` value, truncated to 120 chars
(small helper, avoids log-flooding from a hand-edited giant string). The
value originates in the user's own local config file, and src-tauri has no
`redact_home` equivalent (the one in `tools/src/bin/ct.rs` is CLI-local);
building shared redaction infrastructure is out of scope here.

### Rejected alternatives

- **Normalise absolute→relative at load (option 2):** needs the vault root
  at load time (threading), plus canonicalize I/O on a hot sync load path,
  and silently rewrites a user's file-backed data. Rejection + diagnostic
  is simpler, I/O-free, and matches the ledger's existing lenient style.
- **Vault-relative newtype:** the right long-term shape but a cross-cutting
  refactor of every `link: String` consumer; already noted as out of scope
  in the #142 spec, unchanged here.

## 3. Testing

In `src-tauri/tests/config_leniency.rs` (the load-lenient suite), following
its existing temp-config helpers:

- `load_lenient_rejects_absolute_trusted_link` — ledger with one
  well-formed entry and one absolute-link entry: `config.trusted_links`
  keeps exactly the well-formed one; one diagnostic containing
  "not vault-relative".
- `load_lenient_rejects_parentdir_trusted_link` — `../outside-link` and
  `documents/../secrets` entries: both dropped, diagnostics present,
  sibling entries survive.
- `load_lenient_accepts_wellformed_trusted_links` — relative links
  (including `documents/specs` and the empty string `""`, which passes the
  predicate by design — the empty-link write-path hazard is #143's scope,
  and the load path should stay consistent with the predicate).
- Existing unparseable-entry diagnostics behavior unchanged (no new test
  needed; suite already covers it).

**Sabotage check:** comment out the new predicate call → the two reject
tests must fail; restore → pass.

**Gate:** `cargo test --manifest-path src-tauri/Cargo.toml --test config_leniency`
plus the full `trusted_links` suite (predicate untouched, must stay green)
and `cargo check --manifest-path src-tauri/Cargo.toml`.

## 4. Out of scope

- **#143** — empty/whitespace link at the write path (`approve_into` /
  `classify_link`). Separate PR; note that when #143 tightens the
  predicate, THIS load boundary tightens with it automatically (shared
  predicate), so do not pre-empt it here.
- **#146** — lock error diagnostics; unrelated subsystem.
- Shared redaction infrastructure for src-tauri diagnostics.
