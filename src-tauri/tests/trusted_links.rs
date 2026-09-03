use std::path::{Path, PathBuf};
use tauri_app_lib::trusted_links::{
    approve_into, classify_link, is_vault_relative_link, is_within, DenyReason, LinkVerdict,
    TrustedLink,
};

fn ledger(link: &str, target: &str) -> Vec<TrustedLink> {
    vec![TrustedLink {
        link: link.to_string(),
        target: target.to_string(),
        approved_at: 1_756_512_000,
    }]
}

const VAULT: &str = "/home/me/vault";
const HOME: &str = "/home/me";

#[test]
fn exact_pair_match_is_trusted() {
    let l = ledger("documents/specs", "/home/me/code/proj/docs");
    let verdict = classify_link(
        "documents/specs",
        Path::new("/home/me/code/proj/docs"),
        Path::new(VAULT),
        Some(Path::new(HOME)),
        &l,
    );
    assert_eq!(verdict, LinkVerdict::Trusted);
}

#[test]
fn a_repointed_target_is_pending_not_trusted() {
    let l = ledger("documents/specs", "/home/me/code/proj/docs");
    let verdict = classify_link(
        "documents/specs",
        Path::new("/home/me/code/other/docs"), // repointed
        Path::new(VAULT),
        Some(Path::new(HOME)),
        &l,
    );
    assert_eq!(verdict, LinkVerdict::Pending);
}

#[test]
fn an_unknown_link_is_pending() {
    let verdict = classify_link(
        "documents/new",
        Path::new("/home/me/code/proj/docs"),
        Path::new(VAULT),
        Some(Path::new(HOME)),
        &[],
    );
    assert_eq!(verdict, LinkVerdict::Pending);
}

#[test]
fn filesystem_root_and_home_are_never_approvable() {
    for (target, reason) in [
        ("/", DenyReason::FilesystemRoot),
        (HOME, DenyReason::HomeDirectory),
    ] {
        let l = ledger("documents/x", target);
        let verdict = classify_link(
            "documents/x",
            Path::new(target),
            Path::new(VAULT),
            Some(Path::new(HOME)),
            &l,
        );
        assert_eq!(
            verdict,
            LinkVerdict::Denied(reason),
            "{target} must be denied even when it is in the ledger"
        );
    }
}

#[test]
fn a_target_containing_the_vault_is_denied() {
    let l = ledger("documents/x", "/home/me");
    // /home/me contains the vault; also matches HomeDirectory, so use a
    // non-home ancestor to isolate the ContainsVault rule.
    let verdict = classify_link(
        "documents/x",
        Path::new("/home/me/vault/.."),
        Path::new(VAULT),
        None,
        &l,
    );
    // `vault_root.starts_with(&target)` is always true whenever
    // `is_within(&vault_root, &target)` holds — the `VaultAncestor` framing
    // is unreachable from `classify_link` (Copilot review on PR #124), so
    // this pins the deterministic `ContainsVault` reason rather than the
    // looser `Denied(_)` match this test used before the fix.
    assert_eq!(verdict, LinkVerdict::Denied(DenyReason::ContainsVault));
}

#[test]
fn a_target_that_is_an_ancestor_of_an_already_trusted_target_is_denied() {
    let mut l = ledger("documents/specs", "/home/me/code/proj/docs");
    l.push(TrustedLink {
        link: "documents/wide".to_string(),
        target: "/home/me/code".to_string(),
        approved_at: 1,
    });
    let verdict = classify_link(
        "documents/wide",
        Path::new("/home/me/code"),
        Path::new(VAULT),
        Some(Path::new(HOME)),
        &l,
    );
    assert_eq!(verdict, LinkVerdict::Denied(DenyReason::AncestorOfTrusted));
}

#[test]
fn containment_is_component_wise_not_string_prefix() {
    assert!(is_within(
        Path::new("/home/me/code/proj/docs"),
        Path::new("/home/me/code/proj")
    ));
    assert!(
        !is_within(
            Path::new("/home/me/code/proj-secrets"),
            Path::new("/home/me/code/proj")
        ),
        "proj must not authorize proj-secrets"
    );
}

