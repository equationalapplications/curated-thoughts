//! The virtual-path contract: a symlinked file keeps its vault-relative
//! identity in the DB even though its bytes come from outside the vault.

use tauri_app_lib::entity_id_for_virtual_path;

#[test]
fn virtual_path_under_documents_routes_to_tier_fact_without_canonicalizing() {
    // This path does not exist on disk. entity_id_for_path would canonicalize,
    // fail, and fall back to the raw string; the virtual variant must never
    // touch the filesystem at all.
    let id = entity_id_for_virtual_path("/vault/documents/specs/design.md", Some("/vault"));
    assert_eq!(id, "tier_fact");
}

#[test]
fn virtual_path_outside_documents_routes_to_workspace_tier() {
    let id = entity_id_for_virtual_path("/vault/scratch/notes.md", Some("/vault"));
    assert!(id.starts_with("tier_working::"), "got {id}");
}

#[test]
fn a_target_path_outside_the_vault_would_misroute_and_is_not_what_we_store() {
    // Guard against regressing to canonical-target storage: the target path
    // must NOT resolve to tier_fact, which is exactly why we keep the prefix.
    let id = entity_id_for_virtual_path("/Users/me/code/foo/docs/design.md", Some("/vault"));
    assert!(id.starts_with("tier_working::"), "got {id}");
}
