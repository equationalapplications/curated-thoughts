use crate::db::okf_ddl;
use crate::db::schema::{
    MIGRATION_V1, MIGRATION_V2, MIGRATION_V3, MIGRATION_V4, MIGRATION_V5, MIGRATION_V6,
    MIGRATION_V9,
};
use crate::hasher::hash_bytes;
use crate::vault::VaultConfig;
use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;

fn normalize_workspace_root(path: &str) -> String {
    let mut normalized = path.replace('\\', "/");
    if normalized != "/" {
        normalized = normalized.trim_end_matches('/').to_string();
        if normalized.ends_with(':') {
            normalized.push('/');
        }
        if normalized.is_empty() {
            normalized = "/".to_string();
        }
    }
    normalized
}

fn canonicalize_workspace_root(path: &str) -> String {
    std::path::Path::new(path)
        .canonicalize()
        .map(|p| normalize_workspace_root(&p.to_string_lossy()))
        .unwrap_or_else(|_| normalize_workspace_root(path))
}

fn migrate(conn: &Connection, vault_root: Option<String>) -> Result<()> {
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
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V5))?;
        if let Some(root) = vault_root.as_deref() {
            let normalized_root = canonicalize_workspace_root(root);
            let entity_id = format!(
                "tier_working::{}",
                &hash_bytes(normalized_root.as_bytes())[..16]
            );
            conn.execute(
                "UPDATE chunks SET entity_id = ?1 WHERE entity_id = 'tier_working'",
                [entity_id.as_str()],
            )?;
        } else {
            conn.execute(
                "UPDATE documents
                 SET status = 'pending'
                 WHERE status = 'indexed'
                   AND path NOT LIKE '%/documents/%'
                   AND path NOT LIKE '%/wiki/%'
                   AND EXISTS (
                       SELECT 1 FROM chunks c
                        WHERE c.doc_id = documents.id
                          AND c.entity_id = 'tier_working'
                   )",
                [],
            )?;
        }
    }
    if version < 6 {
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V6))?;
    }
    if version < 9 {
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V9))?;
    }
    if version < 7 {
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", okf_ddl::migration_v7_sql()))?;
    }

    // Phase 5 data migration: fix resolution event taxonomy (run once, gated by version < 8)
    if version < 8 {
        conn.execute_batch(
            "UPDATE llm_wiki_events SET event_type = 'approved'
               WHERE event_type = 'action' AND summary LIKE 'Approved%';
             UPDATE llm_wiki_events SET event_type = 'rejected'
               WHERE event_type = 'observation' AND summary LIKE 'Rejected proposal%';",
        )?;
        // Bump schema_version to 8 so this migration runs only once
        conn.execute(
            "INSERT OR IGNORE INTO schema_version (version) VALUES (8)",
            [],
        )?;
    }

    // 90-day pruning of curated_agent_log (local-only audit trail)
    conn.execute(
        "DELETE FROM curated_agent_log WHERE created_at < unixepoch() - 90*24*60*60",
        [],
    )?;

    // Ensure index on curated_agent_log.created_at for pruning performance
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_curated_agent_log_created_at
         ON curated_agent_log(created_at);",
    )?;

    crate::db::schema_guard::verify_llm_wiki_schema(conn)?;

    Ok(())
}

#[allow(dead_code)]
pub struct AppDb(pub Connection);

impl AppDb {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout = 5000;")?;
        let config_path = path
            .parent()
            .map(|p| p.join("config.json"))
            .unwrap_or_else(|| VaultConfig::default_config_path());
        Self::open_with_config(path, config_path)
    }

    /// Open the brain database, resolving the vault root from an explicit
    /// config path. Callers that honor split `CURATED_BRAIN_DB` /
    /// `CURATED_BRAIN_CONFIG` environments must use this instead of [`AppDb::open`],
    /// which derives config.json from the database's parent directory.
    pub fn open_with_config(path: &Path, config_path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout = 5000;")?;
        let vault_root = VaultConfig::new(config_path.as_ref().to_path_buf())
            .vault_root()
            .unwrap_or(None)
            .map(|root| {
                let root_str = root.to_string_lossy().to_string();
                canonicalize_workspace_root(&root_str)
            });
        migrate(&conn, vault_root.clone())?;
        if let Some(root) = vault_root.as_deref() {
            let vault_path = std::path::Path::new(root);
            if vault_path.is_dir() {
                let _ = crate::db::okf_migration::run_okf_migration(&conn, vault_path);
            }
        }
        Ok(AppDb(conn))
    }
}

