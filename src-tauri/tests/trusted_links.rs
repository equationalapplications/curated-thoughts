use std::path::{Path, PathBuf};
use tauri_app_lib::trusted_links::{
    classify_link, is_within, DenyReason, LinkVerdict, TrustedLink,
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
    assert!(matches!(verdict, LinkVerdict::Denied(_)));
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

/// `approve_link` must canonicalize the vault root before classification,
/// otherwise a vault reached through a symlink lets a link to its physical
/// parent pass as Pending. With a non-canonical vault root (`/var/...` vs the
/// canonical `/private/var/...` on macOS), the canonical target has no
/// matching prefix against the non-canonical vault, so the VaultAncestor
/// check fails and the link returns Pending. Canonicalizing the vault root
/// aligns the prefixes and produces Denied(VaultAncestor).
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
    // With the canonical root, the parent's prefix matches and the link
    // is refused as either an ancestor of the vault (target sits above
    // vault_root) or as containing the vault (vault_root lives inside the
    // target). Both denials are valid — the assertion is that the link is
    // refused, not the specific framing.
    assert!(
        matches!(
            verdict_with_canonical_root,
            LinkVerdict::Denied(DenyReason::VaultAncestor)
                | LinkVerdict::Denied(DenyReason::ContainsVault)
        ),
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
