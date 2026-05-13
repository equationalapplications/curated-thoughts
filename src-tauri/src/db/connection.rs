use crate::db::schema::{MIGRATION_V1, MIGRATION_V2, MIGRATION_V3, MIGRATION_V4, MIGRATION_V5};
use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;

/// Apply bundled migrations exactly once each. `ALTER TABLE` migrations are **not**
/// idempotent, so bodies after V3 must be gated by `schema_version`.
fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch("PRAGMA foreign_keys=ON;")?;
    conn.execute_batch(&format!(
        "BEGIN;\n{}\n{}\n{}\nCOMMIT;",
        MIGRATION_V1, MIGRATION_V2, MIGRATION_V3
    ))?;

    let version: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_version",
        [],
        |r| r.get(0),
    )?;
    if version < 4 {
        conn.execute_batch(MIGRATION_V4)?;
    }
    if version < 5 {
        conn.execute_batch(MIGRATION_V5)?;
    }

    Ok(())
}

#[allow(dead_code)]
pub struct AppDb(pub Connection);

impl AppDb {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        migrate(&conn)?;
        Ok(AppDb(conn))
    }
}

#[cfg(test)]
pub fn open_in_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    migrate(&conn)?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_db_initializes_with_schema_version() {
        let conn = open_in_memory().unwrap();
        let max_version: i64 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(max_version, 5);
    }

    #[test]
    fn test_all_tables_exist() {
        let conn = open_in_memory().unwrap();
        for table in &[
            "documents",
            "chunks",
            "wiki_pages",
            "folder_rules",
            "curated_relationships",
        ] {
            let count: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "table '{}' not found in schema", table);
        }
    }

    #[test]
    fn test_embeddings_table_exists() {
        let conn = open_in_memory().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='embeddings'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_schema_version_is_5() {
        let conn = open_in_memory().unwrap();
        let max_version: i64 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(max_version, 5);
    }

    #[test]
    fn migration_v5_adds_defined_symbol_and_entity_id_columns() {
        let conn = open_in_memory().unwrap();
        let doc_id = crate::db::queries::upsert_document(&conn, "/x/b.md", "h2").unwrap();
        let chunk = crate::chunker::Chunk {
            text: "body".into(),
            start_line: 1,
            end_line: 1,
            symbol_name: Some("MyStruct".into()),
            defined_symbol: Some("mystruct".into()),
            strategy: crate::chunker::ChunkStrategyTag::AstSymbolRust,
        };
        let id = crate::db::queries::insert_chunk(&conn, doc_id, &chunk, 0, "tier_fact").unwrap();
        let (def_sym, eid): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT defined_symbol, entity_id FROM chunks WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(def_sym.as_deref(), Some("mystruct"));
        assert_eq!(eid.as_deref(), Some("tier_fact"));
    }

    #[test]
    fn migration_v4_chunk_columns_roundtrip() {
        let conn = open_in_memory().unwrap();
        let doc_id = crate::db::queries::upsert_document(&conn, "/x/a.md", "h1").unwrap();
        let chunk = crate::chunker::Chunk {
            text: "body".into(),
            start_line: 3,
            end_line: 7,
            symbol_name: Some("foo".into()),
            defined_symbol: None,
            strategy: crate::chunker::ChunkStrategyTag::Declarative,
        };
        let id = crate::db::queries::insert_chunk(&conn, doc_id, &chunk, 0, "tier_working").unwrap();
        let (sl, el, sym, strat): (i64, i64, Option<String>, String) = conn
            .query_row(
                "SELECT start_line, end_line, symbol_name, strategy FROM chunks WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(sl, 3);
        assert_eq!(el, 7);
        assert_eq!(sym.as_deref(), Some("foo"));
        assert_eq!(strat, "declarative");
    }
}
