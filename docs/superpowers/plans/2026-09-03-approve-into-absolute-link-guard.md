# Absolute-Link Guard in `approve_into` — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close issue #142 — refuse non-vault-relative `link` arguments inside the shared `approve_into` helper (both entry points inherit the guard) and redact the one CLI error echo that can carry an absolute path.

**Architecture:** A new public predicate `is_vault_relative_link` in `trusted_links.rs` becomes the single definition of "vault-relative"; `approve_into` fails closed with an error before any join when the predicate rejects. The CLI keeps its earlier, friendlier diagnostic (defense-in-depth) and redacts helper errors via `redact_home`.

**Tech Stack:** Rust (src-tauri lib crate + tools CLI crate), `std::path`, integration tests with `tempfile`.

**Spec:** `docs/superpowers/specs/2026-09-03-approve-into-absolute-link-guard-design.md`

## Global Constraints

- Guard must fire BEFORE `vault_root.join(link)` — no filesystem access on a refused link (spec §2).
- Predicate = first `Path` component is `Prefix(_)` or `RootDir` → NOT vault-relative. Exactly one definition, shared by helper and CLI (spec §2.1).
- No behavior change for relative links: empty string and normal relative paths still reach canonicalize (spec §3).
- Only files touched: `src-tauri/src/trusted_links.rs`, `src-tauri/tests/trusted_links.rs`, `tools/src/bin/ct.rs` (one line).
- Do NOT touch issue #140 (load-boundary validation) or #125 (TOCTOU) — out of scope (spec §5).
- Build flags: never bare `cargo test` for the lib test profile; the integration test target used here compiles without feature gates.

---

### Task 1: Guard predicate + `approve_into` fail-closed check (src-tauri)

**Files:**
- Modify: `src-tauri/src/trusted_links.rs` (insert predicate after `is_within`, guard at top of `approve_into` ~line 106)
- Test: `src-tauri/tests/trusted_links.rs`

**Interfaces:**
- Consumes: existing `approve_into(ledger: &mut Vec<TrustedLink>, link: &str, vault_root: &Path, home: Option<&Path>) -> Result<LinkVerdict, String>` (unchanged signature).
- Produces: `pub fn is_vault_relative_link(link: &str) -> bool` — exported from `tauri_app_lib::trusted_links`; later tasks and the existing CLI comment reference it.

- [ ] **Step 1: Write the failing tests**

Append to `src-tauri/tests/trusted_links.rs` (keep the existing imports; ADD `approve_into` and `is_vault_relative_link` to the `use tauri_app_lib::trusted_links::{...}` list):

```rust
// ---------------------------------------------------------------------------
// Issue #142: the absolute-link guard must live in the SHARED helper, not
// only in the CLI. `Path::join` replaces the base when its argument is
// absolute, so an unvalidated `link` escapes the vault before classification.
// ---------------------------------------------------------------------------

/// A `link` whose first component is RootDir (absolute) must be refused by
/// the predicate — on every platform.
#[test]
fn vault_relative_predicate_rejects_absolute_links() {
    assert!(!is_vault_relative_link("/etc/passwd"));
    assert!(!is_vault_relative_link("/"));
}

/// A plain vault-relative link is accepted by the predicate, including the
/// empty link (rejected later by canonicalize, not by the prefix guard).
#[test]
fn vault_relative_predicate_accepts_relative_links() {
    assert!(is_vault_relative_link("documents/specs"));
    assert!(is_vault_relative_link("a"));
    assert!(is_vault_relative_link(""));
}

/// Windows drive prefixes and rooted paths must also be refused. `Path`
/// parsing of these forms is Windows-only, so the assertions only compile
/// there; on Unix the same strings are just odd relative names.
#[cfg(windows)]
#[test]
fn vault_relative_predicate_rejects_windows_prefixes_and_roots() {
    assert!(!is_vault_relative_link("C:foo"));
    assert!(!is_vault_relative_link("C:\\foo"));
    assert!(!is_vault_relative_link("\\foo"));
}

/// The guard must fire BEFORE any filesystem access: an absolute link is
/// rejected with the vault-relative error even when nothing exists at the
/// joined path, and the ledger is left untouched (no Pending path ran, so
/// nothing could have been appended).
#[test]
fn approve_into_refuses_absolute_link_before_any_join() {
    let tmp = tempfile::TempDir::new().unwrap();
    let vault = tmp.path().join("vault");
    std::fs::create_dir_all(&vault).unwrap();

    let mut l = ledger("documents/specs", "/home/me/code/proj/docs");
    let before = l.clone();

    let err = approve_into(&mut l, "/etc/passwd", &vault, Some(Path::new(HOME)))
        .expect_err("an absolute link must be an error, not a verdict");
    assert!(
        err.contains("not vault-relative"),
        "error must name the vault-relative rule, got: {err}"
    );
    assert_eq!(l, before, "the ledger must be untouched by a refused link");
}

/// The guard is fail-closed on nonexistent relative links too: a RELATIVE
/// link that doesn't exist still produces the resolution error (not the
/// vault-relative one), proving the guard only rewrites the absolute case
/// and did not change relative-link behavior.
#[test]
fn approve_into_still_reports_resolution_errors_for_relative_links() {
    let tmp = tempfile::TempDir::new().unwrap();
    let vault = tmp.path().join("vault");
    std::fs::create_dir_all(&vault).unwrap();

    let mut l = Vec::new();
    let err = approve_into(&mut l, "documents/missing", &vault, Some(Path::new(HOME)))
        .expect_err("a missing link must still fail to resolve");
    assert!(
        err.contains("could not be resolved"),
        "relative links keep the resolution error, got: {err}"
    );
    assert!(l.is_empty(), "nothing may be appended on failure");
}
```

