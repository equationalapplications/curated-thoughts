# Absolute-Link Guard in `approve_into` — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close issue #142 — refuse non-vault-relative `link` arguments inside the shared `approve_into` helper (both entry points inherit the guard), make the CLI use that same shared predicate (one definition, not two), and keep every CLI echo of a link redacted.

**Architecture:** A new public predicate `is_vault_relative_link` in `trusted_links.rs` becomes the single definition of "vault-relative"; `approve_into` fails closed with an error before any join when the predicate rejects. The CLI's pre-join guard is converted to call the same predicate (keeping its friendlier message), and the helper-error print site routes through `redact_home` as defense-in-depth (reachable only if the CLI guard is ever removed — spec §1.1 point 3 as amended).

**Tech Stack:** Rust (src-tauri lib crate + tools CLI crate; tools already depends on `tauri_app_lib`), `std::path`, integration tests with `tempfile`.

**Spec:** `docs/superpowers/specs/2026-09-03-approve-into-absolute-link-guard-design.md`

## Global Constraints

- Guard must fire BEFORE `vault_root.join(link)` — no filesystem access on a refused link (spec §2).
- Predicate = first `Path` component is `Prefix(_)` or `RootDir` → NOT vault-relative. Exactly one definition: `is_vault_relative_link` in `src-tauri/src/trusted_links.rs`; the CLI MUST call it, not keep its inline `matches!` copy (spec §2; GLM 5.3 plan-review Important 1).
- No behavior change for relative links: empty string and normal relative paths still reach canonicalize (spec §3).
- Only files touched: `src-tauri/src/trusted_links.rs`, `src-tauri/tests/trusted_links.rs`, `tools/src/bin/ct.rs`, `tools/tests/ct_trust.rs`.
- Do NOT touch issue #140 (load-boundary validation) or #125 (TOCTOU) — out of scope (spec §5).
- Build flags: never bare `cargo test` for the lib test profile; the integration test target used here compiles without feature gates. Format with `cargo fmt --manifest-path <crate>/Cargo.toml` (there is no workspace Cargo.toml at the repo root; no package named `tauri-app` exists).
- Honest coverage note (plan-review Important 2): the helper's `Err` arm cannot receive an absolute link through the CLI (the CLI guard fires first), so its `redact_home` change is verified by compile + code review only; the behaviorally-tested redaction path is the CLI guard message, via a controlled-HOME test modeled on `trust_list_redacts_home_prefix_in_target`.

---

### Task 1: Guard predicate + `approve_into` fail-closed check (src-tauri)

**Files:**
- Modify: `src-tauri/src/trusted_links.rs` (insert predicate near `is_within`; guard at top of `approve_into` body, ~line 106)
- Test: `src-tauri/tests/trusted_links.rs`

**Interfaces:**
- Consumes: existing `approve_into(ledger: &mut Vec<TrustedLink>, link: &str, vault_root: &Path, home: Option<&Path>) -> Result<LinkVerdict, String>` (unchanged signature).
- Produces: `pub fn is_vault_relative_link(link: &str) -> bool`, exported from `tauri_app_lib::trusted_links`. Task 2 imports this exact name in `tools/src/bin/ct.rs`.

- [ ] **Step 1: Write the failing tests**

Append to `src-tauri/tests/trusted_links.rs` (extend the existing `use tauri_app_lib::trusted_links::{...}` with `approve_into` and `is_vault_relative_link`):

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
Expected: FAIL — `is_vault_relative_link` not found in `tauri_app_lib::trusted_links` (E0432).

- [ ] **Step 3: Implement the predicate and the guard**

