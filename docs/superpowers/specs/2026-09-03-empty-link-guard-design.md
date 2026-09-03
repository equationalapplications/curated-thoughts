# Empty-Link Guard in `approve_into` + `classify_link` — Design

**Date:** 2026-09-03
**Status:** Implemented
**Branch:** `fix/empty-link-guard`
**Priority:** P2 (hygiene hardening on the trust ledger; closes issue #143)

## 1. Problem (as filed, and as verified on main @ 6bf339a)

Issue #143 (filed from the GLM 5.3 review of PR #144): an empty `link`
passes `is_vault_relative_link` (empty string has no offending components —
by design), `vault_root.join("")` yields the vault root, canonicalize
succeeds on any configured vault, and the value flows into
`classify_link("", vault_root, vault_root, ...)`.

**Verified actual behavior today:** the verdict is `Trusted`, not `Pending`.
In `classify_link`, `target == vault_root`, so the `target != vault_root`
guard skips the ContainsVault denial, no ledger ancestor matches, and
`is_within(&vault_root, &vault_root)` is true → in-vault auto-trust branch.
**Nothing is persisted** (the ledger write only happens on `Pending`).
So this is a semantics/correctness wart, not an active ledger-poisoning
bug — but:

- a `Trusted` verdict for a meaningless input is wrong on its face,
- the issue's feared `Pending`-persists-`{link:""}` outcome is one
  classification change away,
- `classify_link` is a public API also called from `walk_vault.rs` and
  directly by tests; it should be fail-closed on nonsense input
  independently of `approve_into`.

## 2. Approach

Two layers, both fail-closed:

1. **Predicate level (write path):** in `is_vault_relative_link`, refuse
   empty and whitespace-only input up front:

   ```rust
   pub fn is_vault_relative_link(link: &str) -> bool {
       if link.trim().is_empty() {
           return false;
       }
       !Path::new(link).components().any(|c| { /* unchanged */ })
   }
   ```

   Because `approve_into`'s first line is the predicate check, an empty or
   `"   "` link now errors with the existing `"... is not vault-relative"`
   message before any join/canonicalize/classify. Whitespace-only is
   included: `Path::new(" ")` yields a `Normal(" ")` component and would
   otherwise pass; joining it produces a nonsense path.

2. **Classify level (belt-and-suspenders, public API):** add a new
   `DenyReason::EmptyLink` (message: `"link is empty"`) and open
   `classify_link` with:

   ```rust
   if link_rel.trim().is_empty() {
       return LinkVerdict::Denied(DenyReason::EmptyLink);
   }
   ```

   **Why `Denied`, not `Pending`:** a `Pending` verdict would make any
   future `approve_into`-like caller PERSIST `TrustedLink { link: "" }` —
   exactly the outcome the issue warns about. `Denied` is the only verdict
   that both refuses persistence and refuses approval. Denials outrank
   ledger entries by construction, so this cannot be un-done by a stale
   ledger row.

   **Why not rely on layer 1 alone:** `classify_link` is reachable without
   `approve_into` (`walk_vault.rs:223`, test suites, any future caller).
   The guard costs one `trim().is_empty()` and makes the public contract
   self-enforcing.

### Test churn note (intentional)

`src-tauri/tests/trusted_links.rs:272` currently asserts
`is_vault_relative_link("") == true`. This PR flips that assertion to
`false` — the flip IS the fix, and the test comment should say so (referencing
issue `#143`). No other existing test feeds an empty link (`walk_vault` only
produces real path strings).

## 3. Testing

In `src-tauri/tests/trusted_links.rs`:

- `vault_relative_predicate_rejects_empty_and_whitespace` — `""`, `"   "`,
  `"\t\n"` → false.
- `approve_into_refuses_empty_link_before_any_join` — empty link on a real
  temp vault errors with "not vault-relative"; ledger byte-identical
  before/after (mirrors the absolute-link test from #144).
- `classify_link_denies_empty_link` —
  `classify_link("", vault_root, vault_root, None, &[])` is
  `Denied(EmptyLink)` — proves the belt-and-suspenders layer bites even
  when the predicate is bypassed.
- `empty_link_deny_reason_message` — `DenyReason::EmptyLink.message()` is
  the expected string (keeps the `message()` match exhaustive).
- Updated line-272 assertion as described above.

**Sabotage check:** revert the predicate early-return → first two tests
fail; revert the `classify_link` guard → the third fails.

**Gate:** `cargo test --manifest-path src-tauri/Cargo.toml --test trusted_links`
plus the `walk_vault` and `config_leniency` suites (the shared predicate
change touches their inputs; must stay green).

## 4. Out of scope

- **#140** — load-boundary validation (separate PR; NOTE: #140 makes the
  load path call this same predicate, so after both land, an empty link in
  a stored ledger is also dropped at load — correct composition, no
  coordination needed).
- **#146** — lock diagnostics; unrelated.
- Rethinking the `Trusted`-on-vault-root classification for NON-empty
  links; unrelated behavior, untouched.