#[test]
fn a_target_inside_the_vault_needs_no_ledger_entry() {
    let inside: PathBuf = Path::new(VAULT).join("documents/real");
    let verdict = classify_link(
        "documents/real",
        &inside,
        Path::new(VAULT),
        Some(Path::new(HOME)),
        &[],
    );
    assert_eq!(verdict, LinkVerdict::Trusted);
}

/// Belt-and-suspenders (issue #143): `classify_link` is a public API also
/// called from `walk_vault.rs` with links read straight from the filesystem,
/// so it must be fail-closed on an empty link independently of the
/// `approve_into` predicate. Denial must outrank the in-vault auto-trust
/// branch — pre-#143 this returned Trusted because `target == vault_root`
/// skipped the ContainsVault check and `is_within(vault, vault)` held.
#[test]
fn classify_link_denies_empty_link() {
    let verdict = classify_link("", Path::new(VAULT), Path::new(VAULT), None, &[]);
    assert_eq!(
        verdict,
        LinkVerdict::Denied(DenyReason::EmptyLink),
        "an empty link must be Denied(EmptyLink) when the predicate is bypassed, got {:?}",
        verdict
    );
    let ws = classify_link("\t  ", Path::new(VAULT), Path::new(VAULT), None, &[]);
    assert_eq!(
        ws,
        LinkVerdict::Denied(DenyReason::EmptyLink),
        "a whitespace-only link must also be Denied(EmptyLink), got {:?}",
        ws
    );
}

/// Keeps the `message()` match exhaustive and pins the user-facing text.
#[test]
fn empty_link_deny_reason_message() {
    assert_eq!(DenyReason::EmptyLink.message(), "link is empty");
}

/// `approve_link` must canonicalize the vault root before classification,
/// otherwise a vault reached through a symlink lets a link to its physical
/// parent pass as Pending. With a non-canonical vault root (`/var/...` vs the
/// canonical `/private/var/...` on macOS), the canonical target has no
/// matching prefix against the non-canonical vault, so the containment
/// check fails and the link returns Pending. Canonicalizing the vault root
/// aligns the prefixes and produces Denied(ContainsVault).
#[test]
fn non_canonical_vault_root_must_be_canonicalized_before_classification() {
    // Real macOS TempDir resolves through /var → /private/var, so a vault
    // reached through /var/folders/... has the canonical form
    // /private/var/folders/.../vault. The vault's physical parent
    // canonicalizes to /private/var/folders/.... With the vault root
    // canonicalized, the component-wise containment check sees the matching
    // /private/var prefix and refuses the link; without that, the prefixes
    // diverge and the link falls through to Pending.
    let canonical_target = "/private/var/folders/abc/T";
    let canonical_vault = "/private/var/folders/abc/T/vault";
    let non_canonical_vault = "/var/folders/abc/T/vault";

    let verdict_with_canonical_root = classify_link(
        "documents/parent",
        Path::new(canonical_target),
        Path::new(canonical_vault),
        Some(Path::new(HOME)),
        &[],
    );
    // With the canonical root, the parent's prefix matches and
    // `classify_link` always reports the deterministic `ContainsVault`
    // reason for a target above the vault root (`VaultAncestor` is
    // unreachable from this function — Copilot review on PR #124).
    assert_eq!(
        verdict_with_canonical_root,
        LinkVerdict::Denied(DenyReason::ContainsVault),
        "with canonical vault root, a link to the physical parent must be refused; got {:?}",
        verdict_with_canonical_root
    );

    let verdict_with_non_canonical_root = classify_link(
        "documents/parent",
        Path::new(canonical_target),
        Path::new(non_canonical_vault),
        Some(Path::new(HOME)),
        &[],
    );
    assert_eq!(
        verdict_with_non_canonical_root,
        LinkVerdict::Pending,
        "without canonicalization, the non-matching prefix leaves the link Pending"
    );
}

