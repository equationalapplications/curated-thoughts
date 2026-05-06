use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;
use crate::db::schema::MIGRATION_V1;

pub struct AppDb(pub Connection);

impl AppDb {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch(MIGRATION_V1)?;
        Ok(AppDb(conn))
    }
}

pub fn open_in_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(MIGRATION_V1)?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_db_initializes_with_schema_version() {
        let conn = open_in_memory().unwrap();
        let version: i64 = conn
            .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 1);
    }

    #[test]
    fn test_all_tables_exist() {
        let conn = open_in_memory().unwrap();
        for table in &["documents", "chunks", "wiki_pages", "folder_rules"] {
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
}
