//! Startup guard: `PRAGMA table_info` on `llm_wiki_*` tables must match the pinned package schema.

use anyhow::{bail, Context, Result};
use rusqlite::Connection;
use std::collections::BTreeSet;

/// Pinned `package.json` dependency — keep in sync with `@equationalapplications/core-llm-wiki`.
pub const PINNED_CORE_LLM_WIKI_VERSION: &str = "9.18.0";

/// Expected column set for one `llm_wiki_*` table. Lifetime-generic so tests
/// can feed expectations loaded from a data fixture (`verify_schema_with`).
pub(crate) struct TableExpectation<'a> {
    pub(crate) name: &'a str,
    pub(crate) columns: &'a [&'a str],
}

const LLM_WIKI_TABLES: &[TableExpectation<'static>] = &[
    TableExpectation {
        name: "llm_wiki_entries",
        columns: &[
            "id",
            "entity_id",
            "title",
            "body",
            "tags",
            "confidence",
            "source_type",
            "source_hash",
            "source_ref",
            "created_at",
            "updated_at",
            "last_accessed_at",
            "access_count",
            "deleted_at",
            "embedding",
            "embedding_blob",
            "okf_type",
            "ontology_checked_at",
            "heal_checked_at",
            "lifecycle_status",
            "stale_after",
            "generated_by",
            "last_verified_at",
            "last_verified_by",
            "okf_sources",
            "okf_verified",
            "okf_usage_window",
            "embedding_failed_at",
            "embedding_failure_kind",
            "embedding_attempts",
            "valid_from",
            "valid_to",
            "superseded_by",
            "superseded_at",
            // core-llm-wiki 9.x engine migrations v16/v18/v19.
            "prior_valid_to",
            "source_pointer",
            "derived_events",
            "tier",
            // V27 (issue #265): per-row embed-scheme stamp, CT-local (not in
            // the upstream package schema).
            "embed_scheme",
            // V30 (PR #352 Task 1): area scope, CT-local; NULL = whole-repo.
            "area",
        ],
    },
    TableExpectation {
        name: "llm_wiki_tasks",
        columns: &[
            "id",
            "entity_id",
            "description",
            "status",
            "priority",
            "created_at",
            "updated_at",
            "resolved_at",
            "deleted_at",
            "okf_type",
            "lifecycle_status",
            "stale_after",
            "generated_by",
            "last_verified_at",
            "last_verified_by",
            "okf_sources",
            "okf_verified",
            "okf_usage_window",
        ],
    },
    TableExpectation {
        name: "llm_wiki_source_ref_index",
        columns: &[
            "id",
            "entity_id",
            "source_hash",
            "source_ref",
            "created_at",
            "deleted_at",
        ],
    },
    TableExpectation {
        name: "llm_wiki_events",
        columns: &[
            "id",
            "entity_id",
            "event_type",
            "summary",
            "related_entry_id",
            "created_at",
            "occurred_at",
            "forgotten_at",
        ],
    },
    TableExpectation {
        name: "llm_wiki_checkpoints",
        columns: &[
            "entity_id",
            "heal_checkpoint",
            "memory_checkpoint",
            "librarian_watermark_at",
            "librarian_watermark_id",
        ],
    },
    TableExpectation {
        name: "llm_wiki_meta",
        columns: &["key", "value"],
    },
    TableExpectation {
        name: "llm_wiki_outbox",
        columns: &[
            "id",
            "entity_id",
            "table_name",
            "record_id",
            "operation",
            "payload",
            "created_at",
        ],
    },
    TableExpectation {
        name: "llm_wiki_edges",
        columns: &[
            "id",
            "entity_id",
            "source_id",
            "target_id",
            "edge_type",
            "created_at",
        ],
    },
    TableExpectation {
        name: "llm_wiki_entity_manifests",
        columns: &["entity_id", "mode", "manifest_json", "updated_at"],
    },
    // core-llm-wiki 9.x source store (engine v17): engine-owned, no CT-local
    // columns allowed (the engine's v17 closure checks the exact column set).
    TableExpectation {
        name: "llm_wiki_source_documents",
        columns: &[
            "id",
            "entity_id",
            "source_ref",
            "source_hash",
            "text",
            "text_hash",
            "char_count",
            "created_at",
        ],
    },
    TableExpectation {
        name: "llm_wiki_source_passages",
        columns: &[
            "id",
            "entity_id",
            "document_id",
            "seq",
            "start_offset",
            "end_offset",
        ],
    },
    // core-llm-wiki 9.x vector-removal ledger (setupDatabase block 2).
    TableExpectation {
        name: "llm_wiki_vector_removals",
        columns: &[
            "seq",
            "entity_id",
            "fact_id",
            "enqueued_at",
            "attempts",
            "last_attempt_at",
            "confirmed_at",
        ],
    },
];