/// A symlinked `$HOME` must still be denied as `HomeDirectory`. `target` is
/// canonicalized by the caller before `classify_link` runs; `home` must be
/// compared on the same footing (canonicalized, symlinks resolved) or a
/// symlinked home directory's *real* path — which is what an approved
/// symlink's canonicalized target would resolve to — slips past the
/// `home == target` check entirely (CodeRabbit review on PR #124).
#[test]
fn a_symlinked_home_directory_is_still_denied() {
    let tmp = tempfile::TempDir::new().unwrap();
    let real_home = tmp.path().join("real-home");
    std::fs::create_dir_all(&real_home).unwrap();
    let home_symlink = tmp.path().join("home-symlink");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&real_home, &home_symlink).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&real_home, &home_symlink).unwrap();

    let vault = tmp.path().join("vault");
    std::fs::create_dir_all(&vault).unwrap();

    // A target that canonicalizes to the REAL home path (as an approved
    // symlink's target would, since callers canonicalize before calling
    // classify_link) must be denied even though `home` is passed as the
    // symlink path, not the resolved one.
    let canonical_real_home = std::fs::canonicalize(&real_home).unwrap();
    let verdict = classify_link(
        "documents/whoops",
        &canonical_real_home,
        &vault,
        Some(&home_symlink),
        &[],
    );
    assert_eq!(
        verdict,
        LinkVerdict::Denied(DenyReason::HomeDirectory),
        "a target resolving to the real (symlinked) home directory must be denied, got {:?}",
        verdict
    );
}

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

/// ParentDir (`..`) components must be refused wherever they appear
/// (CodeRabbit, PR #144): `join` preserves them, so the joined path
/// canonicalizes OUTSIDE the vault and a Pending verdict would persist a
/// traversal string as `TrustedLink::link`. Leading and interior `..` are
/// both traversals.
#[test]
fn vault_relative_predicate_rejects_parent_dir_traversals() {
    assert!(!is_vault_relative_link("../outside-link"));
    assert!(!is_vault_relative_link("a/../../outside-link"));
    assert!(!is_vault_relative_link("documents/../secrets"));
}

/// A plain vault-relative link is accepted by the predicate. The empty link
/// is NOT: as of issue #143 the predicate refuses empty and whitespace-only
/// input up front (the pre-#143 behavior let `""` through because an empty
/// string has no offending components; the flip to `false` IS the fix).
#[test]
fn vault_relative_predicate_accepts_relative_links() {
    assert!(is_vault_relative_link("documents/specs"));
    assert!(is_vault_relative_link("a"));
    assert!(!is_vault_relative_link(""));
}

/// Empty and whitespace-only strings must be refused by the predicate
/// (issue #143): `vault_root.join("")` yields the vault root and
/// `Path::new(" ")` yields a `Normal(" ")` component, so without this guard
/// nonsense input flows into canonicalize/classify.
#[test]
fn vault_relative_predicate_rejects_empty_and_whitespace() {
    assert!(!is_vault_relative_link(""));
    assert!(!is_vault_relative_link("   "));
    assert!(!is_vault_relative_link("\t\n"));
    assert!(!is_vault_relative_link("\t  "));
}

/// The guard must fire BEFORE any filesystem access: an empty link is
/// rejected with the vault-relative error even though
/// `vault_root.join("")` canonicalizes fine, and the ledger is left
/// untouched (no Pending path ran, so nothing could have been appended).
#[test]
fn approve_into_refuses_empty_link_before_any_join() {
    let tmp = tempfile::TempDir::new().unwrap();
    let vault = tmp.path().join("vault");
    std::fs::create_dir_all(&vault).unwrap();

    let mut l = ledger("documents/specs", "/home/me/code/proj/docs");
    let before = l.clone();

    let err = approve_into(&mut l, "", &vault, Some(Path::new(HOME)))
        .expect_err("an empty link must be an error, not a verdict");
    assert!(
        err.contains("not vault-relative"),
        "error must name the vault-relative rule, got: {err}"
    );
    assert_eq!(l, before, "the ledger must be untouched by a refused link");
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