pub fn open_in_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    migrate(&conn, None)?;
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
        assert_eq!(max_version, 9);
    }

    #[test]
    fn test_v7_okf_and_curated_tables_exist() {
        let conn = open_in_memory().unwrap();
        for table in &[
            "llm_wiki_entries",
            "llm_wiki_outbox",
            "llm_wiki_meta",
            "llm_wiki_edges",
            "curated_entities",
            "curated_proposals",
            "curated_proposal_items",
            "curated_proposal_sources",
            "curated_agent_log",
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
        let id =
            crate::db::queries::insert_chunk(&conn, doc_id, &chunk, 0, "tier_fact", "").unwrap();
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
    fn migration_v5_backfills_entity_id_from_document_path_prefix() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        conn.execute_batch(&format!(
            "BEGIN;\n{}\n{}\n{}\n{}\nCOMMIT;",
            MIGRATION_V1, MIGRATION_V2, MIGRATION_V3, MIGRATION_V4
        ))
        .unwrap();

        conn.execute(
            "INSERT INTO documents (path, hash, tier, status) VALUES (?1, ?2, ?3, 'indexed')",
            ["/vault/documents/doc.md", "h1", "user_doc"],
        )
        .unwrap();
        let doc_id: i64 = conn
            .query_row(
                "SELECT id FROM documents WHERE path = ?1",
                ["/vault/documents/doc.md"],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute(
            "INSERT INTO chunks (doc_id, chunk_text, position, start_line, end_line, symbol_name, strategy) VALUES (?1, ?2, ?3, 1, 1, NULL, 'prose')",
            rusqlite::params![doc_id, "body", 0],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO documents (path, hash, tier, status) VALUES (?1, ?2, ?3, 'indexed')",
            ["/vault/src/init.rs", "h2", "user_doc"],
        )
        .unwrap();
        let working_doc_id: i64 = conn
            .query_row(
                "SELECT id FROM documents WHERE path = ?1",
                ["/vault/src/init.rs"],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute(
            "INSERT INTO chunks (doc_id, chunk_text, position, start_line, end_line, symbol_name, strategy) VALUES (?1, ?2, ?3, 1, 1, NULL, 'prose')",
            rusqlite::params![working_doc_id, "body", 0],
        )
        .unwrap();

        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V5))
            .unwrap();

        let entity_id_fact: String = conn
            .query_row(
                "SELECT entity_id FROM chunks WHERE doc_id = ?1",
                [doc_id],
                |r| r.get(0),
            )
            .unwrap();
        let entity_id_working: String = conn
            .query_row(
                "SELECT entity_id FROM chunks WHERE doc_id = ?1",
                [working_doc_id],
                |r| r.get(0),
            )
            .unwrap();

        assert_eq!(entity_id_fact, "tier_fact");
        assert_eq!(entity_id_working, "tier_working");
    }

    #[test]
    fn migration_v5_backfills_working_chunks_with_vault_root_hash() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config_path = tmp.path().join("config.json");
        let cfg = VaultConfig::new(config_path.clone());
        cfg.set_vault_path("/vault").unwrap();

        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        conn.execute_batch(&format!(
            "BEGIN;\n{}\n{}\n{}\n{}\nCOMMIT;",
            MIGRATION_V1, MIGRATION_V2, MIGRATION_V3, MIGRATION_V4
        ))
        .unwrap();

        conn.execute(
            "INSERT INTO documents (path, hash, tier, status) VALUES (?1, ?2, ?3, 'indexed')",
            ["/vault/src/init.rs", "h2", "user_doc"],
        )
        .unwrap();
        let working_doc_id: i64 = conn
            .query_row(
                "SELECT id FROM documents WHERE path = ?1",
                ["/vault/src/init.rs"],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute(
            "INSERT INTO chunks (doc_id, chunk_text, position, start_line, end_line, symbol_name, strategy) VALUES (?1, ?2, ?3, 1, 1, NULL, 'prose')",
            rusqlite::params![working_doc_id, "body", 0],
        )
        .unwrap();

        migrate(&conn, Some("/vault".to_string())).unwrap();

        let entity_id_working: String = conn
            .query_row(
                "SELECT entity_id FROM chunks WHERE doc_id = ?1",
                [working_doc_id],
                |r| r.get(0),
            )
            .unwrap();

        let expected = format!(
            "tier_working::{}",
            &hash_bytes("/vault".replace('\\', "/").trim_end_matches('/').as_bytes())[..16]
        );

        assert_eq!(entity_id_working, expected);
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
        let id =
            crate::db::queries::insert_chunk(&conn, doc_id, &chunk, 0, "tier_working", "").unwrap();
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

    /// Regression test for the Phase 9 chunk-id resolution migration.
    ///
    /// Phase 5 already bumped `schema_version` to 8 on every released DB.
    /// Gating the new migration on `version < 7` made the gate unreachable
    /// on any production database — `ALTER TABLE chunks ADD COLUMN
    /// content_hash` never ran, and `insert_chunk` crashed at runtime
    /// (column missing). The fix renames the gate to `version < 9`.
    ///
    /// This test pre-seeds a connection with `schema_version = 8` and
    /// the OKF V7 DDL (the state of a Phase 5 production DB), runs the
    /// production migration gate (`migrate`), and asserts the
    /// `content_hash` column exists.
    #[test]
    fn migration_v9_adds_content_hash_column_to_phase5_database() {
        // Simulate a Phase 5 production database: V1..V6 + OKF V7 DDL,
        // and `schema_version` is at 8 (set by Phase 5's data migration).
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        conn.execute_batch(&format!(
            "BEGIN;\n{}\n{}\n{}\n{}\n{}\nCOMMIT;",
            MIGRATION_V1, MIGRATION_V2, MIGRATION_V3, MIGRATION_V4, MIGRATION_V5
        ))
        .unwrap();
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V6))
            .unwrap();
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", okf_ddl::migration_v7_sql()))
            .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO schema_version (version) VALUES (8)",
            [],
        )
        .unwrap();

        // Sanity: confirm the pre-seeded state matches a Phase 5 production DB.
        let pre_version: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            pre_version, 8,
            "test precondition: schema_version must be 8 before migrate()"
        );

        // Pre-fix bug: gating on `version < 7` skipped the ALTER TABLE,
        // leaving `chunks` without `content_hash`. With the V9 gate the
        // column is added.
        migrate(&conn, None).expect("migrate must succeed on Phase 5 DB");

        let has_content_hash: bool = conn
            .prepare("PRAGMA table_info(chunks)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(Result::ok)
            .any(|name| name == "content_hash");
        assert!(
            has_content_hash,
            "chunks.content_hash must exist after migrate() on a Phase 5 DB"
        );

        // Post-migration schema_version should be at least 9 (V9 bumped it).
        let post_version: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            post_version >= 9,
            "schema_version must reach >= 9 after migrate(), got {post_version}"
        );
    }
}
