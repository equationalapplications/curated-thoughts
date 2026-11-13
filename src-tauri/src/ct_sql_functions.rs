//! Scalar SQL functions backing the working-memory age-out filter
//! (spec 2026-11-13-working-memory-age).
//!
//! `ct_folder_age_days(absolute_path)` returns the configured age cutoff
//! in days for the folder containing `path`, or NULL when the folder
//! never ages. The ingest-policy snapshot is captured ONCE per
//! registration (per [`register_ct_functions`] call); per-row evaluation
//! touches only the in-memory snapshot — NEVER `fs::canonicalize`, NEVER
//! `fs::read` per row. Config edits take effect on the NEXT registration
//! (`ingest_policy_for_db` re-reads and refreshes its cache on byte
//! change), which is why consumers re-register before every query.

use std::sync::Arc;

/// Register `ct_folder_age_days(absolute_path) -> INTEGER cutoff_days or NULL`.
///
/// Idempotent-by-replacement: calling again simply replaces the function,
/// so consumers may register before every query (the DESIGN — 43 open
/// sites make register-at-open incomplete).
pub fn register_ct_functions(conn: &rusqlite::Connection) {
    // The brain dir is derived from THIS connection's path — the same
    // convention every other policy consumer uses
    // (`ingest_policy_for_db(conn.path())`). An in-memory / pathless
    // connection gets the default policy (everything NULL = never ages).
    let policy = crate::config::ingest_policy_snapshot(conn.path());
    register_ct_functions_with_policy(conn, &policy);
}

/// Register the function against an EXPLICIT policy snapshot. Production
/// callers use [`register_ct_functions`]; this variant exists for tests
/// that must inject a policy an in-memory connection cannot carry.
pub fn register_ct_functions_with_policy(conn: &rusqlite::Connection, policy: &crate::config::IngestPolicy) {
    let policy: Arc<crate::config::IngestPolicy> = Arc::new(policy.clone());
    conn.create_scalar_function(
        "ct_folder_age_days",
        1,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8
            | rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC,
        move |ctx| {
            let raw: String = ctx.get(0)?;
            let Some(root) = policy.vault_root.as_deref() else {
                return Ok(None);
            };
            // Pure string strip_prefix against the captured root; a
            // placement miss => NULL ("never ages").
            let rel = match std::path::Path::new(&raw).strip_prefix(root) {
                Ok(r) => r.to_string_lossy().into_owned(),
                Err(_) => return Ok(None),
            };
            Ok(policy.folder_ages_lookup(&rel)) // deepest-prefix, larger-wins tie
        },
    )
    .expect("ct_folder_age_days registration must succeed");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{IngestConfig, IngestPolicy};
    use std::collections::HashMap;

    fn policy(records_age: Option<u64>) -> IngestPolicy {
        let mut ages = HashMap::new();
        if let Some(d) = records_age {
            ages.insert("records".to_string(), d);
        }
        IngestPolicy {
            tiers: IngestConfig {
                folder_ages: ages,
                ..Default::default()
            },
            vault_root: Some(std::path::PathBuf::from("/vault")),
            ..Default::default()
        }
    }

    #[test]
    fn ct_folder_age_days_resolves_registered_folders() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        register_ct_functions_with_policy(&conn, &policy(Some(90)));
        let ninety: Option<i64> = conn
            .query_row(
                "SELECT ct_folder_age_days('/vault/records/a.md')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(ninety, Some(90));
        let other: Option<i64> = conn
            .query_row(
                "SELECT ct_folder_age_days('/vault/other/a.md')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(other, None, "unaged folder must resolve to NULL");
    }

    #[test]
    fn ct_folder_age_days_placement_miss_is_null() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        register_ct_functions_with_policy(&conn, &policy(Some(90)));
        let outside: Option<i64> = conn
            .query_row(
                "SELECT ct_folder_age_days('/elsewhere/records/a.md')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(outside, None, "path outside the vault root never ages");
    }

    #[test]
    fn empty_folder_ages_config_is_null_everywhere() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        register_ct_functions_with_policy(&conn, &policy(None));
        let none: Option<i64> = conn
            .query_row(
                "SELECT ct_folder_age_days('/vault/records/a.md')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(none, None, "no config => NULL => the WHERE fragment's NULL arm short-circuits");
    }
}
