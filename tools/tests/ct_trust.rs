//! `ct trust` — approve, list, and revoke symlinks the ingest walker may follow.
//!
//! Mirrors the direct-TempDir + env-var harness used by `ct_status.rs` and
//! `ct_watch_exit_codes.rs` rather than introducing a `TestEnv` helper (one
//! of the things the plan defers until multiple test files actually need
//! it; we don't yet).

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::symlink;
use std::process::Command;
use tempfile::TempDir;

/// Build a config.json that points `vault_path` at `vault`, then return the
/// brain dir (where config.json + brain.db live) and the vault dir.
fn seed_env() -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = TempDir::new().unwrap();
    let brain = tmp.path().join("brain");
    let vault = tmp.path().join("vault");
    fs::create_dir_all(&brain).unwrap();
    fs::create_dir_all(&vault).unwrap();
    fs::write(
        brain.join("config.json"),
        format!(r#"{{"vault_path":"{}"}}"#, vault.display()),
    )
    .unwrap();
    (tmp, brain, vault)
}

fn run_ct(brain_dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ct"))
        .env("CURATED_BRAIN_DIR", brain_dir)
        .env_remove("CURATED_BRAIN_DB")
        .env_remove("CURATED_BRAIN_CONFIG")
        .args(args)
        .output()
        .expect("spawn ct")
}

#[test]
fn trust_approves_a_pending_link_and_persists_the_pair() {
    let (_tmp, brain, vault) = seed_env();
    let docs = vault.join("documents");
    fs::create_dir_all(&docs).unwrap();
    let outside = _tmp.path().join("repo-docs");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("spec.md"), "# Spec\n").unwrap();
    symlink(&outside, docs.join("specs")).unwrap();

    let out = run_ct(&brain, &["trust", "documents/specs"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let cfg: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(brain.join("config.json")).unwrap()).unwrap();
    let links = cfg["trusted_links"]
        .as_array()
        .expect("trusted_links array");
    assert_eq!(links.len(), 1);
    assert_eq!(links[0]["link"], "documents/specs");
    assert!(links[0]["target"].as_str().unwrap().ends_with("repo-docs"));
    assert!(links[0]["approved_at"].as_i64().unwrap() > 0);
}

#[test]
fn trust_refuses_a_non_approvable_target_and_names_the_rule() {
    let (_tmp, brain, vault) = seed_env();
    let docs = vault.join("documents");
    fs::create_dir_all(&docs).unwrap();
    symlink(_tmp.path(), docs.join("everything")).unwrap();

    let out = run_ct(&brain, &["trust", "documents/everything"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("vault"),
        "must name the rule, got: {stderr}"
    );
}

/// An absolute `<link>` must be refused before any join. `Path::join`
/// *replaces* the base when its argument is absolute, so without this guard
/// `ct trust /abs/path` escapes the vault entirely, classifies a path the
/// vault does not contain, and persists that absolute string into the ledger
/// as `TrustedLink::link` — a field every consumer documents as
/// vault-relative (CodeRabbit, PR #129; load-boundary gap is #140).
#[test]
fn trust_refuses_an_absolute_link_and_leaves_the_ledger_empty() {
    let (_tmp, brain, _vault) = seed_env();
    let outside = _tmp.path().join("repo-docs");
    fs::create_dir_all(&outside).unwrap();
    let abs_link = _tmp.path().join("abs-link");
    symlink(&outside, &abs_link).unwrap();
    assert!(abs_link.is_absolute());

    let out = run_ct(&brain, &["trust", abs_link.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("vault-relative"),
        "must refuse a non-vault-relative link, got: {stderr}"
    );

    let cfg: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(brain.join("config.json")).unwrap()).unwrap();
    let links = cfg["trusted_links"].as_array();
    assert!(
        links.is_none_or(|l| l.is_empty()),
        "an absolute link must never be persisted, got: {:?}",
        cfg["trusted_links"]
    );
}

#[test]
fn trust_on_an_unknown_link_exits_1() {
    let (_tmp, brain, vault) = seed_env();
    fs::create_dir_all(vault.join("documents")).unwrap();

    let out = run_ct(&brain, &["trust", "documents/nope"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not a symlink") || stderr.contains("no such"),
        "stderr: {stderr}"
    );
}

#[test]
fn trust_list_prints_the_ledger_and_revoke_removes_an_entry() {
    let (_tmp, brain, vault) = seed_env();
    let docs = vault.join("documents");
    fs::create_dir_all(&docs).unwrap();
    let outside = _tmp.path().join("repo-docs");
    fs::create_dir_all(&outside).unwrap();
    symlink(&outside, docs.join("specs")).unwrap();

    let approved = run_ct(&brain, &["trust", "documents/specs"]);
    assert_eq!(approved.status.code(), Some(0));

    let listed = run_ct(&brain, &["trust", "--list"]);
    assert_eq!(listed.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&listed.stdout).contains("documents/specs"));

    let revoked = run_ct(&brain, &["trust", "--revoke", "documents/specs"]);
    assert_eq!(revoked.status.code(), Some(0));

    let cfg: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(brain.join("config.json")).unwrap()).unwrap();
    assert!(cfg["trusted_links"].as_array().unwrap().is_empty());
}