fn schema_version(conn: &Connection) -> Result<i64> {
    conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_version",
        [],
        |r| r.get(0),
    )
    .context("read schema_version")
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
        [table],
        |r| r.get(0),
    )?;
    Ok(count > 0)
}

fn actual_columns(conn: &Connection, table: &str) -> Result<Vec<String>> {
    let sql = format!("PRAGMA table_info({table})");
    let mut stmt = conn
        .prepare(&sql)
        .with_context(|| format!("PRAGMA table_info({table})"))?;
    let cols = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(cols)
}

fn verify_table(conn: &Connection, expect: &TableExpectation<'_>) -> Result<()> {
    if !table_exists(conn, expect.name)? {
        bail!(
            "llm_wiki schema mismatch: table `{}` is missing. \
             Rust expects core-llm-wiki@{PINNED_CORE_LLM_WIKI_VERSION} columns. \
             Additive schema migrations may already have run on this open; \
             upgrade Curated Thoughts or restore from backup.",
            expect.name
        );
    }

    let actual: BTreeSet<_> = actual_columns(conn, expect.name)?.into_iter().collect();
    let expected: BTreeSet<_> = expect.columns.iter().map(|c| (*c).to_string()).collect();

    if actual == expected {
        return Ok(());
    }

    let missing: Vec<_> = expected.difference(&actual).cloned().collect();
    let extra: Vec<_> = actual.difference(&expected).cloned().collect();

    let mut detail = format!("table `{}`", expect.name);
    if !missing.is_empty() {
        detail.push_str(&format!(" missing columns {missing:?}"));
    }
    if !extra.is_empty() {
        detail.push_str(&format!(" unexpected columns {extra:?}"));
    }

    bail!(
        "llm_wiki schema mismatch: {detail}. \
         Rust expects core-llm-wiki@{PINNED_CORE_LLM_WIKI_VERSION} schema \
         (see src-tauri/src/db/schema_guard.rs). \
         Additive schema migrations (e.g. V17, V24) may already have run on this open; \
         upgrade Curated Thoughts or restore from backup."
    );
}

/// Verify all `llm_wiki_*` table column sets after schema V7. No-op on older schema versions.
pub fn verify_llm_wiki_schema(conn: &Connection) -> Result<()> {
    verify_schema_with(conn, LLM_WIKI_TABLES)
}

