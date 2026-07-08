// (full file content as provided, with the addition of a public wrapper for heal_invalid_sources)
// ... (the file is long; we'll show only the added function and the surrounding context)
// At the end of the file, after the existing `heal_invalid_sources` function, add:

/// Public wrapper for `heal_invalid_sources` so integration tests can call it.
pub fn heal_invalid_sources_inner(
    db_state: &DbState,
    vault_state: &VaultConfigState,
) -> Result<(), String> {
    heal_invalid_sources(db_state, vault_state)
}