In `src-tauri/src/trusted_links.rs`, after the existing `is_within` function, add (keep the file's existing imports — `Path` and `Component` are already in scope, verified at line 9):

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
cargo fmt --manifest-path src-tauri/Cargo.toml
git add src-tauri/src/trusted_links.rs src-tauri/tests/trusted_links.rs
git commit -m "fix(vault): refuse non-vault-relative links in approve_into (issue #142)"
```

### Task 2: CLI uses the shared predicate; redact the helper-error echo (tools crate)

**Files:**
- Modify: `tools/src/bin/ct.rs` (inline guard ~line 669-682 → call `is_vault_relative_link`; `Err(e)` arm ~line 734 → `redact_home`)
- Test: `tools/tests/ct_trust.rs` (new controlled-HOME redaction test)

**Interfaces:**
- Consumes: `is_vault_relative_link(&str) -> bool` and `approve_into`'s new error path from Task 1. `tauri_app_lib::trusted_links` is already importable from the tools crate (ct.rs line 594 already imports from it in its test module; `tools/Cargo.toml` depends on `tauri_app_lib`).
- Produces: no API change.

- [ ] **Step 1: Write the failing redaction test FIRST (new test, modeled on `trust_list_redacts_home_prefix_in_target` at tools/tests/ct_trust.rs:200)**

Add to `tools/tests/ct_trust.rs`:

```rust
/// An absolute link UNDER a controlled $HOME must be refused by the vault-
/// relative guard with the home prefix REDACTED (`~/...`), never the
/// absolute path. Modeled on trust_list_redacts_home_prefix_in_target:
/// HOME is controlled and canonicalized so `redact_home`'s component
/// comparison sees the same canonical form the guard's message embeds.
/// This is the behaviorally-reachable redaction path — the helper's Err arm
/// cannot receive an absolute link through the CLI (this guard fires first),
/// so the in-helper redaction added alongside is compile-verified only.
#[test]
fn trust_redacts_home_prefix_when_refusing_absolute_link() {
    use std::process::Command;

    let (tmp, brain, _vault) = seed_env();

    let controlled_home = tmp.path().join("home");
    fs::create_dir_all(&controlled_home).unwrap();
    let controlled_home = fs::canonicalize(&controlled_home).unwrap();
    let controlled_home_str = controlled_home.display().to_string();

    let absolute_link = controlled_home.join("repo-docs"); // under HOME, absolute

    let out = Command::new(env!("CARGO_BIN_EXE_ct"))
        .env("CURATED_BRAIN_DIR", &brain)
        .env("HOME", &controlled_home_str)
        .env_remove("CURATED_BRAIN_DB")
        .env_remove("CURATED_BRAIN_CONFIG")
        .args(["trust", absolute_link.to_str().unwrap()])
        .output()
        .expect("spawn ct (absolute in-home link)");

    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("vault-relative"),
        "must name the rule, got: {stderr}"
    );
    assert!(
        !stderr.contains(&controlled_home_str),
        "stderr must not contain the absolute home prefix, got: {stderr}"
    );
    assert!(
        stderr.contains("~"),
        "expected the redacted ~/... form, got: {stderr}"
    );
}
```

Run: `cargo test --manifest-path tools/Cargo.toml --test ct_trust 2>&1 | tail -5`
Expected: this test PASSES already? No — verify explicitly: it passes iff the guard message already routes through `redact_home` (it does since PR #129: `error: link must be vault-relative, got {}` uses `redact_home`). Record the observed result; the test is regression coverage. If it FAILS, stop and report — the plan's reachability analysis is wrong.

- [ ] **Step 2: Convert the CLI guard to the shared predicate (one definition)**

In `tools/src/bin/ct.rs`, add `is_vault_relative_link` to the existing `use tauri_app_lib::trusted_links::{approve_into, LinkVerdict};` import (line ~594, test module) — and to the main source, add the same import at the top of the file's trust command region if not already imported (check what line 594's module structure requires; the predicate call site is `cmd_trust`'s body).

Replace the inline predicate (~line 669-682):

```rust
    let first_component = std::path::Path::new(&link).components().next();
    if matches!(
        first_component,
        Some(std::path::Component::Prefix(_) | std::path::Component::RootDir)
    ) {
        eprintln!(
            "error: link must be vault-relative, got {}",
            redact_home(&link)
        );
        return Ok(1);
    }
```

with:

```rust
    // Same definition as approve_into's own guard (issue #142): one
    // predicate, shared. The CLI keeps its friendlier message.
    if !is_vault_relative_link(&link) {
        eprintln!(
            "error: link must be vault-relative, got {}",
            redact_home(&link)
        );
        return Ok(1);
    }
```

(Update the surrounding long comment to note the predicate now lives in `tauri_app_lib::trusted_links::is_vault_relative_link`; keep the join-hazard explanation.)

- [ ] **Step 3: Redact the helper-error echo**

In `tools/src/bin/ct.rs`, change the `Err(e)` arm of the trust `match` (~line 734):

```rust
        Err(e) => {
            eprintln!("error: {}", redact_home(&e));
            Ok(1)
        }
```

(`redact_home` is already in scope in that function — the other arms use it. Defense-in-depth: reachable only if the CLI guard above is ever removed.)

- [ ] **Step 4: Compile and run the tools trust tests**

Run: `cargo check --manifest-path tools/Cargo.toml 2>&1 | tail -3`
Expected: success.
Run: `cargo test --manifest-path tools/Cargo.toml --test ct_trust 2>&1 | tail -8`
Expected: PASS — including `trust_refuses_an_absolute_link_and_leaves_the_ledger_empty` (CLI guard still fires first via the shared predicate now) and the new `trust_redacts_home_prefix_when_refusing_absolute_link`.

- [ ] **Step 5: Commit**

```bash
git add tools/src/bin/ct.rs tools/tests/ct_trust.rs
git commit -m "fix(tools): ct trust uses shared vault-relative predicate; redact helper errors (issue #142)"
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

## Plan review history

- GLM 5.3 plan review #1 (session 20260903_010633_0a2631): Changes requested — I1 (CLI must call the shared predicate) → Task 2 Step 2 added; I2 (Step 1b test vacuous/uncompilable) → replaced with controlled-HOME test modeled on `trust_list_redacts_home_prefix_in_target`, honest reachability note added to Global Constraints; M1 (`cargo fmt -p tauri-app` nonexistent) → `--manifest-path` form; M2 (ct_trust.rs missing from `git add`) → fixed in Task 2 Step 5.