- [ ] **Step 2: Run tests to verify they fail to compile**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test trusted_links 2>&1 | tail -5`
Expected: FAIL — `is_vault_relative_link` and `approve_into` not found in `tauri_app_lib::trusted_links` (E0432/E0425).

- [ ] **Step 3: Implement the predicate and the guard**

In `src-tauri/src/trusted_links.rs`, after the existing `is_within` function, add (keep the file's existing imports — `Path` and `Component` are already in scope; if `Component` is not, add it to the `std::path` use):

```rust
/// True when `link` can safely be joined onto `vault_root` as a
/// vault-relative path. `Path::join` **replaces** the base when its argument
/// is absolute — or, on Windows, merely carries a drive prefix (`C:foo`) or a
/// root (`\foo`) — so an unvalidated `link` would make both this function's
/// own `vault_root.join(link)` and the classification below operate on a path
/// outside the vault entirely (issue #142). Both entry points into
/// `approve_into` (the Tauri `approve_link` command and the CLI `ct trust`
/// subcommand) feed it raw user input, so the guard lives here, at the join,
/// not at one caller.
pub fn is_vault_relative_link(link: &str) -> bool {
    !matches!(
        Path::new(link).components().next(),
        Some(Component::Prefix(_) | Component::RootDir)
    )
}
```

At the top of `approve_into`'s body (before the `canonicalize`/`join` line), add the guard and extend the doc comment:

```rust
    if !is_vault_relative_link(link) {
        return Err(format!("{link} is not vault-relative"));
    }
```

Doc-comment addition on `approve_into` (after the existing "`vault_root` should be canonicalized by the caller." line):

```rust
///
/// Errors when `link` is not vault-relative. This must fire BEFORE the
/// canonicalize/join so an absolute `link` never touches the filesystem as a
/// joined path and never reaches the ledger (`TrustedLink::link` is
/// vault-relative by contract; see issue #140 for the load-boundary half of
/// that contract). Echos of `link` in the error string are the caller's to
/// redact.
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test trusted_links 2>&1 | tail -8`
Expected: PASS — all pre-existing tests plus the 4 new Unix tests (5th compiles only on Windows).

- [ ] **Step 5: Format and commit**

```bash
cargo fmt -p tauri-app
git add src-tauri/src/trusted_links.rs src-tauri/tests/trusted_links.rs
git commit -m "fix(vault): refuse non-vault-relative links in approve_into (issue #142)"
```

### Task 2: Redact the CLI helper-error echo (tools crate)

**Files:**
- Modify: `tools/src/bin/ct.rs` (the `Err(e)` arm of the `trust` match, ~line 734)

**Interfaces:**
- Consumes: `approve_into`'s new error path from Task 1 (its string may embed the raw `link`).
- Produces: no API change; one-line print fix.

- [ ] **Step 1: Make the edit**

In `tools/src/bin/ct.rs`, change:

```rust
        Err(e) => {
            eprintln!("error: {e}");
            Ok(1)
        }
```

to:

```rust
        Err(e) => {
            eprintln!("error: {}", redact_home(&e));
            Ok(1)
        }
```

(`redact_home` is already in scope in that function — the other arms use it.)

- [ ] **Step 1b: Assert the redaction in the existing test (GLM 5.3 spec-review M3)**

In `tools/tests/ct_trust.rs`, inside `trust_on_an_unknown_link_exits_1` (~line 120), extend the stderr assertion so the redaction at the print site is covered:

```rust
    // The resolution error embeds the raw link; the print site must route
    // through redact_home so no absolute $HOME path reaches stderr.
    assert!(
        !stderr.contains(&home.path().display().to_string()),
        "stderr must not leak the absolute home prefix, got: {stderr}"
    );
```

(Use the same string-variable names the test already binds for its output; if it does not currently capture stderr, bind it first: `let stderr = String::from_utf8_lossy(&out.stderr).to_string();`)

- [ ] **Step 2: Compile the tools crate**

Run: `cargo check --manifest-path tools/Cargo.toml 2>&1 | tail -3`
Expected: success, no warnings from the changed line.

- [ ] **Step 3: Run the existing CLI trust tests**

Run: `cargo test --manifest-path tools/Cargo.toml --test ct_trust 2>&1 | tail -8`
Expected: PASS — including `trust_refuses_an_absolute_link_and_leaves_the_ledger_empty` (now exercises the helper guard + redaction end-to-end on Unix; the CLI's own earlier guard still fires first, so stderr still contains "vault-relative").

- [ ] **Step 4: Commit**

```bash
git add tools/src/bin/ct.rs
git commit -m "fix(tools): redact home prefix in ct trust helper errors (issue #142)"
```

### Task 3: Full gate + PR

**Files:**
- No source changes; gate + push + PR open.

- [ ] **Step 1: Run the full src-tauri test suite the way CI does**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --features test-utils,mcp-server -- --test-threads=1 2>&1 | tail -6`
Expected: PASS. (Never bare `cargo test` for the lib profile — E0433 `tauri::test` without the feature, a flags trap, not a breakage.)

- [ ] **Step 2: Push and open the PR**

```bash
git push -u origin fix/approve-into-absolute-link-guard
gh pr create --repo equationalapplications/curated-thoughts \
  --title "fix(vault): absolute-link guard in approve_into (issue #142)" \
  --body-file <PR body — see spec §1–2; Fixes #142; links spec + plan>
```

- [ ] **Step 3: Hand off to the PR review loop**

Controller (Tessera) runs the risk-tiered dual review on the diff (security-class → full dual track), watches CI to green, adjudicates CodeRabbit findings. Merge policy: REGULAR merge commit, never squash.