/// Verify `expects` against the live schema after schema V7. No-op on older
/// schema versions. `verify_llm_wiki_schema` is this over `LLM_WIKI_TABLES`;
/// tests pass other expectation sets (e.g. the 8.0.2 fixture).
pub(crate) fn verify_schema_with(conn: &Connection, expects: &[TableExpectation<'_>]) -> Result<()> {
    if schema_version(conn)? < 7 {
        return Ok(());
    }
    for expect in expects {
        verify_table(conn, expect)?;
    }
    Ok(())
}

/// `TableExpectation`s for core-llm-wiki 8.0.2 (CT-local columns included),
/// loaded from the T0 fixture captured at base `78160f98`. Test-only; the
/// parsed data is leaked to obtain `'static` borrows.
#[cfg(test)]
pub(crate) fn core_llm_wiki_8_0_2_expectations() -> Vec<TableExpectation<'static>> {
    const JSON: &str = include_str!("testdata/core-llm-wiki-8.0.2.expectations.json");
    let parsed: serde_json::Value =
        serde_json::from_str(JSON).expect("8.0.2 expectations fixture is valid JSON");
    parsed
        .as_array()
        .expect("fixture root is an array")
        .iter()
        .map(|t| {
            let name: &'static str = Box::leak(
                t["name"]
                    .as_str()
                    .expect("table entry has a string name")
                    .to_owned()
                    .into_boxed_str(),
            );
            let columns: Vec<&'static str> = t["columns"]
                .as_array()
                .expect("table entry has a columns array")
                .iter()
                .map(|c| {
                    &*Box::leak(
                        c.as_str()
                            .expect("column is a string")
                            .to_owned()
                            .into_boxed_str(),
                    )
                })
                .collect();
            TableExpectation {
                name,
                columns: Box::leak(columns.into_boxed_slice()),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::open_in_memory;

    /// Table names created by `LLM_WIKI_PACKAGE_DDL`, in statement order. The
    /// bare-`;` splitter also cuts on `;` inside `--` comments, so a CREATE
    /// TABLE fragment can carry leading comment text — match the keyword
    /// anywhere in the fragment, not by `starts_with`.
    fn package_ddl_create_table_names() -> Vec<String> {
        const KW: &str = "CREATE TABLE IF NOT EXISTS ";
        crate::db::ddl_compat::split_ddl_statements(crate::db::okf_ddl::LLM_WIKI_PACKAGE_DDL)
            .iter()
            .filter_map(|stmt| {
                let at = stmt.find(KW)?;
                let rest = &stmt[at + KW.len()..];
                let end = rest
                    .find(|c: char| c.is_whitespace() || c == '(')
                    .unwrap_or(rest.len());
                Some(rest[..end].to_string())
            })
            .collect()
    }

    #[test]
    fn llm_wiki_tables_cover_every_package_create_table() {
        let ddl_names = package_ddl_create_table_names();
        assert_eq!(
            LLM_WIKI_TABLES.len(),
            ddl_names.len(),
            "LLM_WIKI_TABLES has {} entries but LLM_WIKI_PACKAGE_DDL creates {} tables: {ddl_names:?}",
            LLM_WIKI_TABLES.len(),
            ddl_names.len()
        );
        let guard: BTreeSet<&str> = LLM_WIKI_TABLES.iter().map(|t| t.name).collect();
        let ddl: BTreeSet<&str> = ddl_names.iter().map(String::as_str).collect();
        assert_eq!(guard, ddl, "guard table names must equal the package DDL's");
    }

    /// Raw (no migrate) open of a tempdir copy of the T0 fixture brain —
    /// a V32-stamped brain created under the core-llm-wiki 8.0.2 DDL.
    fn open_raw_8_0_2_fixture_copy(tmp: &tempfile::TempDir) -> Connection {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/db/testdata/ct-brain-v32-core-8.0.2.db");
        let dst = tmp.path().join("brain.db");
        std::fs::copy(&src, &dst).expect("copy 8.0.2 fixture brain");
        Connection::open(&dst).expect("raw open of fixture copy")
    }

    /// Fixture fidelity: the 8.0.2 expectations JSON and the 8.0.2 fixture
    /// brain agree on the 8.0.2 shape, and the 9.18.0 guard rejects that
    /// shape (the fixture really is pre-bump).
    #[test]
    fn expectations_8_0_2_fixture_matches_unmigrated_8_0_2_brain() {
        let old = core_llm_wiki_8_0_2_expectations();
        assert_eq!(old.len(), 9, "8.0.2 guarded nine llm_wiki tables");

        let tmp = tempfile::tempdir().unwrap();
        let conn = open_raw_8_0_2_fixture_copy(&tmp);
        verify_schema_with(&conn, &old).expect("8.0.2 expectations pass on the 8.0.2 fixture");

        let err = verify_llm_wiki_schema(&conn)
            .expect_err("9.18.0 guard must reject the unmigrated 8.0.2 brain")
            .to_string();
        assert!(
            err.contains("missing"),
            "expected a missing-column/table detail, got: {err}"
        );
    }

    /// Spec §5 test 1 (fresh install), file-backed: a fresh ROOTED brain
    /// opened through CT's real open path stamps the full ladder and passes
    /// the 9.18.0 guard. (Fresh path: `migration_v7_sql` runs the 9.18.0
    /// const; CT-local columns arrive by ALTER — a raw-DDL-only DB cannot
    /// pass the guard by design.)
    #[test]
    fn verify_passes_on_fresh_rooted_file_brain() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = tmp.path().join("vault");
        std::fs::create_dir_all(&vault).unwrap();
        let config = tmp.path().join("config.json");
        std::fs::write(
            &config,
            // serde_json serialization: a format! interpolation would emit
            // unescaped backslashes on Windows paths (invalid JSON escapes).
            serde_json::json!({ "vault_path": vault.to_str().unwrap() }).to_string(),
        )
        .unwrap();
        let db = crate::db::connection::AppDb::open_with_config(&tmp.path().join("brain.db"), &config)
            .expect("fresh rooted brain opens (migrate + every-open guard)");
        let stamped: Vec<i64> = {
            let mut s = db
                .0
                .prepare("SELECT version FROM schema_version WHERE version IN (31, 32, 33)")
                .unwrap();
            let rows = s
                .query_map([], |r| r.get::<_, i64>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            rows
        };
        // Membership, not MAX: `>= 32` would pass even if 31/32 never stamped
        // (T4 r1 M6).
        for v in [31, 32, 33] {
            assert!(stamped.contains(&v), "rooted fresh brain must stamp V{v}");
        }
        verify_llm_wiki_schema(&db.0).expect("fresh rooted brain matches the 9.18.0 guard");
    }

    #[test]
    fn verify_passes_on_fresh_v7_database() {
        let conn = open_in_memory().unwrap();
        verify_llm_wiki_schema(&conn).expect("fresh V7 db should match pinned schema");
    }

    #[test]
    fn verify_fails_on_extra_column() {
        let conn = open_in_memory().unwrap();
        conn.execute(
            "ALTER TABLE llm_wiki_entries ADD COLUMN bogus_extra TEXT",
            [],
        )
        .unwrap();

        let err = verify_llm_wiki_schema(&conn).unwrap_err().to_string();
        assert!(
            err.contains("unexpected columns"),
            "expected unexpected-column detail, got: {err}"
        );
        assert!(
            err.contains(PINNED_CORE_LLM_WIKI_VERSION),
            "expected pinned package version in error, got: {err}"
        );
        assert!(
            err.contains("llm_wiki_entries"),
            "expected table name in error, got: {err}"
        );
    }

    #[test]
    fn verify_fails_on_missing_column() {
        let conn = open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE llm_wiki_entries_rebuild AS SELECT id, entity_id, title FROM llm_wiki_entries;
             DROP TABLE llm_wiki_entries;
             ALTER TABLE llm_wiki_entries_rebuild RENAME TO llm_wiki_entries;",
        )
        .unwrap();

        let err = verify_llm_wiki_schema(&conn).unwrap_err().to_string();
        assert!(
            err.contains("missing columns"),
            "expected missing-column detail, got: {err}"
        );
    }

    /// A database created before core-llm-wiki gained the embedding-failure
    /// marker columns (package 6.5.0) must be REJECTED by the guard, not
    /// silently accepted — the guard compares full column sets, so absence
    /// fails the same way as any other missing column.
    #[test]
    fn verify_fails_when_package_embedding_failure_columns_are_missing() {
        let conn = open_in_memory().unwrap();
        conn.execute_batch(
            "ALTER TABLE llm_wiki_entries DROP COLUMN embedding_failed_at;
             ALTER TABLE llm_wiki_entries DROP COLUMN embedding_failure_kind;
             ALTER TABLE llm_wiki_entries DROP COLUMN embedding_attempts;",
        )
        .unwrap();

        let err = verify_llm_wiki_schema(&conn).unwrap_err().to_string();
        assert!(
            err.contains("missing columns"),
            "expected missing-column detail, got: {err}"
        );
        assert!(
            err.contains("embedding_failed_at"),
            "expected the missing package column to be named, got: {err}"
        );
    }
}
