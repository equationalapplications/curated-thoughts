//! Startup guard: `PRAGMA table_info` on `llm_wiki_*` tables must match the pinned package schema.

use anyhow::{bail, Context, Result};
use rusqlite::Connection;
use std::collections::BTreeSet;

/// Pinned `package.json` dependency — keep in sync with `@equationalapplications/core-llm-wiki`.
pub const PINNED_CORE_LLM_WIKI_VERSION: &str = "5.5.1";

struct TableExpectation {
    name: &'static str,
    columns: &'static [&'static str],
}

const LLM_WIKI_TABLES: &[TableExpectation] = &[
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
        ],
    },
    TableExpectation {
        name: "llm_wiki_checkpoints",
        columns: &["entity_id", "heal_checkpoint", "memory_checkpoint"],
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

fn verify_table(conn: &Connection, expect: &TableExpectation) -> Result<()> {
    if !table_exists(conn, expect.name)? {
        bail!(
            "llm_wiki schema mismatch: table `{}` is missing. \
             Rust expects core-llm-wiki@{PINNED_CORE_LLM_WIKI_VERSION} columns. \
             The database was not modified.",
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
         Upgrade Curated Thoughts or restore from backup; the database was not modified."
    );
}

/// Verify all `llm_wiki_*` table column sets after schema V7. No-op on older schema versions.
pub fn verify_llm_wiki_schema(conn: &Connection) -> Result<()> {
    if schema_version(conn)? < 7 {
        return Ok(());
    }
    for expect in LLM_WIKI_TABLES {
        verify_table(conn, expect)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::open_in_memory;

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
}
