# Absolute-Link Guard in `approve_into` — Design

**Date:** 2026-09-03
**Status:** Implemented 2026-09-03 — dual-track implementation review APPROVED (GLM 5.3: SPEC COMPLIANT yes / Approved, 0C/2I/5m, findings folded in; default-model reviewer: SPEC COMPLIANT yes / Approved, 0C/0I/3m). Follow-up empty-link issue filed as #143.
**Branch:** `fix/approve-into-absolute-link-guard`
**Priority:** P1 (security-class robustness gap; closes issue #142, found in the PR #129 review)

## 1. Problem

PR #129 (#129, merged 2026-09-02) closed the P0 security debt, including a
guard in the CLI `ct trust` subcommand that refuses a non-vault-relative
`link` argument **before any `Path::join`**. That guard is CLI-only. The
other entry point into the same approval logic — the Tauri `approve_link`
command (`src-tauri/src/lib.rs:3900-3932`) — passes the front-end-supplied
`link: String` straight into the shared helper
`approve_into` (`src-tauri/src/trusted_links.rs:106`) with no such check
(issue #142, filed from the PR #129 review).

### 1.1 Why an unvalidated `link` escapes the vault

`Path::join` **replaces the base** when its argument is absolute — or, on
Windows, merely carries a drive prefix (`C:foo`) or is rooted (`\foo`):

- `Path::new("/vault").join("/etc/passwd")` → `/etc/passwd`
- `Path::new("C:\\vault").join("C:foo")` → `C:foo`

`approve_into` does `std::fs::canonicalize(vault_root.join(link))` and then
classifies the resolved target. With an absolute `link`:

1. The vault containment decision is made about a path **outside the vault
   entirely** — the classification answers a question nobody asked.
2. On a `Pending` verdict, the raw `link` string is appended to the ledger
   as `TrustedLink::link`, which every consumer documents as
   **vault-relative** (the load-boundary half of that contract is tracked
   separately as issue #140).
3. `approve_into`'s resolution-failure error embeds the raw `link`
   (`"{link} could not be resolved: {e}"`). When the CLI prints that error
   (`tools/src/bin/ct.rs`, the `Err(e)` arm: `eprintln!("error: {e}")`) a
   `$HOME`-rooted absolute path would be echoed **unredacted**. Today the
   CLI's own guard (ct.rs:669) fires before `approve_into` is reached with
   an absolute link, so this is not a live leak — but once the helper owns
   the guard, its new "… is not vault-relative" error embeds the raw link
   and reaches this print arm, so the echo must route through `redact_home`
   (defense-in-depth, per issue #142's own note).

### 1.2 Current-state evidence (verified 2026-09-03, main @ 00abde9, branch @ 6cb4ccf)

- `src-tauri/src/trusted_links.rs:106-118` — `approve_into` joins and
  canonicalizes with no prefix check.
- `src-tauri/src/lib.rs:3900-3932` — `approve_link` canonicalizes the vault
  root, then calls `approve_into` with the raw `link` argument.
- `tools/src/bin/ct.rs:658-679` — the CLI's guard (added in PR #129) with a
  comment explicitly noting `approve_into`'s join hazard; `ct.rs:734-737` —
  the unredacted `Err(e)` print.
- `tools/tests/ct_trust.rs:93` — `trust_refuses_an_absolute_link_and_leaves_the_ledger_empty`
  covers the CLI path only.

## 2. Approach

Move the guard into the **shared helper** so both entry points inherit it,
and redact the one error echo that can carry an absolute path:

1. Add `pub fn is_vault_relative_link(link: &str) -> bool` to
   `trusted_links.rs`: false iff the first `Path` component is
   `Prefix(_)` or `RootDir` (same predicate the CLI guard uses — one
   definition, not two).
2. In `approve_into`, return `Err("{link} is not vault-relative")`
   **before** the canonicalize/join when the predicate fails. Fail-closed,
   zero filesystem access, ledger untouched.
3. In `ct.rs`'s `Err(e)` arm, print `redact_home(&e)` instead of `{e}`.
4. Keep the CLI's earlier, nicer diagnostic exactly where it is: it now
   becomes defense-in-depth + UX (the helper error is a backstop, the CLI
   message stays the first line a user sees).

**Rejected alternative — guard only in the Tauri caller:** leaves the
helper unsafe for the next caller (there are already two), and duplicates
the predicate at N call sites instead of one. The helper is the right home:
it owns the join.

**Rejected alternative — type-level vault-relative newtype:** safer by
construction but a cross-cutting refactor of `TrustedLink::link: String`
consumers; disproportionate for the gap. Noted as a possible follow-up,
not in scope.

## 3. Design details

- **Predicate semantics:** `Path::new(link).components().next()` is
  `Some(Prefix(_) | RootDir)` → not vault-relative. Empty string and normal
  relative paths pass the predicate. Empty-link note (review finding I1):
  the empty link is NOT stopped by canonicalize — `vault_root.join("")`
  yields the vault root, which canonicalizes successfully — so it proceeds
  into `classify_link` (pre-existing main behavior; follow-up filed, not
  a regression of this change).
  ct.rs's inline check is REPLACED by a call to `is_vault_relative_link`
  (its diagnostic message and early-return position are unchanged) — one
  definition, not two.
- **Error contract:** `approve_into`'s new error string embeds the raw
  `link` (useful in the Tauri `Err(String)` channel, where the frontend
  controls display). The CLI — the only place that prints to a terminal —
  must redact. Documented on the helper: "Echos of `link` in the error
  string are the caller's to redact."
- **Windows:** the `Prefix` arm makes `C:foo`, `C:\foo`, `\foo` fail the
  predicate on Windows. On Unix those strings are ordinary relative names
  and pass — correct, since Unix `join` would not replace the base for
  them. Platform-dependent tests are `#[cfg(windows)]`.
- **No behavior change for valid relative links:** the guard only rewrites
  the absolute case; every existing test path (Trusted / Pending / Denied)
  is untouched.

## 4. Testing

New tests in `src-tauri/tests/trusted_links.rs` (integration, same file as
the existing `classify_link` suite). `tempfile` is already a src-tauri
dev-dependency (Cargo.toml `[dev-dependencies]`), so the temp-vault tests
need no dependency change:

- `vault_relative_predicate_rejects_absolute_links` — `/etc/passwd`, `/`.
- `vault_relative_predicate_accepts_relative_links` — `documents/specs`,
  `a`, `""`.
- `#[cfg(windows)] vault_relative_predicate_rejects_windows_prefixes_and_roots`
  — `C:foo`, `C:\foo`, `\foo`.
- `approve_into_refuses_absolute_link_before_any_join` — absolute link on
  a nonexistent-in-vault path still errors with the vault-relative message
  (proves no filesystem access occurred) and the ledger is byte-identical
  before/after.
- `approve_into_still_reports_resolution_errors_for_relative_links` — a
  missing relative link keeps the `could not be resolved` error; nothing
  appended.

Gate: `cargo test --manifest-path src-tauri/Cargo.toml --test trusted_links`
plus a `cargo check` of the `tools` crate (the `redact_home` call-site
change). CI covers src-tauri; the tools crate has no CI test surface, so
the CLI change is verified by compile + existing `ct_trust` tests locally.

## 5. Out of scope / open questions

- **#140 (load-boundary validation of stored `TrustedLink::link`)** —
  separate task; this spec fixes the write path only. Do NOT bundle into
  this PR.
- **#125 TOCTOU in `create_parents_no_symlink`** — unrelated, separate.
- **Open question for Kurt (non-blocking):** should the Tauri
  `approve_link` command surface a friendlier message than the raw helper
  error? Frontend text is product territory; the helper error is correct
  and safe today.