/// `--list --revoke <link>` must not silently print the ledger and exit 0;
/// the user clearly meant to revoke and the wrong action is dangerous.
#[test]
fn trust_rejects_combining_list_and_revoke() {
    let (_tmp, brain, _vault) = seed_env();

    let out = run_ct(&brain, &["trust", "--list", "--revoke", "documents/specs"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("exactly one"),
        "must reject conflicting actions, got: {stderr}"
    );
}

#[test]
fn trust_rejects_combining_link_with_list() {
    let (_tmp, brain, _vault) = seed_env();

    let out = run_ct(&brain, &["trust", "documents/specs", "--list"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("exactly one"), "stderr: {stderr}");
}

#[test]
fn trust_with_no_action_exits_1() {
    let (_tmp, brain, _vault) = seed_env();

    let out = run_ct(&brain, &["trust"]);
    assert_eq!(out.status.code(), Some(1));
}

/// `ct trust --list` must redact the target's home prefix to `~` so
/// sensitive absolute paths (e.g. `/Users/me/.ssh`) are not logged verbatim.
/// Run the child process with a controlled `HOME` and seed one approved
/// symlink beneath that directory + one outside it, so the redaction branch
/// (`~/relative-target`) and the verbatim branch (outside-of-home) are both
/// exercised in a single test. CodeRabbit review on PR #124: the previous
/// version only seeded an outside-of-home target under `TempDir`, which
/// meant a regression that dropped the redaction would pass silently when
/// `TMPDIR` happened to be inside `$HOME`.
#[test]
fn trust_list_redacts_home_prefix_in_target() {
    use std::process::Command;

    let (tmp, brain, vault) = seed_env();
    let docs = vault.join("documents");
    fs::create_dir_all(&docs).unwrap();

    // Controlled HOME that is NOT the tmp dir, so the redaction branch and
    // the verbatim branch target two distinct parents. Canonicalize it —
    // `approve_into` stores the symlink's CANONICALIZED target in the
    // ledger (macOS resolves `/var` → `/private/var`), so `redact_home`'s
    // `dirs::home_dir()` comparison must see the same canonical form or
    // the prefixes never match.
    let controlled_home = tmp.path().join("home");
    fs::create_dir_all(&controlled_home).unwrap();
    let controlled_home = fs::canonicalize(&controlled_home).unwrap();
    let controlled_home_str = controlled_home.display().to_string();

    // In-home target → must render as `~/relative-target`.
    let in_home = controlled_home.join("repo-docs");
    fs::create_dir_all(&in_home).unwrap();
    symlink(&in_home, docs.join("in_home_specs")).unwrap();

    // Outside-home target (under tmp, NOT under controlled_home) → must render
    // verbatim so the user can see the real location when investigating.
    let outside = tmp.path().join("outside-docs");
    fs::create_dir_all(&outside).unwrap();
    symlink(&outside, docs.join("outside_specs")).unwrap();

    // Approve both so the ledger has two rows to render.
    let home_invocation = || -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_ct"))
            .env("CURATED_BRAIN_DIR", &brain)
            .env("HOME", &controlled_home_str)
            .env_remove("CURATED_BRAIN_DB")
            .env_remove("CURATED_BRAIN_CONFIG")
            .args(["trust", "documents/in_home_specs"])
            .output()
            .expect("spawn ct (in-home approve)")
    };
    assert_eq!(home_invocation().status.code(), Some(0));

    let outside_invocation = || -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_ct"))
            .env("CURATED_BRAIN_DIR", &brain)
            .env("HOME", &controlled_home_str)
            .env_remove("CURATED_BRAIN_DB")
            .env_remove("CURATED_BRAIN_CONFIG")
            .args(["trust", "documents/outside_specs"])
            .output()
            .expect("spawn ct (outside-home approve)")
    };
    assert_eq!(outside_invocation().status.code(), Some(0));

    // Now `trust --list` with the same controlled HOME.
    let listed = Command::new(env!("CARGO_BIN_EXE_ct"))
        .env("CURATED_BRAIN_DIR", &brain)
        .env("HOME", &controlled_home_str)
        .env_remove("CURATED_BRAIN_DB")
        .env_remove("CURATED_BRAIN_CONFIG")
        .args(["trust", "--list"])
        .output()
        .expect("spawn ct (--list)");

    assert_eq!(listed.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&listed.stdout);

    assert!(
        stdout.contains("documents/in_home_specs"),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains("documents/outside_specs"),
        "stdout: {stdout}"
    );

    // In-home target should render as `~/...`, never the absolute home
    // path. Accept any non-empty tail after the slash since
    // redact_home preserves the original separators.
    assert!(
        stdout.contains("~/repo-docs"),
        "in-home target must render as `~/repo-docs` (redacted), got stdout: {stdout}"
    );
    assert!(
        !stdout.contains(&controlled_home_str),
        "controlled HOME path must not appear verbatim in --list output: {stdout}"
    );

    // Outside-of-home target stays verbatim — that branch is the user's
    // escape hatch when the home redaction is unhelpful.
    assert!(
        stdout.contains(outside.display().to_string().as_str()),
        "outside-of-home target should appear verbatim, got stdout: {stdout}"
    );
}

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
